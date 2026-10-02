use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};

pub const REMOTE_STATE_SCHEMA_VERSION: u32 = 1;
pub const REMOTE_STATE_FILE_CAP: usize = 256 * 1024;
pub const REMOTE_STATE_RECORD_CAP: usize = 256;
pub const REMOTE_STATE_LIST_CAP: usize = 1024 * 1024;

/// Portable, non-secret state shared by Buoy clients through the SSH host. tmux remains the
/// authority for live processes; this record only supplies identity, presentation and a recipe for
/// an explicitly closed workspace. Credentials and per-device selection never belong here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSessionRecord {
    pub schema_version: u32,
    pub revision: u64,
    pub identity: RemoteSessionIdentity,
    pub runtime: RemoteSessionRuntime,
    pub display: RemoteSessionDisplay,
    pub lifecycle: RemoteSessionLifecycle,
    #[serde(default)]
    pub recovery: RemoteSessionRecovery,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSessionIdentity {
    pub socket_name: String,
    pub session: String,
    #[serde(default)]
    pub tmux_created_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSessionRuntime {
    pub mode: String,
    pub tmux_path: String,
    #[serde(default)]
    pub tmux_version: Option<Vec<u32>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSessionDisplay {
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSessionLifecycle {
    pub state: RemoteLifecycleState,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RemoteLifecycleState {
    Active,
    Closing,
    Closed,
    Restoring,
    Deleted,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteSessionRecovery {
    #[serde(default)]
    pub tabs: Vec<RemoteRecoveryTab>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteRecoveryTab {
    pub window: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub shell: String,
    #[serde(default)]
    pub last_command: String,
}

impl RemoteSessionRecord {
    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != REMOTE_STATE_SCHEMA_VERSION {
            return Err("unsupported remote state schema".into());
        }
        validate_socket_name(&self.identity.socket_name)?;
        validate_session_name(&self.identity.session)?;
        if !matches!(self.runtime.mode.as_str(), "control" | "plain") {
            return Err("invalid remote session mode".into());
        }
        if self.runtime.tmux_path.is_empty()
            || self.runtime.tmux_path.len() > 512
            || !self.runtime.tmux_path.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '/' | '-')
            })
        {
            return Err("invalid remote tmux path".into());
        }
        validate_clean_text(&self.display.title, 120, "remote title")?;
        if self.recovery.tabs.len() > 64 {
            return Err("too many remote recovery tabs".into());
        }
        for tab in &self.recovery.tabs {
            validate_clean_text(&tab.window, 64, "recovery window")?;
            validate_clean_text(&tab.title, 256, "recovery title")?;
            validate_clean_text(&tab.cwd, 4096, "recovery cwd")?;
            validate_clean_text(&tab.shell, 512, "recovery shell")?;
            validate_clean_text(&tab.last_command, 512, "recovery command")?;
        }
        Ok(())
    }
}

fn validate_clean_text(value: &str, cap: usize, field: &str) -> Result<(), String> {
    if value.chars().count() > cap || value.chars().any(char::is_control) {
        Err(format!("invalid {field}"))
    } else {
        Ok(())
    }
}

/// Shell program used on both local and SSH transports. It lists only regular, non-symlink files
/// below `$HOME/.buoy/v1/sessions`, and base64 frames every JSON document so arbitrary titles can
/// never affect line parsing.
pub fn remote_state_list_script() -> String {
    format!(
        "buoy_state_root=\"$HOME/.buoy/v1/sessions\"; buoy_state_count=0; buoy_state_total=0; \
         if [ -d \"$buoy_state_root\" ] && [ ! -L \"$buoy_state_root\" ]; then \
           for buoy_state_dir in \"$buoy_state_root\"/*; do \
             [ -d \"$buoy_state_dir\" ] && [ ! -L \"$buoy_state_dir\" ] || continue; \
             for buoy_state_file in \"$buoy_state_dir\"/*.json; do \
               [ -f \"$buoy_state_file\" ] && [ ! -L \"$buoy_state_file\" ] || continue; \
               buoy_state_size=$(wc -c < \"$buoy_state_file\" 2>/dev/null) || continue; \
               [ \"$buoy_state_size\" -le {REMOTE_STATE_FILE_CAP} ] || continue; \
               buoy_state_total=$((buoy_state_total + buoy_state_size)); \
               [ \"$buoy_state_total\" -le {REMOTE_STATE_LIST_CAP} ] || break 2; \
               printf 'BUOY_STATE\\t'; base64 < \"$buoy_state_file\" | tr -d '\\n'; printf '\\n'; \
               buoy_state_count=$((buoy_state_count + 1)); \
               [ \"$buoy_state_count\" -lt {REMOTE_STATE_RECORD_CAP} ] || break 2; \
             done; \
           done; \
         fi"
    )
}

/// Build an atomic, mode-restricted write under `$HOME/.buoy/v1`. Every pathname component comes
/// from the narrow tmux identity validators; the JSON itself is base64 data, never shell syntax.
pub fn remote_state_write_script(record: &RemoteSessionRecord) -> Result<String, String> {
    record.validate()?;
    let json = serde_json::to_vec(record).map_err(|error| error.to_string())?;
    if json.len() > REMOTE_STATE_FILE_CAP {
        return Err("remote state record is too large".into());
    }
    let payload = STANDARD.encode(json);
    let socket = &record.identity.socket_name;
    let session = &record.identity.session;
    Ok(format!(
        "umask 077; buoy_root=\"$HOME/.buoy/v1\"; buoy_sessions=\"$buoy_root/sessions\"; \
         buoy_locks=\"$buoy_root/locks\"; buoy_dir=\"$buoy_sessions/{socket}\"; \
         buoy_lock=\"$buoy_locks/{socket}--{session}.lock\"; \
         for buoy_path in \"$HOME/.buoy\" \"$buoy_root\" \"$buoy_sessions\" \"$buoy_locks\" \"$buoy_dir\"; do \
           [ ! -L \"$buoy_path\" ] || {{ printf '%s\\n' BUOY_STATE_SYMLINK >&2; exit 70; }}; \
           mkdir -p \"$buoy_path\" || exit 71; chmod 700 \"$buoy_path\" || exit 72; \
         done; \
         mkdir \"$buoy_lock\" 2>/dev/null || {{ printf '%s\\n' BUOY_STATE_LOCKED >&2; exit 73; }}; \
         buoy_tmp=\"$buoy_dir/.{session}.$$\"; \
         trap 'rm -f \"$buoy_tmp\"; rmdir \"$buoy_lock\" 2>/dev/null || :' EXIT HUP INT TERM; \
         printf '%s' {payload} | base64 -d > \"$buoy_tmp\" || exit 74; \
         chmod 600 \"$buoy_tmp\" || exit 75; \
         mv -f \"$buoy_tmp\" \"$buoy_dir/{session}.json\" || exit 76"
    ))
}

/// Decode and validate records emitted by `remote_state_list_script`. Malformed or oversized
/// records are ignored independently so one stale file never hides healthy sessions.
pub fn parse_remote_state_listing(stdout: &[u8]) -> Vec<RemoteSessionRecord> {
    let mut records = Vec::new();
    for encoded in String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(|line| line.strip_prefix("BUOY_STATE\t"))
        .take(REMOTE_STATE_RECORD_CAP)
    {
        let Ok(bytes) = STANDARD.decode(encoded) else { continue };
        if bytes.len() > REMOTE_STATE_FILE_CAP { continue; }
        let Ok(record) = serde_json::from_slice::<RemoteSessionRecord>(&bytes) else { continue };
        if record.validate().is_ok() { records.push(record); }
    }
    records
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshTarget {
    pub user: Option<String>,
    pub host: String,
    pub port: u16,
}

/// Parse the renderer's `[user@]host[:port]` form without ever passing it through a shell.
/// Bracketed and bare IPv6 are accepted; a mobile connection requires an explicit user later.
pub fn parse_ssh_target(input: &str) -> Result<SshTarget, String> {
    if input.is_empty() || input.len() > 255 {
        return Err("host is empty or too long".into());
    }
    let (user, rest) = match input.find('@') {
        Some(at) => {
            let user = &input[..at];
            if !valid_user(user) {
                return Err("invalid SSH user".into());
            }
            (Some(user.to_string()), &input[at + 1..])
        }
        None => (None, input),
    };

    let (host, port, ipv6) = if let Some(stripped) = rest.strip_prefix('[') {
        let close = stripped.find(']').ok_or("unterminated IPv6 bracket")?;
        let host = &stripped[..close];
        let tail = &stripped[close + 1..];
        let port = if tail.is_empty() {
            22
        } else if let Some(value) = tail.strip_prefix(':') {
            parse_port(value)?
        } else {
            return Err("garbage after IPv6 bracket".into());
        };
        (host.to_string(), port, true)
    } else if rest.matches(':').count() >= 2 {
        (rest.to_string(), 22, true)
    } else if let Some(colon) = rest.find(':') {
        (
            rest[..colon].to_string(),
            parse_port(&rest[colon + 1..])?,
            false,
        )
    } else {
        (rest.to_string(), 22, false)
    };

    let valid_host = if ipv6 {
        !host.is_empty() && host.chars().all(|c| c.is_ascii_hexdigit() || c == ':')
    } else {
        valid_dns_host(&host)
    };
    if !valid_host {
        return Err("invalid SSH host".into());
    }
    Ok(SshTarget { user, host, port })
}

pub fn validate_session_name(value: &str) -> Result<(), String> {
    let mut chars = value.chars();
    if value.is_empty()
        || value.len() > 64
        || !matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
    {
        return Err("invalid tmux session name".into());
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') {
        return Err("invalid tmux session name".into());
    }
    Ok(())
}

/// tmux socket names are passed to `tmux -L`, so keep the same deliberately narrow alphabet as
/// session names. `default` is used when adopting a session created outside Buoy.
pub fn validate_socket_name(value: &str) -> Result<(), String> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Err("invalid tmux socket name".into());
    }
    Ok(())
}

/// Return the stable tmux socket used by every Buoy client for a session.
///
/// The name is deliberately platform-neutral: Desktop and Mobile must derive the same remote
/// identity so either client can discover and reattach to a session created by the other. The
/// version tag prevents an upgraded tmux client from talking to an incompatible older server.
/// Control mode keeps one server per session; plain mode shares one server per tmux version.
pub fn tmux_socket_name(mode: &str, version: Option<(u32, u32)>, session: &str) -> String {
    let tag = version
        .map(|(major, minor)| format!("{major}-{minor}"))
        .unwrap_or_default();
    if mode == "control" {
        format!("dtcc{tag}-{session}")
    } else {
        format!("dtapp{tag}")
    }
}

fn valid_user(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn valid_dns_host(value: &str) -> bool {
    let mut chars = value.chars();
    matches!(chars.next(), Some(c) if c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-'))
}

fn parse_port(value: &str) -> Result<u16, String> {
    if value.is_empty() || value.len() > 5 || !value.chars().all(|c| c.is_ascii_digit()) {
        return Err("invalid SSH port".into());
    }
    value
        .parse::<u16>()
        .ok()
        .filter(|port| *port > 0)
        .ok_or_else(|| "SSH port out of range".into())
}

/// Platform capabilities are part of the runtime contract. The frontend renders from these flags
/// instead of spreading target checks throughout product code.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeCapabilities {
    pub platform: &'static str,
    pub local_shell: bool,
    pub native_tabs: bool,
    pub port_forwarding: bool,
    pub background_connection: bool,
    pub file_download: bool,
    pub ssh_host_key_verification: bool,
}

impl RuntimeCapabilities {
    pub const fn desktop() -> Self {
        Self {
            platform: "desktop",
            local_shell: true,
            native_tabs: true,
            port_forwarding: true,
            background_connection: true,
            file_download: true,
            ssh_host_key_verification: true,
        }
    }

    pub const fn mobile() -> Self {
        Self {
            platform: "mobile",
            local_shell: false,
            native_tabs: true,
            port_forwarding: true,
            background_connection: false,
            file_download: true,
            ssh_host_key_verification: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote_record() -> RemoteSessionRecord {
        RemoteSessionRecord {
            schema_version: REMOTE_STATE_SCHEMA_VERSION,
            revision: 42,
            identity: RemoteSessionIdentity {
                socket_name: "dtcc3-7-dt-shared".into(),
                session: "dt-shared".into(),
                tmux_created_at: Some(123),
            },
            runtime: RemoteSessionRuntime {
                mode: "control".into(),
                tmux_path: "/opt/homebrew/bin/tmux".into(),
                tmux_version: Some(vec![3, 7]),
            },
            display: RemoteSessionDisplay { title: "Shared workspace".into() },
            lifecycle: RemoteSessionLifecycle {
                state: RemoteLifecycleState::Closed,
                updated_at: 42,
            },
            recovery: RemoteSessionRecovery {
                tabs: vec![RemoteRecoveryTab {
                    window: "@1".into(),
                    title: "editor".into(),
                    cwd: "/Users/example/project".into(),
                    shell: "/bin/zsh".into(),
                    last_command: "codex".into(),
                }],
            },
        }
    }

    #[test]
    fn mobile_contract_is_remote_foreground_only() {
        let capabilities = RuntimeCapabilities::mobile();
        assert_eq!(capabilities.platform, "mobile");
        assert!(!capabilities.local_shell);
        assert!(!capabilities.background_connection);
        assert!(capabilities.native_tabs);
        assert!(capabilities.port_forwarding);
        assert!(capabilities.file_download);
        assert!(capabilities.ssh_host_key_verification);
    }

    #[test]
    fn parses_mobile_ssh_targets_without_shell_syntax() {
        assert_eq!(
            parse_ssh_target("alice@vpn-host:2202").unwrap(),
            SshTarget {
                user: Some("alice".into()),
                host: "vpn-host".into(),
                port: 2202
            },
        );
        assert_eq!(
            parse_ssh_target("alice@[fd00::1]:22").unwrap().host,
            "fd00::1"
        );
        assert!(parse_ssh_target("-oProxyCommand=bad").is_err());
        assert!(validate_session_name("dt-mobile_1").is_ok());
        assert!(validate_session_name("bad;command").is_err());
        assert!(validate_socket_name("default").is_ok());
        assert!(validate_socket_name("buoy-mobile_dt-1").is_ok());
        assert!(validate_socket_name("bad;command").is_err());
    }

    #[test]
    fn tmux_socket_identity_is_client_neutral_and_versioned() {
        assert_eq!(
            tmux_socket_name("control", Some((3, 7)), "dt-shared"),
            "dtcc3-7-dt-shared"
        );
        assert_eq!(
            tmux_socket_name("plain", Some((3, 7)), "dt-shared"),
            "dtapp3-7"
        );
        assert!(!tmux_socket_name("control", None, "dt-shared").contains("mobile"));
    }

    #[test]
    fn remote_state_round_trips_through_framed_listing() {
        let record = remote_record();
        let bytes = serde_json::to_vec(&record).unwrap();
        let listing = format!("ignored\nBUOY_STATE\t{}\n", STANDARD.encode(bytes));
        assert_eq!(parse_remote_state_listing(listing.as_bytes()), vec![record]);
    }

    #[test]
    fn remote_state_paths_and_payload_never_become_shell_syntax() {
        let record = remote_record();
        let script = remote_state_write_script(&record).unwrap();
        assert!(script.contains("$HOME/.buoy/v1"));
        assert!(script.contains("umask 077"));
        assert!(script.contains("mv -f"));
        assert!(!script.contains(&record.display.title));

        let mut invalid = record;
        invalid.identity.session = "bad; touch pwned".into();
        assert!(remote_state_write_script(&invalid).is_err());
    }

    #[test]
    fn remote_state_listing_ignores_bad_records_independently() {
        let listing = b"BUOY_STATE\tnot-base64!!\nBUOY_STATE\te30=\n";
        assert!(parse_remote_state_listing(listing).is_empty());
    }

    #[test]
    fn remote_state_script_writes_the_versioned_private_layout_atomically() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let home = std::env::temp_dir().join(format!(
                "buoy-remote-state-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
            ));
            std::fs::create_dir_all(&home).unwrap();
            let record = remote_record();
            let status = std::process::Command::new("/bin/sh")
                .args(["-c", &remote_state_write_script(&record).unwrap()])
                .env("HOME", &home)
                .status()
                .unwrap();
            assert!(status.success());
            let root = home.join(".buoy/v1");
            let file = root
                .join("sessions/dtcc3-7-dt-shared/dt-shared.json");
            assert_eq!(
                serde_json::from_slice::<RemoteSessionRecord>(&std::fs::read(&file).unwrap())
                    .unwrap(),
                record,
            );
            assert_eq!(std::fs::metadata(&root).unwrap().permissions().mode() & 0o777, 0o700);
            assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
            let listing = std::process::Command::new("/bin/sh")
                .args(["-c", &remote_state_list_script()])
                .env("HOME", &home)
                .output()
                .unwrap();
            assert!(listing.status.success());
            assert_eq!(parse_remote_state_listing(&listing.stdout), vec![record]);
            let _ = std::fs::remove_dir_all(home);
        }
    }
}
