//! Windows implementations. Every function here is synchronous and blocking.
//! Callers wrap them in `spawn_blocking`. arboard and enigo objects are created per call.

use super::keys::{self, KeyToken, Modifier, NamedKey};
use super::ShotGeometry;
use crate::types::{ActiveWindow, InsertMethod, Screenshot};
use anyhow::{anyhow, bail, Context, Result};
use arboard::Clipboard;
use enigo::{Button, Coordinate, Direction, Enigo, Key, Keyboard, Mouse, Settings};
use std::ffi::c_void;
use std::time::Duration;
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow,
    SetForegroundWindow, ShowWindow, SW_RESTORE,
};

fn hwnd_from_isize(h: isize) -> HWND {
    HWND(h as *mut c_void)
}

fn new_enigo() -> Result<Enigo> {
    Enigo::new(&Settings::default()).map_err(|e| anyhow!("could not initialise keyboard and mouse input: {e}"))
}

// ---- window information ----

pub fn foreground_hwnd() -> isize {
    // SAFETY: GetForegroundWindow takes no arguments and returns a handle value or null.
    let hwnd = unsafe { GetForegroundWindow() };
    hwnd.0 as isize
}

fn window_title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    // SAFETY: the slice is valid for the duration of the call.
    let len = unsafe { GetWindowTextW(hwnd, &mut buf) };
    if len <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..len as usize])
}

fn process_exe_name(hwnd: HWND) -> Result<String> {
    let mut pid: u32 = 0;
    // SAFETY: pid is a valid out pointer for the call.
    let thread = unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
    if thread == 0 || pid == 0 {
        bail!("could not read the process id of the foreground window");
    }
    // SAFETY: plain Win32 call, the handle is closed below.
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }
        .context("could not open the foreground process")?;
    let mut buf = [0u16; 1024];
    let mut size = buf.len() as u32;
    // SAFETY: buf and size describe a valid writable buffer.
    let res = unsafe {
        QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut size)
    };
    // SAFETY: handle came from OpenProcess above and is closed once.
    unsafe {
        let _ = CloseHandle(handle);
    }
    res.context("could not read the process image name")?;
    let path = String::from_utf16_lossy(&buf[..size as usize]);
    let name = path
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(path.as_str())
        .to_lowercase();
    Ok(name)
}

pub fn active_window() -> Result<ActiveWindow> {
    // SAFETY: no arguments.
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        bail!("no foreground window");
    }
    let title = window_title(hwnd);
    let process_name = process_exe_name(hwnd).unwrap_or_default();
    Ok(ActiveWindow { title, process_name, hwnd: hwnd.0 as isize })
}

pub fn focus_window(hwnd: isize) -> Result<()> {
    let target = hwnd_from_isize(hwnd);
    // SAFETY: these calls only read or change window state and accept any handle value.
    unsafe {
        if !IsWindow(target).as_bool() {
            bail!("window handle {hwnd} is no longer valid");
        }
        if IsIconic(target).as_bool() {
            let _ = ShowWindow(target, SW_RESTORE);
        }
        if SetForegroundWindow(target).as_bool() {
            return Ok(());
        }
        // Windows refuses focus changes from background threads. Attach to the input queue of
        // the current foreground thread for the duration of the call.
        let fg = GetForegroundWindow();
        let fg_thread = GetWindowThreadProcessId(fg, None);
        let this_thread = GetCurrentThreadId();
        let attached = fg_thread != 0
            && fg_thread != this_thread
            && AttachThreadInput(this_thread, fg_thread, true).as_bool();
        let ok = SetForegroundWindow(target).as_bool();
        if attached {
            let _ = AttachThreadInput(this_thread, fg_thread, false);
        }
        if !ok {
            bail!("Windows refused to bring window {hwnd} to the foreground");
        }
    }
    Ok(())
}

// ---- clipboard ----

pub fn clipboard_get() -> Result<String> {
    let mut cb = Clipboard::new().map_err(|e| anyhow!("could not open the clipboard: {e}"))?;
    match cb.get_text() {
        Ok(t) => Ok(t),
        Err(arboard::Error::ContentNotAvailable) => Ok(String::new()),
        Err(e) => Err(anyhow!("could not read the clipboard: {e}")),
    }
}

pub fn clipboard_set(text: &str) -> Result<()> {
    let mut cb = Clipboard::new().map_err(|e| anyhow!("could not open the clipboard: {e}"))?;
    cb.set_text(text.to_string())
        .map_err(|e| anyhow!("could not write the clipboard: {e}"))
}

// ---- text and key input ----

pub fn type_text(text: &str) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    let mut enigo = new_enigo()?;
    enigo.text(text).map_err(|e| anyhow!("typing failed: {e}"))
}

pub fn insert_text(text: &str, method: InsertMethod) -> Result<()> {
    if text.is_empty() {
        return Ok(());
    }
    match method {
        InsertMethod::Type => type_text(text),
        InsertMethod::Paste => {
            let mut cb = Clipboard::new().map_err(|e| anyhow!("could not open the clipboard: {e}"))?;
            let previous = cb.get_text().ok();
            cb.set_text(text.to_string())
                .map_err(|e| anyhow!("could not write the clipboard: {e}"))?;
            std::thread::sleep(Duration::from_millis(50));

            let paste_result = (|| -> Result<()> {
                let mut enigo = new_enigo()?;
                enigo
                    .key(Key::Control, Direction::Press)
                    .map_err(|e| anyhow!("key press failed: {e}"))?;
                let click = enigo.key(Key::Unicode('v'), Direction::Click);
                let release = enigo.key(Key::Control, Direction::Release);
                click.map_err(|e| anyhow!("key press failed: {e}"))?;
                release.map_err(|e| anyhow!("key release failed: {e}"))?;
                Ok(())
            })();

            // Give the target app time to read the clipboard before it is restored.
            std::thread::sleep(Duration::from_millis(300));
            if let Some(old) = previous {
                if let Err(e) = cb.set_text(old) {
                    tracing::warn!("could not restore the previous clipboard text: {e}");
                }
            }
            paste_result
        }
    }
}

