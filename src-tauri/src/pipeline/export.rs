//! Transcript export in Txt, Markdown, SRT, VTT and JSON.

use super::speaker_label;
use crate::types::{ExportFormat, TranscriptSegment, TranslationSegment};
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::fmt::Write as _;

/// "HH:MM:SS" from milliseconds. Hours are not capped at 99.
pub(crate) fn fmt_hms(ms: u64) -> String {
    let total = ms / 1000;
    format!("{:02}:{:02}:{:02}", total / 3600, (total / 60) % 60, total % 60)
}

/// "HH:MM:SS,mmm" (SRT).
pub(crate) fn fmt_srt(ms: u64) -> String {
    format!("{},{:03}", fmt_hms(ms), ms % 1000)
}

/// "HH:MM:SS.mmm" (WebVTT).
pub(crate) fn fmt_vtt(ms: u64) -> String {
    format!("{}.{:03}", fmt_hms(ms), ms % 1000)
}

type TranslationIndex<'a> = HashMap<&'a str, Vec<&'a TranslationSegment>>;

fn index_translations<'a>(translations: &'a [TranslationSegment], include: bool) -> TranslationIndex<'a> {
    let mut map: TranslationIndex<'a> = HashMap::new();
    if include {
        for t in translations {
            map.entry(t.segment_id.as_str()).or_default().push(t);
        }
    }
    map
}

