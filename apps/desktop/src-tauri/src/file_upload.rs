//! Desktop drops use a separate SSH channel and the SCP sink protocol. Files stream in bounded
//! chunks; directories are staged privately and published without replacing an existing item.
use crate::{
    session_store::SessionMeta,
    validation::{base64_decode, parse_host, validate_session},
};
use serde::Serialize;
use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, String>;
static SEQUENCE: AtomicU64 = AtomicU64::new(1);

fn terminate_child(child: &mut Child) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGTERM);
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }
}

fn stop_child(child: &mut Child) {
    if child.try_wait().ok().flatten().is_some() {
        return;
    }
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
}

#[derive(Default)]
pub struct Cancellation {
    flag: AtomicBool,
    child: Mutex<Option<Arc<Mutex<Child>>>>,
}
impl std::ops::Deref for Cancellation {
    type Target = AtomicBool;
    fn deref(&self) -> &AtomicBool {
        &self.flag
    }
}
impl Cancellation {
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::Relaxed);
        if let Some(child) = self.child.lock().unwrap().as_ref() {
            terminate_child(&mut child.lock().unwrap());
        }
    }
}

#[derive(Default)]
pub struct UploadState {
    pending: Mutex<Option<(String, Vec<PathBuf>, Instant)>>,
    running: Mutex<Option<(String, Arc<Cancellation>)>>,
}
impl UploadState {
    // Only native OS drop events mint a one-use grant. JS never supplies arbitrary local paths.
    pub fn dropped(&self, paths: Vec<PathBuf>) -> String {
        let token = format!("drop-{}", SEQUENCE.fetch_add(1, Ordering::Relaxed));
        *self.pending.lock().unwrap() = Some((token.clone(), paths, Instant::now()));
        token
    }
    pub fn begin(&self, token: &str) -> Result<(Vec<PathBuf>, Arc<Cancellation>)> {
        let mut running = self.running.lock().unwrap();
        if running.is_some() {
            return Err("Another upload is still running.".into());
        }
        let mut pending = self.pending.lock().unwrap();
        if !pending
            .as_ref()
            .is_some_and(|(id, _, time)| id == token && time.elapsed() < Duration::from_secs(120))
        {
            return Err("This drop expired. Drag the files into Buoy again.".into());
        }
        let (_, paths, _) = pending.take().unwrap();
        let cancel = Arc::new(Cancellation::default());
        *running = Some((token.into(), cancel.clone()));
        Ok((paths, cancel))
    }
    pub fn cancel(&self, token: &str) {
        if let Some((id, flag)) = self.running.lock().unwrap().as_ref() {
            if id == token {
                flag.cancel();
            }
        }
    }
    pub fn cancel_all(&self) {
        if let Some((_, flag)) = self.running.lock().unwrap().as_ref() {
            flag.cancel();
        }
    }
    pub fn finish(&self, token: &str) {
        let mut running = self.running.lock().unwrap();
        if running.as_ref().is_some_and(|(id, _)| id == token) {
            *running = None;
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub token: String,
    pub directory: String,
    pub item: String,
    pub sent: u64,
    pub total: u64,
    pub completed: usize,
    pub count: usize,
    pub phase: &'static str,
}
#[derive(Debug, Serialize)]
pub struct ItemResult {
    pub name: String,
    pub status: String,
    pub detail: String,
    // Delivery to the terminal, not a claim that the foreground CLI accepted an attachment.
    pub inserted: bool,
}
#[derive(Debug, Serialize)]
pub struct UploadReport {
    pub directory: String,
    pub items: Vec<ItemResult>,
    pub warnings: Vec<String>,
    pub cancelled: bool,
}

fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
fn cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        Err("Upload cancelled.".into())
    } else {
        Ok(())
    }
}

