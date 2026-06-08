//! Shared fragment length / ratio threshold logic.

pub const DEFAULT_FILE_MIN_FRAGMENT_LEN: usize = 65;
pub const DEFAULT_FILE_MIN_FRAGMENT_RATIO: f64 = 0.5;

/// File DLP: normalized exact substring must meet min length AND block overlap ratio (AND).
pub fn file_fragment_meets_threshold(
    norm_match_len: usize,
    norm_index_chunk_len: usize,
    norm_haystack_chunk_len: usize,
    min_fragment_len: Option<usize>,
    min_fragment_ratio: Option<f64>,
) -> bool {
    if norm_match_len == 0 || norm_index_chunk_len == 0 || norm_haystack_chunk_len == 0 {
        return false;
    }
    let min_len = min_fragment_len.unwrap_or(DEFAULT_FILE_MIN_FRAGMENT_LEN);
    if norm_match_len < min_len {
        return false;
    }
    let min_ratio = min_fragment_ratio.unwrap_or(DEFAULT_FILE_MIN_FRAGMENT_RATIO);
    let r_index = norm_match_len as f64 / norm_index_chunk_len as f64;
    let r_hay = norm_match_len as f64 / norm_haystack_chunk_len as f64;
    r_index.max(r_hay) > min_ratio
}

pub fn file_min_fragment_len(min_fragment_len: Option<usize>) -> usize {
    min_fragment_len
        .unwrap_or(DEFAULT_FILE_MIN_FRAGMENT_LEN)
        .max(1)
}

/// Minimum fragment length considering absolute min and optional ratio of source text.
pub fn effective_min_fragment_len(
    source_len: usize,
    min_fragment_len: Option<usize>,
    min_fragment_ratio: Option<f64>,
) -> usize {
    let base = min_fragment_len.unwrap_or(8).max(1);
    if let Some(ratio) = min_fragment_ratio {
        let from_ratio = ((source_len as f64) * ratio.clamp(0.0, 1.0)).floor() as usize;
        return base.max(from_ratio).max(1);
    }
    base
}

/// Whether a fragment of `fragment_len` should be considered for matching against `source_len`.
pub fn fragment_meets_threshold(
    source_len: usize,
    fragment_len: usize,
    min_fragment_len: Option<usize>,
    min_fragment_ratio: Option<f64>,
) -> bool {
    if fragment_len == 0 || source_len == 0 {
        return false;
    }
    fragment_len >= effective_min_fragment_len(source_len, min_fragment_len, min_fragment_ratio)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_threshold_requires_min_len_and_ratio() {
        assert!(file_fragment_meets_threshold(
            120,
            8192,
            200,
            Some(65),
            Some(0.5)
        ));
        assert!(!file_fragment_meets_threshold(
            64,
            200,
            200,
            Some(65),
            Some(0.5)
        ));
        assert!(!file_fragment_meets_threshold(
            120,
            8192,
            8192,
            Some(65),
            Some(0.5)
        ));
    }

    #[test]
    fn ratio_increases_min_len() {
        assert_eq!(effective_min_fragment_len(100, Some(8), Some(0.1)), 10);
    }

    #[test]
    fn absolute_min_wins_when_larger() {
        assert_eq!(effective_min_fragment_len(50, Some(20), Some(0.1)), 20);
    }
}
