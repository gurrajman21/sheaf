//! Spell checking with a bundled Hunspell dictionary (en_US, SCOWL; see dict/README_en_US.txt)
//! through `spellbook`. Words the user adds are kept per session (the UI persists them).

use std::collections::HashSet;
use std::sync::OnceLock;

use spellbook::Dictionary;

fn dictionary() -> &'static Dictionary {
    static D: OnceLock<Dictionary> = OnceLock::new();
    D.get_or_init(|| Dictionary::new(include_str!("../dict/en_US.aff"), include_str!("../dict/en_US.dic")).expect("bundled dictionary"))
}

/// Words in `text` worth checking: (char offset, char length, word). Skips anything with
/// digits, all-caps acronyms, web addresses and single letters.
pub fn words(text: &str) -> Vec<(usize, usize, String)> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_alphabetic() {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && (chars[i].is_alphanumeric() || ((chars[i] == '\'' || chars[i] == '’' || chars[i] == '-') && i + 1 < chars.len() && chars[i + 1].is_alphabetic())) {
            i += 1;
        }
        let word: String = chars[start..i].iter().collect();
        let url_like = (start > 0 && matches!(chars[start - 1], '@' | '.' | '/' | ':')) || (i < chars.len() && matches!(chars[i], '@' | '/'));
        let caps = word.chars().filter(|c| c.is_alphabetic()).all(|c| c.is_uppercase());
        if word.chars().count() > 1 && !word.chars().any(|c| c.is_ascii_digit()) && !caps && !url_like {
            out.push((start, i - start, word.replace('’', "'")));
        }
    }
    out
}

pub fn check(word: &str, user: &HashSet<String>) -> bool {
    if user.contains(word) || user.contains(&word.to_lowercase()) {
        return true;
    }
    // Hyphenated compounds are fine when every part is.
    if word.contains('-') {
        return word.split('-').all(|w| w.is_empty() || check(w, user));
    }
    dictionary().check(word)
}

pub fn suggest(word: &str) -> Vec<String> {
    let mut out = Vec::new();
    dictionary().suggest(word, &mut out);
    out.truncate(6);
    out
}

/// Load the dictionary ahead of time (it takes a moment the first time).
pub fn warm() {
    let _ = dictionary().check("warm");
}
