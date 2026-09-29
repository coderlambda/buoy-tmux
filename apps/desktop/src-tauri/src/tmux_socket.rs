//! Version-tagged tmux socket names (DESIGN.md §12/§18). Tag by MAJOR-MINOR so a 3.5 server and a
//! 3.7 client never share a socket (the "connected but no output" bug after a tmux upgrade).
//!
//! CONTROL mode gets a PER-SESSION socket (its own tmux server), isolating workspace lifecycle and
//! control bookkeeping. Multiple Buoy clients may still attach to that same session/socket. The
//! session name is charset-safe ([A-Za-z0-9_-], validated), so it's a valid socket filename, and
//! it's stable across reconnects (derived from the id) so a reconnect reattaches the SAME server.
//!
//! PLAIN mode keeps a shared version-tagged socket.

/// `mode` is "control" or "plain"; `version` is (major, minor) if known; `session` is the tmux
/// session name (used to make control-mode sockets per-session).
pub fn socket_name(mode: &str, version: Option<(u32, u32)>, session: &str) -> String {
    buoy_core::tmux_socket_name(mode, version, session)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tc_ts1_prefixes() {
        assert_eq!(socket_name("control", Some((3, 7)), "dt-abc"), "dtcc3-7-dt-abc");
        assert_eq!(socket_name("plain", Some((3, 7)), "dt-abc"), "dtapp3-7");
    }

    #[test]
    fn tc_ts2_minor_bump_changes_socket() {
        assert_ne!(socket_name("control", Some((3, 5)), "s"), socket_name("control", Some((3, 7)), "s"));
    }

    #[test]
    fn tc_ts3_control_sockets_are_per_session() {
        // Two different workspaces get separate servers; clients sharing one workspace derive the
        // same socket.
        assert_ne!(socket_name("control", Some((3, 7)), "s1"), socket_name("control", Some((3, 7)), "s2"));
    }

    #[test]
    fn tc_ts4_unknown_version() {
        assert_eq!(socket_name("control", None, "s"), "dtcc-s");
        assert_eq!(socket_name("plain", None, "s"), "dtapp");
    }
}
