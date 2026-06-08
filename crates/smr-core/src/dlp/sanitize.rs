use rand::Rng;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Digit,
    Cjk,
    LatinUpper,
    LatinLower,
    LatinExt,
    EuroOther,
    Other,
    NonReadable,
}

pub fn sanitize_range(text: &str, start: usize, end: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut rng = rand::thread_rng();
    chars
        .iter()
        .enumerate()
        .map(|(i, &c)| {
            if i >= start && i < end {
                replace_char(c, &mut rng)
            } else {
                c
            }
        })
        .collect()
}

pub fn sanitize_whole(text: &str) -> String {
    let mut rng = rand::thread_rng();
    text.chars()
        .map(|c| replace_char(c, &mut rng))
        .collect()
}

fn replace_char(c: char, rng: &mut impl Rng) -> char {
    match classify(c) {
        CharClass::NonReadable => c,
        CharClass::Digit => {
            let d = rng.gen_range(0..10);
            char::from(b'0' + d)
        }
        CharClass::Cjk => random_from(CJK_SAMPLE, rng),
        CharClass::LatinUpper => random_from(LATIN_UPPER, rng),
        CharClass::LatinLower => random_from(LATIN_LOWER, rng),
        CharClass::LatinExt => random_from(LATIN_EXT, rng),
        CharClass::EuroOther => random_from(EURO_OTHER, rng),
        CharClass::Other => random_from(OTHER_SAMPLE, rng),
    }
}

fn random_from(pool: &[char], rng: &mut impl Rng) -> char {
    pool[rng.gen_range(0..pool.len())]
}

fn classify(c: char) -> CharClass {
    if c.is_ascii_digit() {
        return CharClass::Digit;
    }
    if c.is_ascii_uppercase() {
        return CharClass::LatinUpper;
    }
    if c.is_ascii_lowercase() {
        return CharClass::LatinLower;
    }
    if c.is_whitespace() || c.is_ascii_punctuation() || c.is_ascii_control() {
        return CharClass::NonReadable;
    }
    let cp = c as u32;
    if (0x4E00..=0x9FFF).contains(&cp)
        || (0x3400..=0x4DBF).contains(&cp)
        || (0x3040..=0x30FF).contains(&cp)
        || (0xAC00..=0xD7AF).contains(&cp)
    {
        return CharClass::Cjk;
    }
    if (0x00C0..=0x024F).contains(&cp) {
        return CharClass::LatinExt;
    }
    if (0x0400..=0x04FF).contains(&cp) || (0x0370..=0x03FF).contains(&cp) {
        return CharClass::EuroOther;
    }
    if c.is_alphabetic() {
        return CharClass::Other;
    }
    CharClass::NonReadable
}

const CJK_SAMPLE: &[char] = &['李', '王', '张', '刘', '陈', '杨', '赵', '黄', '周', '吴'];
const LATIN_UPPER: &[char] = &['A', 'B', 'C', 'D', 'E', 'F', 'G', 'H', 'K', 'M', 'N', 'P', 'R', 'S', 'T'];
const LATIN_LOWER: &[char] = &['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'k', 'm', 'n', 'p', 'r', 's', 't'];
const LATIN_EXT: &[char] = &['à', 'á', 'â', 'ã', 'ä', 'å', 'æ', 'ç', 'è', 'é', 'ê', 'ë'];
const EURO_OTHER: &[char] = &['α', 'β', 'γ', 'δ', 'ε', 'ж', 'з', 'и', 'к', 'л'];
const OTHER_SAMPLE: &[char] = &['ا', 'ب', 'ت', 'ث', 'ก', 'ข', 'ค', 'ง', 'ฮ', 'อ'];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_length() {
        let input = "abc123中文";
        let out = sanitize_whole(input);
        assert_eq!(input.chars().count(), out.chars().count());
    }

    #[test]
    fn preserves_case() {
        let input = "Hello WORLD";
        let out = sanitize_whole(input);
        for (i, c) in out.chars().enumerate() {
            let orig = input.chars().nth(i).unwrap();
            assert_eq!(c.is_ascii_uppercase(), orig.is_ascii_uppercase());
        }
    }

    #[test]
    fn replaces_other_scripts() {
        let input = "مرحبا";
        let out = sanitize_whole(input);
        assert_ne!(input, out);
        assert_eq!(input.chars().count(), out.chars().count());
    }
}
