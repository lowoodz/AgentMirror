//! Token-set overlap prefilter for file DLP scans.
//!
//! - CJK (Chinese / Japanese / Korean): one character per token
//! - Latin-family text: whitespace-separated words
//! - Other scripts: no fast-skip (always run full scan)

use std::collections::HashSet;

pub type TokenSet = HashSet<String>;

#[derive(Debug, Default, Clone)]
pub struct TokenProfile {
    pub cjk: TokenSet,
    pub latin: TokenSet,
    /// Non-CJK, non-Latin alphabetic tokens; presence disables fast-skip.
    pub other: TokenSet,
}

impl TokenProfile {
    pub fn is_empty(&self) -> bool {
        self.cjk.is_empty() && self.latin.is_empty() && self.other.is_empty()
    }
}

pub fn token_profile(text: &str) -> TokenProfile {
    let mut out = TokenProfile::default();
    accumulate_token_profile(text, text.len(), &mut out);
    out
}

pub fn accumulate_token_profile(text: &str, max_bytes: usize, out: &mut TokenProfile) {
    let sample: String = if text.len() <= max_bytes {
        text.to_string()
    } else {
        text.char_indices()
            .take_while(|(i, _)| *i < max_bytes)
            .map(|(_, c)| c)
            .collect()
    };
    tokenize_into_profile(&sample, out);
}

fn supported_tokens(profile: &TokenProfile) -> TokenSet {
    profile.cjk.union(&profile.latin).cloned().collect()
}

/// Whether fast-skip applies (both sides CJK/Latin only, no other scripts).
pub fn token_prefilter_applicable(hay: &TokenProfile, file: &TokenProfile) -> bool {
    hay.other.is_empty() && file.other.is_empty()
}

/// Overlap for diagnostics: max(|∩|/|A|, |∩|/|B|) on supported tokens.
pub fn token_prefilter_overlap(hay: &TokenProfile, file: &TokenProfile) -> f64 {
    let hay_tokens = supported_tokens(hay);
    let file_tokens = supported_tokens(file);
    max_overlap_ratio(&hay_tokens, &file_tokens)
}

/// Returns true when full Bloom/SQLite scan can be skipped safely.
pub fn should_token_prefilter_skip(
    hay: &TokenProfile,
    file: &TokenProfile,
    threshold: f64,
) -> bool {
    if !token_prefilter_applicable(hay, file) {
        return false;
    }
    let hay_tokens = supported_tokens(hay);
    let file_tokens = supported_tokens(file);
    if hay_tokens.is_empty() || file_tokens.is_empty() {
        return false;
    }
    let shared: TokenSet = hay_tokens.intersection(&file_tokens).cloned().collect();
    if shared.is_empty() {
        return true;
    }
    if shared
        .iter()
        .any(|token| token.chars().count() >= 4)
    {
        return false;
    }
    if token_prefilter_overlap(hay, file) >= threshold {
        return false;
    }
    true
}

fn max_overlap_ratio(a: &TokenSet, b: &TokenSet) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let inter = a.intersection(b).count();
    let r_a = inter as f64 / a.len() as f64;
    let r_b = inter as f64 / b.len() as f64;
    r_a.max(r_b)
}

fn tokenize_into_profile(text: &str, out: &mut TokenProfile) {
    let mut word = String::new();
    for c in text.chars() {
        if is_cjk_token_char(c) {
            push_latin_word(&mut word, &mut out.latin);
            out.cjk.insert(c.to_string());
        } else if is_latin_token_char(c) || c.is_ascii_digit() {
            word.push(c.to_ascii_lowercase());
        } else if c.is_whitespace() {
            push_latin_word(&mut word, &mut out.latin);
        } else if is_other_script_char(c) {
            push_latin_word(&mut word, &mut out.latin);
            out.other.insert(c.to_string());
        } else {
            push_latin_word(&mut word, &mut out.latin);
        }
    }
    push_latin_word(&mut word, &mut out.latin);
}

fn push_latin_word(word: &mut String, out: &mut TokenSet) {
    if word.is_empty() {
        return;
    }
    out.insert(std::mem::take(word));
}

fn is_cjk_token_char(c: char) -> bool {
    let cp = c as u32;
    (0x4E00..=0x9FFF).contains(&cp)
        || (0x3400..=0x4DBF).contains(&cp)
        || (0x3040..=0x30FF).contains(&cp)
        || (0xAC00..=0xD7AF).contains(&cp)
}

/// Latin-family scripts tokenized by whitespace (not Cyrillic / Greek / etc.).
fn is_latin_token_char(c: char) -> bool {
    if c.is_ascii_alphabetic() {
        return true;
    }
    let cp = c as u32;
    (0x00C0..=0x024F).contains(&cp)
}

