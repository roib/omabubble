//! Turn what the user types into the handle format chat.db uses (E.164 or email).

pub fn normalize(input: &str, country_code: &str) -> Option<String> {
    let s = input.trim();
    if s.is_empty() {
        return None;
    }
    if s.contains('@') {
        return (!s.contains(char::is_whitespace)).then(|| s.to_lowercase());
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_digit() || " -().+".contains(c))
    {
        return None;
    }
    let digits: String = s.chars().filter(char::is_ascii_digit).collect();
    if digits.len() < 3 {
        return None;
    }
    Some(if s.starts_with('+') {
        format!("+{digits}")
    } else if let Some(national) = digits.strip_prefix('0').filter(|n| n.len() >= 7) {
        // National trunk prefix (UK, EU, …): 020 7946 0958 → +44 20 7946 0958.
        format!("+{country_code}{national}")
    } else if digits.len() == 10 {
        format!("+{country_code}{digits}")
    } else if digits.len() >= 11 {
        format!("+{digits}")
    } else {
        // Short codes and local numbers stay as typed digits.
        digits
    })
}

pub fn same(a: &str, b: &str, country_code: &str) -> bool {
    match (normalize(a, country_code), normalize(b, country_code)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes() {
        for input in [
            "555-123-4567",
            "(555) 123 4567",
            "555.123.4567",
            "15551234567",
            "+1 555 123 4567",
        ] {
            assert_eq!(
                normalize(input, "1").as_deref(),
                Some("+15551234567"),
                "{input}"
            );
        }
        assert_eq!(
            normalize("+44 20 7946 0958", "1").as_deref(),
            Some("+442079460958")
        );
        assert_eq!(
            normalize("020 7946 0958", "44").as_deref(),
            Some("+442079460958"),
            "trunk 0 dropped"
        );
        assert_eq!(
            normalize("07700 900123", "44").as_deref(),
            Some("+447700900123")
        );
        assert_eq!(
            normalize("Friend@Example.com", "1").as_deref(),
            Some("friend@example.com")
        );
        assert_eq!(normalize("12345", "1").as_deref(), Some("12345"));
        assert_eq!(normalize("call me", "1"), None);
        assert_eq!(normalize("", "1"), None);
        assert!(same("555-123-4567", "+15551234567", "1"));
    }
}
