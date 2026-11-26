//! PowerShell execution with a timeout and bounded output.

use crate::types::CommandOutput;
use anyhow::Result;

/// Each of stdout and stderr is cut to this many bytes.
pub const MAX_OUTPUT_BYTES: usize = 8 * 1024;

/// Decode as UTF-8 (lossy) and cut to `MAX_OUTPUT_BYTES` on a character boundary.
pub fn truncate_output(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.len() <= MAX_OUTPUT_BYTES {
        return text.into_owned();
    }
    let mut end = MAX_OUTPUT_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = text[..end].to_string();
    out.push_str("\n[output truncated at 8 KB]");
    out
}

#[cfg(windows)]
pub fn run_powershell(cmd: &str, timeout_secs: u64) -> Result<CommandOutput> {
    use anyhow::Context;
    use std::io::Read;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::sync::mpsc::{channel, Receiver};
    use std::time::{Duration, Instant};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // Reader threads keep at most this much and discard the rest, so a noisy command
    // cannot grow memory without bound.
    const READ_CAP: usize = 64 * 1024;

    fn spawn_reader<R: Read + Send + 'static>(pipe: Option<R>) -> Receiver<Vec<u8>> {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let mut kept: Vec<u8> = Vec::new();
            if let Some(mut pipe) = pipe {
                let mut buf = [0u8; 4096];
                loop {
                    match pipe.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            if kept.len() < READ_CAP {
                                let room = READ_CAP - kept.len();
                                kept.extend_from_slice(&buf[..n.min(room)]);
                            }
                        }
                    }
                }
            }
            let _ = tx.send(kept);
        });
        rx
    }

    let mut child = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg(cmd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .context("failed to start powershell.exe")?;

    let out_rx = spawn_reader(child.stdout.take());
    let err_rx = spawn_reader(child.stderr.take());

    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    let mut timed_out = false;
    let mut exit_code: Option<i32> = None;
    loop {
        match child.try_wait().context("failed to poll powershell.exe")? {
            Some(status) => {
                exit_code = status.code();
                break;
            }
            None => {
                if Instant::now() >= deadline {
                    timed_out = true;
                    let _ = child.kill();
                    let _ = child.wait();
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }

    // A grandchild process can keep a pipe open after the shell exits, so do not wait forever.
    let grace = Duration::from_secs(2);
    let stdout = out_rx.recv_timeout(grace).unwrap_or_default();
    let stderr = err_rx.recv_timeout(grace).unwrap_or_default();

    Ok(CommandOutput {
        stdout: truncate_output(&stdout),
        stderr: truncate_output(&stderr),
        exit_code,
        timed_out,
    })
}

#[cfg(not(windows))]
pub fn run_powershell(_cmd: &str, _timeout_secs: u64) -> Result<CommandOutput> {
    anyhow::bail!("run_powershell is only available on Windows")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_output_is_unchanged() {
        assert_eq!(truncate_output(b"hello"), "hello");
    }

    #[test]
    fn long_output_is_cut_with_a_note() {
        let data = vec![b'a'; MAX_OUTPUT_BYTES + 100];
        let out = truncate_output(&data);
        assert!(out.starts_with(&"a".repeat(MAX_OUTPUT_BYTES)));
        assert!(out.ends_with("[output truncated at 8 KB]"));
    }

    #[test]
    fn cut_respects_char_boundaries() {
        // Two-byte characters: an odd cut point must back up by one byte.
        let data = "é".repeat(MAX_OUTPUT_BYTES);
        let out = truncate_output(data.as_bytes());
        assert!(out.contains("[output truncated"));
        assert!(!out.contains('\u{FFFD}'));
    }
}
