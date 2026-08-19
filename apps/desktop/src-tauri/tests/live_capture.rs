//! Issue #13: capture a fresh, slowly appearing prompt using a real tmux server. No SSH credentials
//! or user sessions are involved; a unique socket is removed even if an assertion fails.
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use buoy_lib::control_backend::{BackendConfig, BackendEvent, ControlBackend};
use buoy_lib::transport::Transport;

struct Fixture {
    tmux: String,
    socket: String,
    backend: Option<ControlBackend>,
}
impl Fixture {
    fn command(&self) -> Command {
        let mut command = Command::new(&self.tmux);
        command.args(["-L", &self.socket, "-f", "/dev/null"]).env_remove("TMUX");
        command
    }
    fn start_server(&self) {
        let quoted_tmux = format!("'{}'", self.tmux.replace('\'', "'\\''"));
        let program = format!("{quoted_tmux} wait-for buoy-capture-start; i=0; while [ $i -lt 20 ]; do printf X; i=$((i+1)); sleep 0.02; done; printf '>_'; sleep 30");
        assert!(self.command().args([
            "new-session", "-d", "-s", "capture-probe", "-x", "90", "-y", "30",
            &program,
        ]).status().unwrap().success());
    }
    fn stop_client(&mut self) {
        if let Some(backend) = self.backend.take() { backend.kill(); }
    }
    fn stop_server(&self) {
        let _ = self.command().arg("kill-server").stdout(Stdio::null()).stderr(Stdio::null()).status();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) { self.stop_client(); self.stop_server(); }
}
fn pause() { thread::sleep(Duration::from_millis(5)); }

fn assert_coherent_capture(data: &str) -> usize {
    let body = data.strip_prefix("\x1b[H\x1b[2J").expect("capture prefix");
    let (cells, cursor) = body.rsplit_once("\x1b[").expect("cursor suffix");
    let (row, col) = cursor.strip_suffix('H').unwrap().split_once(';').unwrap();
    let first_line = cells.split("\r\n").next().unwrap();
    assert_eq!(row, "1", "the fixture never leaves its first row");
    assert_eq!(col.parse::<usize>().unwrap(), first_line.len() + 1,
        "the capture and cursor must describe the same instant, not an empty screen plus a newer cursor: {data:?}");
    first_line.len()
}

#[test]
fn fresh_prompt_capture_stays_coherent_across_reconnect_and_server_restart() {
    let probe = buoy_lib::probe::probe_local_tmux();
    if !probe.probed { eprintln!("SKIP: no local tmux"); return; }
    let mut fixture = Fixture {
        tmux: probe.tmux_path.clone(),
        socket: format!("buoy-capture-{}-{}", std::process::id(), SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()),
        backend: None,
    };
    fixture.start_server();
    for phase in ["new session", "reconnect without a prior command", "server restart"] {
        if phase == "server restart" { fixture.stop_server(); fixture.start_server(); }
        let snapshots = Arc::new(Mutex::new(Vec::<String>::new()));
        let records = snapshots.clone();
        let window = Arc::new(Mutex::new(None::<String>));
        let active = window.clone();
        fixture.backend = Some(ControlBackend::spawn(BackendConfig {
            session: "capture-probe".into(), tmux_path: fixture.tmux.clone(), socket: fixture.socket.clone(),
            tmux_version: probe.version, transport: Transport::Local,
            base_args: vec!["-f".into(), "/dev/null".into()], ..Default::default()
        }, Arc::new(move |event| match event {
            BackendEvent::Data { data, repaint: true, .. } => records.lock().unwrap().push(data),
            BackendEvent::WindowActive { window, .. } => *active.lock().unwrap() = Some(window),
            _ => {}
        }), 90, 30).expect("attach controlled fixture"));
        let deadline = Instant::now() + Duration::from_secs(5);
        while window.lock().unwrap().is_none() && Instant::now() < deadline { pause(); }
        let win = window.lock().unwrap().clone().expect("active window discovered");
        let backend = fixture.backend.as_ref().unwrap();
        backend.capture_window(&win);
        let first_deadline = Instant::now() + Duration::from_secs(2);
        while snapshots.lock().unwrap().is_empty() && Instant::now() < first_deadline { pause(); }
        assert!(!snapshots.lock().unwrap().is_empty(), "{phase}: initial capture arrives");
        if phase != "reconnect without a prior command" {
            assert_eq!(assert_coherent_capture(&snapshots.lock().unwrap()[0]), 0);
            assert!(fixture.command().args(["wait-for", "-S", "buoy-capture-start"]).status().unwrap().success());
        }
        let deadline = Instant::now() + Duration::from_millis(1000);
        while Instant::now() < deadline { backend.capture_window(&win); pause(); }
        let captures = snapshots.lock().unwrap().clone();
        assert!(captures.len() >= 3, "{phase}: capture replies arrived");
        let lengths: Vec<_> = captures.iter().map(|data| assert_coherent_capture(data)).collect();
        if phase != "reconnect without a prior command" {
            assert!(lengths.contains(&0), "{phase}: exercised a screen with no prompt yet");
            assert!(lengths.iter().any(|&n| n > 0 && n < 20), "{phase}: exercised a prompt still appearing");
        }
        assert!(lengths.contains(&22), "{phase}: complete prompt appears without typing or resizing");
        // A closed window produces only one error reply for the compound command. The next
        // valid capture must still correlate correctly instead of being mistaken for its cursor.
        let before = snapshots.lock().unwrap().len();
        backend.capture_window("@99999");
        backend.capture_window(&win);
        let deadline = Instant::now() + Duration::from_secs(2);
        while snapshots.lock().unwrap().len() <= before && Instant::now() < deadline { pause(); }
        let snapshots = snapshots.lock().unwrap();
        assert!(snapshots.len() > before, "{phase}: capture recovers after a missing window");
        assert_eq!(assert_coherent_capture(snapshots.last().unwrap()), 22);
        fixture.stop_client();
    }
}