fn command(meta: &SessionMeta, script: &str, extra_ssh: &[String]) -> Result<Command> {
    let mut command;
    if meta.transport == "local" {
        command = Command::new("/bin/sh");
        command.args(["-c", script]);
    } else {
        let host = parse_host(&meta.host).map_err(|e| e.to_string())?;
        command = Command::new("ssh");
        if let Some(port) = host.port {
            command.args(["-p", &port.to_string()]);
        }
        command.args([
            "-T",
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            "-o",
            "ServerAliveInterval=5",
            "-o",
            "ServerAliveCountMax=3",
            "-o",
            "ControlMaster=no",
            "-o",
            "ControlPath=none",
        ]);
        command.args(extra_ssh);
        let target = host
            .user
            .map(|user| format!("{user}@{}", host.host))
            .unwrap_or(host.host);
        // -c leaves stdin free for the binary SCP stream; piping the script through sh would eat it.
        command.args(["--", &target, &format!("/bin/sh -c {}", quote(script))]);
    }
    command.env("PATH", crate::augmented_path());
    Ok(command)
}

struct ProcessGuard {
    child: Arc<Mutex<Child>>,
    done: Arc<AtomicBool>,
}
impl Drop for ProcessGuard {
    fn drop(&mut self) {
        self.done.store(true, Ordering::Relaxed);
        let mut child = self.child.lock().unwrap();
        stop_child(&mut child);
        let _ = child.wait();
    }
}

