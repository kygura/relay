//! Text pipeline: polish, translate, meeting notes and transcript export.

pub mod cleanup;
mod export;
mod notes;
mod polish;
mod translate;

pub use export::export;
pub use notes::meeting_notes;
pub use polish::{polish, sanitize_instructions, tone_for_app, MAX_INSTRUCTIONS};
pub use translate::translate;

use crate::llm::LlmResponse;
use crate::types::{AudioSource, TranscriptSegment};
use anyhow::{bail, Result};

/// Fail with a clear message when the model did not finish normally.
pub(crate) fn ensure_complete(resp: &LlmResponse, task: &str) -> Result<()> {
    match resp.stop_reason.as_str() {
        "refusal" => bail!("the model declined the {task} request (stop reason: refusal)"),
        "max_tokens" => bail!("the {task} reply was cut off at the token limit (stop reason: max_tokens)"),
        _ => Ok(()),
    }
}

/// Speaker name for display, with a fallback by audio source.
pub(crate) fn speaker_label(seg: &TranscriptSegment) -> String {
    match seg.speaker.as_deref().map(str::trim) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => match seg.source {
            AudioSource::Microphone => "Me".to_string(),
            AudioSource::System => "Speaker".to_string(),
        },
    }
}

/// "mm:ss" with minutes not wrapped at 60, used in prompts.
pub(crate) fn fmt_mmss(ms: u64) -> String {
    let total = ms / 1000;
    format!("{:02}:{:02}", total / 60, total % 60)
}

/// One transcript line in the form "[mm:ss] Speaker: text".
pub(crate) fn prompt_line(seg: &TranscriptSegment) -> String {
    format!("[{}] {}: {}", fmt_mmss(seg.start_ms), speaker_label(seg), seg.text.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn seg(id: &str, source: AudioSource, speaker: Option<&str>, text: &str, start_ms: u64, end_ms: u64) -> TranscriptSegment {
        TranscriptSegment {
            id: id.to_string(),
            source,
            speaker: speaker.map(|s| s.to_string()),
            text: text.to_string(),
            start_ms,
            end_ms,
            is_final: true,
            language: None,
        }
    }

    #[test]
    fn speaker_falls_back_by_source() {
        assert_eq!(speaker_label(&seg("1", AudioSource::Microphone, None, "x", 0, 1)), "Me");
        assert_eq!(speaker_label(&seg("1", AudioSource::System, None, "x", 0, 1)), "Speaker");
        assert_eq!(speaker_label(&seg("1", AudioSource::System, Some("Ana"), "x", 0, 1)), "Ana");
        assert_eq!(speaker_label(&seg("1", AudioSource::System, Some("  "), "x", 0, 1)), "Speaker");
    }

    #[test]
    fn prompt_line_format() {
        let s = seg("1", AudioSource::Microphone, None, " hello ", 75_000, 76_000);
        assert_eq!(prompt_line(&s), "[01:15] Me: hello");
        assert_eq!(fmt_mmss(3_725_000), "62:05");
    }
}
