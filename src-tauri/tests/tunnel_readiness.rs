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

struct Server { port: u16, stop: Arc<std::sync::atomic::AtomicBool>, worker: Option<std::thread::JoinHandle<()>> }
impl Server {
    fn http(delay: Duration) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = stop.clone();
        let worker = std::thread::spawn(move || {
            std::thread::sleep(delay);
            while !flag.load(std::sync::atomic::Ordering::Relaxed) {
                if let Ok((mut stream, _)) = listener.accept() {
                    stream.set_read_timeout(Some(Duration::from_millis(100))).unwrap();
                    let mut request = [0; 256];
                    if stream.read(&mut request).unwrap_or(0) > 0 {
                        let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    }
                } else { std::thread::sleep(Duration::from_millis(10)); }
            }
        });
        Self { port, stop, worker: Some(worker) }
    }
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
