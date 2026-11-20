//! Experimental binary QR transport. Run with:
//! `cargo run -p cashu --example nut16_binary_fountain`

use std::str::FromStr;

use cashu::nuts::{Token, TokenFountainDecoder};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let token = Token::from_str("cashuBpGF0gaJhaUgArSaMTR9YJmFwgaNhYQFhc3hAOWE2ZGJiODQ3YmQyMzJiYTc2ZGIwZGYxOTcyMTZiMjlkM2I4Y2MxNDU1M2NkMjc4MjdmYzFjYzk0MmZlZGI0ZWFjWCEDhhhUP_trhpXfStS6vN6So0qWvc2X3O4NfM-Y1HISZ5JhZGlUaGFuayB5b3VhbXVodHRwOi8vbG9jYWxob3N0OjMzMzhhdWNzYXQ=")?;
    let mut encoder = token.fountain_encoder(64)?;
    let mut decoder = TokenFountainDecoder::default();

    // Simulate missing all the initial source frames.
    for _ in 0..encoder.fragment_count() {
        encoder.next_part()?;
    }
    for _ in 0..1024 {
        let frame = encoder.next_part()?;
        // Applications render this Vec<u8> in QR byte mode and feed the
        // scanner's raw bytes to receive(), without UTF-8 conversion.
        decoder.receive(&frame)?;
        println!("Received information: {:.0}%", decoder.progress() * 100.0);
        if decoder.complete() {
            assert_eq!(decoder.token()?, Some(token));
            println!("Recovered token using repair frames only.");
            return Ok(());
        }
    }
    Err("transfer did not complete within the example's frame budget".into())
}
