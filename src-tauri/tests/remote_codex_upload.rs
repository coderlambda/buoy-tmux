//! Opt-in acceptance test against a real Codex composer in a private remote tmux window.
//! Set BUOY_CODEX_UPLOAD_META to a SessionMeta JSON, BUOY_CODEX_UPLOAD_WINDOW to its @N id,
//! and BUOY_CODEX_UPLOAD_CWD to that pane's temporary directory. No prompt is submitted.
#![cfg(unix)]
use buoy_lib::{
    file_upload::{self, Cancellation},
    session_store::SessionMeta,
    validation,
};
use std::{fs, path::PathBuf, process::Command, sync::Arc, time::Duration};
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}
fn remote(meta: &SessionMeta, script: &str) -> String {
    let out = Command::new("ssh")
        .args([
            "-o",
            "BatchMode=yes",
            "-o",
            "ConnectTimeout=8",
            "--",
            &meta.host,
            script,
        ])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}
#[test]
#[ignore = "requires an explicitly selected SSH host and a private Codex composer"]
fn real_remote_codex_receives_images_without_submitting() {
    let meta: SessionMeta =
        serde_json::from_str(&std::env::var("BUOY_CODEX_UPLOAD_META").unwrap()).unwrap();
    let win = std::env::var("BUOY_CODEX_UPLOAD_WINDOW").unwrap();
    let directory = std::env::var("BUOY_CODEX_UPLOAD_CWD").unwrap();
    let tmux = format!(
        "{} -L {}",
        quote(meta.tmux_path.as_deref().unwrap()),
        quote(meta.socket_name.as_deref().unwrap())
    );
    let capture = format!("{tmux} capture-pane -p -t {}", quote(&win));
    let initial = remote(&meta, &capture);
    assert!(
        initial.contains("OpenAI Codex") && !initial.contains("Do you trust"),
        "Start an empty Codex composer first: {initial}"
    );
    assert!(
        !initial.contains("[Image #"),
        "Use a fresh, private Codex composer"
    );
    remote(
        &meta,
        &format!(
            "{tmux} send-keys -l -t {} {}",
            quote(&win),
            quote("Draft stays: compare these images ")
        ),
    );
    let root = std::env::temp_dir().join(format!("buoy-codex-upload-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    // Valid PNG, with names covering both whitespace and literal shell syntax.
    let png = validation::base64_decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aWQAAAABJRU5ErkJggg==").unwrap();
    let paths: Vec<PathBuf> = [
        "first 中文 image.png",
        "second ' image.png",
        "third $(literal) \"quoted\".png",
    ]
    .iter()
    .map(|name| root.join(name))
    .collect();
    for path in &paths {
        fs::write(path, &png).unwrap();
    }
    let report = file_upload::upload_with_attachment(
        &meta,
        &win,
        "remote-codex",
        &paths,
        Arc::new(Cancellation::default()),
        &[],
        true,
        |_| {},
    )
    .unwrap();
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    assert_eq!(report.directory, directory);
    assert!(
        report
            .items
            .iter()
            .all(|item| item.status == "uploaded" && item.inserted),
        "{report:?}"
    );
    let mut screen = String::new();
    for _ in 0..50 {
        screen = remote(&meta, &capture);
        if screen.contains("[Image #1]")
            && screen.contains("[Image #2]")
            && screen.contains("[Image #3]")
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    println!("REMOTE CODEX SCREEN:\n{screen}");
    assert!(
        screen.contains("[Image #1]")
            && screen.contains("[Image #2]")
            && screen.contains("[Image #3]"),
        "Codex did not show three image attachments"
    );
    assert!(
        screen.contains("Draft stays: compare these images"),
        "existing draft was lost"
    );
    assert!(
        !screen.contains("Working ("),
        "drop must not submit a prompt"
    );
    // Repeat the same drop: both items must be skipped without inserting old destination files.
    let repeated = file_upload::upload_with_attachment(
        &meta,
        &win,
        "repeat",
        &paths,
        Arc::new(Cancellation::default()),
        &[],
        true,
        |_| {},
    )
    .unwrap();
    assert!(repeated
        .items
        .iter()
        .all(|item| item.status == "skipped" && !item.inserted));
    let screen = remote(&meta, &capture);
    assert!(
        !screen.contains("[Image #4]"),
        "collision created an unintended attachment"
    );
    for path in &paths {
        let destination = format!(
            "{directory}/{}",
            path.file_name().unwrap().to_str().unwrap()
        );
        let encoded = remote(&meta, &format!("base64 < {}", quote(&destination)));
        assert_eq!(validation::base64_decode(&encoded).unwrap(), png);
    }
    let document = root.join("notes 中文.txt");
    fs::write(&document, "document contents\n").unwrap();
    let folder = root.join("folder inputs");
    fs::create_dir_all(folder.join("nested/empty")).unwrap();
    fs::write(folder.join("nested/readme.txt"), "folder contents\n").unwrap();
    let references = file_upload::upload_with_attachment(
        &meta,
        &win,
        "files",
        &[document, folder],
        Arc::new(Cancellation::default()),
        &[],
        true,
        |_| {},
    )
    .unwrap();
    assert!(
        references
            .items
            .iter()
            .all(|item| item.status == "uploaded" && item.inserted),
        "{references:?}"
    );
    let screen = remote(&meta, &capture);
    println!("REMOTE FILE/FOLDER REFERENCES:\n{screen}");
    assert!(
        screen.contains("Draft stays:") && screen.contains("notes") && screen.contains("folder")
    );
    assert!(!screen.contains("[Image #4]"));
    remote(
        &meta,
        &format!(
            "test -d {} && test -f {}",
            quote(&format!("{directory}/folder inputs/nested/empty")),
            quote(&format!("{directory}/folder inputs/nested/readme.txt"))
        ),
    );
    fs::remove_dir_all(root).unwrap();
}
