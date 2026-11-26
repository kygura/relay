//! Short, readable versions of backend errors for the overlay and notes.
//!
//! Raw errors stay in the logs. The UI shows one short sentence.

const MAX_FALLBACK: usize = 90;

/// Map a raw error to one short sentence (about 70 characters or less for known failures).
pub fn friendly_error(raw: &str) -> String {
    let lower = raw.to_lowercase();
    let has = |s: &str| lower.contains(s);

    if has("credit balance") {
        return "Claude has no credit left.".into();
    }
    if has("no anthropic api key")
        || has("x-api-key")
        || (has("claude") && (has("http 401") || has("authentication_error")))
    {
        return "No valid Claude API key. Add one in Settings.".into();
    }
    if has("whisper model") && (has("not downloaded") || has("not found") || has("missing")) {
        return "Whisper model missing. Download it in Settings.".into();
    }
    if has("deepgram") && (has("rejected the key") || has("http 401") || has("http 403")) {
        return "Deepgram rejected the key. Check it in Settings.".into();
    }
    if has("rejected the api key") {
        return "The speech server rejected the API key. Check Settings.".into();
    }
    if has("http 429") || has("http 529") || has("rate_limit") || has("overloaded") || has("rate limit") {
        return "Claude is busy. Try again shortly.".into();
    }
    if has("no default microphone") {
        return "No microphone found. Check Windows sound settings.".into();
    }
    if has("microphone") && has("was not found") {
        return "The chosen microphone was not found. Pick another in Settings.".into();
    }
    if has("no default output device") || (has("system audio device") && has("was not found")) {
        return "No system audio source found. Check Settings.".into();
    }
    if has("audio device") || has("audio stream") || has("sample format") || has("read microphone") {
        return "The audio device failed. Pick another in Settings.".into();
    }
    if has("could not reach")
        || has("could not connect")
        || has("did not answer")
        || has("timed out")
        || has("timeout")
        || has("dns")
        || has("connection refused")
        || has("connection error")
        || has("lost the connection")
        || has("stream stalled")
        || has("stream was interrupted")
    {
        return "Could not reach the model server.".into();
    }
    if !has("claude api") {
        if let Some(code) = http_status(&lower) {
            return format!("The model server returned an error (HTTP {code}).");
        }
    }
    fallback(raw)
}

/// A note for the history entry when AI cleanup did not run: "Used Basic cleanup. <reason>".
pub fn cleanup_note(raw: &str) -> String {
    let mut rest = raw.trim();
    for prefix in ["AI cleanup failed, so Basic was used.", "AI cleanup was skipped, so Basic was used."] {
        if let Some(r) = rest.strip_prefix(prefix) {
            rest = r.trim();
            break;
        }
    }
    if rest.is_empty() {
        return "Used Basic cleanup.".into();
    }
    format!("Used Basic cleanup. {}", friendly_error(rest))
}

/// The number after the first "http " in already lowercased text, when it is 3 digits.
fn http_status(lower: &str) -> Option<String> {
    let at = lower.find("http ")?;
    let digits: String = lower[at + 5..].chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() == 3 {
        Some(digits)
    } else {
        None
    }
}

fn fallback(raw: &str) -> String {
    let mut s = raw.trim().lines().next().unwrap_or("").trim();

    // "Claude API returned HTTP 400: invalid_request_error: message" -> "message".
    if s.to_lowercase().starts_with("claude api returned http") {
        if let Some(i) = s.find(':') {
            s = s[i + 1..].trim();
        }
        if let Some(i) = s.find(':') {
            let kind = &s[..i];
            if !kind.is_empty() && kind.chars().all(|c| c.is_ascii_lowercase() || c == '_') {
                s = s[i + 1..].trim();
            }
        }
    }

    // First sentence.
    let mut end = s.len();
    for (i, c) in s.char_indices() {
        if c == '.' || c == '?' {
            let next = s[i + 1..].chars().next();
            if next.map_or(true, |n| n.is_whitespace()) {
                end = i + 1;
                break;
            }
        }
    }
    let s = s[..end].trim();
    if s.is_empty() {
        return "Something went wrong.".into();
    }
    truncate_words(s, MAX_FALLBACK)
}

