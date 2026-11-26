//! Deterministic transcript cleanup, ported unchanged from VoxFlow (core/src/cleanup.rs, `basic`).
//! Removes fillers and stutters, fixes orphaned punctuation and capitalization. Runs locally.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Word,
    /// Punctuation glued to the following word: ¿ ¡ ( " …
    Open,
    /// Punctuation glued to the preceding word: , . ? ! ) …
    Close,
    /// Standalone symbol with spaces on both sides: — & …
    Free,
}

struct Tok {
    kind: Kind,
    text: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Lang {
    En,
    Es,
    Other,
}

/// Whitespace chunks split into leading punct / word / trailing punct. Punctuation
/// inside a chunk ("3.5", "e.g", "don't") stays part of the word.
fn tokenize(text: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    let mut push = |kind, text: &str| {
        out.push(Tok {
            kind,
            text: text.to_owned(),
        })
    };
    for chunk in text.split_whitespace() {
        let Some(first) = chunk.find(char::is_alphanumeric) else {
            let kind = if chunk.chars().all(|c| ",.;:!?…".contains(c)) {
                Kind::Close
            } else if chunk.chars().all(|c| "¿¡([{“‘\"'".contains(c)) {
                Kind::Open
            } else if chunk.chars().all(|c| ")]}”’".contains(c)) {
                Kind::Close // spaced "( um )": the closer pairs with its opener
            } else {
                Kind::Free
            };
            push(kind, chunk);
            continue;
        };
        let last = chunk.rfind(char::is_alphanumeric).expect("found above");
        let end = last + chunk[last..].chars().next().map_or(1, char::len_utf8);
        if first > 0 {
            push(Kind::Open, &chunk[..first]);
        }
        push(Kind::Word, &chunk[first..end]);
        if end < chunk.len() {
            push(Kind::Close, &chunk[end..]);
        }
    }
    out
}

fn is_comma(t: &Tok) -> bool {
    t.kind == Kind::Close && t.text == ","
}

/// Comma or ellipsis: what brackets a parenthetical filler.
fn is_delim(t: &Tok) -> bool {
    t.kind == Kind::Close && matches!(t.text.as_str(), "," | "..." | "…")
}

fn is_terminal(t: &Tok) -> bool {
    t.kind == Kind::Close && !is_delim(t) && t.text.ends_with(['.', '!', '?'])
}

/// `w` spells `pat` with each letter repeated ≥ 1 times ("ummm" ~ "um"). A doubled
/// letter in `pat` means "at least two" ("mm" matches "mmm" but not "m").
fn elongated(w: &str, pat: &str) -> bool {
    let (w, p): (Vec<char>, Vec<char>) = (w.chars().collect(), pat.chars().collect());
    let mut i = 0;
    for (k, &c) in p.iter().enumerate() {
        if w.get(i) != Some(&c) {
            return false;
        }
        i += 1;
        if p.get(k + 1) != Some(&c) {
            while w.get(i) == Some(&c) {
                i += 1;
            }
        }
    }
    i == w.len()
}

/// Always-noise fillers. Bare "um" isn't universal (Portuguese article), "er"/"eh"/"em" aren't either.
fn is_filler(w: &str, lang: Lang) -> bool {
    const UNIVERSAL: &[&str] = &["uh", "uhm", "hm", "mm", "umm"];
    let extra: &[&str] = match lang {
        Lang::En => &["um", "erm"],
        Lang::Es => &["um", "eh", "em", "ehm"],
        Lang::Other => &[],
    };
    (lang == Lang::En && w == "er") || UNIVERSAL.iter().chain(extra).any(|p| elongated(w, p))
}

/// Fillers that are real words elsewhere; removed only when parenthetical.
fn context_phrases(lang: Lang) -> &'static [&'static [&'static str]] {
    match lang {
        Lang::En => &[&["like"], &["you", "know"], &["i", "mean"]],
        Lang::Es => &[&["este"], &["o", "sea"], &["pues"], &["bueno"]],
        Lang::Other => &[],
    }
}

/// Legit doubles that must not be collapsed as stutters.
const KEEP_DOUBLE: &[&str] = &[
    "had", "that", "is", "no", "yes", "very", "so", "really", "bye", "ha", "sí", "si", "muy", "ya",
];

fn detect(toks: &[Tok]) -> Lang {
    const EN: &[&str] = &[
        "the", "and", "is", "i", "you", "to", "of", "it", "that", "what", "this", "was", "for",
        "with", "my", "we", "are", "have", "be", "not", "in", "on", "do",
    ];
    const ES: &[&str] = &[
        "el", "la", "que", "de", "y", "los", "las", "es", "un", "una", "por", "para", "con", "lo",
        "se", "yo", "pero", "está", "qué", "del", "al", "muy", "como", "mi", "sí",
    ];
    let (mut en, mut es) = (0, 0);
    for t in toks.iter().filter(|t| t.kind == Kind::Word) {
        let w = t.text.to_lowercase();
        en += EN.contains(&w.as_str()) as u32;
        es += ES.contains(&w.as_str()) as u32;
    }
    match en.cmp(&es) {
        std::cmp::Ordering::Greater => Lang::En,
        std::cmp::Ordering::Less => Lang::Es,
        std::cmp::Ordering::Equal => Lang::Other,
    }
}

/// Remove `toks[i..i + n]` plus the punctuation that bracketed it:
/// "It's, like, huge" → "It's huge", "um, b" → "b", "a, um." → "a.". `filler`: always-noise
/// word (vs a context phrase). Returns where the removal started.
fn remove(toks: &mut Vec<Tok>, i: usize, n: usize, filler: bool) -> usize {
    let next = toks.get(i + n);
    let next_delim = next.is_some_and(is_delim);
    let next_end = next.is_none_or(is_terminal);
    let prev_comma = i > 0 && is_comma(&toks[i - 1]);
    if prev_comma && next_delim && keeps_comma(toks, i - 1, i + n + 1, filler) {
        toks.drain(i..i + n + 1); // "Well, um, I think" → "Well, I think"
        return i;
    }
    let start = if prev_comma && (next_delim || next_end) {
        i - 1
    } else {
        i
    };
    toks.drain(start..i + n + next_delim as usize);
    start
}

/// Comma at `c` before a removed filler survives when it closes a one-word segment that is
/// a discourse marker ("Well,"), a list item ("a, um, b, c"), or any one-word segment before
/// a pure filler ("a, um, b"); a context phrase mid-clause loses both commas ("It's, like,
/// huge." → "It's huge."). Two-word segments keep it only after a greeting ("Hi John, um,
/// how" → "Hi John, how"), so "I think, uh, we" → "I think we". `after` = first token past
/// the filler. ponytail: word-count heuristic, no parsing; "It's, um, huge" keeps its comma.
fn keeps_comma(toks: &[Tok], c: usize, after: usize, filler: bool) -> bool {
    const GREETINGS: &[&str] = &["hi", "hello", "hey", "thanks", "dear", "hola", "gracias", "buenas"];
    const MARKERS: &[&str] = &[
        "well", "wait", "so", "okay", "ok", "oh", "yes", "yeah", "no", "right", "now", "look",
        "hey", "anyway", "actually", "bueno", "pues", "vale", "mira", "oye", "sí", "bien",
        "entonces", "claro",
    ];
    let word = |k: usize| toks.get(k).filter(|t| t.kind == Kind::Word);
    let Some(w) = c.checked_sub(1).and_then(word) else {
        return false;
    };
    let starts = |k: Option<usize>| {
        k.map(|k| &toks[k])
            .is_none_or(|t| t.kind == Kind::Open || is_terminal(t) || is_comma(t))
    };
    if starts(c.checked_sub(2)) {
        let before = c.checked_sub(2).map(|k| &toks[k]);
        return filler
            || before.is_some_and(is_comma)
            || MARKERS.contains(&w.text.to_lowercase().as_str())
            || (word(after).is_some() && toks.get(after + 1).is_some_and(is_comma));
    }
    c.checked_sub(2)
        .and_then(word)
        .is_some_and(|g| GREETINGS.contains(&g.text.to_lowercase().as_str()))
        && starts(c.checked_sub(3))
}

/// Deterministic cleanup (SPEC v2 "basic"). `lang` is the language setting ("auto" or
/// ISO-639-1). Conservative: when in doubt the word stays. Input made only of fillers
/// ("Hmmm") returns "" — callers treat empty as nothing to paste.
pub fn basic(text: &str, lang: &str) -> String {
    let mut toks = tokenize(text);
    let lang = match lang {
        "en" => Lang::En,
        "es" => Lang::Es,
        "auto" => detect(&toks),
        _ => Lang::Other,
    };

    // Fillers, then parenthetical context fillers.
    let mut i = 0;
    'scan: while i < toks.len() {
        if toks[i].kind == Kind::Word {
            if is_filler(&toks[i].text.to_lowercase(), lang) {
                i = remove(&mut toks, i, 1, true);
                continue;
            }
            for phrase in context_phrases(lang) {
                let n = phrase.len();
                let matches = toks.len() >= i + n
                    && toks[i..i + n]
                        .iter()
                        .zip(phrase.iter())
                        .all(|(t, p)| t.kind == Kind::Word && t.text.to_lowercase() == *p);
                if !matches {
                    continue;
                }
                let prev = i.checked_sub(1).map(|j| &toks[j]);
                let next = toks.get(i + n);
                let prev_boundary =
                    prev.is_none_or(|t| t.kind == Kind::Open || is_terminal(t) || is_delim(t));
                let parenthetical = (prev_boundary && next.is_some_and(is_delim))
                    || (prev.is_some_and(is_delim) && next.is_none_or(is_terminal));
                if parenthetical {
                    i = remove(&mut toks, i, n, false);
                    continue 'scan;
                }
            }
        }
        i += 1;
    }

