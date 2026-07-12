//! Terminal-safe rendering of potentially untrusted strings.

use std::fmt::Write as _;

/// Escapes control and bidirectional formatting characters in `input`.
///
/// The returned string is safe to include in terminal output and single-line
/// logs. Printable characters are preserved, while C0/C1 controls, DEL, and
/// Unicode bidirectional formatting characters are rendered visibly.
pub fn escape_control(input: &str) -> String {
    let mut escaped = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '\0' => escaped.push_str("\\0"),
            '\u{07}' => escaped.push_str("\\a"),
            '\u{08}' => escaped.push_str("\\b"),
            '\t' => escaped.push_str("\\t"),
            '\n' => escaped.push_str("\\n"),
            '\u{0B}' => escaped.push_str("\\v"),
            '\u{0C}' => escaped.push_str("\\f"),
            '\r' => escaped.push_str("\\r"),
            '\u{1B}' => escaped.push_str("\\e"),
            ch if ch.is_control() || is_bidi_control_character(ch) => {
                write!(escaped, "\\u{{{:02x}}}", ch as u32)
                    .expect("writing to a String cannot fail");
            }
            ch => escaped.push(ch),
        }
    }
    escaped
}

/// Returns whether `ch` has Unicode's `Bidi_Control` property.
pub const fn is_bidi_control_character(ch: char) -> bool {
    matches!(
        ch,
        '\u{061c}'
            | '\u{200e}'
            | '\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2066}'..='\u{2069}'
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn printable_text_passes_through() {
        assert_eq!(escape_control("lnbc2500u1pjexample"), "lnbc2500u1pjexample");
    }

    #[test]
    fn printable_unicode_is_preserved() {
        assert_eq!(escape_control("₿ ünïcodé 🚀"), "₿ ünïcodé 🚀");
    }

    #[test]
    fn escapes_cr_lf_and_tabs() {
        assert_eq!(escape_control("a\nb\rc\td"), "a\\nb\\rc\\td");
    }

    #[test]
    fn escapes_esc_and_csi_sequence() {
        assert_eq!(escape_control("\u{1b}[2Jevil"), "\\e[2Jevil");
    }

    #[test]
    fn escapes_osc_sequence_with_bel_terminator() {
        assert_eq!(escape_control("\u{1b}]0;pwned\u{07}"), "\\e]0;pwned\\a");
    }

    #[test]
    fn escapes_c1_controls() {
        assert_eq!(escape_control("\u{85}\u{9b}"), "\\u{85}\\u{9b}");
    }

    #[test]
    fn escapes_del_and_null() {
        assert_eq!(escape_control("\u{7f}\0"), "\\u{7f}\\0");
    }

    #[test]
    fn escapes_unicode_bidi_controls() {
        let input = "\u{061c}\u{200e}\u{200f}\u{202a}\u{202e}\u{2066}\u{2069}";
        let output = escape_control(input);

        assert_eq!(
            output,
            "\\u{61c}\\u{200e}\\u{200f}\\u{202a}\\u{202e}\\u{2066}\\u{2069}"
        );
        assert!(!output.chars().any(is_bidi_control_character));
    }
}
