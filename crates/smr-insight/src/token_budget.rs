use crate::models::CognitiveEvent;

/// Max estimated tokens per event batch (per AgentMirror spec).
pub const MAX_BATCH_TOKENS: usize = 100_000;

/// Token estimate: CJK/Hangana/Kana → 1 per char; Latin (incl. FR/DE) → whitespace words;
/// other scripts → 1 per char. Whitespace between Latin words is not counted.
pub fn estimate_tokens(text: &str) -> usize {
    let mut tokens = 0usize;
    let mut latin_word_open = false;

    for ch in text.chars() {
        if ch.is_whitespace() {
            latin_word_open = false;
            continue;
        }
        if is_cjk(ch) {
            tokens += 1;
            latin_word_open = false;
        } else if is_latin_letter(ch) {
            if !latin_word_open {
                tokens += 1;
                latin_word_open = true;
            }
        } else {
            tokens += 1;
            latin_word_open = false;
        }
    }
    tokens
}

fn is_cjk(ch: char) -> bool {
    matches!(
        ch,
        '\u{4E00}'..='\u{9FFF}'
            | '\u{3400}'..='\u{4DBF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{3040}'..='\u{30FF}'
            | '\u{AC00}'..='\u{D7AF}'
    )
}

fn is_latin_letter(ch: char) -> bool {
    ch.is_ascii_alphabetic()
        || matches!(
            ch,
            '\u{00C0}'..='\u{024F}' | '\u{1E00}'..='\u{1EFF}'
        )
}

pub fn format_event_line(event: &CognitiveEvent) -> String {
    format!(
        "#{} [{}] {}\n",
        event.seq,
        event.kind.as_str(),
        event.summary
    )
}

/// Split events into batches where each batch's formatted lines stay within `max_tokens`.
pub fn batch_events<'a>(
    events: &'a [CognitiveEvent],
    max_tokens: usize,
) -> Vec<Vec<&'a CognitiveEvent>> {
    if events.is_empty() {
        return Vec::new();
    }
    let mut batches: Vec<Vec<&'a CognitiveEvent>> = Vec::new();
    let mut current: Vec<&'a CognitiveEvent> = Vec::new();
    let mut current_tokens = 0usize;

    for event in events {
        let line_tokens = estimate_tokens(&format_event_line(event));
        if !current.is_empty() && current_tokens.saturating_add(line_tokens) > max_tokens {
            batches.push(current);
            current = Vec::new();
            current_tokens = 0;
        }
        current_tokens = current_tokens.saturating_add(line_tokens);
        current.push(event);
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

pub fn format_batch_trajectory(events: &[&CognitiveEvent]) -> String {
    let mut out = String::new();
    for event in events {
        out.push_str(&format_event_line(event));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::EventKind;
    use chrono::Utc;

    fn ev(seq: u32, summary: &str) -> CognitiveEvent {
        CognitiveEvent {
            id: format!("e{seq}"),
            run_id: "r1".into(),
            seq,
            kind: EventKind::Action,
            timestamp: Utc::now(),
            summary: summary.into(),
            audit_id: "a1".into(),
            confidence: 1.0,
            metadata: serde_json::Value::Null,
        }
    }

    #[test]
    fn cjk_counts_per_character() {
        assert_eq!(estimate_tokens("论文摘要"), 4);
    }

    #[test]
    fn latin_counts_whitespace_words() {
        assert_eq!(estimate_tokens("hello world"), 2);
        assert_eq!(estimate_tokens("résumé du rapport"), 3);
    }

    #[test]
    fn batches_respect_token_limit() {
        let events: Vec<_> = (0..50)
            .map(|i| ev(i, "查看论文摘要并整理要点"))
            .collect();
        let batches = batch_events(&events, 40);
        assert!(batches.len() > 1);
        for batch in &batches {
            let text = format_batch_trajectory(batch);
            assert!(estimate_tokens(&text) <= 40 || batch.len() == 1);
        }
    }
}
