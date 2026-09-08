//! On-demand `ssh -L` port forwarding for localhost URLs in remote output (DESIGN.md §18).
//! A URL like http://localhost:3000 points at the REMOTE host's loopback; to open it in the local
//! browser we forward a free LOCAL port to the remote's localhost:<port> and hand the browser the
//! local URL. Tunnels are separate ssh processes (never the -CC channel), tracked per session and
//! reused across clicks to the same remote port, and torn down when the session closes.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::validation::{parse_host, ValidationError};

/// One forwarded port's status for the sidebar: the remote port, its local port if a tunnel is
/// currently open, and whether the forward is ACTIVE (something is really answering on the remote).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TunnelStatus {
    pub remote: u16,
    pub local: Option<u16>,
    pub active: bool,
    pub scheme: TunnelScheme,
}
#[derive(Debug, Default, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TunnelScheme { #[default] Http, Https }
impl TunnelScheme {
    pub fn as_str(self) -> &'static str { match self { Self::Http => "http", Self::Https => "https" } }
}

/// A parsed loopback URL: the remote port to reach and the path (incl. query) to open.
#[derive(Debug, PartialEq)]
pub struct LoopbackUrl {
    pub scheme: TunnelScheme,
    pub port: u16,
    pub path: String, // begins with '/', includes any ?query; '' -> "/"
}

/// Parse a clicked URL into (host, LoopbackUrl) when it targets a loopback host in `loopback_hosts`.
/// Accepts `host:port`, `http://host:port/path`, `https://…`. Returns None for non-loopback or
/// unparseable input. The returned host is the matched loopback token (for logging only).
pub fn classify_loopback(url: &str, loopback_hosts: &[String]) -> Option<(String, LoopbackUrl)> {
    // Strip scheme if present.
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);
    // authority is up to the first '/', '?' or end.
    let auth_end = rest.find(|c| c == '/' || c == '?' || c == '#').unwrap_or(rest.len());
    let authority = &rest[..auth_end];
    let mut tail = &rest[auth_end..];
    if tail.is_empty() { tail = "/"; }

    // authority = host:port
    let (host, port_str) = authority.rsplit_once(':')?;
    if !loopback_hosts.iter().any(|h| h == host) {
        return None;
    }
    let port: u16 = port_str.parse().ok()?;
    if port == 0 { return None; }

    // sanitize path: keep only a leading-'/' path + query, reject control/space (untrusted text).
    let path = if tail.starts_with('/') { tail.to_string() } else { format!("/{}", tail) };
    if path.chars().any(|c| c.is_control() || c == ' ') {
        return None;
    }
    Some((host.to_string(), LoopbackUrl { port, path, scheme: if url.starts_with("https://") { TunnelScheme::Https } else { TunnelScheme::Http } }))
}

/// Pick a free local TCP port by binding to 127.0.0.1:0 and reading the assigned port. There's an
/// inherent TOCTOU gap (the port could be taken before ssh binds it) but it's tiny and ssh's
/// ExitOnForwardFailure surfaces a collision as a spawn we can detect.
pub fn free_local_port() -> std::io::Result<u16> {
    let l = TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}

