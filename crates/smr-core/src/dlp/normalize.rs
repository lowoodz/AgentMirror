//! Text normalization for file DLP matching: drop whitespace, punctuation, and other non-readable chars.

/// Normalized text plus mapping back to UTF-8 byte offsets in the original string.
#[derive(Debug, Clone)]
pub struct Normalized {
    pub text: String,
    orig_byte_starts: Vec<usize>,
}

pub fn normalize_with_map(original: &str) -> Normalized {
    let mut text = String::new();
    let mut orig_byte_starts = Vec::new();
    for (byte_idx, c) in original.char_indices() {
        if is_skipped_for_match(c) {
            continue;
        }
        text.push(c);
        orig_byte_starts.push(byte_idx);
    }
    Normalized {
        text,
        orig_byte_starts,
    }
}

impl Normalized {
    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Map a normalized byte range `[norm_start, norm_start + norm_len)` to original UTF-8 byte range.
    pub fn orig_byte_range(
        &self,
        original: &str,
        norm_start: usize,
        norm_len: usize,
    ) -> Option<(usize, usize)> {
        if norm_len == 0 || norm_start >= self.orig_byte_starts.len() {
            return None;
        }
        let byte_start = self.orig_byte_starts[norm_start];
        let norm_end = norm_start.saturating_add(norm_len);
        let last_idx = norm_end.saturating_sub(1).min(self.orig_byte_starts.len() - 1);
        let last_start = self.orig_byte_starts[last_idx];
        let last_char = original[last_start..].chars().next()?;
        let byte_end = last_start + last_char.len_utf8();
        if byte_end <= byte_start {
            return None;
        }
        Some((byte_start, byte_end))
    }
}

fn is_skipped_for_match(c: char) -> bool {
    if c.is_ascii_digit()
        || c.is_ascii_uppercase()
        || c.is_ascii_lowercase()
        || c.is_alphabetic()
    {
        return false;
    }
    if c.is_whitespace() || c.is_ascii_punctuation() || c.is_ascii_control() {
        return true;
    }
    let cp = c as u32;
    if (0x4E00..=0x9FFF).contains(&cp)
        || (0x3400..=0x4DBF).contains(&cp)
        || (0x3040..=0x30FF).contains(&cp)
        || (0xAC00..=0xD7AF).contains(&cp)
        || (0x00C0..=0x024F).contains(&cp)
        || (0x0400..=0x04FF).contains(&cp)
        || (0x0370..=0x03FF).contains(&cp)
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drops_whitespace_and_punctuation() {
        let norm = normalize_with_map("Hello, World!\n\t123");
        assert_eq!(norm.text, "HelloWorld123");
    }

    #[test]
    fn maps_normalized_range_to_original_bytes() {
        let original = "a, b, c";
        let norm = normalize_with_map(original);
        assert_eq!(norm.text, "abc");
        let (start, end) = norm.orig_byte_range(original, 1, 1).unwrap();
        assert_eq!(&original[start..end], "b");
    }

    #[test]
    fn full_range_covers_suffix() {
        let original = "x y z";
        let norm = normalize_with_map(original);
        let (start, end) = norm.orig_byte_range(original, 0, norm.text.len()).unwrap();
        assert_eq!(&original[start..end], "x y z");
    }
}
