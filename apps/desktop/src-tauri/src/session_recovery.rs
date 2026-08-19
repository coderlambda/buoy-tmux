//! Tmux lifecycle helpers used by explicit Close/Resume and "Check open sessions".
//!
//! Detach never comes through this module: it only drops the local client. Close takes an
//! authoritative per-window snapshot over a short, separate SSH exec, then kills the tmux
//! session. Resume reconstructs that session before the normal control client attaches.

use std::collections::BTreeMap;
use std::process::{Command, Output};

use crate::session_store::{RecoveryTab, SessionMeta};
use crate::validation::{base64_decode, base64_encode, parse_host, validate_session};

#[derive(Debug, Clone)]
pub struct CommandHint {
    pub window: String,
    pub title: String,
    pub last_command: String,
}

fn socket(meta: &SessionMeta) -> String {
    meta.socket_name.clone().unwrap_or_else(|| {
        crate::tmux_socket::socket_name(&meta.mode, meta.tmux_version, &meta.session)
    })
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn validate(meta: &SessionMeta) -> Result<(), String> {
    validate_session(&meta.session).map_err(|error| error.to_string())?;
    let path = meta.tmux_path.as_deref().unwrap_or("tmux");
    if !crate::validation::is_safe_tmux_path(path) {
        return Err("invalid persisted tmux path".into());
    }
    Ok(())
}

fn run_script(meta: &SessionMeta, script: &str) -> Result<Output, String> {
    validate(meta)?;
    if meta.transport == "local" {
        return Command::new("/bin/sh")
            .args(["-c", script])
            .env("PATH", crate::augmented_path())
            .output()
            .map_err(|error| format!("local tmux command failed: {error}"));
    }
    let host = parse_host(&meta.host).map_err(|error| error.to_string())?;
    let target = match host.user {
        Some(user) => format!("{user}@{}", host.host),
        None => host.host,
    };
    let encoded = base64_encode(script.as_bytes());
    let remote = format!("printf %s {encoded} | base64 -d | /bin/sh");
    let mut args = Vec::new();
    if let Some(port) = host.port {
        args.extend(["-p".to_string(), port.to_string()]);
    }
    args.extend([
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ConnectTimeout=8".into(),
        "--".into(),
        target,
        remote,
    ]);
    Command::new("ssh")
        .args(args)
        .env("PATH", crate::augmented_path())
        .output()
        .map_err(|error| format!("SSH recovery command failed: {error}"))
}

fn field(value: &str) -> String {
    base64_decode(value)
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .unwrap_or_default()
}

// `has-session` also fails for authentication, permission, and missing-binary errors. Only the
// known tmux missing-session/socket messages mean the remote session is already closed.
const CHECK_SESSION: &str = r#"
check_session() {
  if error=$(LC_ALL=C "$t" -L "$s" has-session -t "=$n" 2>&1); then return 0; fi
  case "$error" in
    "can't find session: "*|"no server running on "*|"error connecting to "*" (No such file or directory)") return 44 ;;
  esac
  printf '%s\n' "$error" >&2
  return 1
}
"#;

fn cached_tabs(meta: &SessionMeta, hints: &[CommandHint]) -> Vec<RecoveryTab> {
    let mut tabs = meta.recovery_tabs.clone();
    for hint in hints {
        if let Some(tab) = tabs.iter_mut().find(|tab| tab.window == hint.window) {
            if !hint.title.is_empty() { tab.title = hint.title.clone(); }
            if !hint.last_command.is_empty() { tab.last_command = hint.last_command.clone(); }
        } else {
            tabs.push(RecoveryTab {
                window: hint.window.clone(), title: hint.title.clone(),
                cwd: String::new(), shell: String::new(), last_command: hint.last_command.clone(),
            });
        }
    }
    tabs
}