    // Stutters: "I I think", "the the", "I, I think" (not numbers, not KEEP_DOUBLE).
    if lang != Lang::Other {
        let mut i = 0;
        while i < toks.len() {
            let j = i + 1 + toks.get(i + 1).is_some_and(is_comma) as usize;
            let w = toks[i].text.to_lowercase();
            let repeat = toks[i].kind == Kind::Word
                && toks
                    .get(j)
                    .is_some_and(|t| t.kind == Kind::Word && t.text.to_lowercase() == w)
                && !w.chars().any(|c| c.is_numeric())
                && !KEEP_DOUBLE.contains(&w.as_str());
            if repeat {
                toks.drain(i..j);
            } else {
                i += 1;
            }
        }
    }

    // Orphan / duplicate punctuation left behind.
    let mut out: Vec<Tok> = Vec::with_capacity(toks.len());
    for t in toks {
        if t.kind == Kind::Close {
            let Some(prev) = out.last() else { continue };
            if prev.kind == Kind::Open {
                // "¡Eh! ¡Tú!" → "¡Tú!", "(um) hi" → "hi": the opener goes with its closer.
                if !is_delim(&t) {
                    out.pop();
                }
                continue;
            }
            if (is_comma(&t) && prev.kind == Kind::Close)
                || (is_terminal(&t) && (is_terminal(prev) || (is_delim(prev) && !is_comma(prev))))
            {
                continue;
            }
            if is_comma(prev) {
                out.pop();
            }
        }
        if t.kind == Kind::Open && out.last().is_some_and(|p| p.kind == Kind::Open && p.text == t.text) {
            continue;
        }
        out.push(t);
    }
    while out.last().is_some_and(|t| is_comma(t) || t.kind == Kind::Open) {
        out.pop();
    }
    if !out.iter().any(|t| t.kind == Kind::Word) {
        return String::new();
    }

