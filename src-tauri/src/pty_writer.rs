//! PTY writes must never run under the control parser or application session locks. A full
//! duplex PTY can block in write_all until its output is read, which needs those same locks.
use std::io::Write;
use std::sync::{atomic::{AtomicBool, Ordering}, mpsc, Arc};

pub type Completion = mpsc::Sender<Result<(), String>>;
pub struct WriteReceipt(mpsc::Receiver<Result<(), String>>);
impl WriteReceipt {
    pub fn pending() -> (Self, Completion) {
        let (tx, rx) = mpsc::channel();
        (Self(rx), tx)
    }
    pub fn finished(result: Result<(), String>) -> Self {
        let (receipt, done) = Self::pending();
        let _ = done.send(result);
        receipt
    }
    /// Called on a blocking worker, after releasing all backend/application locks.
    pub fn wait(self) -> Result<(), String> {
        self.0.recv().unwrap_or_else(|_| Err("Terminal input was cancelled".into()))
    }
}

enum Job { Bytes(Vec<u8>), Complete(Completion), Stop }
pub struct PtyWriter {
    tx: mpsc::Sender<Job>,
    stopped: Arc<AtomicBool>,
    // Pure backend state-machine tests retain an immediate recording writer.
    #[cfg(test)]
    inline: Option<Box<dyn Write + Send>>,
}
impl PtyWriter {
    pub fn new(mut writer: Box<dyn Write + Send>) -> Self {
        let (tx, rx) = mpsc::channel();
        let stopped = Arc::new(AtomicBool::new(false));
        let stop = stopped.clone();
        std::thread::spawn(move || {
            let mut failure = None;
            while let Ok(job) = rx.recv() {
                if stop.load(Ordering::Acquire) { break; }
                match job {
                    Job::Bytes(data) if failure.is_none() => {
                        if let Err(error) = writer.write_all(&data).and_then(|_| writer.flush()) {
                            failure = Some(format!("Terminal write failed: {error}"));
                        }
                    }
                    Job::Complete(done) => { let _ = done.send(failure.clone().map_or(Ok(()), Err)); }
                    Job::Stop => break,
                    _ => {}
                }
            }
            // Dropping queued completions rejects their receipts. Killing the child unblocks any
            // current OS write; no teardown code needs to wait for this thread or a writer lock.
        });
        Self { tx, stopped, #[cfg(test)] inline: None }
    }

    #[cfg(test)]
    pub fn inline(writer: Box<dyn Write + Send>) -> Self {
        let (tx, _) = mpsc::channel();
        Self { tx, stopped: Arc::new(AtomicBool::new(false)), inline: Some(writer) }
    }

    pub fn send(&mut self, data: &[u8]) {
        if self.stopped.load(Ordering::Acquire) { return; }
        #[cfg(test)]
        if let Some(writer) = &mut self.inline { writer.write_all(data).unwrap(); return; }
        let _ = self.tx.send(Job::Bytes(data.to_vec()));
    }

    /// FIFO fence: acknowledge input only after all its bytes have reached the OS writer. The
    /// renderer waits for this before sending its next bounded chunk, providing backpressure.
    pub fn complete(&self, done: Completion) {
        if self.stopped.load(Ordering::Acquire) { return; }
        #[cfg(test)]
        if self.inline.is_some() { let _ = done.send(Ok(())); return; }
        let _ = self.tx.send(Job::Complete(done));
    }

    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Release);
        let _ = self.tx.send(Job::Stop);
    }
}
impl Drop for PtyWriter { fn drop(&mut self) { self.stop(); } }

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    struct BlockedWriter { entered: mpsc::Sender<()>, release: mpsc::Receiver<()>, bytes: Arc<Mutex<Vec<u8>>> }
    impl Write for BlockedWriter {
        fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
            let _ = self.entered.send(());
            self.release.recv().unwrap();
            self.bytes.lock().unwrap().extend_from_slice(data);
            Ok(data.len())
        }
        fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
    }

    #[test]
    fn blocked_os_write_does_not_hold_caller_lock_and_receipt_waits_for_delivery() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let writer = Mutex::new(PtyWriter::new(Box::new(BlockedWriter { entered: entered_tx, release: release_rx, bytes: bytes.clone() })));
        let (receipt, done) = WriteReceipt::pending();
        { let mut guard = writer.lock().unwrap(); guard.send(b"first"); guard.complete(done); }
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(writer.try_lock().is_ok(), "reader/resize/kill can take the caller lock while the PTY is full");
        assert!(matches!(receipt.0.try_recv(), Err(mpsc::TryRecvError::Empty)));
        release_tx.send(()).unwrap();
        assert_eq!(receipt.wait(), Ok(()));
        assert_eq!(*bytes.lock().unwrap(), b"first");
    }

    #[test]
    fn failed_writes_reject_receipts_and_never_report_success() {
        struct Broken;
        impl Write for Broken {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> { Err(std::io::ErrorKind::BrokenPipe.into()) }
            fn flush(&mut self) -> std::io::Result<()> { Ok(()) }
        }
        let mut writer = PtyWriter::new(Box::new(Broken));
        for _ in 0..2 {
            let (receipt, done) = WriteReceipt::pending();
            writer.send(b"input"); writer.complete(done);
            assert!(receipt.wait().unwrap_err().contains("Terminal write failed"));
        }
    }

    #[test]
    fn stop_cancels_pending_input_without_waiting_for_the_os_writer() {
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let mut writer = PtyWriter::new(Box::new(BlockedWriter { entered: entered_tx, release: release_rx, bytes: bytes.clone() }));
        writer.send(b"in flight");
        entered_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        let (receipt, done) = WriteReceipt::pending();
        writer.send(b"must not replay"); writer.complete(done);
        writer.stop();
        release_tx.send(()).unwrap();
        assert!(receipt.wait().is_err());
        assert_eq!(*bytes.lock().unwrap(), b"in flight");
    }

    #[test]
    #[cfg(unix)]
    fn real_full_pty_remains_cancellable() {
        use portable_pty::{native_pty_system, PtySize};
        // Keep the slave open without a consumer: the kernel input/echo queues fill, just like
        // an SSH child whose peer has stopped reading. No credentials or test subprocess needed.
        let pair = native_pty_system().openpty(PtySize::default()).unwrap();
        // Canonical terminals may discard an overlong line; SSH/control mode use raw input.
        let fd = pair.master.as_raw_fd().unwrap();
        unsafe {
            let mut termios: libc::termios = std::mem::zeroed();
            assert_eq!(libc::tcgetattr(fd, &mut termios), 0);
            libc::cfmakeraw(&mut termios);
            assert_eq!(libc::tcsetattr(fd, libc::TCSANOW, &termios), 0);
        }
        let mut writer = PtyWriter::new(pair.master.take_writer().unwrap());
        let (receipt, done) = WriteReceipt::pending();
        writer.send(&vec![b'x'; 1024 * 1024]); writer.complete(done);
        assert!(matches!(receipt.0.recv_timeout(Duration::from_millis(100)), Err(mpsc::RecvTimeoutError::Timeout)),
            "the real PTY must be full before testing cancellation");
        writer.stop(); // must return while the OS write is still blocked
        drop(pair.slave);
        assert!(matches!(receipt.0.recv_timeout(Duration::from_secs(2)), Err(mpsc::RecvTimeoutError::Disconnected)),
            "closing the slave cancels the pending receipt without leaving a waiting thread");
    }
}