// Kill a cancelled/stalled child even if the worker is blocked writing stdin or waiting for an
// acknowledgement. Keep stderr draining, with a bounded retained error message.
fn run<T>(
    mut command: Command,
    cancel: &Arc<Cancellation>,
    operation: impl FnOnce(
        std::process::ChildStdin,
        BufReader<std::process::ChildStdout>,
        &dyn Fn(),
    ) -> Result<T>,
) -> Result<T> {
    cancelled(cancel)?;
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Could not start file transfer: {e}"))?;
    let input = child.stdin.take().unwrap();
    let output = BufReader::new(child.stdout.take().unwrap());
    let mut error = child.stderr.take().unwrap();
    let child = Arc::new(Mutex::new(child));
    *cancel.child.lock().unwrap() = Some(child.clone());
    let done = Arc::new(AtomicBool::new(false));
    let _guard = ProcessGuard {
        child: child.clone(),
        done: done.clone(),
    };
    let start = Instant::now();
    let activity = AtomicU64::new(0);
    let timed_out = AtomicBool::new(false);
    thread::scope(|scope| {
        let errors = scope.spawn(move || {
            let mut saved = Vec::new();
            let mut buf = [0; 4096];
            while let Ok(n) = error.read(&mut buf) {
                if n == 0 {
                    break;
                }
                let remaining = 16_384usize.saturating_sub(saved.len());
                saved.extend_from_slice(&buf[..n.min(remaining)]);
            }
            String::from_utf8_lossy(&saved).trim().to_string()
        });
        scope.spawn(|| {
            while !done.load(Ordering::Relaxed) {
                if cancel.load(Ordering::Relaxed)
                    || start
                        .elapsed()
                        .as_secs()
                        .saturating_sub(activity.load(Ordering::Relaxed))
                        > 120
                {
                    timed_out.store(!cancel.load(Ordering::Relaxed), Ordering::Relaxed);
                    terminate_child(&mut child.lock().unwrap());
                    thread::sleep(Duration::from_millis(500));
                    stop_child(&mut child.lock().unwrap());
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        });
        let touch = || {
            activity.store(start.elapsed().as_secs(), Ordering::Relaxed);
        };
        let result = operation(input, output, &touch);
        if result.is_err() {
            terminate_child(&mut child.lock().unwrap());
        }
        let finishing = Instant::now();
        let status = loop {
            if result.is_err() && finishing.elapsed() > Duration::from_millis(500) {
                stop_child(&mut child.lock().unwrap());
            }
            match child.lock().unwrap().try_wait() {
                Ok(Some(status)) => break Ok(status),
                Err(e) => break Err(e.to_string()),
                _ => thread::sleep(Duration::from_millis(20)),
            }
        };
        done.store(true, Ordering::Relaxed);
        let stderr = errors.join().unwrap_or_default();
        // Successful publication may race a late Cancel click. A confirmed success wins.
        if result.is_ok() && status.as_ref().is_ok_and(|status| status.success()) {
            return result;
        }
        cancelled(cancel)?;
        if timed_out.load(Ordering::Relaxed) {
            return Err("Upload stalled for two minutes. Try again.".into());
        }
        if !stderr.is_empty() {
            return Err(match result {
                Err(error) => format!("{error}\n{stderr}"),
                Ok(_) => stderr,
            });
        }
        if result.is_err() {
            return result;
        }
        Err("File transfer failed before completion was confirmed.".into())
    })
}

fn line(reader: &mut impl BufRead) -> Result<String> {
    let mut data = Vec::new();
    reader
        .take(16_385)
        .read_until(b'\n', &mut data)
        .map_err(|e| e.to_string())?;
    if data.last() != Some(&b'\n') || data.len() > 16_384 {
        return Err("Invalid or incomplete file-transfer response.".into());
    }
    data.pop();
    String::from_utf8(data).map_err(|_| "Invalid file-transfer response.".into())
}

#[derive(Debug)]
struct Target {
    directory: String,
    pane: String,
    identity: String,
}

// The foreground process group changes when an agent exits back to its shell. Include its
// start time as well as the pane/server identity so a replacement cannot inherit a pending drop.
fn recipient_script(tmux: &str, socket: &str) -> String {
    format!(
        r#"
tm() {{ {tmux} -u -L {socket} "$@"; }}
recipient() {{
  info=$(tm display-message -p -t "$pane" '#{{window_id}}:#{{pane_id}}:#{{pane_pid}}:#{{pid}}:#{{pane_current_command}}:#{{pane_in_mode}}') || return 1
  pid=$(tm display-message -p -t "$pane" '#{{pane_pid}}') || return 1
  group=$(ps -o tpgid= -p "$pid" | tr -d ' ') || return 1
  case "$group" in ''|*[!0-9]*|0) return 1 ;; esac
  process=$(ps -o pid=,lstart=,comm= -p "$group") || return 1
  [ -n "$process" ] || return 1
  printf '%s\n%s' "$info" "$process"
}}
"#,
        tmux = quote(tmux),
        socket = quote(socket)
    )
}

fn resolve_target(
    meta: &SessionMeta,
    win: &str,
    cancel: &Arc<Cancellation>,
    extra: &[String],
    attach: bool,
) -> Result<Target> {
    validate_session(&meta.session).map_err(|e| e.to_string())?;
    if meta.mode == "local" {
        return Err("File drops require a tmux terminal tab.".into());
    }
    if !(win.starts_with('@') && win[1..].bytes().all(|b| b.is_ascii_digit()) && win.len() > 1)
        && !(win == "@single" && meta.mode == "plain")
    {
        return Err("Switch to a terminal tab to upload files.".into());
    }
    let tmux = meta.tmux_path.as_deref().unwrap_or("tmux");
    if !crate::validation::is_safe_tmux_path(tmux) {
        return Err("Invalid tmux path.".into());
    }
    let socket = meta.socket_name.clone().unwrap_or_else(|| {
        crate::tmux_socket::socket_name(&meta.mode, meta.tmux_version, &meta.session)
    });
    let script = format!(
        r#"set -eu
{recipient}
rows=$({tmux} -u -L {socket} list-panes -s -t {session} -F '#{{window_id}} #{{pane_id}} #{{pane_active}} #{{window_active}}')
pane=$(printf '%s\n' "$rows" | while read -r w p a active; do
  if [ "$a" = 1 ] && {{ [ "$w" = {win} ] || {{ [ {win} = '@single' ] && [ "$active" = 1 ]; }}; }}; then printf '%s' "$p"; break; fi
done)
[ -n "$pane" ] || {{ echo 'The target terminal no longer exists.' >&2; exit 1; }}
printf '%s\n' "$pane"
identity=
if [ {attach} = true ]; then identity=$(recipient) || identity=; fi
printf '%s' "$identity" | base64 | tr -d '\r\n'
printf '\n'
{tmux} -u -L {socket} display-message -p -t "$pane" '#{{pane_current_path}}' | base64
"#,
        recipient = recipient_script(tmux, &socket),
        attach = attach,
        tmux = quote(tmux),
        socket = quote(&socket),
        session = quote(&format!("={}", meta.session)),
        win = quote(win)
    );
    let output = run(
        command(meta, &script, extra)?,
        cancel,
        |input, mut out, _| {
            drop(input);
            let mut bytes = Vec::new();
            out.by_ref()
                .take(32_768)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            Ok(bytes)
        },
    )?;
    let output = String::from_utf8(output).map_err(|_| "Invalid terminal target response.")?;
    let mut fields = output.splitn(3, '\n');
    let pane = fields.next().unwrap_or_default().to_string();
    if !pane.starts_with('%') || pane.len() < 2 || !pane[1..].bytes().all(|b| b.is_ascii_digit()) {
        return Err("Invalid terminal pane.".into());
    }
    let identity = fields.next().unwrap_or_default().to_string();
    let bytes = base64_decode(fields.next().unwrap_or_default())
        .ok_or("Could not resolve the terminal directory.")?;
    let mut directory =
        String::from_utf8(bytes).map_err(|_| "The terminal directory is not UTF-8.")?;
    if directory.ends_with('\n') {
        directory.pop();
    }
    if !directory.starts_with('/') || directory.contains('\0') {
        return Err("Could not resolve an absolute terminal directory.".into());
    }
    Ok(Target {
        directory,
        pane,
        identity,
    })
}

pub fn resolve_directory(
    meta: &SessionMeta,
    win: &str,
    cancel: &Arc<Cancellation>,
    extra: &[String],
) -> Result<String> {
    resolve_target(meta, win, cancel, extra, false).map(|target| target.directory)
}

fn paste_path(path: &str) -> Result<String> {
    if path.chars().any(char::is_control) {
        return Err(
            "This path contains control characters and cannot be added to terminal input.".into(),
        );
    }
    // Finder-style escaping is understood by both shells and CLI image-path paste handlers.
    // Shell concatenations such as 'a'"'"'b.png' are not understood by Claude's paste parser.
    let mut text = String::new();
    for c in path.chars() {
        if !c.is_alphanumeric() && !matches!(c, '/' | '.' | '_' | '-') {
            text.push('\\');
        }
        text.push(c);
    }
    // Ordinary file/directory references need a separator before the next drop or typed word.
    // Image handlers trim this space before resolving the path.
    text.push(' ');
    Ok(text)
}

fn insert_path(
    meta: &SessionMeta,
    target: &Target,
    path: &str,
    cancel: &Arc<Cancellation>,
    extra: &[String],
) -> Result<()> {
    if target.identity.is_empty() {
        return Err(
            "Uploaded, but the terminal recipient could not be verified. Add the path manually."
                .into(),
        );
    }
    let text = paste_path(path)?;
    let tmux = meta.tmux_path.as_deref().unwrap_or("tmux");
    let socket = meta.socket_name.clone().unwrap_or_else(|| {
        crate::tmux_socket::socket_name(&meta.mode, meta.tmux_version, &meta.session)
    });
    let buffer = format!(
        "buoy-drop-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    );
    let script = format!(
        r#"set -eu
{recipient}
pane={pane}
buffer={buffer}
check_recipient() {{
  current=$(recipient | base64 | tr -d '\r\n')
  [ "$current" = {identity} ] && [ "$(tm display-message -p -t "$pane" '#{{pane_in_mode}}')" = 0 ] || {{
    echo 'Uploaded, but the terminal program changed. Add the path manually.' >&2; exit 1;
  }}
}}
check_recipient
trap 'tm delete-buffer -b "$buffer" 2>/dev/null || :' EXIT
tm load-buffer -b "$buffer" -
check_recipient
# -p uses the recipient's bracketed-paste mode; -d consumes only our private buffer.
tm paste-buffer -p -d -b "$buffer" -t "$pane"
printf 'INSERTED\n'
"#,
        recipient = recipient_script(tmux, &socket),
        pane = quote(&target.pane),
        buffer = quote(&buffer),
        identity = quote(&target.identity)
    );
    run(
        command(meta, &script, extra)?,
        cancel,
        |mut input, mut out, _| {
            input
                .write_all(text.as_bytes())
                .map_err(|e| e.to_string())?;
            drop(input);
            if line(&mut out)? != "INSERTED" {
                return Err("Could not confirm path delivery.".into());
            }
            Ok(())
        },
    )
}

struct Entry {
    path: PathBuf,
    name: String,
    metadata: fs::Metadata,
    children: Option<Vec<Entry>>,
}
fn plan(
    path: &Path,
    warnings: &mut Vec<String>,
    cancel: &AtomicBool,
    depth: usize,
) -> Result<Option<Entry>> {
    cancelled(cancel)?;
    if depth > 128 {
        return Err("Folder nesting exceeds 128 levels.".into());
    }
    let metadata = fs::symlink_metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if metadata.file_type().is_symlink() || !(metadata.is_file() || metadata.is_dir()) {
        if warnings.len() < 100 {
            warnings.push(format!("Skipped link or special file: {}", path.display()));
        }
        return Ok(None);
    }
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("File name is not UTF-8.")?
        .to_string();
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err("File names containing control characters are not supported by SCP.".into());
    }
    let children = if metadata.is_dir() {
        let mut paths = fs::read_dir(path)
            .map_err(|e| e.to_string())?
            .map(|e| e.map(|e| e.path()))
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(|e| e.to_string())?;
        paths.sort();
        let mut entries = Vec::new();
        for path in paths {
            if let Some(entry) = plan(&path, warnings, cancel, depth + 1)? {
                entries.push(entry);
            }
        }
        Some(entries)
    } else {
        None
    };
    Ok(Some(Entry {
        path: path.into(),
        name,
        metadata,
        children,
    }))
}
impl Entry {
    fn bytes(&self) -> u64 {
        self.children
            .as_ref()
            .map(|c| c.iter().map(Entry::bytes).sum())
            .unwrap_or(self.metadata.len())
    }
    fn mode(&self) -> u32 {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            self.metadata.permissions().mode() & 0o777
        }
        #[cfg(not(unix))]
        {
            if self.children.is_some() {
                0o755
            } else {
                0o644
            }
        }
    }
}
fn ack(output: &mut impl BufRead) -> Result<()> {
    let mut byte = [0];
    output
        .read_exact(&mut byte)
        .map_err(|e| format!("SCP connection closed: {e}"))?;
    match byte[0] {
        0 => Ok(()),
        1 | 2 => Err(line(output)?),
        _ => Err("Unexpected SCP acknowledgement.".into()),
    }
}
fn send(
    entry: &Entry,
    input: &mut impl Write,
    output: &mut impl BufRead,
    progress: &mut Progress,
    notify: &mut impl FnMut(&Progress),
    cancel: &AtomicBool,
    touch: &dyn Fn(),
) -> Result<()> {
    cancelled(cancel)?;
    touch();
    if let Some(children) = &entry.children {
        writeln!(input, "D{:04o} 0 {}", entry.mode(), entry.name).map_err(|e| e.to_string())?;
        ack(output)?;
        for child in children {
            send(child, input, output, progress, notify, cancel, touch)?;
        }
        input.write_all(b"E\n").map_err(|e| e.to_string())?;
        ack(output)?;
    } else {
        let mut options = fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NOFOLLOW);
        }
        let mut file = options.open(&entry.path).map_err(|e| e.to_string())?;
        let now = file.metadata().map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if now.ino() != entry.metadata.ino() || now.dev() != entry.metadata.dev() {
                return Err("A source file changed while preparing the upload. Try again.".into());
            }
        }
        if !now.is_file() || now.len() != entry.metadata.len() {
            return Err("A source file changed while preparing the upload. Try again.".into());
        }
        writeln!(input, "C{:04o} {} {}", entry.mode(), now.len(), entry.name)
            .map_err(|e| e.to_string())?;
        ack(output)?;
        let mut remaining = now.len();
        let mut buf = [0; 64 * 1024];
        while remaining > 0 {
            cancelled(cancel)?;
            touch();
            let wanted = remaining.min(buf.len() as u64) as usize;
            let n = file.read(&mut buf[..wanted]).map_err(|e| e.to_string())?;
            if n == 0 {
                return Err("A source file shrank during upload. Try again.".into());
            }
            input.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            remaining -= n as u64;
            progress.sent += n as u64;
            notify(progress);
        }
        let end = file.metadata().map_err(|e| e.to_string())?;
        if end.len() != now.len() || end.modified().ok() != now.modified().ok() {
            return Err("A source file changed during upload. Try again.".into());
        }
        input.write_all(&[0]).map_err(|e| e.to_string())?;
        ack(output)?;
    }
    Ok(())
}

