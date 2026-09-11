//! Isolated loopback SSH integration. No remote commands or existing sessions are used.
//! Start a disposable sshd on 127.0.0.1, then set BUOY_TUNNEL_TEST_SSH_PORT and
//! BUOY_TUNNEL_TEST_KEY and run: cargo test --test tunnel_readiness -- --ignored
use buoy_lib::tunnel::{TunnelRegistry, TunnelScheme};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Barrier};
use std::time::{Duration, Instant};

fn connection() -> (String, Vec<String>) {
    let port: u16 = std::env::var("BUOY_TUNNEL_TEST_SSH_PORT").expect("disposable local sshd port").parse().unwrap();
    let key = std::env::var("BUOY_TUNNEL_TEST_KEY").expect("disposable local sshd client key");
    (format!("127.0.0.1:{port}"), vec!["-F".into(), "/dev/null".into(), "-i".into(), key,
        "-o".into(), "StrictHostKeyChecking=no".into(), "-o".into(), "UserKnownHostsFile=/dev/null".into()])
}

struct Server {
    port: u16,
    stop: Arc<std::sync::atomic::AtomicBool>,
    response_delay_ms: Arc<std::sync::atomic::AtomicU64>,
    drop_next: Arc<std::sync::atomic::AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
}
impl Server {
    fn http(delay: Duration) -> Self {
        Self::http_at("127.0.0.1:0", delay)
    }

    fn http_at(address: &str, delay: Duration) -> Self {
        let listener = TcpListener::bind(address).unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        let response_delay_ms = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let latency = response_delay_ms.clone();
        let drop_next = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let drop_request = drop_next.clone();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(delay);
            while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                if let Ok((mut stream, _)) = listener.accept() {
                    stream.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
                    let mut request = [0; 256];
                    if stream.read(&mut request).unwrap_or(0) > 0 {
                        if drop_request.swap(false, std::sync::atomic::Ordering::Relaxed) { continue; }
                        std::thread::sleep(Duration::from_millis(latency.load(std::sync::atomic::Ordering::Relaxed)));
                        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    }
                } else { std::thread::sleep(Duration::from_millis(10)); }
            }
        });
        Self { port, stop, response_delay_ms, drop_next, worker: Some(worker) }
    }
}

#[test]
#[ignore = "requires a disposable loopback sshd"]
fn same_port_reconnect_does_not_silently_move_when_the_local_port_is_taken() {
    let (host, args) = connection();
    // The destination listens only on IPv6; ssh binds the local IPv4 side at the SAME port.
    // This models two machines while keeping the test isolated on one loopback SSH server.
    let server = Server::http_at("[::1]:0", Duration::ZERO);
    let reg = TunnelRegistry::new();
    assert_eq!(reg.force_same_port("pinned", &host, server.port, &args).unwrap(), server.port);
    reg.close_session("pinned");
    let occupying = TcpListener::bind(("127.0.0.1", server.port)).unwrap();
    let result = reg.ensure_ready("pinned", &host, server.port, TunnelScheme::Http, &args);
    let automatic = reg.restore_ready_if("pinned", &host, &args, || true);
    // Clean up even against the old implementation, which silently creates a random mapping.
    reg.close_session("pinned");
    drop(occupying);
    let retried = reg.ensure_ready("pinned", &host, server.port, TunnelScheme::Http, &args);
    reg.forget_session("pinned");
    assert!(result.is_err(), "same-port reconnect must report a conflict, not move to {result:?}");
    assert!(automatic.is_empty(), "automatic restore must also keep the pinned port");
    assert_eq!(retried.unwrap(), server.port, "retry retains the requested port after it becomes free");
}

#[test]
#[ignore = "requires a disposable loopback sshd"]
fn a_slow_service_does_not_replace_a_healthy_same_port_ssh() {
    let (host, args) = connection();
    let server = Server::http_at("[::1]:0", Duration::ZERO);
    let path = std::env::temp_dir().join(format!("buoy-slow-same-{}.json", std::process::id()));
    let reg = TunnelRegistry::with_store(path.clone());
    reg.force_same_port("slow", &host, server.port, &args).unwrap();
    let before: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    server.response_delay_ms.store(1200, std::sync::atomic::Ordering::Relaxed);
    let result = reg.force_same_port("slow", &host, server.port, &args);
    let active = reg.status("slow")[0].active;
    let after: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    reg.forget_session("slow");
    let _ = std::fs::remove_file(path);
    assert_eq!(result.unwrap(), server.port, "a 1.2-second HTTP response is still reachable");
    assert!(active, "a slow HTTP response must not make a healthy tunnel look disconnected");
    assert_eq!(after["slow"][0]["pid"], before["slow"][0]["pid"], "probing must not tear down healthy browser/WebSocket connections");
}

#[test]
#[ignore = "requires a disposable loopback sshd"]
fn one_failed_probe_does_not_replace_a_healthy_ssh() {
    let (host, args) = connection();
    let server = Server::http(Duration::ZERO);
    let path = std::env::temp_dir().join(format!("buoy-transient-probe-{}.json", std::process::id()));
    let reg = TunnelRegistry::with_store(path.clone());
    let local = reg.ensure_ready("retry", &host, server.port, TunnelScheme::Http, &args).unwrap();
    let before: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    server.drop_next.store(true, std::sync::atomic::Ordering::Relaxed);
    let result = reg.ensure_ready("retry", &host, server.port, TunnelScheme::Http, &args);
    let after: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    reg.forget_session("retry");
    let _ = std::fs::remove_file(path);
    assert_eq!(result.unwrap(), local);
    assert_eq!(after["retry"][0]["pid"], before["retry"][0]["pid"], "one failed request should retry the existing SSH child");
}

