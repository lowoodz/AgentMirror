//! In-memory fixed-string search via ripgrep's grep core libraries (no `rg` CLI).

use grep_matcher::Matcher;
use grep_regex::RegexMatcherBuilder;

/// Return needles from `needles` that appear as substrings in `haystack`.
pub fn find_matching_needles(haystack: &str, needles: &[String]) -> Vec<String> {
    if needles.is_empty() || haystack.is_empty() {
        return Vec::new();
    }

    let mut found = Vec::new();
    for needle in needles {
        if needle.is_empty() {
            continue;
        }
        if contains_fixed_string(haystack.as_bytes(), needle.as_bytes()) {
            found.push(needle.clone());
        }
    }
    found
}

/// All byte offsets where `needle` occurs in `haystack` (non-overlapping scan, step 1 after each hit).
pub fn find_literal_byte_offsets(haystack: &[u8], needle: &[u8]) -> Vec<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return Vec::new();
    }

    if let Ok(needle_str) = std::str::from_utf8(needle) {
        if let Ok(matcher) = RegexMatcherBuilder::new()
            .fixed_strings(true)
            .build(needle_str)
        {
            let mut out = Vec::new();
            let mut from = 0usize;
            while from < haystack.len() {
                match matcher.find(&haystack[from..]) {
                    Ok(Some(m)) => {
                        out.push(from + m.start());
                        from += m.start() + 1;
                    }
                    _ => break,
                }
            }
            return out;
        }
    }

    haystack
        .windows(needle.len())
        .enumerate()
        .filter(|(_, window)| *window == needle)
        .map(|(idx, _)| idx)
        .collect()
}

fn contains_fixed_string(haystack: &[u8], needle: &[u8]) -> bool {
    !find_literal_byte_offsets(haystack, needle).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_literal_needle_in_haystack() {
        let needles = vec!["secret-token".to_string()];
        let hay = "user pasted secret-token here";
        assert_eq!(find_matching_needles(hay, &needles), needles);
    }

    #[test]
    fn misses_absent_needle() {
        let needles = vec!["missing".to_string()];
        let hay = "nothing relevant";
        assert!(find_matching_needles(hay, &needles).is_empty());
    }

    #[test]
    fn handles_multiline_haystack() {
        let needles = vec!["line-two".to_string()];
        let hay = "line-one\nline-two\nline-three";
        assert_eq!(find_matching_needles(hay, &needles), needles);
    }

    #[test]
    fn handles_unicode() {
        let needles = vec!["机密项目".to_string()];
        let hay = "这是机密项目的内容";
        assert_eq!(find_matching_needles(hay, &needles), needles);
    }

    #[test]
    fn returns_multiple_matching_needles() {
        let needles = vec![
            "alpha".to_string(),
            "beta".to_string(),
            "gamma".to_string(),
        ];
        let hay = "has alpha and gamma";
        assert_eq!(
            find_matching_needles(hay, &needles),
            vec!["alpha".to_string(), "gamma".to_string()]
        );
    }

    #[test]
    fn finds_all_byte_offsets() {
        let hay = b"abc abc abc";
        let needle = b"abc";
        assert_eq!(find_literal_byte_offsets(hay, needle), vec![0, 4, 8]);
    }
}