fn enigo_modifier(m: Modifier) -> Key {
    match m {
        Modifier::Ctrl => Key::Control,
        Modifier::Shift => Key::Shift,
        Modifier::Alt => Key::Alt,
        Modifier::Win => Key::Meta,
    }
}

fn enigo_key(token: KeyToken) -> Key {
    match token {
        KeyToken::Char(c) => Key::Unicode(c),
        KeyToken::Named(n) => match n {
            NamedKey::Enter => Key::Return,
            NamedKey::Tab => Key::Tab,
            NamedKey::Escape => Key::Escape,
            NamedKey::Space => Key::Space,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Delete => Key::Delete,
            NamedKey::Insert => Key::Insert,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::PageUp => Key::PageUp,
            NamedKey::PageDown => Key::PageDown,
            NamedKey::Up => Key::UpArrow,
            NamedKey::Down => Key::DownArrow,
            NamedKey::Left => Key::LeftArrow,
            NamedKey::Right => Key::RightArrow,
            NamedKey::F(n) => match n {
                1 => Key::F1,
                2 => Key::F2,
                3 => Key::F3,
                4 => Key::F4,
                5 => Key::F5,
                6 => Key::F6,
                7 => Key::F7,
                8 => Key::F8,
                9 => Key::F9,
                10 => Key::F10,
                11 => Key::F11,
                _ => Key::F12,
            },
        },
    }
}

pub fn press_keys(combo: &str) -> Result<()> {
    let parsed = keys::parse_combo(combo)?;
    let mut enigo = new_enigo()?;
    let mut first_err: Option<anyhow::Error> = None;
    let mut pressed: Vec<Key> = Vec::new();

    for m in &parsed.modifiers {
        let k = enigo_modifier(*m);
        match enigo.key(k, Direction::Press) {
            Ok(()) => pressed.push(k),
            Err(e) => {
                first_err = Some(anyhow!("key press failed: {e}"));
                break;
            }
        }
    }
    if first_err.is_none() {
        if let Some(token) = parsed.key {
            if let Err(e) = enigo.key(enigo_key(token), Direction::Click) {
                first_err = Some(anyhow!("key press failed: {e}"));
            }
        }
    }
    // Always release what was pressed, in reverse order.
    for k in pressed.into_iter().rev() {
        if let Err(e) = enigo.key(k, Direction::Release) {
            if first_err.is_none() {
                first_err = Some(anyhow!("key release failed: {e}"));
            }
        }
    }
    match first_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

// ---- mouse and screen ----

/// Click at absolute physical pixel coordinates.
pub fn click_abs(x: i32, y: i32, button: &str) -> Result<()> {
    let btn = match button.trim().to_lowercase().as_str() {
        "left" => Button::Left,
        "right" => Button::Right,
        "middle" => Button::Middle,
        other => bail!("unknown mouse button '{other}'. Use left, right or middle"),
    };
    let mut enigo = new_enigo()?;
    enigo
        .move_mouse(x, y, Coordinate::Abs)
        .map_err(|e| anyhow!("mouse move failed: {e}"))?;
    std::thread::sleep(Duration::from_millis(30));
    enigo
        .button(btn, Direction::Click)
        .map_err(|e| anyhow!("mouse click failed: {e}"))
}

const MAX_SHOT_WIDTH: u32 = 1280;

/// Capture the primary monitor. Returns the screenshot and the geometry needed to map clicks back.
pub fn screenshot_png() -> Result<(Screenshot, ShotGeometry)> {
    let monitors = xcap::Monitor::all().map_err(|e| anyhow!("could not list monitors: {e}"))?;
    let monitor = monitors
        .iter()
        .find(|m| m.is_primary())
        .or_else(|| monitors.first())
        .ok_or_else(|| anyhow!("no monitor found"))?;
    let origin_x = monitor.x();
    let origin_y = monitor.y();
    let captured = monitor
        .capture_image()
        .map_err(|e| anyhow!("screen capture failed: {e}"))?;

    // Rebuild the buffer with this crate's image version, in case xcap links another one.
    let (w, h) = (captured.width(), captured.height());
    let raw = captured.into_raw();
    let img = image::RgbaImage::from_raw(w, h, raw)
        .ok_or_else(|| anyhow!("captured image has an unexpected buffer size"))?;

    let (img, scale) = if w > MAX_SHOT_WIDTH {
        let nh = ((h as f64) * (MAX_SHOT_WIDTH as f64) / (w as f64)).round().max(1.0) as u32;
        let resized = image::imageops::resize(&img, MAX_SHOT_WIDTH, nh, image::imageops::FilterType::Triangle);
        (resized, (w as f64) / (MAX_SHOT_WIDTH as f64))
    } else {
        (img, 1.0)
    };

    let (out_w, out_h) = (img.width(), img.height());
    let mut png: Vec<u8> = Vec::new();
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .context("PNG encoding failed")?;

    Ok((
        Screenshot { png, width: out_w, height: out_h, scale },
        ShotGeometry { scale, origin_x, origin_y },
    ))
}

// ---- URLs ----

pub fn open_url_checked(url: &str) -> Result<()> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // rundll32 passes the URL to the shell handler without cmd.exe parsing, so characters
    // such as & and % in the URL cannot be interpreted as shell syntax.
    std::process::Command::new("rundll32.exe")
        .arg("url.dll,FileProtocolHandler")
        .arg(url)
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context("could not start the default browser")?;
    Ok(())
}
