//! Exercise real PTY backpressure and tmux parsing without a remote host or credentials.
use buoy_lib::control_backend::{BackendConfig, BackendEvent, ControlBackend};
use buoy_lib::transport::Transport;
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

struct Fixture { tmux: String, socket: String, dir: std::path::PathBuf }
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = Command::new(&self.tmux).args(["-L", &self.socket, "kill-server"])
            .stdout(Stdio::null()).stderr(Stdio::null()).status();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn run_paste(case: &str, payload: String) {
    let probe = buoy_lib::probe::probe_local_tmux();
    if !probe.probed { eprintln!("SKIP: tmux unavailable"); return; }
    let socket = format!("buoy-paste-{case}-{}", std::process::id());
    let dir = std::env::temp_dir().join(&socket);
    std::fs::create_dir_all(&dir).unwrap();
    let fixture = Fixture { tmux: probe.tmux_path.clone(), socket: socket.clone(), dir: dir.clone() };
    let received = dir.join("received");
    let script = dir.join("receive.py");
    std::fs::write(&script, format!(r#"
import os, tty, time
tty.setraw(0)
os.write(1, b'RECEIVER_READY')
remaining = {}
with open({}, 'wb') as out:
    while remaining:
        data = os.read(0, min(4096, remaining))
        out.write(data)
        remaining -= len(data)
        os.write(1, b'busy output\r\n' * 50)
os.write(1, b'PASTE_COMPLETE')
time.sleep(30)
"#, payload.len(), serde_json::to_string(received.to_str().unwrap()).unwrap())).unwrap();
    let status = Command::new(&probe.tmux_path).args(["-L", &socket, "-f", "/dev/null", "new-session", "-d", "-s", "paste", "python3"])
        .arg(&script).env("LC_ALL", "en_US.UTF-8").status().unwrap();
    assert!(status.success());
    let (events_tx, events_rx) = mpsc::channel();
    let backend = ControlBackend::spawn(BackendConfig {
        session: "paste".into(), tmux_path: probe.tmux_path, socket,
        transport: Transport::Local, ..BackendConfig::default()
    }, Arc::new(move |event| { let _ = events_tx.send(event); }), 100, 30).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if matches!(events_rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).unwrap(), BackendEvent::Ready) { break; }
    }
    let expected = payload.into_bytes();
    let data = String::from_utf8(expected.clone()).unwrap();
    let (sent_tx, sent_rx) = mpsc::channel();
    // A timeout is essential: the old code blocks with the parser lock held. Drop the fixture
    // on failure to kill only this test's tmux and unblock its writer before the process exits.
    std::thread::spawn(move || { backend.write_to(&data, Some("@0")); let _ = sent_tx.send(backend); });
    let backend = sent_rx.recv_timeout(Duration::from_secs(10)).expect("paste must not deadlock the PTY reader");
    let deadline = Instant::now() + Duration::from_secs(30);
    while std::fs::metadata(&received).map(|m| m.len()).unwrap_or(0) < expected.len() as u64 {
        assert!(Instant::now() < deadline, "large paste did not reach the receiver in full");
        std::thread::sleep(Duration::from_millis(20));
    }
    let actual = std::fs::read(&received).unwrap();
    assert_eq!(actual.len(), expected.len());
    assert!(actual == expected, "paste bytes changed, reordered, or expanded by tmux");
    backend.resize(90, 24);
    backend.kill();
    drop(fixture);
}

#[test]
fn large_paste_drains_while_output_is_busy_and_preserves_bytes() {
    let payload = format!("\x1b[200~{}\x1b[201~", "paste abc 中文🙂 \\\" $HOME ${PATH} ; #{pane_id}\r".repeat(20000));
    run_paste("multiline", payload);
}

#[test]
fn fifty_thousand_character_single_line_paste_arrives_intact() {
    let payload = format!("\x1b[200~{}\x1b[201~", "abc $HOME;".repeat(5000));
    run_paste("singleline", payload);
}