/// Snapshot all tmux windows and kill the remote session only after the snapshot command succeeds.
/// An already missing session is closed successfully using the previously saved recovery hints.
pub fn snapshot_and_kill(
    meta: &SessionMeta,
    hints: &[CommandHint],
) -> Result<Vec<RecoveryTab>, String> {
    let tmux = meta.tmux_path.as_deref().unwrap_or("tmux");
    let socket = socket(meta);
    let session = &meta.session;
    let script = format!(
        "t={}; s={}; n={}; {CHECK_SESSION} \
         check_session || exit $?; \
         \"$t\" -L \"$s\" list-windows -t \"=$n\" -F '#{{window_id}}' | while IFS= read -r w; do \
           title=$(\"$t\" -L \"$s\" display-message -p -t \"$w\" '#{{window_name}}'); \
           cwd=$(\"$t\" -L \"$s\" display-message -p -t \"$w\" '#{{pane_current_path}}'); \
           pid=$(\"$t\" -L \"$s\" display-message -p -t \"$w\" '#{{pane_pid}}'); \
           shell=$(ps -p \"$pid\" -o comm= 2>/dev/null | sed 's/^[[:space:]]*//;s/[[:space:]]*$//'); \
           printf '%s\\t' \"$w\"; \
           printf %s \"$title\" | base64 | tr -d '\\n'; printf '\\t'; \
           printf %s \"$cwd\" | base64 | tr -d '\\n'; printf '\\t'; \
           printf %s \"$shell\" | base64 | tr -d '\\n'; printf '\\n'; \
         done; \
         \"$t\" -L \"$s\" kill-session -t \"=$n\" || {{ \
           check_session; code=$?; [ \"$code\" -eq 44 ] && exit 0; exit 1; \
         }}",
        shell_quote(tmux), shell_quote(&socket), shell_quote(session),
    );
    let output = run_script(meta, &script)?;
    if output.status.code() == Some(44) {
        return Ok(cached_tabs(meta, hints));
    }
    if !output.status.success() {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if message.is_empty() {
            format!(
                "could not snapshot and close tmux (status {:?})",
                output.status.code()
            )
        } else {
            message
        });
    }
    let hints: BTreeMap<_, _> = hints
        .iter()
        .map(|hint| (hint.window.as_str(), hint))
        .collect();
    let mut tabs = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut fields = line.splitn(4, '\t');
        let Some(window) = fields.next().filter(|window| window.starts_with('@')) else {
            continue;
        };
        let title = field(fields.next().unwrap_or_default());
        let cwd = field(fields.next().unwrap_or_default());
        let shell = field(fields.next().unwrap_or_default());
        let hint = hints.get(window).copied().or_else(|| {
            (hints.len() == 1)
                .then(|| hints.values().next().copied())
                .flatten()
        });
        tabs.push(RecoveryTab {
            window: window.to_string(),
            title: if title.is_empty() {
                hint.map(|hint| hint.title.clone()).unwrap_or_default()
            } else {
                title
            },
            cwd,
            shell,
            last_command: hint
                .map(|hint| hint.last_command.clone())
                .unwrap_or_default(),
        });
    }
    if tabs.is_empty() {
        return Err("tmux closed but its window snapshot was empty".into());
    }
    Ok(tabs)
}

/// Recreate a previously closed tmux session, including one window per saved tab and its cwd.
/// Bash/zsh panes also receive a non-executing history insertion so Up recalls that tab's last
/// command. The command is added to shell history; it is never executed by the recovery routine.
pub fn restore(meta: &SessionMeta) -> Result<(), String> {
    if meta.recovery_tabs.is_empty() {
        return Ok(());
    }
    let tmux = meta.tmux_path.as_deref().unwrap_or("tmux");
    let socket = socket(meta);
    let session = &meta.session;
    let mut script = format!(
        "t={}; s={}; n={}; if \"$t\" -L \"$s\" has-session -t \"$n\" 2>/dev/null; then exit 0; fi; ",
        shell_quote(tmux), shell_quote(&socket), shell_quote(session),
    );
    for (index, tab) in meta.recovery_tabs.iter().enumerate() {
        let cwd = if tab.cwd.is_empty() {
            "$HOME".to_string()
        } else {
            shell_quote(&tab.cwd)
        };
        let title = if tab.title.is_empty() {
            "shell"
        } else {
            &tab.title
        };
        if index == 0 {
            script.push_str(&format!(
                "\"$t\" -L \"$s\" new-session -d -s \"$n\" -n {} -c {}; ",
                shell_quote(title),
                cwd,
            ));
        } else {
            script.push_str(&format!(
                "\"$t\" -L \"$s\" new-window -d -t \"$n\" -n {} -c {}; ",
                shell_quote(title),
                cwd,
            ));
        }
    }
    script.push_str("sleep 0.2; ");
    for (index, tab) in meta.recovery_tabs.iter().enumerate() {
        if tab.last_command.is_empty() {
            continue;
        }
        let history = if tab.shell.to_ascii_lowercase().contains("zsh") {
            format!(" print -s -- {}", shell_quote(&tab.last_command))
        } else {
            format!(" history -s -- {}", shell_quote(&tab.last_command))
        };
        script.push_str(&format!(
            "\"$t\" -L \"$s\" send-keys -t \"$n\":{} -l -- {}; \
             \"$t\" -L \"$s\" send-keys -t \"$n\":{} Enter; ",
            index,
            shell_quote(&history),
            index,
        ));
    }
    let active = meta
        .last_tab
        .as_ref()
        .and_then(|window| {
            meta.recovery_tabs
                .iter()
                .position(|tab| &tab.window == window)
        })
        .unwrap_or(0);
    script.push_str(&format!(
        "\"$t\" -L \"$s\" select-window -t \"$n\":{active}"
    ));
    let output = run_script(meta, &script)?;
    if output.status.success() {
        Ok(())
    } else {
        let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if message.is_empty() {
            "could not reconstruct tmux session".into()
        } else {
            message
        })
    }
}

