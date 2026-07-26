//! NUT-16 animated QR code example
//!
//! Shows the full sender/receiver flow without any QR rendering library:
//! a token is split into UR fragments (one per QR frame) and reassembled
//! from the frames, tolerating dropped frames via the fountain code.
//!
//! Run with: `cargo run -p cashu --example nut16_animated_qr`

use std::str::FromStr;

use cashu::nuts::nut16::DEFAULT_MAX_FRAGMENT_LENGTH;
use cashu::nuts::{Token, TokenUrDecoder};

const TOKEN_STR: &str = "cashuBpGF0gaJhaUgArSaMTR9YJmFwgaNhYQFhc3hAOWE2ZGJiODQ3YmQyMzJiYTc2ZGIwZGYxOTcyMTZiMjlkM2I4Y2MxNDU1M2NkMjc4MjdmYzFjYzk0MmZlZGI0ZWFjWCEDhhhUP_trhpXfStS6vN6So0qWvc2X3O4NfM-Y1HISZ5JhZGlUaGFuayB5b3VhbXVodHRwOi8vbG9jYWxob3N0OjMzMzhhdWNzYXQ=";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let token = Token::from_str(TOKEN_STR)?;

    // --- Animated case: small fragment budget splits the token into an
    // unbounded stream of frames; the sender loops them until the receiver
    // signals completion ---
    let mut encoder = token.ur_encoder(100)?;
    println!(
        "Animated: token split into {} fragments\n",
        encoder.fragment_count()
    );

    let mut decoder = TokenUrDecoder::default();
    while !decoder.complete() {
        let part = encoder.next_part()?;
        println!("QR frame {}: {part}", encoder.current_index());
        // Receiver scans each frame, in any order
        decoder.receive(&part)?;
        println!(
            "  scanned {}/{} fragments",
            decoder.resolved_fragment_count().unwrap_or(0),
            decoder.fragment_count()
        );
    }

    let recovered = decoder.token()?.expect("decoder is complete");
    assert_eq!(recovered, token);
    println!("\nToken reassembled from scanned frames");

    // --- Dropped frames: the fountain code recovers from frames missed by
    // the scanner (here: every second frame is dropped) ---
    let mut encoder = token.ur_encoder(100)?;
    let mut decoder = TokenUrDecoder::default();
    while !decoder.complete() {
        let part = encoder.next_part()?;
        if encoder.current_index() % 2 == 0 {
            decoder.receive(&part)?;
        }
    }
    assert_eq!(decoder.token()?.expect("decoder is complete"), token);
    println!("Token recovered despite every second frame being dropped");

    // --- Static case: a fragment budget that fits the whole token yields a
    // single frame (no animation) ---
    let mut encoder = token.ur_encoder(DEFAULT_MAX_FRAGMENT_LENGTH * 10)?;
    if encoder.is_single_fragment() {
        println!("\nStatic: token fits one frame\n  {}", encoder.next_part()?);
    }

    Ok(())
}
