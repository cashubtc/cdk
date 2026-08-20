//! Terminal-safe rendering helpers for CLI output formats.

use std::fmt::Write as _;

use cdk_common::terminal::is_bidi_control_character;

pub use cdk_common::terminal::escape_control;

/// Escapes terminal controls inside JSON strings without changing JSON layout.
///
/// `input` must be valid JSON produced by a serializer. Structural whitespace
/// is preserved, while controls and bidirectional formatting characters inside
/// string values are replaced with JSON Unicode escapes.
pub fn escape_json(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut in_string = false;
    let mut previous_was_escape = false;

    for ch in input.chars() {
        if in_string && (ch.is_control() || is_bidi_control_character(ch)) {
            write!(output, "\\u{:04x}", ch as u32).expect("writing to a String cannot fail");
        } else {
            output.push(ch);
        }

        if in_string {
            if previous_was_escape {
                previous_was_escape = false;
            } else {
                match ch {
                    '\\' => previous_was_escape = true,
                    '"' => in_string = false,
                    _ => {}
                }
            }
        } else if ch == '"' {
            in_string = true;
        }
    }

    output
}

/// Escapes terminal controls in CBOR diagnostic text while preserving layout.
///
/// Newlines emitted by the diagnostic formatter are retained. Newlines and
/// other controls inside quoted text strings are escaped visibly.
pub fn escape_cbor_diag(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut segment_start = 0;
    let mut in_text_string = false;
    let mut previous_was_escape = false;

    for (index, ch) in input.char_indices() {
        if ch == '\n' && !in_text_string {
            output.push_str(&escape_control(&input[segment_start..index]));
            output.push('\n');
            segment_start = index + ch.len_utf8();
            continue;
        }

        if in_text_string {
            if previous_was_escape {
                previous_was_escape = false;
            } else {
                match ch {
                    '\\' => previous_was_escape = true,
                    '"' => in_text_string = false,
                    _ => {}
                }
            }
        } else if ch == '"' {
            in_text_string = true;
        }
    }

    output.push_str(&escape_control(&input[segment_start..]));
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_escaping_preserves_layout_and_value() {
        let value = serde_json::json!({
            "id": "safe\u{007f}\u{009b}\u{202e}spoof",
            "nested": { "value": "line\nfeed" },
        });
        let input = serde_json::to_string_pretty(&value).expect("JSON serialization succeeds");
        let escaped = escape_json(&input);

        assert!(escaped.contains('\n'));
        assert!(!escaped
            .chars()
            .any(|ch| matches!(ch, '\u{007f}' | '\u{009b}' | '\u{202e}')));
        assert!(escaped.contains("safe\\u007f\\u009b\\u202espoof"));
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&escaped).expect("escaped JSON is valid"),
            value
        );
    }

    #[test]
    fn json_escaping_handles_escaped_quotes_and_backslashes() {
        let value = serde_json::json!({"text": "quoted: \" path: \\\\ \u{009d}"});
        let input = serde_json::to_string_pretty(&value).expect("JSON serialization succeeds");
        let escaped = escape_json(&input);

        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&escaped).expect("escaped JSON is valid"),
            value
        );
        assert!(escaped.contains("\\u009d"));
    }

    #[test]
    fn cbor_diag_escaping_preserves_only_structural_newlines() {
        let input = "{\n    \"d\": \"line one\nline two\u{1b}]52;evil\u{07}\u{202e}\",\n}";
        let escaped = escape_cbor_diag(input);

        assert_eq!(
            escaped,
            "{\n    \"d\": \"line one\\nline two\\e]52;evil\\a\\u{202e}\",\n}"
        );
    }

    #[test]
    fn cbor_diag_escaping_tracks_escaped_quotes_and_backslashes() {
        let input = concat!(
            "{\n",
            "    \"d\": \"quoted: \\\" slash: \\\\ text",
            "\n",
            "unsafe\",\n",
            "}"
        );
        let escaped = escape_cbor_diag(input);

        assert_eq!(escaped.matches('\n').count(), 2);
        assert!(escaped.contains(r#"quoted: \" slash: \\ text\nunsafe"#));
    }
}
