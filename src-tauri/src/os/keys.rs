//! Key combo parsing. Pure logic, no OS calls, so it is unit-tested on any platform.

use anyhow::{bail, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    Ctrl,
    Shift,
    Alt,
    Win,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedKey {
    Enter,
    Tab,
    Escape,
    Space,
    Backspace,
    Delete,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    Up,
    Down,
    Left,
    Right,
    /// Function key, 1 to 12.
    F(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyToken {
    Named(NamedKey),
    /// A single printable character, stored as typed in lower case.
    Char(char),
}

/// A parsed combo: modifiers pressed first (in the order given), then an optional main key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Combo {
    pub modifiers: Vec<Modifier>,
    pub key: Option<KeyToken>,
}

pub const VALID_KEYS: &str = "modifiers: ctrl, shift, alt, win; \
named keys: enter, tab, esc, space, backspace, delete, insert, home, end, pageup, pagedown, \
up, down, left, right, f1 to f12; or any single letter, digit or punctuation character";

enum Part {
    Mod(Modifier),
    Key(KeyToken),
}

fn parse_part(raw: &str, combo: &str) -> Result<Part> {
    let p = raw.trim().to_lowercase();
    let part = match p.as_str() {
        "ctrl" | "control" => Part::Mod(Modifier::Ctrl),
        "shift" => Part::Mod(Modifier::Shift),
        "alt" => Part::Mod(Modifier::Alt),
        "win" | "windows" | "meta" | "super" | "cmd" => Part::Mod(Modifier::Win),
        "enter" | "return" => Part::Key(KeyToken::Named(NamedKey::Enter)),
        "tab" => Part::Key(KeyToken::Named(NamedKey::Tab)),
        "esc" | "escape" => Part::Key(KeyToken::Named(NamedKey::Escape)),
        "space" | "spacebar" => Part::Key(KeyToken::Named(NamedKey::Space)),
        "backspace" => Part::Key(KeyToken::Named(NamedKey::Backspace)),
        "delete" | "del" => Part::Key(KeyToken::Named(NamedKey::Delete)),
        "insert" | "ins" => Part::Key(KeyToken::Named(NamedKey::Insert)),
        "home" => Part::Key(KeyToken::Named(NamedKey::Home)),
        "end" => Part::Key(KeyToken::Named(NamedKey::End)),
        "pageup" | "pgup" => Part::Key(KeyToken::Named(NamedKey::PageUp)),
        "pagedown" | "pgdn" | "pgdown" => Part::Key(KeyToken::Named(NamedKey::PageDown)),
        "up" | "arrowup" => Part::Key(KeyToken::Named(NamedKey::Up)),
        "down" | "arrowdown" => Part::Key(KeyToken::Named(NamedKey::Down)),
        "left" | "arrowleft" => Part::Key(KeyToken::Named(NamedKey::Left)),
        "right" | "arrowright" => Part::Key(KeyToken::Named(NamedKey::Right)),
        other => {
            if let Some(num) = other.strip_prefix('f') {
                if let Ok(n) = num.parse::<u8>() {
                    if (1..=12).contains(&n) {
                        return Ok(Part::Key(KeyToken::Named(NamedKey::F(n))));
                    }
                }
            }
            let mut chars = other.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) if c.is_alphanumeric() || c.is_ascii_punctuation() => {
                    Part::Key(KeyToken::Char(c))
                }
                _ => bail!("unknown key name '{raw}' in combo '{combo}'. Valid keys: {VALID_KEYS}"),
            }
        }
    };
    Ok(part)
}

/// Parse "ctrl+shift+t", "alt+tab", "enter", "win+r", "f5", "pagedown", "ctrl++".
pub fn parse_combo(combo: &str) -> Result<Combo> {
    let trimmed = combo.trim();
    if trimmed.is_empty() {
        bail!("empty key combo. Valid keys: {VALID_KEYS}");
    }
    // A trailing "+" key, as in "ctrl++" or "+".
    let (rest, plus_key) = if trimmed == "+" {
        ("", true)
    } else if let Some(head) = trimmed.strip_suffix("++") {
        (head, true)
    } else {
        (trimmed, false)
    };

    let mut parts: Vec<Part> = Vec::new();
    if !rest.is_empty() {
        for raw in rest.split('+') {
            if raw.trim().is_empty() {
                bail!("empty key name in combo '{combo}'. Valid keys: {VALID_KEYS}");
            }
            parts.push(parse_part(raw, combo)?);
        }
    }
    if plus_key {
        parts.push(Part::Key(KeyToken::Char('+')));
    }

    let mut modifiers = Vec::new();
    let mut key = None;
    let total = parts.len();
    for (i, part) in parts.into_iter().enumerate() {
        match part {
            Part::Mod(m) => modifiers.push(m),
            Part::Key(k) => {
                if i + 1 != total {
                    bail!("in combo '{combo}', the non-modifier key must come last");
                }
                key = Some(k);
            }
        }
    }
    Ok(Combo { modifiers, key })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_modifier_and_letter() {
        let c = parse_combo("ctrl+shift+t").unwrap();
        assert_eq!(c.modifiers, vec![Modifier::Ctrl, Modifier::Shift]);
        assert_eq!(c.key, Some(KeyToken::Char('t')));
    }

    #[test]
    fn parses_single_named_keys() {
        assert_eq!(parse_combo("enter").unwrap().key, Some(KeyToken::Named(NamedKey::Enter)));
        assert_eq!(parse_combo("PageDown").unwrap().key, Some(KeyToken::Named(NamedKey::PageDown)));
        assert_eq!(parse_combo("f5").unwrap().key, Some(KeyToken::Named(NamedKey::F(5))));
    }

    #[test]
    fn parses_win_and_alt_tab() {
        let c = parse_combo("win+r").unwrap();
        assert_eq!(c.modifiers, vec![Modifier::Win]);
        let c = parse_combo("alt+tab").unwrap();
        assert_eq!(c.key, Some(KeyToken::Named(NamedKey::Tab)));
    }

    #[test]
    fn modifier_only_combo_has_no_key() {
        let c = parse_combo("ctrl+shift").unwrap();
        assert_eq!(c.key, None);
        assert_eq!(c.modifiers.len(), 2);
    }

    #[test]
    fn plus_key_is_supported() {
        let c = parse_combo("ctrl++").unwrap();
        assert_eq!(c.modifiers, vec![Modifier::Ctrl]);
        assert_eq!(c.key, Some(KeyToken::Char('+')));
    }

    #[test]
    fn rejects_unknown_and_misordered_keys() {
        let err = parse_combo("ctrl+banana").unwrap_err().to_string();
        assert!(err.contains("Valid keys"));
        assert!(parse_combo("f13").is_err());
        assert!(parse_combo("t+ctrl").is_err());
        assert!(parse_combo("a+b").is_err());
        assert!(parse_combo("").is_err());
        assert!(parse_combo("ctrl+").is_err());
    }
}
