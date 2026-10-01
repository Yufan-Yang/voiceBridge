//! Deterministic local normalization and the fallback used when the prompt
//! model fails. Neither touches the raw transcript.

pub const FALLBACK_NOTICE: &str =
    "Prompt compilation failed. The transcription has been preserved.";

/// Splits an identifier into lowercase words:
/// `useUserQuery` → `["use", "user", "query"]`.
fn spoken_words(term: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut prev_lower = false;
    for ch in term.chars() {
        if !ch.is_ascii_alphanumeric() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            prev_lower = false;
            continue;
        }
        if ch.is_ascii_uppercase() && prev_lower && !current.is_empty() {
            words.push(std::mem::take(&mut current));
        }
        prev_lower = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        current.push(ch.to_ascii_lowercase());
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn is_word_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Replaces spoken forms of multi-word project identifiers ("use user query")
/// with the identifier itself (`useUserQuery`). ASCII case-insensitive, whole
/// words only.
fn correct_identifiers(text: &str, terms: &[String]) -> String {
    let mut out = text.to_string();
    // Longer spoken forms first so "user id token" wins over "user id".
    let mut candidates: Vec<(String, &String)> = terms
        .iter()
        .filter_map(|t| {
            let words = spoken_words(t);
            (words.len() >= 2).then(|| (words.join(" "), t))
        })
        .collect();
    candidates.sort_by_key(|c| std::cmp::Reverse(c.0.len()));
    for (spoken, term) in candidates {
        let mut from = 0;
        loop {
            // to_ascii_lowercase keeps byte offsets identical.
            let hay = out.to_ascii_lowercase();
            let Some(rel) = hay[from..].find(&spoken) else {
                break;
            };
            let start = from + rel;
            let end = start + spoken.len();
            let bytes = hay.as_bytes();
            let left_ok = start == 0 || !is_word_char(bytes[start - 1]);
            let right_ok = end == bytes.len() || !is_word_char(bytes[end]);
            if left_ok && right_ok {
                out.replace_range(start..end, term);
                from = start + term.len();
            } else {
                from = end;
            }
            if from >= out.len() {
                break;
            }
        }
    }
    out
}

fn is_cjk(c: char) -> bool {
    matches!(c as u32, 0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF)
}

/// Chinese and Japanese text: recognizers often emit ASCII punctuation and
/// stray spaces between characters. Use full-width punctuation after a CJK
/// character and drop spaces between two CJK characters.
fn tidy_cjk(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    for (i, &c) in chars.iter().enumerate() {
        let prev = out.chars().next_back();
        let next = chars[i + 1..].iter().copied().find(|n| *n != ' ');
        let after_cjk = prev.is_some_and(|p| is_cjk(p) || "，。？！：；".contains(p));
        if c == ' '
            && after_cjk
            && next.is_none_or(|n| is_cjk(n) || "，。？！：；,.?!:;".contains(n))
        {
            continue;
        }
        let mapped = match c {
            ',' if after_cjk => '，',
            '?' if after_cjk => '？',
            '!' if after_cjk => '！',
            ':' if after_cjk => '：',
            ';' if after_cjk => '；',
            // A full stop, but not a decimal point or a file extension.
            '.' if after_cjk && next.is_none_or(|n| !n.is_ascii_alphanumeric()) => '。',
            other => other,
        };
        out.push(mapped);
    }
    out
}

/// Punctuation, spacing and project-identifier correction.
pub fn normalize_transcript(raw: &str, terms: &[String]) -> String {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return collapsed;
    }
    let mut text = correct_identifiers(&collapsed, terms);
    if text.chars().any(is_cjk) {
        text = tidy_cjk(&text);
    }
    let starts_with_identifier = terms.iter().any(|t| text.starts_with(t.as_str()));
    if !starts_with_identifier {
        if let Some(first) = text.chars().next() {
            if first.is_ascii_lowercase() {
                text.replace_range(0..1, &first.to_ascii_uppercase().to_string());
            }
        }
    }
    if text
        .chars()
        .last()
        .is_some_and(|c| c.is_ascii_alphanumeric())
    {
        text.push('.');
    }
    text
}

/// Fallback `compiled_prompt` when the prompt model fails.
pub fn fallback_prompt(normalized_transcript: &str) -> String {
    normalized_transcript.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn corrects_spoken_identifiers() {
        let t = terms(&["useUserQuery", "userId", "TanStack Query", "enabled"]);
        let out = normalize_transcript("change that  use user query thing so it takes user id", &t);
        assert_eq!(out, "Change that useUserQuery thing so it takes userId.");
    }

    #[test]
    fn respects_word_boundaries_and_snake_case() {
        let t = terms(&["load_settings", "userId"]);
        assert_eq!(
            normalize_transcript("superuser ideas", &t),
            "Superuser ideas."
        );
        assert_eq!(
            normalize_transcript("call load settings twice", &t),
            "Call load_settings twice."
        );
    }

    #[test]
    fn punctuation_and_empty_input() {
        assert_eq!(normalize_transcript("   ", &[]), "");
        assert_eq!(normalize_transcript("done already!", &[]), "Done already!");
        assert_eq!(normalize_transcript("修复登录错误", &[]), "修复登录错误");
        let t = terms(&["useUserQuery"]);
        assert_eq!(
            normalize_transcript("use user query is broken", &t),
            "useUserQuery is broken."
        );
    }

    #[test]
    fn chinese_gets_full_width_punctuation_and_no_stray_spaces() {
        assert_eq!(
            normalize_transcript("把登录接口的超时时间改成30秒,然后加一个重试机制.", &[]),
            "把登录接口的超时时间改成30秒，然后加一个重试机制。"
        );
        assert_eq!(
            normalize_transcript("给API加上分页参数 Page和Page Size 默认每页20条", &[]),
            "给API加上分页参数 Page和Page Size 默认每页20条"
        );
        assert_eq!(
            normalize_transcript("先跑 测试, 失败了吗?", &[]),
            "先跑测试，失败了吗？"
        );
        // File names, versions and English sentences are untouched.
        assert_eq!(
            normalize_transcript("修改main.rs和v1.2版本", &[]),
            "修改main.rs和v1.2版本"
        );
        assert_eq!(
            normalize_transcript("fix it, then test.", &[]),
            "Fix it, then test."
        );
    }

    #[test]
    fn spoken_word_splitting() {
        assert_eq!(spoken_words("useUserQuery"), vec!["use", "user", "query"]);
        assert_eq!(spoken_words("load_settings"), vec!["load", "settings"]);
        assert_eq!(spoken_words("my-app"), vec!["my", "app"]);
        assert_eq!(spoken_words("HTTPServer"), vec!["httpserver"]);
    }
}