fn transfer_script(directory: &str, name: &str) -> String {
    format!(
        r#"set -eu
cd -- {directory}
name={name}
if [ -e "$name" ] || [ -L "$name" ]; then printf 'SKIPPED\n'; exit 0; fi
stage=$(mktemp -d './.buoy-upload-XXXXXXXXXX')
trap 'chmod -R u+rwX "$stage" 2>/dev/null || :; rm -rf -- "$stage"' EXIT
trap 'exit 1' HUP INT TERM
printf 'READY\n'
scp -p -r -t "$stage" || exit 1
# The parent-directory form makes mv -n skip a concurrently-created file OR folder, without
# accidentally moving a folder inside an existing namesake. Both GNU and BSD mv support -n.
if ! mv -n -- "$stage/$name" ./; then
  if [ -e "$name" ] || [ -L "$name" ]; then printf 'SKIPPED\n'; exit 0; fi
  exit 1
fi
if [ -e "$stage/$name" ] || [ -L "$stage/$name" ]; then printf 'SKIPPED\n'; else printf 'UPLOADED\n'; fi
"#,
        directory = quote(directory),
        name = quote(name)
    )
}

pub fn upload(
    meta: &SessionMeta,
    win: &str,
    token: &str,
    paths: &[PathBuf],
    cancel: Arc<Cancellation>,
    extra: &[String],
    notify: impl FnMut(&Progress),
) -> Result<UploadReport> {
    upload_with_attachment(meta, win, token, paths, cancel, extra, false, notify)
}

