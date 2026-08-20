const INVALID_PATH_CHARACTER: &str =
    "control, bidirectional formatting, and double-quote characters are not allowed";

pub(crate) fn validate_migration_path(path: &str) -> Result<(), &'static str> {
    if path
        .chars()
        .any(|ch| ch.is_control() || is_bidi_control(ch) || ch == '"')
    {
        Err(INVALID_PATH_CHARACTER)
    } else {
        Ok(())
    }
}

pub(crate) fn rust_string_literal(value: &str) -> String {
    format!("{value:?}")
}

const fn is_bidi_control(ch: char) -> bool {
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
    fn accepts_normal_migration_paths() {
        assert_eq!(
            validate_migration_path("wallet/migrations/001_initialize.sql"),
            Ok(())
        );
    }

    #[test]
    fn rejects_source_and_output_injection_characters() {
        for path in [
            "wallet/migrations/001_newline\n.sql",
            "wallet/migrations/001_carriage\rreturn.sql",
            "wallet/migrations/001_escape\u{1b}.sql",
            "wallet/migrations/001_bell\u{07}.sql",
            "wallet/migrations/001_\"#], malicious.sql",
            "wallet/migrations/001_bidi\u{202e}.sql",
        ] {
            assert_eq!(
                validate_migration_path(path),
                Err(INVALID_PATH_CHARACTER),
                "path should be rejected: {path:?}"
            );
        }
    }

    #[test]
    fn source_literals_are_escaped_defensively() {
        assert_eq!(rust_string_literal("a\"b\n.sql"), "\"a\\\"b\\n.sql\"");
    }
}
