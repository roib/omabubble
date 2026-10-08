//! Keep phone numbers and email addresses out of the log. Messages puts the
//! recipient into its error text, e.g. `Can't get chat id "iMessage;-;x@y"`.

pub fn redact(s: &str) -> String {
    s.split_whitespace()
        .map(|w| {
            if w.contains('@') {
                "<email>"
            } else if w.chars().filter(char::is_ascii_digit).count() >= 7 {
                "<number>"
            } else {
                w
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::redact;

    #[test]
    fn hides_handles_keeps_errors() {
        assert_eq!(
            redact(
                r#"43:92: execution error: Messages got an error: Can’t get chat id "iMessage;-;someone@gmail.com". (-1728)"#
            ),
            "43:92: execution error: Messages got an error: Can’t get chat id <email> (-1728)"
        );
        assert_eq!(
            redact(r#"Can’t get participant "+1 (555) 123-4567"."#),
            "Can’t get participant \"+1 (555) <number>"
        );
        assert_eq!(
            redact("osascript timed out after 30s"),
            "osascript timed out after 30s"
        );
    }
}
