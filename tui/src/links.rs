//! Web links in message text, for opening with `o`.
//!
//! Only `http://`, `https://` and bare `www.` links count. Messages come from
//! other people, so other schemes (`file:`, app-specific ones) are never
//! handed to the desktop to open.

/// The links in `text`, in order, without repeats. `www.` links get `https://`.
pub fn find(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        let Some(url) = link(word) else { continue };
        if !out.contains(&url) {
            out.push(url);
        }
    }
    out
}

/// The host of a link, for "Opened example.com".
pub fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}

fn link(word: &str) -> Option<String> {
    // Quotes and brackets around a link aren't part of it.
    let word = word.trim_start_matches(['(', '[', '<', '"', '\'', '“', '‘']);
    let lower = word.to_ascii_lowercase();
    let url = if lower.starts_with("http://") || lower.starts_with("https://") {
        word.to_string()
    } else if lower.starts_with("www.") {
        format!("https://{word}")
    } else {
        return None;
    };
    let url = trim_end(&url);
    let host = host(url);
    // A host needs a dot or a port, and no characters a host can't have.
    let valid = !host.is_empty()
        && (host.contains('.') || host.contains(':'))
        && host
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | ':' | '[' | ']' | '@'));
    (valid && !url.chars().any(char::is_control)).then(|| url.to_string())
}

/// Drops sentence punctuation after a link ("see https://x.org/a."), and a
/// closing bracket unless the link opened one (Wikipedia-style paths do).
fn trim_end(url: &str) -> &str {
    let mut url = url;
    loop {
        let Some(last) = url.chars().last() else {
            return url;
        };
        let strip = match last {
            '.' | ',' | ';' | ':' | '!' | '?' | '"' | '\'' | '”' | '’' | '>' => true,
            ')' => url.matches(')').count() > url.matches('(').count(),
            ']' => url.matches(']').count() > url.matches('[').count(),
            _ => false,
        };
        if !strip {
            return url;
        }
        url = &url[..url.len() - last.len_utf8()];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_links_in_text() {
        assert_eq!(
            find("see https://example.com/a?b=1#c and http://x.org"),
            ["https://example.com/a?b=1#c", "http://x.org"]
        );
        assert_eq!(
            find("www.example.com/page"),
            ["https://www.example.com/page"]
        );
        assert_eq!(find("HTTPS://Example.com"), ["HTTPS://Example.com"]);
        assert_eq!(find("no links here, 10:30"), Vec::<String>::new());
    }

    #[test]
    fn trims_surrounding_punctuation() {
        assert_eq!(find("look: https://x.org/a."), ["https://x.org/a"]);
        assert_eq!(find("(https://x.org/a), ok?"), ["https://x.org/a"]);
        assert_eq!(find("“https://x.org/a”"), ["https://x.org/a"]);
        assert_eq!(
            find("https://en.wikipedia.org/wiki/Rust_(programming_language)."),
            ["https://en.wikipedia.org/wiki/Rust_(programming_language)"]
        );
    }

    #[test]
    fn only_web_links() {
        assert!(find("file:///etc/passwd").is_empty());
        assert!(find("javascript:alert(1)").is_empty());
        assert!(find("ssh://host.example").is_empty());
        assert!(find("https:// nothing").is_empty());
        assert!(find("https://localhost").is_empty(), "no dot or port");
        assert_eq!(find("http://localhost:8080/x"), ["http://localhost:8080/x"]);
    }

    #[test]
    fn repeats_are_listed_once() {
        assert_eq!(find("https://x.org https://x.org."), ["https://x.org"]);
    }

    #[test]
    fn hosts() {
        assert_eq!(host("https://example.com/a?b"), "example.com");
        assert_eq!(host("https://x.org#top"), "x.org");
        assert_eq!(host("http://localhost:8080"), "localhost:8080");
    }
}
