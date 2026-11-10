//! Real tmux restart regression. By default SSH is a local command adapter; run
//! `python3 test/run-tab-recovery.py` to exercise the same test through a disposable real sshd.
#![cfg(unix)]

use buoy_lib::{
    control_backend::{BackendConfig, BackendEvent},
    session_store::{RecoveryWindow, SessionMeta, SessionStore},
    supervisor::{real_backend_factory, Supervisor, SupervisorOpts},
    transport::Transport,
};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

#[derive(Default)]
struct Observed {
    ready: usize,
    order: Vec<String>,
    windows: Vec<RecoveryWindow>,
}

struct Fixture {
    root: PathBuf,
    cfg: BackendConfig,
    supervisor: Option<Arc<Supervisor>>,
    store: Arc<SessionStore>,
    observed: Arc<Mutex<Observed>>,
}

fn executable(path: &std::path::Path, script: &str) {
    fs::write(path, script).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

impl Fixture {
    fn new(tmux: &str, version: Option<(u32, u32)>) -> Self {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        // Short enough for macOS's Unix socket path limit; no user tmux sockets are touched.
        let root = std::env::temp_dir().join(format!("buoy-names-{}-{nonce}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let wrapper = root.join("tmux");
        let locale = if cfg!(target_os = "macos") { "en_US.UTF-8" } else { "C.UTF-8" };
        executable(&wrapper, &format!(
            "#!/bin/sh\nexport LC_ALL={locale}\nexec '{}' -f /dev/null \"$@\"\n",
            tmux.replace('\'', "'\\''")
        ));
        let (host, base_args) = if let Ok(port) = std::env::var("BUOY_RECOVERY_SSH_PORT") {
            (format!("127.0.0.1:{port}"), vec![
                "-F".into(), "/dev/null".into(), "-i".into(),
                std::env::var("BUOY_RECOVERY_SSH_KEY").unwrap(),
                "-o".into(), "StrictHostKeyChecking=no".into(),
                "-o".into(), "UserKnownHostsFile=/dev/null".into(),
            ])
        } else {
            let adapter = root.join("ssh");
            executable(&adapter, &format!(
                "#!/bin/sh\nexport XDG_CACHE_HOME='{}' SHELL=/bin/sh\nfor arg do remote=$arg; done\nexec /bin/sh -c \"$remote\"\n",
                root.join("cache").display()
            ));
            std::env::set_var("BUOY_SSH_BIN", &adapter);
            ("localhost".into(), vec![])
        };
        let cfg = BackendConfig {
            host, session: "names".into(), tmux_path: wrapper.to_string_lossy().into_owned(),
            tmux_version: version, socket: format!("buoy-names-{nonce}"),
            transport: Transport::Ssh, base_args, recovery_windows: vec![],
        };
        let store = Arc::new(SessionStore::new(root.join("sessions.json")));
        let meta: SessionMeta = serde_json::from_value(serde_json::json!({
            "id":"names", "host":cfg.host, "session":"names", "transport":"ssh",
            "mode":"control", "socketName":cfg.socket,
        })).unwrap();
        store.save(&[meta]);
        Self { root, cfg, supervisor: None, store, observed: Arc::default() }
    }

    fn start(&mut self) {
        let store = self.store.clone();
        let observed = self.observed.clone();
        let start = Instant::now();
        let sup = Supervisor::new(self.cfg.clone(), SupervisorOpts {
            backoff_base_ms: 100, backoff_max_ms: 500, ..SupervisorOpts::default()
        }, real_backend_factory(), Arc::new(move |event| {
            if let BackendEvent::RecoverySnapshot { ref windows } = event {
                store.set_recovery_windows("names", windows);
            }
            let mut rec = observed.lock().unwrap();
            match event {
                BackendEvent::Ready => rec.ready += 1,
                BackendEvent::RecoverySnapshot { windows } => rec.windows = windows,
                BackendEvent::WindowActive { order, .. }
                | BackendEvent::WindowAdd { order, .. }
                | BackendEvent::WindowClose { order, .. } => rec.order = order,
                _ => {}
            }
        }), Arc::new(|_| {}), Arc::new(thread::sleep),
            Arc::new(move || start.elapsed().as_millis() as u64));
        sup.start(90, 30);
        self.supervisor = Some(sup);
    }

    fn tmux(&self, args: &[&str]) -> String {
        let output = Command::new(&self.cfg.tmux_path).args(["-L", &self.cfg.socket])
            .args(args).env_remove("TMUX").output().unwrap();
        assert!(output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    fn wait(&self, label: &str, condition: impl Fn(&Observed) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if condition(&self.observed.lock().unwrap()) { return; }
            assert!(Instant::now() < deadline, "timed out: {label}; windows={:?}",
                self.observed.lock().unwrap().windows);
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn stop_client(&mut self) {
        if let Some(sup) = self.supervisor.take() { sup.close(); }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.stop_client();
        let _ = Command::new(&self.cfg.tmux_path).args(["-L", &self.cfg.socket, "kill-server"])
            .stdout(Stdio::null()).stderr(Stdio::null()).status();
        std::env::remove_var("BUOY_SSH_BIN");
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[test]
fn manual_tab_names_survive_server_loss_and_app_restart() {
    let probe = buoy_lib::probe::probe_local_tmux();
    if !probe.probed { eprintln!("SKIP: local tmux is required"); return; }
    let mut fixture = Fixture::new(&probe.tmux_path, probe.version);
    fixture.start();
    fixture.wait("first connection", |r| r.ready == 1 && r.order.len() == 1);
    let sup = fixture.supervisor.as_ref().unwrap().clone();
    sup.new_window();
    fixture.wait("two tabs", |r| r.order.len() == 2 && r.windows.len() == 2);
    let order = fixture.observed.lock().unwrap().order.clone();
    let manual = "Review 中文 #{pane_current_command}";
    // A foreground program must not replace the manual name in the recovery recipe.
    fixture.tmux(&["respawn-pane", "-k", "-t", &order[0], "sleep 120"]);
    fixture.wait("foreground program started", |_| fixture.tmux(&[
        "display-message", "-p", "-t", &order[0], "#{pane_current_command}",
    ]) == "sleep");
    sup.rename_window(&order[0], manual);
    fixture.wait("manual rename saved", |r| r.windows[0].name == manual
        && r.windows[0].command == "sleep" && r.windows[0].automatic_rename == Some(false));

    // Renaming to the current automatic name then clearing it need not emit window-renamed.
    // The mode still has to be saved immediately, before the 15-second topology poll.
    let second_name = fixture.observed.lock().unwrap().windows[1].name.clone();
    sup.rename_window(&order[1], &second_name);
    fixture.wait("second tab pinned", |r| r.windows[1].automatic_rename == Some(false));
    sup.rename_window(&order[1], "");
    fixture.wait("automatic naming saved", |r| r.windows[1].automatic_rename == Some(true));

    fixture.tmux(&["kill-server"]); // Simulate the tmux loss caused by a target host reboot.
    fixture.wait("automatic reconnect rebuilt both tabs", |r| r.ready >= 2
        && r.windows.len() == 2 && r.windows[0].name == manual
        && r.windows[0].command != "sleep" && r.windows[0].automatic_rename == Some(false)
        && r.windows[1].automatic_rename == Some(true));
    // The manual title also survives a pane-title change; automatic tabs still follow titles.
    let order = fixture.observed.lock().unwrap().order.clone();
    sup.write_to("printf '\\033]2;changed-program\\007'; sleep 120\n", Some(&order[0]));
    sup.write_to("printf '\\033]2;AUTO_AFTER_REBOOT\\007'; sleep 120\n", Some(&order[1]));
    fixture.wait("automatic tab still follows pane title", |r| {
        let name = fixture.tmux(&["display-message", "-p", "-t", &order[1], "#{window_name}"]);
        name == "AUTO_AFTER_REBOOT" && r.windows[1].name == name
    });
    assert_eq!(fixture.tmux(&["display-message", "-p", "-t", &order[0], "#{window_name}"]), manual);

    // A second rename must survive reopening Buoy AND another lost tmux server. Read the actual
    // JSON file through SessionStore rather than reusing the supervisor's in-memory snapshot.
    let renamed = "Second review 中文 #{pane_current_command}";
    sup.rename_window(&order[0], renamed);
    fixture.wait("latest name persisted", |r| r.windows[0].name == renamed);
    fixture.stop_client();
    fixture.cfg.recovery_windows = fixture.store.load()[0].recovery_windows.clone();
    assert_eq!(fixture.cfg.recovery_windows[0].name, renamed);
    fixture.tmux(&["kill-server"]);
    *fixture.observed.lock().unwrap() = Observed::default();
    fixture.start();
    fixture.wait("app restart restored persisted names", |r| r.ready >= 1
        && r.windows.len() == 2 && r.windows[0].name == renamed
        && r.windows[0].automatic_rename == Some(false) && r.windows[1].automatic_rename == Some(true));
}
