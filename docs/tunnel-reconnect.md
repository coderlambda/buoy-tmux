# Tunnel reconnect behavior

Click a workspace's forwarded port or open icon to connect that workspace in the background, wait for its connection, and verify the SSH forward before opening the browser. A workspace already reconnecting shares its pending attempt. Repeated clicks on one port are ignored while it is busy. The active terminal stays selected. Detach keeps the saved ports visible so they can reconnect directly.

The backend verifies a remote response instead of assuming that a live SSH process or a listening local socket means a working forward. HTTP probes require response bytes; HTTPS probes require a TLS handshake and preserve the original scheme. The HTTPS reachability probe permits development certificates and sends no credentials or page requests. Browser certificate verification remains unchanged. SSH startup is bounded, and an unresponsive existing child is replaced while retaining its local port when available.

This matters because OpenSSH's `ExitOnForwardFailure` checks forwarding setup, not whether the ultimate destination accepts connections. See the [OpenSSH configuration reference](https://man.openbsd.org/ssh_config#ExitOnForwardFailure).

Tunnel changes serialize per workspace, preventing duplicate children from clicks racing automatic recovery. Same-port remapping keeps the old forward until its replacement responds; a conflict or failed SSH startup leaves the old mapping intact. Explicit stop removes the saved forward, and cancelled reconnect workers cannot restore detached sessions. Status reads that are overtaken by a new tunnel event are discarded. Session persistence uses atomic read-modify-write updates so background reconnects cannot overwrite concurrent workspace preferences.

Session reconnect waits up to 30 seconds; tunnel readiness waits up to 10 seconds plus the bounded final probe. Failures leave the port available for retry and display the reason. SSH startup, probes and tunnel teardown run off the platform UI thread.

## Validation

- `npm run typecheck` and `npm test`
- `cargo test --manifest-path src-tauri/Cargo.toml --lib`
- Build the UI-test app, then `npx wdio run wdio.conf.ts`. The tunnel suite covers unopened/detached/dead/reconnecting sessions, shared reconnects, failure/retry, timeout, cancellation, secure URLs and stale status responses.
- `python3 test/run-tunnel-readiness.py` creates disposable keys, a self-signed test certificate and a loopback-only sshd, runs three real SSH integration tests, then removes the fixture. It requires Python 3, OpenSSH client/server tools and OpenSSL. The tests cover slow startup, concurrent opens, a suspended SSH process, stable local ports, cancelled recovery and HTTPS persistence. They do not use existing SSH sessions or modify remote machines.
