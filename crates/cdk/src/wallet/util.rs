//! Wallet Utility Functions

use super::Error;
use crate::nuts::{CurrencyUnit, Proofs, Token};
use crate::UncheckedUrl;

/// Extract token from text
pub fn token_from_text(text: &str) -> Option<&str> {
    let text = text.trim();
    if let Some(start) = text.find("cashu") {
        match text[start..].find(' ') {
            Some(end) => return Some(&text[start..(end + start)]),
            None => return Some(&text[start..]),
        }
    }

    None
}

/// Convert proofs to token
pub fn proof_to_token(
    mint_url: UncheckedUrl,
    proofs: Proofs,
    memo: Option<String>,
    unit: Option<CurrencyUnit>,
) -> Result<Token, Error> {
    Ok(Token::new(mint_url, proofs, memo, unit)?)
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_token_from_text() {
        let text = " Here is some ecash: cashuAeyJ0b2tlbiI6W3sicHJvb2ZzIjpbeyJhbW91bnQiOjIsInNlY3JldCI6ImI2Zjk1ODIxYmZlNjUyYjYwZGQ2ZjYwMDU4N2UyZjNhOTk4MzVhMGMyNWI4MTQzODNlYWIwY2QzOWFiNDFjNzUiLCJDIjoiMDI1YWU4ZGEyOTY2Y2E5OGVmYjA5ZDcwOGMxM2FiZmEwZDkxNGUwYTk3OTE4MmFjMzQ4MDllMjYxODY5YTBhNDJlIiwicmVzZXJ2ZWQiOmZhbHNlLCJpZCI6IjAwOWExZjI5MzI1M2U0MWUifSx7ImFtb3VudCI6Miwic2VjcmV0IjoiZjU0Y2JjNmNhZWZmYTY5MTUyOTgyM2M1MjU1MDkwYjRhMDZjNGQ3ZDRjNzNhNDFlZTFkNDBlM2ExY2EzZGZhNyIsIkMiOiIwMjMyMTIzN2JlYjcyMWU3NGI1NzcwNWE5MjJjNjUxMGQwOTYyYzAzNzlhZDM0OTJhMDYwMDliZTAyNjA5ZjA3NTAiLCJyZXNlcnZlZCI6ZmFsc2UsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSJ9LHsiYW1vdW50IjoxLCJzZWNyZXQiOiJhNzdhM2NjODY4YWM4ZGU3YmNiOWMxMzJmZWI3YzEzMDY4Nzg3ODk5Yzk3YTk2NWE2ZThkZTFiMzliMmQ2NmQ3IiwiQyI6IjAzMTY0YTMxNWVhNjM0NGE5NWI2NzM1NzBkYzg0YmZlMTQ2NDhmMTQwM2EwMDJiZmJlMDhlNWFhMWE0NDQ0YWE0MCIsInJlc2VydmVkIjpmYWxzZSwiaWQiOiIwMDlhMWYyOTMyNTNlNDFlIn1dLCJtaW50IjoiaHR0cHM6Ly90ZXN0bnV0LmNhc2h1LnNwYWNlIn1dLCJ1bml0Ijoic2F0In0= fdfdfg
        sdfs";
        let token = token_from_text(text).unwrap();

        let token_str = "cashuAeyJ0b2tlbiI6W3sicHJvb2ZzIjpbeyJhbW91bnQiOjIsInNlY3JldCI6ImI2Zjk1ODIxYmZlNjUyYjYwZGQ2ZjYwMDU4N2UyZjNhOTk4MzVhMGMyNWI4MTQzODNlYWIwY2QzOWFiNDFjNzUiLCJDIjoiMDI1YWU4ZGEyOTY2Y2E5OGVmYjA5ZDcwOGMxM2FiZmEwZDkxNGUwYTk3OTE4MmFjMzQ4MDllMjYxODY5YTBhNDJlIiwicmVzZXJ2ZWQiOmZhbHNlLCJpZCI6IjAwOWExZjI5MzI1M2U0MWUifSx7ImFtb3VudCI6Miwic2VjcmV0IjoiZjU0Y2JjNmNhZWZmYTY5MTUyOTgyM2M1MjU1MDkwYjRhMDZjNGQ3ZDRjNzNhNDFlZTFkNDBlM2ExY2EzZGZhNyIsIkMiOiIwMjMyMTIzN2JlYjcyMWU3NGI1NzcwNWE5MjJjNjUxMGQwOTYyYzAzNzlhZDM0OTJhMDYwMDliZTAyNjA5ZjA3NTAiLCJyZXNlcnZlZCI6ZmFsc2UsImlkIjoiMDA5YTFmMjkzMjUzZTQxZSJ9LHsiYW1vdW50IjoxLCJzZWNyZXQiOiJhNzdhM2NjODY4YWM4ZGU3YmNiOWMxMzJmZWI3YzEzMDY4Nzg3ODk5Yzk3YTk2NWE2ZThkZTFiMzliMmQ2NmQ3IiwiQyI6IjAzMTY0YTMxNWVhNjM0NGE5NWI2NzM1NzBkYzg0YmZlMTQ2NDhmMTQwM2EwMDJiZmJlMDhlNWFhMWE0NDQ0YWE0MCIsInJlc2VydmVkIjpmYWxzZSwiaWQiOiIwMDlhMWYyOTMyNTNlNDFlIn1dLCJtaW50IjoiaHR0cHM6Ly90ZXN0bnV0LmNhc2h1LnNwYWNlIn1dLCJ1bml0Ijoic2F0In0=";

        assert_eq!(token, token_str)
    }
}
