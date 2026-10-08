//! Emoji lookup for `:shortcode:` completion and the Ctrl+E picker.

use emojis::{Emoji, SkinTone};

/// Commonly used emojis rank ahead of the category order, so `:fi` finds 🔥
/// before 🐟. Your own recents still come first.
const POPULAR: &[&str] = &[
    "👍", "❤️", "😂", "🔥", "🙏", "😭", "😊", "🎉", "😍", "👀", "🤔", "😅", "💯", "✅", "👋", "🙌",
    "😎", "🥳", "😢", "🤣", "😘", "🥰", "👏", "💪", "😮", "🙄", "😬", "🤷",
];

fn is_code_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '+' | '-')
}

/// A `:` at the start or after whitespace, so times (10:30) and URLs don't count.
fn starts_word(before: &str, colon: usize) -> bool {
    before[..colon]
        .chars()
        .next_back()
        .is_none_or(char::is_whitespace)
}

/// The `:query` being typed right before the cursor: (byte offset of the
/// `:`, query). Needs two characters, so a lone `:` doesn't pop anything up.
pub fn token_at(input: &str, cursor: usize) -> Option<(usize, &str)> {
    let before = &input[..cursor];
    let colon = before.rfind(':')?;
    let query = &before[colon + 1..];
    (query.len() >= 2 && query.chars().all(is_code_char) && starts_word(before, colon))
        .then_some((colon, query))
}

/// A complete `:code:` right before the cursor: (byte offset of the first
/// `:`, emoji).
pub fn code_before(input: &str, cursor: usize) -> Option<(usize, &'static str)> {
    let rest = input[..cursor].strip_suffix(':')?;
    let colon = rest.rfind(':')?;
    let code = &rest[colon + 1..];
    if code.is_empty() || !code.chars().all(is_code_char) || !starts_word(rest, colon) {
        return None;
    }
    emojis::get_by_shortcode(code).map(|e| (colon, e.as_str()))
}

/// (recent position, not popular, category, shortcode length); lower sorts first.
type RankKey = (usize, bool, emojis::Group, usize);

/// Shortcode prefix matches first, then emojis whose shortcode or name
/// contains the query. Within prefix matches: recently used first, then
/// commonly used, then by category (faces and people before symbols and
/// flags), then shortest code.
/// Skin-tone variants are left out.
pub fn search(query: &str, recents: &[String], limit: usize) -> Vec<&'static Emoji> {
    let q = query.to_lowercase();
    let recent_rank = |e: &Emoji| {
        recents
            .iter()
            .position(|r| r == e.as_str())
            .unwrap_or(usize::MAX)
    };
    let popular = |e: &Emoji| !POPULAR.contains(&e.as_str());
    let mut prefix: Vec<(RankKey, &'static Emoji)> = Vec::new();
    let mut contains = Vec::new();
    for e in emojis::iter() {
        if e.skin_tone().is_some_and(|t| t != SkinTone::Default) {
            continue;
        }
        if let Some(len) = e
            .shortcodes()
            .filter(|s| s.starts_with(&q))
            .map(str::len)
            .min()
        {
            prefix.push(((recent_rank(e), popular(e), e.group(), len), e));
        } else if e.shortcodes().any(|s| s.contains(&q)) || e.name().to_lowercase().contains(&q) {
            contains.push(e);
        }
    }
    prefix.sort_by_key(|(key, _)| *key);
    prefix
        .into_iter()
        .map(|(_, e)| e)
        .chain(contains)
        .take(limit)
        .collect()
}

/// `:thumbsup:`-style label, preferring the shortcode that matches what's
/// being typed; the Unicode name when there's no shortcode.
pub fn label(e: &Emoji, query: &str) -> String {
    match e
        .shortcodes()
        .find(|s| s.starts_with(query))
        .or(e.shortcode())
    {
        Some(s) => format!(":{s}:"),
        None => e.name().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens() {
        assert_eq!(token_at("hi :thu", 7), Some((3, "thu")));
        assert_eq!(token_at(":fi", 3), Some((0, "fi")));
        assert_eq!(token_at("hi :t", 5), None, "one character isn't enough");
        assert_eq!(token_at("at 10:30", 8), None, "times aren't codes");
        assert_eq!(token_at("see http://x", 12), None);
        assert_eq!(token_at("hi :thu bar", 11), None, "space ends it");
    }

    #[test]
    fn complete_codes() {
        assert_eq!(code_before("so :fire:", 9), Some((3, "🔥")));
        assert_eq!(code_before("so :nope_not_real:", 18), None);
        assert_eq!(code_before("10:30:", 6), None);
    }

    #[test]
    fn ranking() {
        let th = search("th", &[], 6);
        assert_eq!(th[0].as_str(), "🤔", "faces before symbols like :three:");
        assert!(th.iter().any(|e| e.as_str() == "👍"));
        assert!(!th.iter().any(|e| e.as_str() == "3️⃣"));
        assert_eq!(search("thu", &[], 6)[0].as_str(), "👍");
        assert_eq!(
            search("th", &["👍".into()], 6)[0].as_str(),
            "👍",
            "recents first"
        );
        assert!(search("fi", &[], 6).iter().any(|e| e.as_str() == "🔥"));
        assert!(
            search("party popper", &[], 8)
                .iter()
                .any(|e| e.as_str() == "🎉"),
            "names match too"
        );
        assert!(
            search("wave", &[], 20)
                .iter()
                .all(|e| e.skin_tone().is_none_or(|t| t == SkinTone::Default))
        );
        assert_eq!(label(emojis::get("👍").unwrap(), "thu"), ":thumbsup:");
    }
}
