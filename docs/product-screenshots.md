# Product screenshots

The current README and website screenshots use the v0.1.4 frontend with demonstration
workspaces and reserved `.example` hostnames. They are captures of the actual renderer,
xterm, file viewer, and upload UI, not artwork or a redesigned mock interface.

The screenshot fixture replaces only the Tauri command/event boundary using the existing
UI test bridge. It never connects to SSH or reads the owner's sessions, files, or tokens.
Browser-rendered screenshots omit the operating system's title bar; native window chrome
can differ. Do not present the demonstration output as a real server or test run.

## Capture

```sh
npm ci
node test/product-preview.mjs
```

Open http://127.0.0.1:4174/product-preview. The 1200 × 760 frame contains the app;
the navigation above it is screenshot tooling and must stay outside the captured region.
Capture the frame with the browser screenshot tool. Move the pointer to the tooling header
before capture, so it does not obscure the interface.

| Image | Scene / action |
| --- | --- |
| `workspace-overview.png` | Workspace, with the shell tab selected |
| `html-preview.png` | Click `reports/preview.html` in the shell output |
| `file-preview.png` | Click `docs/plan.md` in the shell output |
| `localhost-tunnel.png` | Tunnel scene, with the server tab selected |
| `file-upload.png` | Upload scene; wait until Upload complete appears |
| `file-drop.png` | Drop scene; wait until the drop target appears |

Save captures in `docs/screenshots/` and copy the same files to `website/assets/`.
Inspect each capture before publication. Update alt text and copy if the interaction changes.
The fixture is local development tooling; it is neither bundled into Buoy nor included
in the Pages deployment artifact.

## September 22, 2026 refresh

The official v0.1.4 universal macOS app passed Developer ID signature and notarization
checks. A native smoke check confirmed that its compact UI started and reattached an
existing local tmux workspace before the screenshots were refreshed.

The public description now prioritizes five capabilities: stable tmux connections;
in-place file and HTML previews; localhost address recognition; automatic SSH tunnels;
and file/folder drops. Normal website and localhost URLs open in the system browser.