pub fn upload_with_attachment(
    meta: &SessionMeta,
    win: &str,
    token: &str,
    paths: &[PathBuf],
    cancel: Arc<Cancellation>,
    extra: &[String],
    attach: bool,
    mut notify: impl FnMut(&Progress),
) -> Result<UploadReport> {
    let target = resolve_target(meta, win, &cancel, extra, attach)?;
    let directory = target.directory.clone();
    let mut report = UploadReport {
        directory: directory.clone(),
        items: Vec::new(),
        warnings: Vec::new(),
        cancelled: false,
    };
    let mut progress = Progress {
        token: token.into(),
        directory,
        item: String::new(),
        sent: 0,
        total: 0,
        completed: 0,
        count: paths.len(),
        phase: "upload",
    };
    notify(&progress);
    for path in paths {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }
        let name = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        progress.item = name.clone();
        progress.sent = 0;
        progress.total = 0;
        notify(&progress);
        let result = (|| {
            let Some(entry) = plan(path, &mut report.warnings, &cancel, 0)? else {
                return Ok("unsupported");
            };
            progress.total = entry.bytes();
            notify(&progress);
            run(
                command(
                    meta,
                    &transfer_script(&progress.directory, &entry.name),
                    extra,
                )?,
                &cancel,
                |mut input, mut out, touch| {
                    match line(&mut out)?.as_str() {
                        "SKIPPED" => return Ok("skipped"),
                        "READY" => {}
                        _ => return Err("Unexpected file-transfer response.".into()),
                    }
                    ack(&mut out)?;
                    send(
                        &entry,
                        &mut input,
                        &mut out,
                        &mut progress,
                        &mut notify,
                        &cancel,
                        touch,
                    )?;
                    cancelled(&cancel)?;
                    drop(input); // EOF lets the remote SCP sink finish before publishing the item.
                    match line(&mut out)?.as_str() {
                        "UPLOADED" => Ok("uploaded"),
                        "SKIPPED" => Ok("skipped"),
                        _ => {
                            Err("File transfer completed but publication was not confirmed.".into())
                        }
                    }
                },
            )
        })();
        let (status, detail) = match result {
            Ok("uploaded") => ("uploaded", String::new()),
            Ok("skipped") => (
                "skipped",
                "A file or folder with this name already exists.".into(),
            ),
            Ok(_) => (
                "skipped",
                "Links and special files are not uploaded.".into(),
            ),
            Err(error) => ("failed", error),
        };
        report.items.push(ItemResult {
            name,
            status: status.into(),
            detail,
            inserted: false,
        });
        progress.completed += 1;
        notify(&progress);
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = progress.completed < progress.count || status == "failed";
            break;
        }
    }
    if attach && !report.cancelled {
        progress.phase = "attach";
        for item in &mut report.items {
            if item.status != "uploaded" {
                continue;
            }
            if cancel.load(Ordering::Relaxed) {
                report.cancelled = true;
                break;
            }
            progress.item = item.name.clone();
            notify(&progress);
            let path = format!("{}/{}", report.directory.trim_end_matches('/'), item.name);
            match insert_path(meta, &target, &path, &cancel, extra) {
                Ok(()) => item.inserted = true,
                Err(error) => item.detail = format!("Not added to terminal: {error}"),
            }
            // Keep separate paste events distinct in both CLI input parsers, even over a fast
            // local connection. All payload bytes remain bounded; no artificial Enter is sent.
            thread::sleep(Duration::from_millis(100));
        }
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_drop_grants_are_required_one_use_and_cannot_redirect_paths() {
        let state = UploadState::default();
        assert!(state.begin("drop-0").is_err());
        let old = state.dropped(vec!["/old".into()]);
        let token = state.dropped(vec!["/chosen".into()]);
        assert!(state.begin(&old).is_err());
        let (paths, cancel) = state.begin(&token).unwrap();
        assert_eq!(paths, vec![PathBuf::from("/chosen")]);
        assert!(state.begin(&token).is_err());
        state.cancel("another job");
        assert!(!cancel.load(Ordering::Relaxed));
        state.cancel(&token);
        assert!(cancel.load(Ordering::Relaxed));
        state.finish(&token);
        assert!(state.begin(&token).is_err());
        let expired = state.dropped(vec!["/expired".into()]);
        state.pending.lock().unwrap().as_mut().unwrap().2 =
            Instant::now() - Duration::from_secs(121);
        assert!(state.begin(&expired).is_err());
    }
    #[test]
    #[cfg(unix)]
    fn cancellation_interrupts_a_blocked_protocol_without_waiting_for_output() {
        let cancel = Arc::new(Cancellation::default());
        let flag = cancel.clone();
        let killer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            flag.cancel();
        });
        let start = Instant::now();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30"]);
        let result = run(command, &cancel, |input, mut output, _| {
            drop(input);
            line(&mut output)
        });
        killer.join().unwrap();
        assert!(result.unwrap_err().contains("cancelled"));
        assert!(start.elapsed() < Duration::from_secs(3));
    }
    #[test]
    fn scp_errors_and_truncated_acknowledgements_are_errors() {
        assert!(ack(&mut std::io::Cursor::new(b"\x02Permission denied\n"))
            .unwrap_err()
            .contains("Permission denied"));
        assert!(ack(&mut std::io::Cursor::new(b"")).is_err());
        assert!(line(&mut std::io::Cursor::new(vec![b'x'; 40_000])).is_err());
    }
}