fn truncate_words(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let room = max.saturating_sub(3);
    let cut: String = s.chars().take(room).collect();
    // Cut back to the last space when the next character continues a word.
    let continues = s.chars().nth(room).map_or(false, |c| !c.is_whitespace());
    let cut = match (continues, cut.rfind(' ')) {
        (true, Some(i)) if i > 0 => cut[..i].to_string(),
        _ => cut,
    };
    format!("{}...", cut.trim_end_matches(|c: char| c.is_whitespace() || c == ',' || c == ';' || c == ':'))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CREDIT: &str = "Claude API returned HTTP 400: invalid_request_error: Your credit balance is too low to access the Anthropic API. Please go to Plans & Billing to upgrade or purchase credits.";

    #[test]
    fn credit_balance() {
        assert_eq!(friendly_error(CREDIT), "Claude has no credit left.");
    }

    #[test]
    fn cleanup_note_credit() {
        let raw = format!("AI cleanup failed, so Basic was used. {CREDIT}");
        assert_eq!(cleanup_note(&raw), "Used Basic cleanup. Claude has no credit left.");
        assert_eq!(cleanup_note(CREDIT), "Used Basic cleanup. Claude has no credit left.");
        assert_eq!(cleanup_note("AI cleanup was skipped, so Basic was used."), "Used Basic cleanup.");
    }

    #[test]
    fn api_key() {
        let k = "No valid Claude API key. Add one in Settings.";
        assert_eq!(friendly_error("No Anthropic API key set. Add it in Settings > API keys."), k);
        assert_eq!(friendly_error("Claude API returned HTTP 401: authentication_error: invalid x-api-key"), k);
        assert_eq!(friendly_error("Claude API returned HTTP 401: nope"), k);
    }

    #[test]
    fn busy() {
        let k = "Claude is busy. Try again shortly.";
        assert_eq!(friendly_error("Claude API returned HTTP 429: rate_limit_error: slow down"), k);
        assert_eq!(friendly_error("Claude API returned HTTP 529: overloaded_error: Overloaded"), k);
    }

    #[test]
    fn network() {
        let k = "Could not reach the model server.";
        assert_eq!(friendly_error("Could not reach the Claude API: error sending request"), k);
        assert_eq!(friendly_error("The Claude API did not answer within 30 seconds."), k);
        assert_eq!(friendly_error("dns error: failed to lookup address"), k);
        assert_eq!(friendly_error("The Claude stream stalled for 60 seconds."), k);
    }

    #[test]
    fn whisper_missing() {
        assert_eq!(
            friendly_error("The Whisper model is not downloaded yet (C:\\m\\ggml-base.bin). Download it in Settings > Speech to text."),
            "Whisper model missing. Download it in Settings."
        );
    }

    #[test]
    fn audio() {
        assert_eq!(
            friendly_error("No default microphone found. Check Windows sound settings and microphone privacy permissions."),
            "No microphone found. Check Windows sound settings."
        );
        assert_eq!(
            friendly_error("Microphone 'USB' was not found. Pick another one in Settings > Audio."),
            "The chosen microphone was not found. Pick another in Settings."
        );
        assert_eq!(
            friendly_error("No default output device for system audio. Check Windows sound settings."),
            "No system audio source found. Check Settings."
        );
        assert_eq!(
            friendly_error("The mic audio device did not start within 10 seconds. Pick another device in Settings > Audio."),
            "The audio device failed. Pick another in Settings."
        );
    }

    #[test]
    fn model_server_status() {
        assert_eq!(
            friendly_error("Transcription request failed with HTTP 500: internal error"),
            "The model server returned an error (HTTP 500)."
        );
        assert_eq!(
            friendly_error("The transcription server returned HTTP 404 for http://localhost:11434/v1/audio/transcriptions. Check openai_base_url."),
            "The model server returned an error (HTTP 404)."
        );
    }

    #[test]
    fn speech_keys() {
        assert_eq!(
            friendly_error("Deepgram rejected the key (HTTP 401). Check the Deepgram key in Settings > API keys."),
            "Deepgram rejected the key. Check it in Settings."
        );
    }

    #[test]
    fn fallback_strips_prefix_and_takes_first_sentence() {
        assert_eq!(
            friendly_error("Claude API returned HTTP 400: invalid_request_error: max_tokens is too large. Lower it."),
            "max_tokens is too large."
        );
        assert_eq!(friendly_error("Something odd. More text."), "Something odd.");
        assert_eq!(friendly_error(""), "Something went wrong.");
    }

    #[test]
    fn fallback_truncates_at_word_boundary() {
        let raw = "This is a very long sentence without any full stop that keeps going and going well past the limit of ninety characters";
        let out = friendly_error(raw);
        assert!(out.ends_with("..."));
        assert!(out.chars().count() <= 90, "{}", out.chars().count());
        assert!(!out.trim_end_matches("...").ends_with(' '));
        assert!(raw.starts_with(out.trim_end_matches("...")));
        assert!(!out.contains("characte"), "{out}");
    }

    #[test]
    fn known_messages_are_short() {
        for raw in [CREDIT, "No Anthropic API key set.", "Claude API returned HTTP 529: overloaded_error"] {
            assert!(friendly_error(raw).chars().count() <= 70);
        }
    }
}