    // Capitalize sentence starts (not after abbreviations like "e.g." / "Dr."); "i" → "I".
    if lang != Lang::Other {
        const ABBREV: &[&str] = &[
            "mr", "mrs", "ms", "dr", "st", "sr", "sra", "srta", "etc", "vs",
        ];
        let mut cap_next = true;
        for k in 0..out.len() {
            match out[k].kind {
                Kind::Word => {
                    let lower = out[k].text.to_lowercase();
                    let pronoun_i = lang == Lang::En
                        && (lower == "i" || lower.starts_with("i'") || lower.starts_with("i’"));
                    let link = out[k].text.contains("://") || out[k].text.contains('@');
                    if (cap_next && !link) || pronoun_i {
                        let mut c = out[k].text.chars();
                        if let Some(f) = c.next().filter(|f| f.is_lowercase()) {
                            out[k].text = f.to_uppercase().chain(c).collect();
                        }
                    }
                    cap_next = false;
                }
                Kind::Close if is_terminal(&out[k]) => {
                    let abbrev = out[k].text == "."
                        && out[k - 1].kind == Kind::Word
                        && (out[k - 1].text.contains('.')
                            || ABBREV.contains(&out[k - 1].text.to_lowercase().as_str()));
                    cap_next = !abbrev;
                }
                Kind::Close => cap_next = false,
                Kind::Open | Kind::Free => {}
            }
        }
    }

