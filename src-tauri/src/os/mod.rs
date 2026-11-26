//! Windows integration: active window, clipboard, keyboard and mouse input, screenshots,
//! URLs and PowerShell. All functions are synchronous. Async callers wrap them in
//! `tokio::task::spawn_blocking`.
//!
//! On other platforms every function returns an error, so the pure logic in this module
//! (key parsing, coordinate mapping, output truncation) still compiles and is unit-tested.

mod keys;
mod shell;
#[cfg(windows)]
mod win;

#[cfg(not(windows))]
mod win {
    //! Stubs for non-Windows builds.
    use super::ShotGeometry;
    use crate::types::{ActiveWindow, InsertMethod, Screenshot};
    use anyhow::{bail, Result};

    const MSG: &str = "this function is only available on Windows";

    pub fn foreground_hwnd() -> isize {
        0
    }
    pub fn active_window() -> Result<ActiveWindow> {
        bail!(MSG)
    }
    pub fn focus_window(_hwnd: isize) -> Result<()> {
        bail!(MSG)
    }
    pub fn clipboard_get() -> Result<String> {
        bail!(MSG)
    }
    pub fn clipboard_set(_text: &str) -> Result<()> {
        bail!(MSG)
    }
    pub fn type_text(_text: &str) -> Result<()> {
        bail!(MSG)
    }
    pub fn insert_text(_text: &str, _method: InsertMethod) -> Result<()> {
        bail!(MSG)
    }
    pub fn press_keys(_combo: &str) -> Result<()> {
        bail!(MSG)
    }
    pub fn click_abs(_x: i32, _y: i32, _button: &str) -> Result<()> {
        bail!(MSG)
    }
    pub fn screenshot_png() -> Result<(Screenshot, ShotGeometry)> {
        bail!(MSG)
    }
    pub fn open_url_checked(_url: &str) -> Result<()> {
        bail!(MSG)
    }
}

use crate::types::{ActiveWindow, CommandOutput, InsertMethod, Screenshot};
use anyhow::{bail, Result};
use std::sync::{Mutex, OnceLock};

/// How the last screenshot maps back to screen pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ShotGeometry {
    /// Physical pixels per screenshot pixel.
    pub scale: f64,
    /// Top-left corner of the captured monitor in virtual screen coordinates.
    pub origin_x: i32,
    pub origin_y: i32,
}

impl Default for ShotGeometry {
    fn default() -> Self {
        ShotGeometry { scale: 1.0, origin_x: 0, origin_y: 0 }
    }
}

static LAST_SHOT: OnceLock<Mutex<ShotGeometry>> = OnceLock::new();

fn geometry_cell() -> &'static Mutex<ShotGeometry> {
    LAST_SHOT.get_or_init(|| Mutex::new(ShotGeometry::default()))
}

fn set_geometry(g: ShotGeometry) {
    *geometry_cell().lock().unwrap_or_else(|e| e.into_inner()) = g;
}

fn get_geometry() -> ShotGeometry {
    *geometry_cell().lock().unwrap_or_else(|e| e.into_inner())
}

/// Map a point in screenshot space to physical screen coordinates.
pub(crate) fn map_point(x: i32, y: i32, g: ShotGeometry) -> (i32, i32) {
    let px = g.origin_x as f64 + (x as f64) * g.scale;
    let py = g.origin_y as f64 + (y as f64) * g.scale;
    (px.round() as i32, py.round() as i32)
}

/// Title, executable name (lower case) and handle of the foreground window.
pub fn active_window() -> Result<ActiveWindow> {
    win::active_window()
}

pub fn clipboard_get() -> Result<String> {
    win::clipboard_get()
}

pub fn clipboard_set(text: &str) -> Result<()> {
    win::clipboard_set(text)
}

/// `Paste`: save the clipboard text, set the new text, send Ctrl+V, wait 300 ms, restore the old
/// text. `Type`: send the text as Unicode keystrokes.
pub fn insert_text(text: &str, method: InsertMethod) -> Result<()> {
    win::insert_text(text, method)
}

pub fn type_text(text: &str) -> Result<()> {
    win::type_text(text)
}

/// Press a key combination such as "ctrl+shift+t", "alt+tab", "enter", "win+r", "f5".
/// Unknown key names return an error that lists the valid names.
pub fn press_keys(combo: &str) -> Result<()> {
    win::press_keys(combo)
}

/// Click at a point given in the coordinates of the last screenshot. If no screenshot has been
/// taken yet, the point is used as a physical pixel position.
pub fn mouse_click(x: i32, y: i32, button: &str) -> Result<()> {
    let (px, py) = map_point(x, y, get_geometry());
    win::click_abs(px, py, button)
}

/// Capture the primary monitor, scaled down so the width is at most 1280 pixels.
pub fn screenshot_png() -> Result<Screenshot> {
    let (shot, geometry) = win::screenshot_png()?;
    set_geometry(geometry);
    Ok(shot)
}

/// Open an http or https URL in the default browser. Other schemes are rejected.
pub fn open_url(url: &str) -> Result<()> {
    let url = validate_url(url)?;
    win::open_url_checked(&url)
}

fn validate_url(url: &str) -> Result<String> {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    if !(lower.starts_with("http://") || lower.starts_with("https://")) {
        bail!("only http and https URLs can be opened");
    }
    if url.len() > 2048 {
        bail!("URL is longer than 2048 characters");
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control() || c == '"') {
        bail!("URL contains whitespace, control characters or quotes");
    }
    let rest = &url[lower.find("://").map(|i| i + 3).unwrap_or(0)..];
    if rest.is_empty() || rest.starts_with('/') {
        bail!("URL has no host");
    }
    Ok(url.to_string())
}

/// Run a command with `powershell.exe -NoProfile -NonInteractive -Command`. The process is killed
/// when `timeout_secs` passes. stdout and stderr are each cut to 8 KB.
pub fn run_powershell(cmd: &str, timeout_secs: u64) -> Result<CommandOutput> {
    shell::run_powershell(cmd, timeout_secs)
}

/// Bring a window to the foreground, for example to restore focus before inserting dictation.
pub fn focus_window(hwnd: isize) -> Result<()> {
    win::focus_window(hwnd)
}

/// Handle of the current foreground window, or 0 when there is none.
pub fn foreground_hwnd() -> isize {
    win::foreground_hwnd()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_point_applies_scale_and_origin() {
        let g = ShotGeometry { scale: 1.5, origin_x: 100, origin_y: -50 };
        assert_eq!(map_point(0, 0, g), (100, -50));
        assert_eq!(map_point(100, 200, g), (250, 250));
        assert_eq!(map_point(10, 10, ShotGeometry::default()), (10, 10));
    }

    #[test]
    fn url_validation_accepts_http_and_https_only() {
        assert!(validate_url("https://example.com/a?b=1&c=2").is_ok());
        assert!(validate_url("HTTP://example.com").is_ok());
        assert!(validate_url("file:///C:/Windows/notepad.exe").is_err());
        assert!(validate_url("javascript:alert(1)").is_err());
        assert!(validate_url("ms-settings:").is_err());
        assert!(validate_url("https://exa mple.com").is_err());
        assert!(validate_url("https://").is_err());
        assert!(validate_url("https:///path").is_err());
    }
}
