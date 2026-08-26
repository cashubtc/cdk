//! JSON Canonicalization Scheme (RFC 8785)
//!
//! Produces the byte sequence a signature is computed over, so two
//! implementations that serialize the same JSON value differently still sign
//! the same bytes. Used by NUT-06 for the mint info signature.

use std::fmt::Write;

use serde_json::Value;
use thiserror::Error;

/// The `MintInfo` tree is a handful of levels deep. The cap only exists so a
/// hostile `Value` cannot recurse until the stack runs out.
const MAX_DEPTH: usize = 64;

/// RFC 8785 numbers are IEEE-754 doubles, so integers above this have no exact
/// representation and a conformant implementation would emit the nearest
/// double rather than the literal digits.
const MAX_EXACT_INTEGER: i128 = 1 << 53;

/// JCS Error
#[derive(Debug, Error)]
pub enum Error {
    /// Number with no canonical form under this implementation
    #[error("Number {0} cannot be canonicalized")]
    UncanonicalizableNumber(String),
    /// Value nested deeper than the recursion cap
    #[error("JSON value nested more than {MAX_DEPTH} levels deep")]
    TooDeep,
    /// Serde JSON error
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// Formatting error
    #[error(transparent)]
    Format(#[from] std::fmt::Error),
}

/// Canonicalize a JSON value per RFC 8785.
///
/// Only integers exactly representable as IEEE-754 doubles are accepted.
/// Formatting anything else means implementing the ECMAScript number rules,
/// which is the one part of the scheme that is easy to get subtly wrong, and no
/// Cashu type that reaches here carries such a number.
pub fn to_canonical_string(value: &Value) -> Result<String, Error> {
    let mut out = String::new();
    write_value(value, 0, &mut out)?;
    Ok(out)
}

/// [`to_canonical_string`] as the UTF-8 bytes a signature is computed over.
pub fn to_canonical_bytes(value: &Value) -> Result<Vec<u8>, Error> {
    Ok(to_canonical_string(value)?.into_bytes())
}

fn write_value(value: &Value, depth: usize, out: &mut String) -> Result<(), Error> {
    if depth > MAX_DEPTH {
        return Err(Error::TooDeep);
    }

    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => {
            let integer = number
                .as_u64()
                .map(i128::from)
                .or_else(|| number.as_i64().map(i128::from))
                .filter(|value| value.abs() <= MAX_EXACT_INTEGER)
                .ok_or_else(|| Error::UncanonicalizableNumber(number.to_string()))?;

            write!(out, "{integer}")?;
        }
        Value::String(string) => write_string(string, out)?,
        Value::Array(elements) => {
            out.push('[');
            for (index, element) in elements.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_value(element, depth + 1, out)?;
            }
            out.push(']');
        }
        Value::Object(members) => {
            let mut sorted: Vec<(&String, &Value)> = members.iter().collect();
            sorted.sort_by(|(left, _), (right, _)| left.encode_utf16().cmp(right.encode_utf16()));

            out.push('{');
            for (index, (key, member)) in sorted.into_iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(key, out)?;
                out.push(':');
                write_value(member, depth + 1, out)?;
            }
            out.push('}');
        }
    }

    Ok(())
}

/// serde_json's string escaping already matches RFC 8785 section 3.2.2.2: short
/// escapes for the named control characters, `\u00xx` for the rest, and no
/// escaping of `/` or of non-ASCII.
fn write_string(string: &str, out: &mut String) -> Result<(), Error> {
    out.push_str(&serde_json::to_string(string)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// RFC 8785 appendix B. `\u{1f600}` sorts before `\u{fb33}` under UTF-16
    /// code units and after it under UTF-8 bytes, so this vector is what
    /// separates a correct implementation from one that leans on serde_json's
    /// `BTreeMap` ordering.
    #[test]
    fn keys_sort_by_utf16_code_units() {
        let value = json!({
            "\u{20ac}": "Euro Sign",
            "\r": "Carriage Return",
            "\u{fb33}": "Hebrew Letter Dalet With Dagesh",
            "1": "One",
            "\u{1f600}": "Emoji: Grinning Face",
            "\u{80}": "Control",
            "\u{f6}": "Latin Small Letter O With Diaeresis",
        });

        assert_eq!(
            to_canonical_string(&value).expect("canonicalize"),
            concat!(
                r#"{"\r":"Carriage Return","#,
                r#""1":"One","#,
                "\"\u{80}\":\"Control\",",
                "\"\u{f6}\":\"Latin Small Letter O With Diaeresis\",",
                "\"\u{20ac}\":\"Euro Sign\",",
                "\"\u{1f600}\":\"Emoji: Grinning Face\",",
                "\"\u{fb33}\":\"Hebrew Letter Dalet With Dagesh\"}"
            )
        );
    }

    /// RFC 8785 section 3.2.2.2: short escapes where they exist, lowercase
    /// `\u00xx` for the remaining controls, and no escaping of `/` or of
    /// anything above U+001F.
    #[test]
    fn strings_use_the_minimal_escaping() {
        let value = Value::String("\u{20ac}$\u{f}\nA'B\"\\/".to_string());

        assert_eq!(
            to_canonical_string(&value).expect("canonicalize"),
            "\"\u{20ac}$\\u000f\\nA'B\\\"\\\\/\""
        );
    }

    #[test]
    fn output_carries_no_whitespace() {
        let value = json!({"b": [1, 2], "a": {"c": true}});

        assert_eq!(
            to_canonical_string(&value).expect("canonicalize"),
            r#"{"a":{"c":true},"b":[1,2]}"#
        );
    }

    #[test]
    fn array_order_is_preserved() {
        let value = json!(["c", "a", "b"]);

        assert_eq!(
            to_canonical_string(&value).expect("canonicalize"),
            r#"["c","a","b"]"#
        );
    }

    #[test]
    fn integers_are_written_verbatim() {
        let value = json!({"big": 9_007_199_254_740_992_u64, "neg": -42, "zero": 0});

        assert_eq!(
            to_canonical_string(&value).expect("canonicalize"),
            r#"{"big":9007199254740992,"neg":-42,"zero":0}"#
        );
    }

    #[test]
    fn floats_are_rejected_rather_than_guessed_at() {
        let value = json!({"amount": 1.5});

        assert!(matches!(
            to_canonical_string(&value),
            Err(Error::UncanonicalizableNumber(_))
        ));
    }

    /// Past 2^53 an integer has no exact double, so a conformant
    /// implementation would emit the nearest one. Erroring beats emitting
    /// bytes another implementation would disagree with.
    #[test]
    fn integers_beyond_exact_double_range_are_rejected() {
        let value = json!({"amount": 9_007_199_254_740_993_u64});

        assert!(matches!(
            to_canonical_string(&value),
            Err(Error::UncanonicalizableNumber(_))
        ));
    }

    #[test]
    fn literals_round_trip() {
        let value = json!([null, true, false]);

        assert_eq!(
            to_canonical_string(&value).expect("canonicalize"),
            "[null,true,false]"
        );
    }

    #[test]
    fn nesting_past_the_cap_is_an_error() {
        let mut value = Value::Null;
        for _ in 0..=MAX_DEPTH {
            value = Value::Array(vec![value]);
        }

        assert!(matches!(to_canonical_string(&value), Err(Error::TooDeep)));
    }
}
