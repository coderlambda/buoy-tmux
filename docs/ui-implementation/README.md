# Compact blue UI implementation

Implements the approved compact blue design. Existing SSH, tmux and terminal behavior is preserved.

- English, compact chrome with the original blue accent. Dark and light appearances include matching terminal backgrounds and cursors.
- No app logo/title in the sidebar and no workspace header above the terminal. New-workspace and sidebar-collapse controls stay at the top left; terminal tabs begin at the top right.
- Workspace titles contain connection details and an explicit actions menu. Hover provides descriptions only. Rename, color, move, reconnect, detach and end remain available.
- SSH forwards stay under their owning workspace. `=` uses the same local port, external-link opens the address, and `×` stops only the forward. These controls stay visible. More forwards expand inline; occupied-port errors retain the prior mapping and show local feedback. Pending operations ignore duplicate clicks.
- History is a main view reached from the footer. Restoration reviews the saved windows and directories before opening new shells. Hidden terminals retain their buffers without issuing layout resizes; notifications still arrive while History is open.
- Creation/import use explicit icon modes, inline validation, host history and native-tab selection. Escape, the backdrop and close/cancel buttons dismiss the dialog. Destructive actions and HTML script enablement use a confirmation dialog.
- File previews retain copy-path, download and script controls as visible icons. HTML sandbox isolation is unchanged.

Validation: TypeScript checks, production frontend and Rust builds, and 53 unit tests passed. All seven native WebKit UI suites passed (rename, reorder, notifications, creation/import, terminal repaint, recovery and compact controls); the host-history test initially timed out and the creation/import suite passed on an unchanged rerun. Browser visual checks covered both appearances and widths of 1024, 736, 360 and 320 pixels.

Screenshots render the real frontend and xterm with fixture sessions, without opening live SSH connections:

- `workspace-dark.png` / `workspace-light.png`: desktop workspace.
- `new-workspace-dark.png` / `new-workspace-light.png`: creation dialog.
- `workspace-native.png`: native WebKit screenshot at the test window's compact size.