#[test]
#[ignore = "requires a disposable loopback sshd"]
fn same_port_recovers_a_stalled_ssh_adopted_after_an_app_restart() {
    let (host, args) = connection();
    let server = Server::http_at("[::1]:0", Duration::ZERO);
    let path = std::env::temp_dir().join(format!("buoy-adopted-same-{}.json", std::process::id()));
    let original = TunnelRegistry::with_store(path.clone());
    original.force_same_port("adopted", &host, server.port, &args).unwrap();
    let before: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let pid = before["adopted"][0]["pid"].as_u64().unwrap().to_string();
    // A relaunched app only has the persisted PID, not the original Child handle. Retain the
    // original registry here solely to reap the test process even if the regression fails.
    let adopted = TunnelRegistry::with_store(path.clone());
    assert!(std::process::Command::new("kill").args(["-STOP", &pid]).status().unwrap().success());
    let restored = adopted.restore_ready_if("adopted", &host, &args, || true);
    let after: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    adopted.forget_session("adopted");
    original.forget_session("adopted");
    let _ = std::fs::remove_file(path);
    assert_eq!(restored, vec![(server.port, server.port)], "the old listener must release the pinned local port");
    assert_ne!(after["adopted"][0]["pid"], before["adopted"][0]["pid"]);
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = self.worker.take().unwrap().join();
    }
}

#[test]
#[ignore = "requires a disposable loopback sshd"]
fn waits_for_service_and_serializes_concurrent_opens() {
    let (host, args) = connection();
    let server = Server::http(Duration::from_millis(1400));
    let reg = Arc::new(TunnelRegistry::new());
    let barrier = Arc::new(Barrier::new(5));
    let start = Instant::now();
    let workers: Vec<_> = (0..4).map(|_| {
        let (reg, barrier, host, args) = (reg.clone(), barrier.clone(), host.clone(), args.clone());
        let remote = server.port;
        std::thread::spawn(move || { barrier.wait(); reg.ensure_ready("test", &host, remote, TunnelScheme::Http, &args).unwrap() })
    }).collect();
    barrier.wait();
    let ports: Vec<_> = workers.into_iter().map(|worker| worker.join().unwrap()).collect();
    assert!(start.elapsed() >= Duration::from_millis(1300), "must wait for a real response");
    assert!(ports.iter().all(|port| *port == ports[0]), "one SSH child/local port per remote");
    assert!(reg.status("test")[0].active);
    reg.forget_session("test");
    assert!(reg.list("test").is_empty());
}

#[test]
#[ignore = "requires a disposable loopback sshd"]
fn repairs_a_live_but_wedged_ssh_and_keeps_the_local_port() {
    let (host, args) = connection();
    let server = Server::http(Duration::ZERO);
    let path = std::env::temp_dir().join(format!("buoy-ready-{}.json", std::process::id()));
    let reg = TunnelRegistry::with_store(path.clone());
    let local = reg.ensure_ready("test", &host, server.port, TunnelScheme::Http, &args).unwrap();
    let stored: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let pid = stored["test"][0]["pid"].as_u64().unwrap().to_string();
    assert!(std::process::Command::new("kill").args(["-STOP", &pid]).status().unwrap().success());
    // The child still exists and still owns its listener, but cannot forward packets.
    let restored = reg.restore_ready_if("test", &host, &args, || true);
    assert_eq!(restored, vec![(server.port, local)]);
    assert!(reg.status("test")[0].active);
    let after: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_ne!(after["test"][0]["pid"], stored["test"][0]["pid"]);
    reg.close_session("test");
    assert!(reg.restore_ready_if("test", &host, &args, || false).is_empty(), "cancelled restore must not revive detached forwards");
    assert!(reg.list("test").is_empty());
    reg.forget_session("test");
    let _ = std::fs::remove_file(path);
}

#[test]
#[ignore = "requires a disposable loopback sshd and BUOY_TUNNEL_TEST_PKCS12"]
fn https_probe_and_persistence_keep_the_secure_scheme() {
    let (host, args) = connection();
    let certificate = std::fs::read(std::env::var("BUOY_TUNNEL_TEST_PKCS12").unwrap()).unwrap();
    let identity = native_tls::Identity::from_pkcs12(&certificate, "test").unwrap();
    let acceptor = native_tls::TlsAcceptor::new(identity).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let remote = listener.local_addr().unwrap().port();
    let worker = std::thread::spawn(move || {
        for stream in listener.incoming().take(2) {
            let stream = stream.unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let _ = acceptor.accept(stream);
        }
    });
    let path = std::env::temp_dir().join(format!("buoy-ready-tls-{}.json", std::process::id()));
    let reg = TunnelRegistry::with_store(path.clone());
    reg.ensure_ready("tls", &host, remote, TunnelScheme::Https, &args).unwrap();
    let statuses = reg.status("tls");
    assert!(statuses[0].active);
    assert_eq!(statuses[0].scheme, TunnelScheme::Https);
    worker.join().unwrap();
    reg.close_session("tls");
    let restored = TunnelRegistry::with_store(path.clone());
    assert_eq!(restored.status("tls")[0].scheme, TunnelScheme::Https);
    reg.forget_session("tls");
    let _ = std::fs::remove_file(path);
}
