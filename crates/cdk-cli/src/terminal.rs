//! Terminal-safe rendering of potentially attacker-controlled strings.
//!
//! Values supplied by mints, tokens, or OIDC providers may contain terminal
//! control characters (ESC, BEL, C1 controls, CR/LF) that can forge or
//! corrupt CLI output. Escaping renders them visibly while preserving
//! diagnostic evidence, and is preferred over silently stripping data.

use std::fmt::Write as _;

/// Escapes control characters in `input` so the result is safe to print to a
/// terminal.
///
/// Printable characters, including printable Unicode, are preserved as-is.
/// Common control characters use their familiar escape sequences (`\n`,
/// `\r`, `\t`, `\a`, `\b`, `\v`, `\f`, `\e`, `\0`); all remaining control
/// characters, including DEL and the C1 range, are rendered as `\u{XX}`.
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
            ch if ch.is_control() => {
                write!(escaped, "\\u{{{:02x}}}", ch as u32)
                    .expect("writing to a String cannot fail");
            }
            ch => escaped.push(ch),
        }
    }
    escaped
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
        // CSI "clear screen" sequence loses its ESC and becomes inert text
        assert_eq!(escape_control("\u{1b}[2Jevil"), "\\e[2Jevil");
    }

    #[test]
    fn escapes_osc_sequence_with_bel_terminator() {
        // OSC window-title sequence terminated by BEL
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
}
