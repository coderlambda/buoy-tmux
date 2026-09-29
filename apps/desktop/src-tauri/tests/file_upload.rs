//! Real SCP sink + private tmux server. Optional loopback SSH uses the same cases end to end.
#![cfg(unix)]
use buoy_lib::{
    file_upload::{self, Cancellation, UploadReport},
    session_store::SessionMeta,
};
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
static SEQ: AtomicU64 = AtomicU64::new(0);
struct Fixture {
    root: PathBuf,
    meta: SessionMeta,
    tmux: String,
    extra: Vec<String>,
}
impl Fixture {
    fn new(ssh: bool) -> Self {
        let root = std::env::temp_dir().join(format!(
            "buoy-upload-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        for dir in ["input", "first", "second space 中文 ' $()", "third"] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        let tmux = buoy_lib::probe::probe_local_tmux().tmux_path;
        let socket = format!(
            "buoy-upload-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        );
        let host = if ssh {
            format!(
                "127.0.0.1:{}",
                std::env::var("BUOY_UPLOAD_SSH_PORT").unwrap()
            )
        } else {
            String::new()
        };
        let meta: SessionMeta = serde_json::from_value(serde_json::json!({
            "id":"upload", "host":host, "session":"upload", "transport":if ssh {"ssh"} else {"local"},
            "mode":"control", "tmuxPath":tmux, "socketName":socket
        })).unwrap();
        let extra = if ssh {
            vec![
                "-F".into(),
                "/dev/null".into(),
                "-i".into(),
                std::env::var("BUOY_UPLOAD_SSH_KEY").unwrap(),
                "-o".into(),
                "StrictHostKeyChecking=no".into(),
                "-o".into(),
                "UserKnownHostsFile=/dev/null".into(),
            ]
        } else {
            vec![]
        };
        let fixture = Self {
            root,
            meta,
            tmux,
            extra,
        };
        fixture.tmux(&[
            "new-session",
            "-d",
            "-s",
            "upload",
            "-c",
            fixture.root.join("first").to_str().unwrap(),
        ]);
        fixture.tmux(&[
            "new-window",
            "-d",
            "-t",
            "upload",
            "-c",
            fixture.target().to_str().unwrap(),
        ]);
        fixture
    }
    fn target(&self) -> PathBuf {
        self.root.join("second space 中文 ' $()")
    }
    fn tmux(&self, args: &[&str]) {
        let result = Command::new(&self.tmux)
            .args(["-L", self.meta.socket_name.as_deref().unwrap()])
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{:?}: {}",
            args,
            String::from_utf8_lossy(&result.stderr)
        );
    }
    fn upload(&self, paths: &[PathBuf]) -> UploadReport {
        file_upload::upload(
            &self.meta,
            "@1",
            "test",
            paths,
            Arc::new(Cancellation::default()),
            &self.extra,
            |_| {},
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = Command::new(&self.tmux)
            .args([
                "-L",
                self.meta.socket_name.as_deref().unwrap(),
                "kill-server",
            ])
            .output();
        let _ = fs::remove_dir_all(&self.root);
    }
}
fn round_trip(ssh: bool) {
    let f = Fixture::new(ssh);
    let input = f.root.join("input");
    let file = input.join("-space 中文 ' $(literal).bin");
    let bytes: Vec<u8> = (0..4_000_000).map(|i| (i % 251) as u8).collect();
    fs::write(&file, &bytes).unwrap();
    let folder = input.join("folder 中文");
    fs::create_dir_all(folder.join("nested/empty")).unwrap();
    fs::write(folder.join("nested/a.txt"), "content\0中文\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&file, folder.join("link-outside")).unwrap();
    let cancel = Arc::new(Cancellation::default());
    let mut moved = false;
    let report = file_upload::upload(
        &f.meta,
        "@1",
        "job",
        &[file.clone(), folder.clone()],
        cancel,
        &f.extra,
        |progress| {
            if !moved && !progress.directory.is_empty() {
                // After target resolution, both tab selection AND its cwd change must not redirect the batch.
                f.tmux(&["select-window", "-t", "@0"]);
                f.tmux(&[
                    "respawn-pane",
                    "-k",
                    "-t",
                    "@1",
                    "-c",
                    f.root.join("third").to_str().unwrap(),
                ]);
                moved = true;
            }
        },
    )
    .unwrap();
    assert!(
        report.items.iter().all(|item| item.status == "uploaded"),
        "{report:?}"
    );
    assert_eq!(
        fs::read(f.target().join(file.file_name().unwrap())).unwrap(),
        bytes
    );
    assert_eq!(
        fs::read(f.target().join("folder 中文/nested/a.txt")).unwrap(),
        "content\0中文\n".as_bytes()
    );
    assert!(f.target().join("folder 中文/nested/empty").is_dir());
    assert!(!f.target().join("folder 中文/link-outside").exists());
    assert!(!f
        .root
        .join("first")
        .join(file.file_name().unwrap())
        .exists());
    assert!(!f
        .root
        .join("third")
        .join(file.file_name().unwrap())
        .exists());
    assert!(fs::read_dir(f.target()).unwrap().all(|e| !e
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".buoy-upload-")));
}
#[test]
fn local_scp_files_folders_and_frozen_tab_directory() {
    round_trip(false);
}
#[test]
#[ignore = "requires disposable loopback sshd"]
fn ssh_scp_files_folders_and_frozen_tab_directory() {
    round_trip(true);
}

#[test]
fn existing_items_are_skipped_including_dangling_links_and_folders() {
    let f = Fixture::new(false);
    let input = f.root.join("input");
    fs::write(input.join("file"), "new").unwrap();
    fs::write(f.target().join("file"), "old").unwrap();
    fs::create_dir(input.join("folder")).unwrap();
    fs::write(input.join("folder/new"), "new").unwrap();
    fs::create_dir(f.target().join("folder")).unwrap();
    fs::write(f.target().join("folder/old"), "old").unwrap();
    let mut sources = vec![input.join("file"), input.join("folder")];
    #[cfg(unix)]
    {
        fs::write(input.join("dangling"), "new").unwrap();
        std::os::unix::fs::symlink("missing", f.target().join("dangling")).unwrap();
        sources.push(input.join("dangling"));
    }
    let report = f.upload(&sources);
    assert!(
        report.items.iter().all(|i| i.status == "skipped"),
        "{report:?}"
    );
    assert_eq!(fs::read_to_string(f.target().join("file")).unwrap(), "old");
    assert!(!f.target().join("folder/new").exists());
}

fn cancel_transfer(ssh: bool) {
    let f = Fixture::new(ssh);
    let file = f.root.join("input/large.bin");
    fs::File::create(&file)
        .unwrap()
        .set_len(64 * 1024 * 1024)
        .unwrap();
    let cancel = Arc::new(Cancellation::default());
    let flag = cancel.clone();
    let report = file_upload::upload(
        &f.meta,
        "@1",
        "cancel",
        &[file],
        cancel,
        &f.extra,
        |progress| {
            if progress.sent >= 128 * 1024 {
                flag.cancel();
            }
        },
    )
    .unwrap();
    assert!(report.cancelled, "{report:?}");
    assert!(!f.target().join("large.bin").exists());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    while fs::read_dir(f.target()).unwrap().any(|e| {
        e.unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".buoy-upload-")
    }) && std::time::Instant::now() < deadline
    {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        fs::read_dir(f.target()).unwrap().all(|e| !e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".buoy-upload-")),
        "cancel cleans staging"
    );
}

#[test]
fn cancelled_file_never_publishes_partial_content() {
    cancel_transfer(false);
}
#[test]
#[ignore = "requires disposable loopback sshd"]
fn ssh_cancelled_file_never_publishes_partial_content() {
    cancel_transfer(true);
}

#[test]
#[ignore = "requires disposable loopback sshd"]
fn ssh_auth_failure_never_falls_back_to_local_copy() {
    let mut f = Fixture::new(true);
    f.extra
        .extend(["-o".into(), "PubkeyAuthentication=no".into()]);
    let error =
        file_upload::resolve_directory(&f.meta, "@1", &Arc::new(Cancellation::default()), &f.extra)
            .unwrap_err();
    assert!(error.contains("Permission denied"), "{error}");
    assert_eq!(fs::read_dir(f.target()).unwrap().count(), 0);
}

#[test]
fn missing_or_foreign_window_never_falls_back_to_active_directory() {
    let f = Fixture::new(false);
    let cancel = Arc::new(Cancellation::default());
    assert!(file_upload::resolve_directory(&f.meta, "@99", &cancel, &[]).is_err());
    f.tmux(&["new-session", "-d", "-s", "foreign", "-c", "/tmp"]);
    assert!(file_upload::resolve_directory(&f.meta, "@2", &cancel, &[]).is_err());
    assert!(file_upload::resolve_directory(&f.meta, "view:1", &cancel, &[]).is_err());
}

#[test]
fn concurrent_name_created_during_transfer_is_not_replaced() {
    let f = Fixture::new(false);
    let file = f.root.join("input/race");
    fs::write(&file, vec![7; 1_000_000]).unwrap();
    let mut created = false;
    let report = file_upload::upload(
        &f.meta,
        "@1",
        "race",
        &[file],
        Arc::new(Cancellation::default()),
        &[],
        |progress| {
            if progress.sent > 0 && !created {
                fs::write(f.target().join("race"), "keep").unwrap();
                created = true;
            }
        },
    )
    .unwrap();
    assert_eq!(report.items[0].status, "skipped", "{report:?}");
    assert_eq!(fs::read_to_string(f.target().join("race")).unwrap(), "keep");
}

// This process owns a real raw terminal, enables bracketed paste and records exactly the input
// delivered by tmux. It never executes pasted text, and remains alive so recipient checks are real.
fn start_receiver(f: &Fixture, suffix: &str) -> PathBuf {
    let output = f.root.join(format!("received-{suffix}"));
    let script = f.root.join(format!("receive-{suffix}.py"));
    fs::write(
        &script,
        r#"import os, sys, tty
from pathlib import Path
tty.setraw(0)
os.write(1, b'\x1b[?2004h')
path = Path(sys.argv[1])
path.touch()
with path.open('ab', buffering=0) as out:
    while True:
        out.write(os.read(0, 65536))
"#,
    )
    .unwrap();
    let python = Command::new("which").arg("python3").output().unwrap();
    let python = String::from_utf8(python.stdout).unwrap().trim().to_string();
    f.tmux(&[
        "respawn-pane",
        "-k",
        "-t",
        "@1",
        &python,
        script.to_str().unwrap(),
        output.to_str().unwrap(),
    ]);
    for _ in 0..100 {
        if output.exists() {
            return output;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    panic!("receiver did not start");
}

#[test]
fn attachments_use_separate_bracketed_pastes_and_never_submit_or_reuse_collisions() {
    let f = Fixture::new(false);
    let received = start_receiver(&f, "original");
    let paths = ["first 中文 image.png", "second'image.png", "existing.png"]
        .map(|name| f.root.join("input").join(name));
    for path in &paths {
        fs::write(path, b"upload payload").unwrap();
    }
    fs::write(f.target().join("existing.png"), b"old content").unwrap();
    let report = file_upload::upload_with_attachment(
        &f.meta,
        "@1",
        "attach",
        &paths,
        Arc::new(Cancellation::default()),
        &[],
        true,
        |_| {},
    )
    .unwrap();
    assert_eq!(
        report.items.iter().filter(|i| i.inserted).count(),
        2,
        "{report:?}"
    );
    assert_eq!(report.items[2].status, "skipped");
    let received = fs::read(received).unwrap();
    let text = String::from_utf8(received).unwrap();
    assert_eq!(text.matches("\x1b[200~").count(), 2, "{text:?}");
    assert_eq!(text.matches("\x1b[201~").count(), 2, "{text:?}");
    assert!(text.contains("first\\ 中文\\ image.png"), "{text:?}");
    assert!(text.contains("second\\'image.png"), "{text:?}");
    assert!(!text.contains("existing.png"));
    assert!(!text.contains(['\r', '\n']), "must never submit: {text:?}");
    assert_eq!(
        fs::read(f.target().join("existing.png")).unwrap(),
        b"old content"
    );
}

#[test]
fn attachments_do_not_reach_a_replaced_program_or_pane() {
    let f = Fixture::new(false);
    start_receiver(&f, "before");
    let source = f.root.join("input/image.png");
    fs::write(&source, vec![1u8; 100_000]).unwrap();
    let mut changed = false;
    let mut after = PathBuf::new();
    let report = file_upload::upload_with_attachment(
        &f.meta,
        "@1",
        "attach",
        &[source],
        Arc::new(Cancellation::default()),
        &[],
        true,
        |progress| {
            if progress.phase == "upload" && progress.sent > 0 && !changed {
                changed = true;
                after = start_receiver(&f, "after");
            }
        },
    )
    .unwrap();
    assert!(changed);
    assert_eq!(report.items[0].status, "uploaded");
    assert!(!report.items[0].inserted);
    assert!(report.items[0].detail.contains("changed"), "{report:?}");
    assert!(fs::read(after).unwrap().is_empty());
}

#[test]
fn cancelled_upload_never_inserts_even_completed_files() {
    let f = Fixture::new(false);
    let received = start_receiver(&f, "cancel");
    let source = f.root.join("input/image.png");
    fs::write(&source, b"payload").unwrap();
    let cancel = Arc::new(Cancellation::default());
    let report = file_upload::upload_with_attachment(
        &f.meta,
        "@1",
        "attach",
        &[source],
        cancel.clone(),
        &[],
        true,
        |progress| {
            if progress.phase == "attach" {
                cancel.cancel();
            }
        },
    )
    .unwrap();
    assert!(report.cancelled);
    assert_eq!(report.items[0].status, "uploaded");
    assert!(!report.items[0].inserted);
    assert!(fs::read(received).unwrap().is_empty());
}
