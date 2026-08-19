//! Native drop ingress for the main WebviewWindow. Tauri routes its WindowContent webview's
//! drag events to WindowEvent::DragDrop, not WebviewEvent::DragDrop (which is for child views).
use crate::file_upload::UploadState;
use serde_json::{json, Value};
use tauri::{DragDropEvent, WindowEvent};

pub fn handle_window_event(
    label: &str,
    event: &WindowEvent,
    uploads: &UploadState,
) -> Option<Value> {
    if label != "main" {
        return None;
    }
    match event {
        WindowEvent::DragDrop(DragDropEvent::Enter { paths, .. }) if !paths.is_empty() => {
            Some(json!({ "kind": "enter", "count": paths.len() }))
        }
        WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. }) if !paths.is_empty() => {
            let token = uploads.dropped(paths.clone());
            Some(json!({ "kind": "drop", "count": paths.len(), "token": token }))
        }
        // Text/URL drags can have no file paths. Dismiss the overlay without minting a grant.
        WindowEvent::DragDrop(
            DragDropEvent::Leave | DragDropEvent::Enter { .. } | DragDropEvent::Drop { .. },
        ) => Some(json!({ "kind": "leave" })),
        WindowEvent::Destroyed => {
            uploads.cancel_all();
            None
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{path::PathBuf, sync::atomic::Ordering};

    fn drop_event(paths: Vec<PathBuf>) -> WindowEvent {
        WindowEvent::DragDrop(DragDropEvent::Drop {
            paths,
            position: (10.0, 20.0).into(),
        })
    }

    #[test]
    fn window_drop_grants_exact_native_paths_once() {
        let uploads = UploadState::default();
        let paths = vec![
            PathBuf::from("/tmp/图片 ' $().png"),
            PathBuf::from("/tmp/A folder"),
        ];
        let payload = handle_window_event("main", &drop_event(paths.clone()), &uploads).unwrap();
        assert_eq!(payload["kind"], "drop");
        assert_eq!(payload["count"], 2);
        assert!(payload.get("paths").is_none());
        let token = payload["token"].as_str().unwrap();
        let (granted, _) = uploads.begin(token).unwrap();
        assert_eq!(granted, paths);
        uploads.finish(token);
        assert!(uploads.begin(token).is_err());
    }

    #[test]
    fn window_drag_enter_over_leave_do_not_mint_grants() {
        let uploads = UploadState::default();
        let enter = WindowEvent::DragDrop(DragDropEvent::Enter {
            paths: vec!["/tmp/image.png".into()],
            position: (1.0, 2.0).into(),
        });
        assert_eq!(
            handle_window_event("main", &enter, &uploads),
            Some(json!({ "kind": "enter", "count": 1 }))
        );
        let over = WindowEvent::DragDrop(DragDropEvent::Over {
            position: (3.0, 4.0).into(),
        });
        assert!(handle_window_event("main", &over, &uploads).is_none());
        let leave = WindowEvent::DragDrop(DragDropEvent::Leave);
        assert_eq!(
            handle_window_event("main", &leave, &uploads),
            Some(json!({ "kind": "leave" }))
        );
        assert!(uploads.begin("no-drop").is_err());
    }

    #[test]
    fn empty_drop_does_not_replace_a_file_grant() {
        let uploads = UploadState::default();
        let event = drop_event(vec!["/tmp/file.txt".into()]);
        let payload = handle_window_event("main", &event, &uploads).unwrap();
        assert_eq!(
            handle_window_event("main", &drop_event(vec![]), &uploads),
            Some(json!({ "kind": "leave" }))
        );
        assert!(uploads.begin(payload["token"].as_str().unwrap()).is_ok());
    }

    #[test]
    fn other_windows_cannot_grant_paths_or_cancel_the_upload() {
        let uploads = UploadState::default();
        let event = drop_event(vec!["/tmp/file.txt".into()]);
        let payload = handle_window_event("main", &event, &uploads).unwrap();
        assert!(handle_window_event("preview", &event, &uploads).is_none());
        let (_, cancel) = uploads.begin(payload["token"].as_str().unwrap()).unwrap();
        assert!(handle_window_event("preview", &WindowEvent::Destroyed, &uploads).is_none());
        assert!(!cancel.load(Ordering::Relaxed));
        assert!(handle_window_event("main", &WindowEvent::Destroyed, &uploads).is_none());
        assert!(cancel.load(Ordering::Relaxed));
    }
}