/// Collapse blank lines and surrounding whitespace so a cue never contains an empty line,
/// which would end the cue in SRT and VTT.
fn cue_text(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn end_time(seg: &TranscriptSegment) -> u64 {
    seg.end_ms.max(seg.start_ms)
}

/// Render the transcript. Translations are included only when `include_translations` is true.
pub fn export(
    transcript: &[TranscriptSegment],
    translations: &[TranslationSegment],
    format: ExportFormat,
    include_translations: bool,
) -> String {
    let tr = index_translations(translations, include_translations);
    let mut out = String::new();
    match format {
        ExportFormat::Txt => {
            for seg in transcript {
                let _ = writeln!(out, "[{}] {}: {}", fmt_hms(seg.start_ms), speaker_label(seg), seg.text.trim());
                for t in tr.get(seg.id.as_str()).into_iter().flatten() {
                    let _ = writeln!(out, "    [{}] {}", t.target_lang, t.text.trim());
                }
            }
        }
        ExportFormat::Markdown => {
            out.push_str("# Transcript\n\n");
            for seg in transcript {
                let _ = writeln!(
                    out,
                    "- **{}** ({}): {}",
                    speaker_label(seg),
                    fmt_hms(seg.start_ms),
                    seg.text.trim()
                );
                for t in tr.get(seg.id.as_str()).into_iter().flatten() {
                    let _ = writeln!(out, "  - *{}* ({})", t.text.trim(), t.target_lang);
                }
            }
        }
        ExportFormat::Srt => {
            for (i, seg) in transcript.iter().enumerate() {
                let _ = writeln!(out, "{}", i + 1);
                let _ = writeln!(out, "{} --> {}", fmt_srt(seg.start_ms), fmt_srt(end_time(seg)));
                let _ = writeln!(out, "{}", cue_text(&seg.text));
                for t in tr.get(seg.id.as_str()).into_iter().flatten() {
                    let _ = writeln!(out, "{}", cue_text(&t.text));
                }
                out.push('\n');
            }
        }
        ExportFormat::Vtt => {
            out.push_str("WEBVTT\n\n");
            for seg in transcript {
                let _ = writeln!(out, "{} --> {}", fmt_vtt(seg.start_ms), fmt_vtt(end_time(seg)));
                let _ = writeln!(out, "{}", cue_text(&seg.text));
                for t in tr.get(seg.id.as_str()).into_iter().flatten() {
                    let _ = writeln!(out, "{}", cue_text(&t.text));
                }
                out.push('\n');
            }
        }
        ExportFormat::Json => {
            let segments: Vec<Value> = transcript
                .iter()
                .map(|seg| {
                    let mut v = serde_json::to_value(seg).unwrap_or_else(|_| json!({}));
                    if include_translations {
                        let mut map = Map::new();
                        for t in tr.get(seg.id.as_str()).into_iter().flatten() {
                            map.insert(t.target_lang.clone(), Value::String(t.text.clone()));
                        }
                        if let Some(obj) = v.as_object_mut() {
                            obj.insert("translations".to_string(), Value::Object(map));
                        }
                    }
                    v
                })
                .collect();
            out = serde_json::to_string_pretty(&json!({ "segments": segments })).unwrap_or_else(|_| "{}".to_string());
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::AudioSource;

    fn seg(id: &str, source: AudioSource, speaker: Option<&str>, text: &str, start: u64, end: u64) -> TranscriptSegment {
        TranscriptSegment {
            id: id.into(),
            source,
            speaker: speaker.map(|s| s.to_string()),
            text: text.into(),
            start_ms: start,
            end_ms: end,
            is_final: true,
            language: Some("en".into()),
        }
    }

    fn sample() -> (Vec<TranscriptSegment>, Vec<TranslationSegment>) {
        let segs = vec![
            seg("a", AudioSource::Microphone, None, "Hello there.", 1_500, 3_250),
            seg("b", AudioSource::System, Some("Ana"), "Good morning.", 3_661_007, 3_662_000),
        ];
        let tr = vec![
            TranslationSegment { segment_id: "a".into(), target_lang: "es".into(), text: "Hola.".into() },
            TranslationSegment { segment_id: "a".into(), target_lang: "fr".into(), text: "Bonjour.".into() },
        ];
        (segs, tr)
    }

    #[test]
    fn timestamp_formats() {
        assert_eq!(fmt_hms(0), "00:00:00");
        assert_eq!(fmt_hms(3_661_007), "01:01:01");
        assert_eq!(fmt_srt(3_661_007), "01:01:01,007");
        assert_eq!(fmt_vtt(3_661_007), "01:01:01.007");
        assert_eq!(fmt_srt(999), "00:00:00,999");
        assert_eq!(fmt_hms(100 * 3600 * 1000), "100:00:00");
    }

    #[test]
    fn srt_structure() {
        let (s, t) = sample();
        let out = export(&s, &t, ExportFormat::Srt, true);
        let expected = "1\n00:00:01,500 --> 00:00:03,250\nHello there.\nHola.\nBonjour.\n\n\
                        2\n01:01:01,007 --> 01:01:02,000\nGood morning.\n\n";
        assert_eq!(out, expected);
    }

    #[test]
    fn srt_without_translations() {
        let (s, t) = sample();
        let out = export(&s, &t, ExportFormat::Srt, false);
        assert!(!out.contains("Hola."));
        assert!(out.starts_with("1\n"));
    }

    #[test]
    fn vtt_structure() {
        let (s, t) = sample();
        let out = export(&s, &t, ExportFormat::Vtt, true);
        assert!(out.starts_with("WEBVTT\n\n00:00:01.500 --> 00:00:03.250\nHello there.\nHola.\nBonjour.\n\n"));
        assert!(out.contains("01:01:01.007 --> 01:01:02.000\nGood morning.\n"));
        assert!(!out.contains(','));
    }

    #[test]
    fn cue_never_contains_a_blank_line() {
        let s = vec![seg("a", AudioSource::Microphone, None, "one\n\n  two  \n", 0, 1000)];
        let out = export(&s, &[], ExportFormat::Srt, false);
        assert_eq!(out, "1\n00:00:00,000 --> 00:00:01,000\none\ntwo\n\n");
    }

    #[test]
    fn txt_uses_speaker_fallbacks_and_indents_translations() {
        let (s, t) = sample();
        let out = export(&s, &t, ExportFormat::Txt, true);
        assert_eq!(
            out,
            "[00:00:01] Me: Hello there.\n    [es] Hola.\n    [fr] Bonjour.\n[01:01:01] Ana: Good morning.\n"
        );
    }

    #[test]
    fn markdown_nests_translations() {
        let (s, t) = sample();
        let out = export(&s, &t, ExportFormat::Markdown, true);
        assert!(out.starts_with("# Transcript\n\n- **Me** (00:00:01): Hello there.\n  - *Hola.* (es)\n"));
    }

    #[test]
    fn json_maps_translations_by_language() {
        let (s, t) = sample();
        let v: Value = serde_json::from_str(&export(&s, &t, ExportFormat::Json, true)).unwrap();
        assert_eq!(v["segments"][0]["translations"]["es"], "Hola.");
        assert_eq!(v["segments"][0]["startMs"], 1500);
        assert_eq!(v["segments"][1]["translations"], json!({}));
        let v: Value = serde_json::from_str(&export(&s, &t, ExportFormat::Json, false)).unwrap();
        assert!(v["segments"][0].get("translations").is_none());
    }
}