/// Is this local port bindable right now? The same check ssh's own listener makes, surfaced early so
/// we can fall back instead of spawning an `ssh -L` that immediately dies on
/// `ExitOnForwardFailure=yes`.
fn local_port_free(port: u16) -> bool {
    port != 0 && TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// Choose the LOCAL port for a forward, PREFERRING the port this (session, remote port) was last
/// forwarded on. Stickiness is the point: a forwarded URL the user already has open in a browser tab
/// — or bookmarked, or pasted into a config — names one specific `localhost:<local>`, so handing out
/// a fresh random port after every reconnect silently breaks every page pointing at the old one.
/// The remembered port is abandoned only if something else now holds it.
fn pick_local_port(sticky: Option<u16>) -> std::io::Result<u16> {
    if let Some(p) = sticky.filter(|p| *p != 0) {
        if local_port_free(p) { return Ok(p); }
        crate::dlog!("tunnel: sticky local port {} is taken, falling back to a fresh one", p);
    }
    free_local_port()
}

/// Build the `ssh` argv for a forward. Split out from `spawn_tunnel` so the flag that actually
/// decides the user-visible URL — `-L 127.0.0.1:<local>:localhost:<remote>` — can be asserted without
/// spawning ssh. Returning the right local port from `ensure()` means nothing if the argv that gets
/// executed carries a different one.
fn tunnel_argv(host: &str, local_port: u16, remote_port: u16, base_args: &[String])
    -> Result<Vec<String>, String>
{
    let parts = parse_host(host).map_err(|e: ValidationError| e.to_string())?;
    let target = match &parts.user {
        Some(u) => format!("{}@{}", u, parts.host),
        None => parts.host.clone(),
    };

    let mut args: Vec<String> = Vec::new();
    if let Some(p) = parts.port { args.push("-p".into()); args.push(p.to_string()); }
    args.extend([
        "-o".into(), "BatchMode=yes".into(),
        "-o".into(), "ExitOnForwardFailure=yes".into(),
        "-o".into(), "ConnectTimeout=8".into(),
        "-o".into(), "ServerAliveInterval=10".into(),
        "-o".into(), "ServerAliveCountMax=2".into(),
        // This registry owns and checks the child. Do not let user multiplex/background settings
        // move the listener into another process whose lifetime we cannot control.
        "-o".into(), "ControlMaster=no".into(),
        "-o".into(), "ControlPath=none".into(),
        "-o".into(), "ForkAfterAuthentication=no".into(),
        "-N".into(),
        // bind the LOCAL side to 127.0.0.1 so only this machine can use the forward.
        "-L".into(), format!("127.0.0.1:{}:localhost:{}", local_port, remote_port),
    ]);
    args.extend(base_args.iter().cloned());
    args.push("--".into());
    args.push(target);
    Ok(args)
}

struct Tunnel {
    local_port: u16,
    remote_port: u16,
    pid: u32,
    // Some when WE spawned it this run (lets us reap the whole process group via Child); None when
    // ADOPTED from a previous run's orphan (we only know its pid, killed via `kill <pid>`).
    child: Option<Child>,
}

impl Tunnel {
    fn kill(&mut self) {
        match self.child.as_mut() {
            Some(c) => { let _ = c.kill(); let _ = c.wait(); }
            None if is_our_ssh_pid(self.pid, self.local_port, self.remote_port) => {
                let _ = Command::new("kill").arg(self.pid.to_string()).status();
                let deadline = Instant::now() + Duration::from_secs(1);
                while is_our_ssh_pid(self.pid, self.local_port, self.remote_port) && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
            None => {}
        }
    }
    // Alive if our Child hasn't exited, or (adopted) the pid is still one of our ssh -L procs.
    fn alive(&mut self) -> bool {
        match self.child.as_mut() {
            Some(c) => matches!(c.try_wait(), Ok(None)),
            None => is_our_ssh_pid(self.pid, self.local_port, self.remote_port),
        }
    }
}

/// Probe a local forwarded port: connect + send a minimal HTTP HEAD and see if we get ANY bytes
/// back. A plain TCP connect is NOT enough — ssh's local listener accepts before forwarding, so a
/// dead remote port still "connects" (verified). Only an actual round-trip distinguishes them.
pub fn probe_local_port(local_port: u16) -> bool {
    probe_forward(local_port, local_port, TunnelScheme::Http)
}

fn probe_forward(local_port: u16, remote_port: u16, scheme: TunnelScheme) -> bool {
    let addr = format!("127.0.0.1:{}", local_port);
    let stream = TcpStream::connect_timeout(
        &addr.parse().unwrap(), Duration::from_millis(600));
    let mut s = match stream { Ok(s) => s, Err(_) => return false };
    let _ = s.set_read_timeout(Some(Duration::from_millis(700)));
    let _ = s.set_write_timeout(Some(Duration::from_millis(500)));
    if scheme == TunnelScheme::Https {
        // Reachability only: dev servers commonly use self-signed certificates. This handshake
        // carries no credentials/content; the browser still performs its normal certificate checks.
        let connector = match native_tls::TlsConnector::builder()
            .danger_accept_invalid_certs(true).danger_accept_invalid_hostnames(true).build() {
            Ok(connector) => connector, Err(_) => return false,
        };
        return connector.connect("localhost", s).is_ok();
    }
    // A TCP accept only proves ssh's local listener exists. Require a remote response, including
    // 4xx/5xx: availability of the user's application is separate from a healthy forwarding path.
    let request = format!("HEAD / HTTP/1.1\r\nHost: localhost:{}\r\nConnection: close\r\n\r\n", remote_port);
    if s.write_all(request.as_bytes()).is_err() { return false; }
    let mut buf = [0u8; 16];
    matches!(s.read(&mut buf), Ok(n) if n > 0)
}

/// Per-session set of live tunnels, keyed by remote port (so repeat clicks reuse one). Also
/// persists the set of remote ports per session to disk, so after an app restart the sidebar can
/// show the (inactive) rows and the user can re-open them.
// Persisted per-port record: the remote port, the LOCAL port it was forwarded on, and the PID of
// the ssh -L that forwarded it (0 if not currently open). On the next launch we ADOPT a still-alive
// orphan (same pid still forwarding our local->remote) instead of leaking it — so ports are reused,
// not duplicated. We only kill on explicit close or app exit.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct PortRec { remote: u16, #[serde(default)] local: u16, #[serde(default)] pid: u32, #[serde(default)] scheme: TunnelScheme }

type Persisted = BTreeMap<String, Vec<PortRec>>;

pub struct TunnelRegistry {
    operations: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    // session id -> (remote_port -> Tunnel)
    by_session: Mutex<HashMap<String, HashMap<u16, Tunnel>>>,
    // session id -> persisted port records (remote port + last ssh -L pid); source of truth for
    // the sidebar list AND for reaping orphaned tunnels on the next launch.
    persisted: Mutex<Persisted>,
    store_path: Option<PathBuf>,
}

impl Default for TunnelRegistry {
    fn default() -> Self { Self::new() }
}

impl TunnelRegistry {
    pub fn new() -> Self {
        TunnelRegistry {
            operations: Mutex::new(HashMap::new()),
            by_session: Mutex::new(HashMap::new()),
            persisted: Mutex::new(BTreeMap::new()),
            store_path: None,
        }
    }

    /// Registry backed by a JSON file at `path` (tunnels.json); loads persisted ports and ADOPTS
    /// any still-alive orphaned ssh -L from a previous run (same pid still forwarding its local
    /// port) so those tunnels are REUSED, not leaked/duplicated. A recorded pid that's no longer
    /// one of our ssh -L procs is just cleared (its row shows inactive, re-openable). We never kill
    /// on startup — only on explicit close or app exit.
    pub fn with_store(path: PathBuf) -> Self {
        let mut persisted: Persisted = std::fs::read_to_string(&path).ok()
            .and_then(|s| serde_json::from_str::<Persisted>(&s).ok())
            .unwrap_or_default();
        let mut adopted: HashMap<String, HashMap<u16, Tunnel>> = HashMap::new();
        for (sid, recs) in persisted.iter_mut() {
            for r in recs.iter_mut() {
                if r.pid != 0 && r.local != 0 && is_our_ssh_pid(r.pid, r.local, r.remote) {
                    // Orphan is still alive and forwarding — adopt it (reuse across restarts).
                    crate::dlog!("tunnel: adopting orphan pid={} local {} -> remote {} for {}", r.pid, r.local, r.remote, sid);
                    adopted.entry(sid.clone()).or_default()
                        .insert(r.remote, Tunnel { local_port: r.local, remote_port: r.remote, pid: r.pid, child: None });
                } else {
                    r.pid = 0;   // gone -> row shows inactive, user can re-open
                }
            }
        }
        let reg = TunnelRegistry {
            operations: Mutex::new(HashMap::new()),
            by_session: Mutex::new(adopted),
            persisted: Mutex::new(persisted),
            store_path: Some(path),
        };
        let snapshot = reg.persisted.lock().unwrap().clone();
        reg.save_persisted(&snapshot);
        reg
    }

    fn save_persisted(&self, map: &Persisted) {
        if let Some(path) = &self.store_path {
            if let Some(dir) = path.parent() { let _ = std::fs::create_dir_all(dir); }
            if let Ok(json) = serde_json::to_string_pretty(map) {
                let tmp = path.with_extension("json.tmp");
                if std::fs::write(&tmp, json).is_ok() { let _ = std::fs::rename(&tmp, path); }
            }
        }
    }

    /// Record (or update) a forwarded port's local port + pid; persist (so a later run can adopt it).
    fn remember(&self, session_id: &str, remote_port: u16, local_port: u16, pid: u32) {
        let mut p = self.persisted.lock().unwrap();
        let recs = p.entry(session_id.to_string()).or_default();
        match recs.iter_mut().find(|r| r.remote == remote_port) {
            Some(r) => { r.local = local_port; r.pid = pid; }
            None => recs.push(PortRec { remote: remote_port, local: local_port, pid, scheme: TunnelScheme::Http }),
        }
        recs.sort_by_key(|r| r.remote);
        self.save_persisted(&p);
    }

    /// The LOCAL port this (session, remote port) was last forwarded on, if we've ever forwarded it.
    /// Survives both a reconnect (the record outlives the ssh child) and an app restart (it's on
    /// disk), which is what makes the local port stable rather than per-connection.
    fn remembered_local(&self, session_id: &str, remote_port: u16) -> Option<u16> {
        self.persisted.lock().unwrap().get(session_id)
            .and_then(|recs| recs.iter().find(|r| r.remote == remote_port))
            .map(|r| r.local).filter(|l| *l != 0)
    }

    fn forget(&self, session_id: &str, remote_port: u16) {
        let mut p = self.persisted.lock().unwrap();
        if let Some(recs) = p.get_mut(session_id) {
            recs.retain(|r| r.remote != remote_port);
            if recs.is_empty() { p.remove(session_id); }
        }
        self.save_persisted(&p);
    }

    /// The persisted+live status of a session's forwarded ports, for the sidebar. Each remote port
    /// (persisted or currently live) is probed: `active` is true only if the forward really answers.
    /// Reuses a live tunnel's local port; a persisted-but-not-open port has `local: None, active:false`.
    pub fn status(&self, session_id: &str) -> Vec<TunnelStatus> {
        // union of persisted ports and currently-live ones
        let persisted: Vec<u16> = self.persisted.lock().unwrap()
            .get(session_id).map(|recs| recs.iter().map(|r| r.remote).collect()).unwrap_or_default();
        let live: Vec<(u16, u16)> = self.list(session_id); // prunes dead ssh
        let live_map: HashMap<u16, u16> = live.iter().cloned().collect();

        let mut ports: Vec<u16> = persisted.clone();
        for (rp, _) in &live { if !ports.contains(rp) { ports.push(*rp); } }
        ports.sort();

        ports.into_iter().map(|remote| {
            let local = live_map.get(&remote).copied();
            let scheme = self.remembered_scheme(session_id, remote);
            let active = match local { Some(lp) => probe_forward(lp, remote, scheme), None => false };
            TunnelStatus { remote, local, active, scheme }
        }).collect()
    }

    /// Ensure a tunnel exists for (session, remote_port); return the LOCAL port. Reuses a live one;
    /// otherwise re-opens on the SAME local port this remote port was last forwarded on (see
    /// `pick_local_port`), falling back to a fresh one only if that port is now taken.
    pub fn ensure(&self, session_id: &str, host: &str, remote_port: u16, base_args: &[String])
        -> Result<u16, String>
    {
        let operation = self.operation(session_id);
        let _guard = operation.lock().unwrap();
        self.ensure_locked(session_id, host, remote_port, base_args)
    }

    fn ensure_locked(&self, session_id: &str, host: &str, remote_port: u16, base_args: &[String]) -> Result<u16, String> {
        // Reuse a live tunnel.
        {
            let mut map = self.by_session.lock().unwrap();
            if let Some(per) = map.get_mut(session_id) {
                if let Some(t) = per.get_mut(&remote_port) {
                    if t.alive() { return Ok(t.local_port); }   // reuse (incl. adopted orphan)
                    let mut dead = per.remove(&remote_port).unwrap(); dead.kill();
                }
            }
        }
        // Re-open on the remembered local port so URLs already handed to the browser keep working.
        // NOTE: the dead tunnel above was just killed, so its old local port is free again — this is
        // exactly the reconnect case, and re-picking randomly here was the bug.
        let sticky = self.remembered_local(session_id, remote_port);
        let local_port = pick_local_port(sticky).map_err(|e| e.to_string())?;
        self.spawn_tunnel(session_id, host, local_port, remote_port, base_args)
    }

    /// Re-establish every tunnel this session is supposed to have, on their ORIGINAL local ports.
    /// Called when the session's connection comes back (§18): the tunnels are separate ssh processes
    /// that die with the network, and nothing else would bring them back — the user would be left
    /// with greyed-out rows and dead browser tabs until they clicked each port again.
    ///
    /// Driven off the PERSISTED record, not the live map, precisely because the live entries are the
    /// ones that just died. Already-alive tunnels are left alone (`ensure` reuses them), so this is
    /// safe to call on every reconnect. Returns the (remote, local) pairs now forwarding.
    pub fn reestablish(&self, session_id: &str, host: &str, base_args: &[String]) -> Vec<(u16, u16)> {
        let operation = self.operation(session_id);
        let _guard = operation.lock().unwrap();
        let wanted: Vec<u16> = self.persisted.lock().unwrap()
            .get(session_id).map(|recs| recs.iter().map(|r| r.remote).collect()).unwrap_or_default();
        let mut out = Vec::new();
        for remote in wanted {
            match self.ensure_locked(session_id, host, remote, base_args) {
                Ok(local) => {
                    crate::dlog!("tunnel: reestablished remote {} on local {} for {}", remote, local, session_id);
                    out.push((remote, local));
                }
                // One port failing (remote server gone, local port stolen) must not stop the others.
                Err(e) => crate::dlog!("tunnel: reestablish remote {} for {} failed: {}", remote, session_id, e),
            }
        }
        out.sort_by_key(|(r, _)| *r);
        out
    }

    /// Production reconnect path: repair half-open SSH children too. Check cancellation after
    /// acquiring the same lock used by close/detach so a stale reconnect worker cannot revive them.
    pub fn restore_ready_if(&self, session_id: &str, host: &str, base_args: &[String], current: impl Fn() -> bool) -> Vec<(u16, u16)> {
        let operation = self.operation(session_id);
        let _guard = operation.lock().unwrap();
        let wanted = self.persisted.lock().unwrap().get(session_id).cloned().unwrap_or_default();
        let mut restored = Vec::new();
        for rec in wanted {
            if !current() { break; }
            match self.ensure_ready_locked(session_id, host, rec.remote, rec.scheme, false, base_args) {
                Ok(local) => restored.push((rec.remote, local)),
                Err(error) => crate::dlog!("tunnel: restore {} for {}: {}", rec.remote, session_id, error),
            }
        }
        restored
    }

    /// FORCE a tunnel that binds the LOCAL side to the SAME port number as the remote (so a remote
    /// localhost:3000 becomes localhost:3000 locally — matches apps that hardcode their port in
    /// redirects). Errors if that local port is already in use (the UI alerts). Replaces any
    /// existing tunnel for this remote port.
    pub fn force_same_port(&self, session_id: &str, host: &str, remote_port: u16, base_args: &[String])
        -> Result<u16, String>
    {
        let operation = self.operation(session_id);
        let _guard = operation.lock().unwrap();
        let scheme = self.remembered_scheme(session_id, remote_port);
        self.ensure_ready_locked(session_id, host, remote_port, scheme, true, base_args)
    }

    fn operation(&self, session_id: &str) -> Arc<Mutex<()>> {
        self.operations.lock().unwrap().entry(session_id.into()).or_default().clone()
    }

    fn remembered_scheme(&self, session_id: &str, remote: u16) -> TunnelScheme {
        self.persisted.lock().unwrap().get(session_id)
            .and_then(|recs| recs.iter().find(|r| r.remote == remote))
            .map(|r| r.scheme).unwrap_or_default()
    }

    fn set_scheme(&self, session_id: &str, remote: u16, scheme: TunnelScheme) {
        let mut persisted = self.persisted.lock().unwrap();
        if let Some(rec) = persisted.get_mut(session_id).and_then(|recs| recs.iter_mut().find(|r| r.remote == remote)) {
            rec.scheme = scheme;
        }
        self.save_persisted(&persisted);
    }

    /// Verify an end-to-end response, repairing a stale child before returning a browser URL.
    /// All opens, automatic restores and remaps for a session share the same operation lock.
    pub fn ensure_ready(&self, session_id: &str, host: &str, remote: u16, scheme: TunnelScheme, base_args: &[String]) -> Result<u16, String> {
        let operation = self.operation(session_id);
        let _guard = operation.lock().unwrap();
        self.ensure_ready_locked(session_id, host, remote, scheme, false, base_args)
    }

    fn ensure_ready_locked(&self, session_id: &str, host: &str, remote: u16, scheme: TunnelScheme, same: bool, base_args: &[String]) -> Result<u16, String> {
        if remote == 0 { return Err("invalid remote port".into()); }
        let previous = self.list(session_id).into_iter().find(|(rp, _)| *rp == remote).map(|(_, lp)| lp);
        if let Some(local) = previous {
            if (!same || local == remote) && probe_forward(local, remote, scheme) {
                self.set_scheme(session_id, remote, scheme);
                return Ok(local);
            }
        }
        if same && previous != Some(remote) && !local_port_free(remote) {
            return Err(format!("local port {} is already in use", remote));
        }
        // A remap can be tested beside the old forward. Only retire it after the replacement
        // works. Repairing on the same local port necessarily closes the stale child first.
        if !same || previous == Some(remote) {
            if let Some(mut old) = self.by_session.lock().unwrap().get_mut(session_id).and_then(|per| per.remove(&remote)) {
                old.kill();
            }
        }
        let local = if same { remote } else { pick_local_port(self.remembered_local(session_id, remote)).map_err(|e| e.to_string())? };
        let mut candidate = self.spawn_child(host, local, remote, base_args)?;
        if let Err(error) = wait_ready(&mut candidate, scheme, Duration::from_secs(10)) {
            candidate.kill();
            // Preserve existing mappings, and keep a new failed row so it can be retried.
            if self.remembered_local(session_id, remote).is_none() { self.remember(session_id, remote, local, 0); }
            self.set_scheme(session_id, remote, scheme);
            return Err(error);
        }
        self.install(session_id, candidate);
        self.set_scheme(session_id, remote, scheme);
        Ok(local)
    }

    fn spawn_child(&self, host: &str, local_port: u16, remote_port: u16, base_args: &[String]) -> Result<Tunnel, String> {
        let args = tunnel_argv(host, local_port, remote_port, base_args)?;
        let child = Command::new("ssh").args(&args).env("PATH", crate::augmented_path())
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null()).spawn()
            .map_err(|e| format!("ssh -L failed to start: {}", e))?;
        Ok(Tunnel { local_port, remote_port, pid: child.id(), child: Some(child) })
    }

    fn install(&self, session_id: &str, tunnel: Tunnel) {
        let (remote, local, pid) = (tunnel.remote_port, tunnel.local_port, tunnel.pid);
        if let Some(mut old) = self.by_session.lock().unwrap().entry(session_id.into()).or_default().insert(remote, tunnel) { old.kill(); }
        self.remember(session_id, remote, local, pid);
    }

    fn spawn_tunnel(&self, session_id: &str, host: &str, local_port: u16, remote_port: u16, base_args: &[String]) -> Result<u16, String> {
        let child = self.spawn_child(host, local_port, remote_port, base_args)?;
        self.install(session_id, child);
        Ok(local_port)
    }

    /// List a session's live tunnels as (remote_port, local_port), pruning any whose ssh died.
    /// Sorted by remote port for stable display.
    pub fn list(&self, session_id: &str) -> Vec<(u16, u16)> {
        let mut map = self.by_session.lock().unwrap();
        let per = match map.get_mut(session_id) { Some(p) => p, None => return Vec::new() };
        // prune dead ones (ssh exited) in one mutable pass, collecting the live ones.
        let mut dead: Vec<u16> = Vec::new();
        let mut out: Vec<(u16, u16)> = Vec::new();
        for (rp, t) in per.iter_mut() {
            if t.alive() { out.push((*rp, t.local_port)); }
            else { dead.push(*rp); }
        }
        for rp in dead { if let Some(mut t) = per.remove(&rp) { t.kill(); } }
        out.sort_by_key(|(rp, _)| *rp);
        out
    }

    /// Close ONE tunnel (by remote port) for a session AND forget it from disk — this is the user
    /// explicitly dismissing the row, so it should not reappear on next launch. Returns true if a
    /// live tunnel was killed (also succeeds/forgets for a persisted-but-not-open port).
    pub fn close(&self, session_id: &str, remote_port: u16) -> bool {
        let operation = self.operation(session_id);
        let _guard = operation.lock().unwrap();
        let killed = {
            let mut map = self.by_session.lock().unwrap();
            match map.get_mut(session_id).and_then(|per| per.remove(&remote_port)) {
                Some(mut t) => { t.kill();
                    crate::dlog!("tunnel: closed local {} (remote {}) for {}", t.local_port, remote_port, session_id);
                    true }
                None => false,
            }
        };
        self.forget(session_id, remote_port);
        killed
    }

    /// Tear down all LIVE tunnels for a session but KEEP the persisted port list (detach: the
    /// session survives, so its ports should still show — inactive — on next launch/reconnect).
    /// Clears the persisted pids (the children are dead now).
    pub fn close_session(&self, session_id: &str) {
        let operation = self.operation(session_id);
        let _guard = operation.lock().unwrap();
        self.close_session_locked(session_id);
    }

    fn close_session_locked(&self, session_id: &str) {
        if let Some(mut per) = self.by_session.lock().unwrap().remove(session_id) {
            for (_, mut t) in per.drain() {
                t.kill();
                crate::dlog!("tunnel: closed local {} (remote {})", t.local_port, t.remote_port);
            }
        }
        let mut p = self.persisted.lock().unwrap();
        // Clear the pid only — `local` is deliberately KEPT so a reattach re-opens on the same local
        // port (see pick_local_port), rather than inventing a new one and breaking open browser tabs.
        if let Some(recs) = p.get_mut(session_id) { for r in recs.iter_mut() { r.pid = 0; } }
        self.save_persisted(&p);
    }

    /// Kill a session for good: tear down tunnels AND forget its persisted ports.
    pub fn forget_session(&self, session_id: &str) {
        let operation = self.operation(session_id);
        let _guard = operation.lock().unwrap();
        self.close_session_locked(session_id);
        let mut p = self.persisted.lock().unwrap();
        p.remove(session_id);
        self.save_persisted(&p);
    }

}

/// Is `pid` still one of OUR ssh -L forward processes? Checks the live process's command line
/// (via `ps`) so a dead pid — or a pid recycled by an unrelated process — is not mistaken for our
/// tunnel (used both to adopt live orphans and to decide an adopted tunnel is still alive).
fn is_our_ssh_pid(pid: u32, local: u16, remote: u16) -> bool {
    if pid == 0 { return false; }
    let cmd = Command::new("ps").args(["-o", "command=", "-p", &pid.to_string()])
        .output().ok().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default();
    let args: Vec<_> = cmd.split_whitespace().collect();
    let is_ssh = args.first().and_then(|arg| std::path::Path::new(arg).file_name()).is_some_and(|name| name == "ssh");
    is_ssh && args.windows(2).any(|pair| pair[0] == "-L" && pair[1] == format!("127.0.0.1:{}:localhost:{}", local, remote))
}

fn wait_ready(tunnel: &mut Tunnel, scheme: TunnelScheme, timeout: Duration) -> Result<(), String> {
    let deadline = Instant::now() + timeout;
    loop {
        if !tunnel.alive() { return Err("SSH tunnel exited before it became ready. Check SSH access and local port availability.".into()); }
        if probe_forward(tunnel.local_port, tunnel.remote_port, scheme) && tunnel.alive() { return Ok(()); }
        if Instant::now() >= deadline { return Err(format!("Tunnel to remote port {} did not respond. Check that the remote service is running.", tunnel.remote_port)); }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn placeholder(local: u16, remote: u16) -> Tunnel {
        let child = Command::new("sleep").arg("30").spawn().unwrap();
        Tunnel { local_port: local, remote_port: remote, pid: child.id(), child: Some(child) }
    }

    #[test]
    #[cfg(unix)]
    fn same_port_is_idempotent_for_an_existing_healthy_forward() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 256]; let _ = stream.read(&mut request);
            stream.write_all(b"HTTP/1.1 404 Not Found\r\n\r\n").unwrap();
        });
        let reg = TunnelRegistry::new();
        reg.install("same", placeholder(port, port));
        assert_eq!(reg.force_same_port("same", "unused", port, &[]).unwrap(), port);
        reg.forget_session("same");
        worker.join().unwrap();
    }

    #[test]
    #[cfg(unix)]
    fn failed_replacement_preserves_old_forward_and_persisted_mapping() {
        let old_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let local = old_listener.local_addr().unwrap().port();
        let remote = free_local_port().unwrap();
        let closed_ssh = free_local_port().unwrap();
        let reg = TunnelRegistry::new();
        reg.install("remap", placeholder(local, remote));
        let before = reg.by_session.lock().unwrap()["remap"][&remote].pid;
        let error = reg.force_same_port("remap", &format!("127.0.0.1:{closed_ssh}"), remote, &["-F".into(), "/dev/null".into()]).unwrap_err();
        assert!(error.contains("SSH tunnel exited"), "{error}");
        assert_eq!(reg.list("remap"), vec![(remote, local)]);
        assert_eq!(reg.remembered_local("remap", remote), Some(local));
        assert_eq!(reg.by_session.lock().unwrap()["remap"][&remote].pid, before);
        reg.forget_session("remap");
    }

    #[test]
    #[cfg(unix)]
    fn listener_without_a_remote_response_is_not_ready() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let worker = std::thread::spawn(move || {
            // Like ssh accepting locally while the destination is unreachable.
            let (stream, _) = listener.accept().unwrap();
            std::thread::sleep(Duration::from_millis(800));
            drop(stream);
        });
        let mut tunnel = placeholder(port, 3000);
        assert!(wait_ready(&mut tunnel, TunnelScheme::Http, Duration::from_millis(200)).is_err());
        tunnel.kill();
        worker.join().unwrap();
    }

    #[test]
    fn cancelled_restore_does_not_spawn_or_forget_persisted_ports() {
        let reg = TunnelRegistry::new();
        reg.remember("closed", 3000, 45000, 0);
        assert!(reg.restore_ready_if("closed", "unused", &[], || false).is_empty());
        assert!(reg.list("closed").is_empty());
        assert_eq!(reg.remembered_local("closed", 3000), Some(45000));
    }

    fn lh() -> Vec<String> { vec!["localhost".into(), "127.0.0.1".into()] }

    #[test]
    fn classify_bare_host_port() {
        assert_eq!(classify_loopback("localhost:3000", &lh()),
            Some(("localhost".into(), LoopbackUrl { scheme: TunnelScheme::Http, port: 3000, path: "/".into() })));
        assert_eq!(classify_loopback("127.0.0.1:8080", &lh()),
            Some(("127.0.0.1".into(), LoopbackUrl { scheme: TunnelScheme::Http, port: 8080, path: "/".into() })));
    }

    #[test]
    fn classify_with_scheme_and_path() {
        assert_eq!(classify_loopback("http://localhost:5173/app?x=1", &lh()),
            Some(("localhost".into(), LoopbackUrl { scheme: TunnelScheme::Http, port: 5173, path: "/app?x=1".into() })));
        assert_eq!(classify_loopback("https://127.0.0.1:443/", &lh()),
            Some(("127.0.0.1".into(), LoopbackUrl { scheme: TunnelScheme::Https, port: 443, path: "/".into() })));
    }

    #[test]
    fn classify_rejects_non_loopback_and_junk() {
        assert_eq!(classify_loopback("https://github.com/x", &lh()), None);
        assert_eq!(classify_loopback("http://example.com:80", &lh()), None);
        assert_eq!(classify_loopback("localhost", &lh()), None);        // no port
        assert_eq!(classify_loopback("localhost:0", &lh()), None);      // port 0
        assert_eq!(classify_loopback("localhost:99999", &lh()), None);  // out of u16 range
        // 0.0.0.0 not in the default loopback set
        assert_eq!(classify_loopback("0.0.0.0:3000", &lh()), None);
    }

    #[test]
    fn classify_respects_configurable_hosts() {
        let hosts = vec!["localhost".into(), "0.0.0.0".into()];
        assert!(classify_loopback("0.0.0.0:3000", &hosts).is_some());
        assert!(classify_loopback("127.0.0.1:3000", &hosts).is_none());  // not configured here
    }

    #[test]
    fn classify_rejects_control_chars_in_path() {
        assert_eq!(classify_loopback("localhost:3000/a\nb", &lh()), None);
    }

    #[test]
    fn free_port_is_usable() {
        let p = free_local_port().unwrap();
        assert!(p > 0);
        // can bind again after the picker dropped its listener
        assert!(TcpListener::bind(("127.0.0.1", p)).is_ok());
    }

    // probe: a real local HTTP-ish listener that answers -> active; nothing listening -> inactive.
    #[test]
    fn probe_active_vs_inactive() {
        use std::io::{Read, Write};
        // a listener that reads the request and writes a minimal response = "active"
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let h = std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut b = [0u8; 64]; let _ = s.read(&mut b);
                let _ = s.write_all(b"HTTP/1.0 200 OK\r\n\r\nok");
            }
        });
        assert!(probe_local_port(port), "a listener that answers is active");
        let _ = h.join();

        // a port with nothing listening -> inactive
        let dead = free_local_port().unwrap();
        assert!(!probe_local_port(dead), "no listener -> inactive");
    }

    // force_same_port refuses when the local port is already taken (surfaces as the UI alert).
    #[test]
    fn force_same_port_errors_when_local_taken() {
        // hold the port so force_same_port's bind-test fails; no ssh is ever spawned.
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let reg = TunnelRegistry::new();
        let err = reg.force_same_port("s", "me@h", port, &[]).unwrap_err();
        assert!(err.contains("already in use"), "reports port-in-use: {}", err);
    }

    // persistence: remembered ports survive a reload; status() shows them inactive (no live tunnel).
    #[test]
    fn persists_and_reports_inactive() {
        let dir = std::env::temp_dir();
        let path = dir.join(format!("dt_tun_test_{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let reg = TunnelRegistry::with_store(path.clone());
        reg.remember("sess", 3000, 40000, 0);   // pid 0 = persisted-only (not live)
        reg.remember("sess", 5173, 40001, 0);

        // a fresh registry from the same file sees the persisted ports as inactive rows (pid 0 =>
        // nothing to adopt)
        let reg2 = TunnelRegistry::with_store(path.clone());
        let st = reg2.status("sess");
        assert_eq!(st.iter().map(|s| s.remote).collect::<Vec<_>>(), vec![3000, 5173]);
        assert!(st.iter().all(|s| s.local.is_none() && !s.active), "persisted-only -> inactive");

        // close() forgets one; forget_session clears the rest
        reg2.close("sess", 3000);
        let reg3 = TunnelRegistry::with_store(path.clone());
        assert_eq!(reg3.status("sess").iter().map(|s| s.remote).collect::<Vec<_>>(), vec![5173]);
        reg3.forget_session("sess");
        assert!(TunnelRegistry::with_store(path.clone()).status("sess").is_empty());

        let _ = std::fs::remove_file(&path);
    }

    // A persisted pid that is NOT one of our ssh -L procs (e.g. pid 1) must be cleared on load,
    // not adopted — so a recycled/dead pid never gets treated as a live tunnel.
    #[test]
    fn stale_pid_not_adopted() {
        let path = std::env::temp_dir().join(format!("dt_tun_stale_{}.json", std::process::id()));
        let _ = std::fs::remove_file(&path);
        // hand-write a record with a bogus-but-live pid (1 = init/launchd, not our ssh)
        std::fs::write(&path, r#"{"sess":[{"remote":3000,"local":40000,"pid":1}]}"#).unwrap();

        let reg = TunnelRegistry::with_store(path.clone());
        let st = reg.status("sess");
        assert_eq!(st.len(), 1);
        assert_eq!(st[0].remote, 3000);
        assert!(st[0].local.is_none() && !st[0].active, "non-ssh pid not adopted -> inactive");
        assert!(!is_our_ssh_pid(1, 40000, 3000), "pid 1 is not our ssh -L");

        let _ = std::fs::remove_file(&path);
    }

    // --- §18 sticky local ports (TC-TP) ------------------------------------------------------
    // A forwarded URL names ONE `localhost:<local>`. Once it's in a browser tab, re-picking a random
    // local port on the next connect silently breaks that tab, which is the reported bug.

    fn tmp_store(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("dt_tun_{}_{}.json", tag, std::process::id()));
        let _ = std::fs::remove_file(&p);
        p
    }

    // TC-TP1 pick_local_port is the whole policy: prefer the remembered port, fall back only when
    // it's genuinely unavailable. Tested directly because the ensure() path needs a real ssh spawn.
    #[test]
    fn tc_tp1_pick_local_port_prefers_the_remembered_port() {
        // A port nobody holds -> handed straight back, unchanged.
        //
        // Getting a port that is STILL free when pick_local_port re-checks it is a race: cargo runs
        // these tests as threads in one process, several of them bind 127.0.0.1:0, and the kernel can
        // hand a sibling the port `free_local_port` just released. That made this assert fail roughly
        // 1 run in 5. So retry a few times rather than trusting a single draw — the property under
        // test ("a free port is reused verbatim") is unaffected, and a genuine regression still fails
        // every attempt.
        let mut reused = None;
        for _ in 0..20 {
            let free = free_local_port().unwrap();
            if pick_local_port(Some(free)).unwrap() == free { reused = Some(free); break; }
        }
        assert!(reused.is_some(), "a free sticky port is reused verbatim");

        // A port something else now holds -> fall back rather than spawn an ssh doomed to fail on
        // ExitOnForwardFailure=yes.
        let held = TcpListener::bind("127.0.0.1:0").unwrap();
        let taken = held.local_addr().unwrap().port();
        let got = pick_local_port(Some(taken)).unwrap();
        assert_ne!(got, taken, "a taken sticky port is not handed out");
        assert!(got != 0);

        // No memory of this port yet (first-ever open, or a record written before `local` existed) ->
        // just pick something free. 0 is the serde default and must never be treated as sticky.
        assert!(pick_local_port(None).unwrap() != 0);
        assert!(pick_local_port(Some(0)).unwrap() != 0, "local:0 means 'unknown', not port 0");
    }

    // TC-TP2 the remembered local port survives the events that used to lose it: the ssh dying
    // (pid cleared) and an app restart (reload from disk).
    #[test]
    fn tc_tp2_local_port_is_remembered_across_death_and_restart() {
        let path = tmp_store("sticky");
        let reg = TunnelRegistry::with_store(path.clone());
        reg.remember("sess", 3000, 45001, 12345);
        assert_eq!(reg.remembered_local("sess", 3000), Some(45001));

        // close_session = detach/drop: kills the children and clears pids, but the local port must
        // stay so the reattach reuses it.
        reg.close_session("sess");
        assert_eq!(reg.remembered_local("sess", 3000), Some(45001),
            "detach must not forget the local port");

        // Restart: a fresh registry over the same file still knows the port.
        let reg2 = TunnelRegistry::with_store(path.clone());
        assert_eq!(reg2.remembered_local("sess", 3000), Some(45001),
            "the local port survives an app restart");
        // …and the row reads inactive-but-known (local:None, since no tunnel is live).
        let st = reg2.status("sess");
        assert_eq!(st.len(), 1);
        assert!(st[0].local.is_none() && !st[0].active);

        // Explicitly closing the row DOES forget it (the user dismissed that port).
        reg2.close("sess", 3000);
        assert_eq!(reg2.remembered_local("sess", 3000), None);
        let _ = std::fs::remove_file(&path);
    }

    // TC-TP3 ensure() re-opens on the SAME local port after the previous tunnel died. This is the
    // reported bug end-to-end, minus a live remote: the host is unresolvable so each ssh exits
    // immediately, which is precisely the "connection broke" state — and the port must not move.
    #[test]
    fn tc_tp3_ensure_reuses_the_local_port_after_the_tunnel_dies() {
        let path = tmp_store("ensure_sticky");
        let reg = TunnelRegistry::with_store(path.clone());
        // Unresolvable by RFC 6761: ssh spawns, fails to resolve, exits — no network dependency.
        let host = "me@dt-sticky-test.invalid";

        let first = reg.ensure("sess", host, 3000, &[]).unwrap();
        assert!(first != 0);
        assert_eq!(reg.remembered_local("sess", 3000), Some(first), "the chosen port is recorded");

        // Let the doomed ssh exit so the live entry is dead — the reconnect case.
        std::thread::sleep(Duration::from_millis(900));
        let second = reg.ensure("sess", host, 3000, &[]).unwrap();
        assert_eq!(second, first,
            "a re-opened tunnel keeps its local port (was: a new random port every reconnect)");

        // A DIFFERENT remote port gets its own local port; stickiness is per (session, remote).
        let other = reg.ensure("sess", host, 5173, &[]).unwrap();
        assert_ne!(other, first, "different remote ports don't share a local port");

        reg.forget_session("sess");
        let _ = std::fs::remove_file(&path);
    }

    // TC-TP4 reestablish() re-opens every persisted port for a session — the reconnect hook. It reads
    // the PERSISTED list, not the live map, precisely because the live tunnels are the ones that just
    // died with the network.
    #[test]
    fn tc_tp4_reestablish_reopens_all_persisted_ports_on_their_own_local_ports() {
        let path = tmp_store("reestablish");
        let reg = TunnelRegistry::with_store(path.clone());
        let host = "me@dt-sticky-test.invalid";

        // Two ports the user had opened, then a drop (pids cleared, local ports kept).
        let p3000 = reg.ensure("sess", host, 3000, &[]).unwrap();
        let p5173 = reg.ensure("sess", host, 5173, &[]).unwrap();
        reg.close_session("sess");
        std::thread::sleep(Duration::from_millis(300));

        let restored = reg.reestablish("sess", host, &[]);
        assert_eq!(restored, vec![(3000, p3000), (5173, p5173)],
            "both ports come back on the SAME local ports they had before the drop");

        // Nothing persisted -> nothing to do (a session that never forwarded anything).
        assert!(reg.reestablish("no-such-session", host, &[]).is_empty());

        reg.forget_session("sess");
        let _ = std::fs::remove_file(&path);
    }

    // TC-TP5 a port the user explicitly closed must NOT come back on reconnect — reestablish is
    // driven by the persisted list, and close() removes the row from it.
    #[test]
    fn tc_tp5_reestablish_skips_explicitly_closed_ports() {
        let path = tmp_store("reestablish_closed");
        let reg = TunnelRegistry::with_store(path.clone());
        let host = "me@dt-sticky-test.invalid";

        reg.ensure("sess", host, 3000, &[]).unwrap();
        let keep = reg.ensure("sess", host, 5173, &[]).unwrap();
        reg.close("sess", 3000);          // user dismissed this row
        std::thread::sleep(Duration::from_millis(300));

        let restored = reg.reestablish("sess", host, &[]);
        assert_eq!(restored, vec![(5173, keep)], "the dismissed port stays gone after a reconnect");

        reg.forget_session("sess");
        let _ = std::fs::remove_file(&path);
    }

    // TC-TP6 the chosen local port must land in the ssh argv that's actually executed. Returning the
    // right number from ensure() is worthless if `-L` carries a different one — the browser talks to
    // the flag, not to our bookkeeping.
    #[test]
    fn tc_tp6_the_local_port_reaches_the_ssh_forward_flag() {
        let args = tunnel_argv("me@host", 45123, 3000, &[]).unwrap();
        let i = args.iter().position(|a| a == "-L").expect("-L present");
        assert_eq!(args[i + 1], "127.0.0.1:45123:localhost:3000",
            "the forward binds the chosen LOCAL port to the remote's loopback port");
        // Local side pinned to loopback (not 0.0.0.0): only this machine can use the forward.
        assert!(args[i + 1].starts_with("127.0.0.1:"));
        // A collision must fail the spawn rather than silently forward a different port.
        assert!(args.windows(2).any(|w| w[0] == "-o" && w[1] == "ExitOnForwardFailure=yes"));
        // `--` before the target so a host that looks like a flag can't inject one.
        assert_eq!(args[args.len() - 2], "--");
        assert_eq!(args[args.len() - 1], "me@host");

        // A non-default ssh port rides along without disturbing the -L mapping.
        let args2 = tunnel_argv("me@host:2222", 45124, 8080, &[]).unwrap();
        assert_eq!(args2[0], "-p");
        assert_eq!(args2[1], "2222");
        let j = args2.iter().position(|a| a == "-L").unwrap();
        assert_eq!(args2[j + 1], "127.0.0.1:45124:localhost:8080");
        assert_eq!(args2[args2.len() - 1], "me@host", "the :port is stripped from the target");

        // Flag-injection guard (§6.1): a hostile host string is rejected, not forwarded.
        assert!(tunnel_argv("-oProxyCommand=x", 45125, 3000, &[]).is_err());
    }
}