fn is_other_script_char(c: char) -> bool {
    if c.is_whitespace() || c.is_ascii_punctuation() || c.is_ascii_control() {
        return false;
    }
    if is_cjk_token_char(c) || is_latin_token_char(c) || c.is_ascii_digit() {
        return false;
    }
    c.is_alphabetic()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latin_words_split_on_whitespace() {
        let p = token_profile("Hello World  hello");
        assert!(p.latin.contains("hello"));
        assert!(p.latin.contains("world"));
        assert_eq!(p.latin.len(), 2);
        assert!(p.cjk.is_empty());
    }

    #[test]
    fn chinese_uses_character_tokens() {
        let p = token_profile("上海泰坦");
        assert!(p.cjk.contains("上"));
        assert!(p.cjk.contains("海"));
        assert_eq!(p.cjk.len(), 4);
        assert!(p.latin.is_empty());
    }

    #[test]
    fn english_hay_vs_chinese_file_is_low_overlap() {
        let file = token_profile(
            "上海泰坦科技股份有限公司2025年半年度报告688133重要提示本公司董事会",
        );
        let hay = token_profile(&"The quick brown fox jumps over the lazy dog. ".repeat(100));
        assert!(token_prefilter_applicable(&hay, &file));
        assert!(token_prefilter_overlap(&hay, &file) < 0.5);
    }

    #[test]
    fn pasted_secret_never_prefilter_skips() {
        let secret = "TOPSECRETALPHANUMERIC12345".repeat(3);
        let file = token_profile(&secret);
        let hay = token_profile(&format!("notes {secret} tail"));
        assert!(!should_token_prefilter_skip(&hay, &file, 0.5));
    }

    #[test]
    fn excerpt_hay_shares_many_chinese_tokens_with_file() {
        let file_text = "上海泰坦科技股份有限公司2025年半年度报告重要提示本公司董事会";
        let file = token_profile(file_text);
        let hay = token_profile(&format!("{file_text} 请总结上述内容"));
        assert!(token_prefilter_overlap(&hay, &file) >= 0.5);
    }

    #[test]
    fn other_script_disables_prefilter() {
        let hay = token_profile("مرحبا hello");
        let file = token_profile("hello world");
        assert!(!hay.other.is_empty());
        assert!(!token_prefilter_applicable(&hay, &file));
    }

    #[test]
    fn cyrillic_is_other_not_latin() {
        let p = token_profile("Привет мир");
        assert!(p.latin.is_empty());
        assert!(!p.other.is_empty());
        assert!(!token_prefilter_applicable(&p, &token_profile("hello")));
    }

    #[test]
    fn prefilter_negative_latin_hay_cjk_file_low_overlap() {
        let file = token_profile("内部机密报告上海泰坦科技");
        let hay = token_profile("weather forecast public API documentation");
        assert!(token_prefilter_applicable(&hay, &file));
        assert!(token_prefilter_overlap(&hay, &file) < 0.5);
    }

    #[test]
    fn prefilter_negative_cjk_hay_latin_file_low_overlap() {
        let file = token_profile("TOPSECRETCORPORATEDATABASEKEY");
        let hay = token_profile("今天北京天气晴朗适合出行");
        assert!(token_prefilter_applicable(&hay, &file));
        assert!(token_prefilter_overlap(&hay, &file) < 0.5);
    }

    #[test]
    fn prefilter_positive_latin_hay_shares_secret_tokens() {
        let secret = test_secret_for_profile("POSITIVE-LATIN-OVERLAP-KEY");
        let file = token_profile(&secret);
        let hay = token_profile(&format!("please analyze {secret} now"));
        assert!(!should_token_prefilter_skip(&hay, &file, 0.5));
    }

    #[test]
    fn prefilter_positive_cjk_hay_shares_many_chars() {
        let body = test_cjk_for_profile("核心机密摘要上海泰坦");
        let file = token_profile(&body);
        let hay = token_profile(&format!("{body} 请总结"));
        assert!(!should_token_prefilter_skip(&hay, &file, 0.5));
    }

    #[test]
    fn prefilter_negative_arabic_hay_latin_file_not_applicable() {
        let file = token_profile("english secret token value");
        let hay = token_profile("مرحبا السؤال عن الطقس");
        assert!(!token_prefilter_applicable(&hay, &file));
    }

    #[test]
    fn prefilter_negative_unrelated_latin_both_sides() {
        let file = token_profile("alpha bravo charlie delta secret");
        let hay = token_profile("foo bar baz public news headline");
        assert!(token_prefilter_applicable(&hay, &file));
        assert!(should_token_prefilter_skip(&hay, &file, 0.5));
    }

    #[test]
    fn prefilter_negative_english_vs_chinese_skips() {
        let file = token_profile("上海泰坦科技股份有限公司年报");
        let hay = token_profile("weather forecast public API documentation");
        assert!(should_token_prefilter_skip(&hay, &file, 0.5));
    }

    fn test_secret_for_profile(base: &str) -> String {
        let mut s = base
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .collect::<String>();
        while s.len() < 65 {
            s.push('X');
        }
        s
    }

    fn test_cjk_for_profile(base: &str) -> String {
        let mut s: String = base.chars().filter(|c| !c.is_whitespace()).collect();
        while s.chars().count() < 65 {
            s.push('密');
        }
        s
    }
}