    let mut s = String::new();
    for (k, t) in out.iter().enumerate() {
        if k > 0 && t.kind != Kind::Close && out[k - 1].kind != Kind::Open {
            s.push(' ');
        }
        s.push_str(&t.text);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_table() {
        #[rustfmt::skip]
        let cases: &[(&str, &str, &str)] = &[
            // (lang, input, expected)
            // --- EN fillers
            ("en", "um I think so", "I think so"),
            ("en", "Um, I think we should go.", "I think we should go."),
            ("en", "I think, uh, we should go.", "I think we should go."),
            ("en", "So ummm we left. Uhh, then what?", "So we left. Then what?"),
            ("en", "We should, erm.", "We should."),
            ("en", "Hmmm... okay.", "Okay."),
            ("en", "I brought an umbrella and a hammer.", "I brought an umbrella and a hammer."),
            ("en", "To err is human.", "To err is human."),
            ("en", "Hmmm", ""),
            ("en", "um, uh.", ""),
            // --- EN stutters
            ("en", "I I think the the plan works", "I think the plan works"),
            ("en", "I, I think so.", "I think so."),
            ("en", "The the cat sat.", "The cat sat."),
            ("en", "What it is is fine, and he had had enough.", "What it is is fine, and he had had enough."),
            ("en", "I know that that is true.", "I know that that is true."),
            ("en", "No, no, no.", "No, no, no."),
            ("en", "Call 5 5 5 1 2.", "Call 5 5 5 1 2."),
            // --- EN context fillers
            ("en", "I like pizza.", "I like pizza."),
            ("en", "It's, like, huge.", "It's huge."),
            ("en", "Like, what is this?", "What is this?"),
            ("en", "you know the answer", "You know the answer"),
            ("en", "You know, it works.", "It works."),
            ("en", "It was great, you know.", "It was great."),
            ("en", "I mean, it's fine.", "It's fine."),
            ("en", "That's what I mean.", "That's what I mean."),
            ("en", "It costs something like 5 dollars.", "It costs something like 5 dollars."),
            // --- EN spacing / punctuation / capitalization
            ("en", "hello  world , how are you ?", "Hello world, how are you?"),
            ("en", "i think i'm right. yes i am", "I think I'm right. Yes I am"),
            ("en", "It is 3.5 percent, e.g. a lot.", "It is 3.5 percent, e.g. a lot."),
            ("en", "He said \"hi\" (twice).", "He said \"hi\" (twice)."),
            // --- ES
            ("es", "eh creo que sí", "Creo que sí"),
            ("es", "Em, vamos a la playa.", "Vamos a la playa."),
            ("es", "Quiero de de verdad ir.", "Quiero de verdad ir."),
            ("es", "Me gusta este libro.", "Me gusta este libro."),
            ("es", "Bueno, vamos.", "Vamos."),
            ("es", "Está bueno.", "Está bueno."),
            ("es", "Bueno.", "Bueno."),
            ("es", "pues claro que sí", "Pues claro que sí"),
            ("es", "Pues, no sé.", "No sé."),
            ("es", "Era, o sea, enorme.", "Era enorme."),
            ("es", "¿eh, qué hora es?", "¿Qué hora es?"),
            ("es", "¿Bueno, este, qué hacemos?", "¿Qué hacemos?"),
            ("es", "¡Qué bien! mmm sí.", "¡Qué bien! Sí."),
            ("es", "Sí, sí, claro.", "Sí, sí, claro."),
            ("es", "¡Eh! ¡Tú!", "¡Tú!"),
            ("es", "¿Eh? ¿Qué hora es?", "¿Qué hora es?"),
            // --- orphaned openers, clause commas, pins
            ("en", "(um) hello", "Hello"),
            ("en", "hello (um) world", "Hello world"),
            ("en", "hello (um", "Hello"),
            ("en", "Well, um, I think", "Well, I think"),
            ("en", "Wait, like, like, what?", "Wait, what?"),
            ("en", "a, um, b, uh, c", "A, b, c"),
            ("en", "a, um, b", "A, b"),
            ("en", "Hi John, um, how are you", "Hi John, how are you"),
            ("es", "Hola Ana, eh, ¿cómo estás?", "Hola Ana, ¿cómo estás?"),
            ("en", "hi ( um ) there", "Hi there"),
            ("en", "hi [ uh ] there", "Hi there"),
            ("en", "I think... um.", "I think..."),
            ("en", "https://example.com/a?b=1", "https://example.com/a?b=1"),
            ("en", "see https://example.com/a?b=1", "See https://example.com/a?b=1"),
            ("en", "me@x.org", "me@x.org"),
            ("en", "It costs $3.50.", "It costs $3.50."),
            ("en", "UM I THINK", "I THINK"),
            ("en", "I'm I'm sure.", "I'm sure."),
            ("en", "Errr hello", "Errr hello"),
            // --- auto detection
            ("auto", "um I think that the plan is good", "I think that the plan is good"),
            ("auto", "eh, creo que la casa es muy bonita", "Creo que la casa es muy bonita"),
            ("auto", "Like, you know, it is what it is.", "It is what it is."),
            // --- other languages: only universal fillers, no casing/stutter/context rules
            ("pt", "uh eu tenho um carro", "eu tenho um carro"),
            ("de", "hmm das das ist gut", "das das ist gut"),
            ("auto", "uhh ja", "ja"),
        ];
        let mut failures = Vec::new();
        for (lang, input, want) in cases {
            let got = basic(input, lang);
            if got != *want {
                failures.push(format!(
                    "[{lang}] {input:?}\n   want {want:?}\n    got {got:?}"
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "{} failures:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

}
