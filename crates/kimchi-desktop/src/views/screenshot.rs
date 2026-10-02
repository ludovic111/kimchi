//! `ui.screenshot`: a PNG of the kimchi window, as the window server shows it
//! (macOS `screencapture -l <window id>`, so other windows never end up in it).

use gpui::Window;
use kimchi_control::CmdResult;
use serde_json::{Value, json};

pub fn capture(path: Option<&str>, window: &mut Window) -> CmdResult<Value> {
    let path = path.map(std::path::PathBuf::from).unwrap_or_else(|| std::env::temp_dir().join(format!("kimchi-{}.png", chrono::Utc::now().format("%Y%m%d-%H%M%S"))));
    #[cfg(target_os = "macos")]
    {
        let id = window_number(window).ok_or("Couldn't find the kimchi window.")?;
        let out = std::process::Command::new("screencapture").args(["-x", "-o", &format!("-l{id}")]).arg(&path).output().map_err(|e| format!("screencapture: {e}"))?;
        if !out.status.success() || !path.exists() {
            let detail = String::from_utf8_lossy(&out.stderr).trim().to_string();
            return Err(format!(
                "The screenshot of window {id} failed{}. macOS may need Screen Recording permission for kimchi (System Settings › Privacy & Security).",
                if detail.is_empty() { String::new() } else { format!(" ({detail})") }
            ));
        }
        let b = window.bounds();
        Ok(json!({ "path": path, "width": f32::from(b.size.width), "height": f32::from(b.size.height) }))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (path, window);
        Err("ui.screenshot is only available on macOS for now.".into())
    }
}

/// The window server's number for this window (`[[view window] windowNumber]`).
#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)] // objc's msg_send! checks a cfg newer compilers don't know.
fn window_number(window: &Window) -> Option<i64> {
    use objc::runtime::Object;
    use objc::{msg_send, sel, sel_impl};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    let handle = HasWindowHandle::window_handle(window).ok()?;
    let RawWindowHandle::AppKit(h) = handle.as_raw() else { return None };
    let view = h.ns_view.as_ptr() as *mut Object;
    // SAFETY: ns_view is the live NSView of this window, used on the main thread.
    unsafe {
        let ns_window: *mut Object = msg_send![view, window];
        if ns_window.is_null() {
            return None;
        }
        let n: i64 = msg_send![ns_window, windowNumber];
        Some(n)
    }
}