pub fn is_open(meta: &SessionMeta) -> Result<bool, String> {
    let tmux = meta.tmux_path.as_deref().unwrap_or("tmux");
    let script = format!(
        "{} -L {} has-session -t {} 2>/dev/null",
        shell_quote(tmux),
        shell_quote(&socket(meta)),
        shell_quote(&meta.session),
    );
    let output = run_script(meta, &script)?;
    Ok(output.status.success())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_meta(name: &str, tmux_path: &str) -> SessionMeta {
        serde_json::from_value(serde_json::json!({
            "id": name, "host": "", "session": format!("dt-{name}"),
            "transport": "local", "mode": "control", "tmuxPath": tmux_path,
            "socketName": format!("buoy-{name}-{}", std::process::id()),
        })).unwrap()
    }

    #[test]
    fn close_missing_session_is_idempotent_and_does_not_kill_a_prefix_match() {
        let probe = crate::probe::probe_local_tmux();
        if probe.version.is_none() { return; }
        let mut meta = test_meta("missing-close", &probe.tmux_path);
        meta.recovery_tabs = vec![RecoveryTab {
            window: "@2".into(), title: "old title".into(), cwd: "/tmp".into(),
            shell: "zsh".into(), last_command: "pwd".into(),
        }];
        let hints = vec![CommandHint {
            window: "@2".into(), title: "shell".into(), last_command: "echo latest".into(),
        }];
        let tabs = snapshot_and_kill(&meta, &hints).unwrap();
        assert_eq!(tabs[0].cwd, "/tmp");
        assert_eq!(tabs[0].shell, "zsh");
        assert_eq!(tabs[0].title, "shell");
        assert_eq!(tabs[0].last_command, "echo latest");
        assert_eq!(snapshot_and_kill(&meta, &hints).unwrap(), tabs);
        assert!(!is_open(&meta).unwrap(), "closing must not create a new session");

        struct ServerCleanup(SessionMeta);
        impl Drop for ServerCleanup {
            fn drop(&mut self) {
                let _ = Command::new(self.0.tmux_path.as_ref().unwrap())
                    .args(["-L", &socket(&self.0), "kill-server"]).output();
            }
        }
        let _cleanup = ServerCleanup(meta.clone());
        let sibling = format!("{}-other", meta.session);
        assert!(Command::new(&probe.tmux_path)
            .args(["-L", &socket(&meta), "new-session", "-d", "-s", &sibling])
            .status().unwrap().success());
        assert_eq!(snapshot_and_kill(&meta, &hints).unwrap(), tabs);
        assert!(Command::new(&probe.tmux_path)
            .args(["-L", &socket(&meta), "has-session", "-t", &format!("={sibling}")])
            .status().unwrap().success(), "a similarly named live session must survive");
        meta.recovery_tabs.clear();
        assert!(snapshot_and_kill(&meta, &[]).unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn close_does_not_treat_command_or_permission_errors_as_a_missing_session() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("buoy-close-errors-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("tmux");
        let meta = test_meta("close-errors", path.to_str().unwrap());
        assert!(snapshot_and_kill(&meta, &[]).is_err(), "missing tmux binary is an error");
        for message in ["error connecting to /tmp/socket (Permission denied)", "Permission denied (publickey).", "unexpected tmux failure"] {
            std::fs::write(&path, format!("#!/bin/sh\nprintf '%s\\n' {} >&2\nexit 1\n", shell_quote(message))).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            assert!(snapshot_and_kill(&meta, &[]).unwrap_err().contains(message));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn shell_quote_handles_commands_without_making_them_executable_syntax() {
        assert_eq!(
            shell_quote("echo 'hi'; touch /tmp/no"),
            "'echo '\"'\"'hi'\"'\"'; touch /tmp/no'"
        );
    }

    #[test]
    fn local_tmux_close_and_resume_restores_windows_cwds_and_history() {
        let probe = crate::probe::probe_local_tmux();
        let Some(version) = probe.version else { return };
        let unique = format!("recovery{}", std::process::id());
        let session = format!("dt-{unique}");
        let root = std::env::temp_dir().join(format!("buoy-{unique}"));
        let one = root.join("one");
        let two = root.join("two");
        std::fs::create_dir_all(&one).unwrap();
        std::fs::create_dir_all(&two).unwrap();
        let meta = SessionMeta {
            id: unique,
            host: String::new(),
            session: session.clone(),
            transport: "local".into(),
            mode: "control".into(),
            tmux_path: Some(probe.tmux_path.clone()),
            tmux_version: Some(version),
            socket_name: None,
            title: Some("recovery".into()),
            order: 0,
            attach_ok: true,
            color: None,
            last_tab: None,
            tab_order: vec![],
            tab_colors: Default::default(),
            archived: false,
            archived_at: None,
            detached: false,
            recovery_tabs: vec![],
            restore_pending: false,
            recovery_windows: vec![],
        };
        let socket = socket(&meta);
        let status = Command::new(&probe.tmux_path)
            .args([
                "-L",
                &socket,
                "new-session",
                "-d",
                "-s",
                &session,
                "-n",
                "first",
                "-c",
                one.to_str().unwrap(),
            ])
            .status()
            .unwrap();
        assert!(status.success());
        assert!(Command::new(&probe.tmux_path)
            .args([
                "-L",
                &socket,
                "new-window",
                "-d",
                "-t",
                &session,
                "-n",
                "second",
                "-c",
                two.to_str().unwrap(),
            ])
            .status()
            .unwrap()
            .success());
        let windows = Command::new(&probe.tmux_path)
            .args([
                "-L",
                &socket,
                "list-windows",
                "-t",
                &session,
                "-F",
                "#{window_id}",
            ])
            .output()
            .unwrap();
        let ids: Vec<_> = String::from_utf8_lossy(&windows.stdout)
            .lines()
            .map(str::to_string)
            .collect();
        assert_eq!(ids.len(), 2);
        let hints = vec![
            CommandHint {
                window: ids[0].clone(),
                title: "first".into(),
                last_command: "echo first-history".into(),
            },
            CommandHint {
                window: ids[1].clone(),
                title: "second".into(),
                last_command: "echo second-history".into(),
            },
        ];
        let tabs = snapshot_and_kill(&meta, &hints).unwrap();
        assert_eq!(tabs.len(), 2);
        let one_real = one.canonicalize().unwrap();
        let two_real = two.canonicalize().unwrap();
        assert_eq!(tabs[0].cwd, one_real.to_string_lossy());
        assert_eq!(tabs[1].cwd, two_real.to_string_lossy());
        assert!(!is_open(&meta).unwrap());

        let restored = SessionMeta {
            recovery_tabs: tabs.clone(),
            restore_pending: true,
            ..meta.clone()
        };
        restore(&restored).unwrap();
        assert!(is_open(&restored).unwrap());
        let listing = Command::new(&probe.tmux_path)
            .args([
                "-L",
                &socket,
                "list-windows",
                "-t",
                &session,
                "-F",
                "#{pane_current_path}\t#{window_name}",
            ])
            .output()
            .unwrap();
        let listing = String::from_utf8_lossy(&listing.stdout);
        assert!(listing.contains(&format!("{}\tfirst", one_real.to_string_lossy())));
        assert!(listing.contains(&format!("{}\tsecond", two_real.to_string_lossy())));

        // The seed command is added to history but never executed. On common bash/zsh panes, Up
        // therefore recalls it while no command output ("first-history") has appeared beforehand.
        if tabs[0].shell.contains("bash") || tabs[0].shell.contains("zsh") {
            std::thread::sleep(std::time::Duration::from_millis(350));
            let _ = Command::new(&probe.tmux_path)
                .args([
                    "-L",
                    &socket,
                    "send-keys",
                    "-t",
                    &format!("{session}:0"),
                    "Up",
                ])
                .status();
            std::thread::sleep(std::time::Duration::from_millis(100));
            let pane = Command::new(&probe.tmux_path)
                .args([
                    "-L",
                    &socket,
                    "capture-pane",
                    "-p",
                    "-t",
                    &format!("{session}:0"),
                ])
                .output()
                .unwrap();
            assert!(String::from_utf8_lossy(&pane.stdout).contains("echo first-history"));
        }
        let _ = Command::new(&probe.tmux_path)
            .args(["-L", &socket, "kill-session", "-t", &session])
            .status();
        let _ = std::fs::remove_dir_all(&root);
    }
}
