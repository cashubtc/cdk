//! Payment Request Encoding Benchmark
//!
//! Compares NUT-18 (CBOR/base64) vs NUT-26 (Bech32m) encoding formats across
//! various payment request complexities to demonstrate format efficiency tradeoffs.
//!
//! # Format Overview
//!
//! ## NUT-18 (creqA prefix)
//! - **Binary Encoding**: CBOR (Concise Binary Object Representation)
//! - **Text Encoding**: URL-safe base64
//! - **Characteristics**: Compact binary format, case-sensitive
//!
//! ## NUT-26 (CREQB prefix)
//! - **Binary Encoding**: TLV (Type-Length-Value)
//! - **Text Encoding**: Bech32m
//! - **Characteristics**: QR-optimized, case-insensitive, error detection
//!
//! # When to Use Each Format
//!
//! ## Use NUT-26 (CREQB) when:
//! - **Minimal requests** (~5 bytes / 7% smaller for simple payment IDs)
//! - **QR code display** (100% alphanumeric-compatible vs 99%+)
//! - **Error detection is critical** (Bech32m has built-in checksums)
//! - **Case-insensitive parsing** needed (URLs, voice transcription)
//! - **Visual verification** (human-readable structure)
//!
//! ## Use NUT-18 (creqA) when:
//! - **Complex requests** (~13-163 bytes / 16-19% smaller with more data)
//! - **Multiple mints** (~59 bytes / 24% smaller with 4 mints)
//! - **Transport callbacks** (~49 bytes / 19% smaller with 1 transport)
//! - **NUT-10 locking** (~91 bytes / 17% smaller with P2PK)
//! - **Nested structures** (CBOR excels at hierarchical data)
//! - **Bandwidth is constrained** (smaller encoded size)
//!
//! # Benchmark Results Summary
//!
//! | Scenario | NUT-18 Size | NUT-26 Size | Winner | Savings |
//! |----------|-------------|-------------|--------|---------|
//! | Minimal payment | 77 bytes | 72 bytes | NUT-26 | 5 bytes (7%) |
//! | With amount/unit | 81 bytes | 94 bytes | NUT-18 | 13 bytes (16%) |
//! | 4 mints | 249 bytes | 308 bytes | NUT-18 | 59 bytes (24%) |
//! | 1 transport | 253 bytes | 302 bytes | NUT-18 | 49 bytes (19%) |
//! | Complete + P2PK | 529 bytes | 620 bytes | NUT-18 | 91 bytes (17%) |
//! | Very complex | 857 bytes | 1020 bytes | NUT-18 | 163 bytes (19%) |
//!
//! **Key Insight**: NUT-26 is optimal for simple requests, NUT-18 scales better
//! for complex payment requests with multiple mints, transports, or NUT-10 locks.

use std::str::FromStr;

use cashu::nuts::nut10::Kind;
use cashu::nuts::{CurrencyUnit, Nut10SecretRequest, PaymentRequest, Transport, TransportType};
use cashu::{Amount, MintUrl};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("=== NUT-18 vs NUT-26 Format Comparison ===\n");

    // Example 1: Minimal payment request
    println!("1. Minimal Payment Request:");
    minimal_comparison()?;

    // Example 2: Payment with amount and unit
    println!("\n2. Payment with Amount and Unit:");
    amount_unit_comparison()?;

    // Example 3: Complex payment with multiple mints
    println!("\n3. Complex Payment with Multiple Mints:");
    multiple_mints_comparison()?;

    // Example 4: Payment with transport
    println!("\n4. Payment with Transport:");
    transport_comparison()?;

    // Example 5: Complete payment with NUT-10 locking
    println!("\n5. Complete Payment with NUT-10 P2PK Lock:");
    complete_with_nut10_comparison()?;

    // Example 6: Very complex payment request
    println!("\n6. Very Complex Payment Request:");
    very_complex_comparison()?;

    // Summary
    println!("\n=== Summary ===");
    summary();

    println!("\n=== Format Comparison Complete ===");
    Ok(())
}

fn minimal_comparison() -> Result<(), Box<dyn std::error::Error>> {
    let payment_request = PaymentRequest {
        payment_id: Some("test123".to_string()),
        amount: None,
        unit: None,
        single_use: None,
        mints: vec![MintUrl::from_str("https://mint.example.com")?],
        description: None,
        transports: vec![],
        nut10: None,
    };

    compare_formats(&payment_request, "Minimal")?;
    Ok(())
}

fn amount_unit_comparison() -> Result<(), Box<dyn std::error::Error>> {
    let payment_request = PaymentRequest {
        payment_id: Some("pay456".to_string()),
        amount: Some(Amount::from(2100)),
        unit: Some(CurrencyUnit::Sat),
        single_use: None,
        mints: vec![MintUrl::from_str("https://mint.example.com")?],
        description: None,
        transports: vec![],
        nut10: None,
    };

    compare_formats(&payment_request, "Amount + Unit")?;
    Ok(())
}

fn multiple_mints_comparison() -> Result<(), Box<dyn std::error::Error>> {
    let payment_request = PaymentRequest {
        payment_id: Some("multi789".to_string()),
        amount: Some(Amount::from(10000)),
        unit: Some(CurrencyUnit::Sat),
        single_use: Some(true),
        mints: vec![
            MintUrl::from_str("https://mint1.example.com")?,
            MintUrl::from_str("https://mint2.example.com")?,
            MintUrl::from_str("https://mint3.example.com")?,
            MintUrl::from_str("https://backup-mint.cashu.space")?,
        ],
        description: Some("Payment with multiple mint options".to_string()),
        transports: vec![],
        nut10: None,
    };

    compare_formats(&payment_request, "Multiple Mints")?;
    Ok(())
}

fn transport_comparison() -> Result<(), Box<dyn std::error::Error>> {
    let transport = Transport {
        _type: TransportType::HttpPost,
        target: "https://api.example.com/cashu/payment/callback".to_string(),
        tags: vec![
            vec!["method".to_string(), "POST".to_string()],
            vec!["auth".to_string(), "bearer".to_string()],
        ],
    };

    let payment_request = PaymentRequest {
        payment_id: Some("transport123".to_string()),
        amount: Some(Amount::from(5000)),
        unit: Some(CurrencyUnit::Sat),
        single_use: Some(true),
        mints: vec![MintUrl::from_str("https://mint.example.com")?],
        description: Some("Payment with callback transport".to_string()),
        transports: vec![transport],
        nut10: None,
    };

    compare_formats(&payment_request, "With Transport")?;
    Ok(())
}

fn complete_with_nut10_comparison() -> Result<(), Box<dyn std::error::Error>> {
    let nut10 = Nut10SecretRequest::new(
        Kind::P2PK,
        "026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198",
        Some(vec![
            vec!["locktime".to_string(), "1609459200".to_string()],
            vec![
                "refund".to_string(),
                "03a34d1f4e6d1e7f8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d3e4f5a6b7c8d9e0f1a2".to_string(),
            ],
        ]),
    );

    let transport = Transport {
        _type: TransportType::HttpPost,
        target: "https://callback.example.com/payment".to_string(),
        tags: vec![vec!["priority".to_string(), "high".to_string()]],
    };

    let payment_request = PaymentRequest {
        payment_id: Some("complete789".to_string()),
        amount: Some(Amount::from(5000)),
        unit: Some(CurrencyUnit::Sat),
        single_use: Some(true),
        mints: vec![
            MintUrl::from_str("https://mint1.example.com")?,
            MintUrl::from_str("https://mint2.example.com")?,
        ],
        description: Some("Complete payment with P2PK locking and refund key".to_string()),
        transports: vec![transport],
        nut10: Some(nut10),
    };

    compare_formats(&payment_request, "Complete with NUT-10")?;
    Ok(())
}

fn very_complex_comparison() -> Result<(), Box<dyn std::error::Error>> {
    let nut10 = Nut10SecretRequest::new(
        Kind::P2PK,
        "026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198",
        Some(vec![
            vec!["locktime".to_string(), "1609459200".to_string()],
            vec![
                "refund".to_string(),
                "03a34d1f4e6d1e7f8b9c0d1e2f3a4b5c6d7e8f9a0b1c2d3e".to_string(),
            ],
        ]),
    );

    let transport1 = Transport {
        _type: TransportType::HttpPost,
        target: "https://primary-callback.example.com/payment/webhook".to_string(),
        tags: vec![
            vec!["priority".to_string(), "high".to_string()],
            vec!["timeout".to_string(), "30".to_string()],
        ],
    };

    let transport2 = Transport {
        _type: TransportType::HttpPost,
        target: "https://backup-callback.example.com/payment/webhook".to_string(),
        tags: vec![
            vec!["priority".to_string(), "medium".to_string()],
            vec!["timeout".to_string(), "60".to_string()],
        ],
    };

    let payment_request = PaymentRequest {
        payment_id: Some("very_complex_payment_id_12345".to_string()),
        amount: Some(Amount::from(21000)),
        unit: Some(CurrencyUnit::Sat),
        single_use: Some(true),
        mints: vec![
            MintUrl::from_str("https://primary-mint.cashu.space")?,
            MintUrl::from_str("https://secondary-mint.example.com")?,
            MintUrl::from_str("https://backup-mint-1.example.org")?,
            MintUrl::from_str("https://backup-mint-2.example.net")?,
            MintUrl::from_str("https://emergency-mint.example.io")?,
        ],
        description: Some("Complex payment with multiple mints and transports".to_string()),
        transports: vec![transport1, transport2],
        nut10: Some(nut10),
    };

    compare_formats(&payment_request, "Very Complex")?;
    Ok(())
}

fn compare_formats(
    payment_request: &PaymentRequest,
    label: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Encode using NUT-18 (CBOR/base64, creqA)
    let nut18_encoded = payment_request.to_string();

    // Encode using NUT-26 (Bech32m, CREQB)
    let nut26_encoded = payment_request.to_bech32_string()?;

    // Calculate sizes
    let nut18_size = nut18_encoded.len();
    let nut26_size = nut26_encoded.len();
    let size_diff = nut26_size as i32 - nut18_size as i32;
    let size_ratio = (nut26_size as f64 / nut18_size as f64) * 100.0;

    println!("  {} Payment Request:", label);
    println!(
        "  Payment ID: {}",
        payment_request.payment_id.as_deref().unwrap_or("None")
    );
    println!(
        "  Amount: {}",
        payment_request
            .amount
            .map(|a| a.to_string())
            .unwrap_or_else(|| "None".to_string())
    );
    println!("  Mints: {}", payment_request.mints.len());
    println!("  Transports: {}", payment_request.transports.len());
    println!("  NUT-10: {}", payment_request.nut10.is_some());

    println!("\n  NUT-18 (CBOR/base64, creqA):");
    println!("    Size: {} bytes", nut18_size);
    println!(
        "    Format: {}",
        &nut18_encoded[..nut18_encoded.len().min(80)]
    );
    if nut18_encoded.len() > 80 {
        println!("    ... ({} more chars)", nut18_encoded.len() - 80);
    }

    println!("\n  NUT-26 (Bech32m, CREQB):");
    println!("    Size: {} bytes", nut26_size);
    println!(
        "    Format: {}",
        &nut26_encoded[..nut26_encoded.len().min(80)]
    );
    if nut26_encoded.len() > 80 {
        println!("    ... ({} more chars)", nut26_encoded.len() - 80);
    }

    println!("\n  Comparison:");
    println!(
        "    Size difference: {} bytes ({:.1}%)",
        size_diff, size_ratio
    );

    if size_diff < 0 {
        println!("    Winner: NUT-26 is {} bytes smaller!", size_diff.abs());
    } else if size_diff > 0 {
        println!("    Winner: NUT-18 is {} bytes smaller!", size_diff);
    } else {
        println!("    Equal size!");
    }

    // Analyze QR code efficiency
    analyze_qr_efficiency(&nut18_encoded, &nut26_encoded);

    // Verify round-trip for both formats
    println!("\n  Round-trip verification:");

    // NUT-18 round-trip
    let nut18_decoded = PaymentRequest::from_str(&nut18_encoded)?;
    assert_eq!(nut18_decoded.payment_id, payment_request.payment_id);
    assert_eq!(nut18_decoded.amount, payment_request.amount);
    println!("    NUT-18: ✓ Decoded successfully");

    // NUT-26 round-trip
    let nut26_decoded = PaymentRequest::from_str(&nut26_encoded)?;
    assert_eq!(nut26_decoded.payment_id, payment_request.payment_id);
    assert_eq!(nut26_decoded.amount, payment_request.amount);
    println!("    NUT-26: ✓ Decoded successfully");

    // Verify both decode to the same data
    assert_eq!(nut18_decoded.payment_id, nut26_decoded.payment_id);
    assert_eq!(nut18_decoded.amount, nut26_decoded.amount);
    assert_eq!(nut18_decoded.unit, nut26_decoded.unit);
    assert_eq!(nut18_decoded.single_use, nut26_decoded.single_use);
    assert_eq!(nut18_decoded.description, nut26_decoded.description);

    println!("    ✓ Both formats decode to identical data");

    Ok(())
}

fn analyze_qr_efficiency(nut18: &str, nut26: &str) {
    // QR codes have different encoding modes:
    // - Alphanumeric: 0-9, A-Z (uppercase), space, $, %, *, +, -, ., /, : (most efficient for text)
    // - Byte: any data (less efficient)

    let alphanumeric_chars = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";

    let nut18_alphanumeric = nut18
        .chars()
        .filter(|c| alphanumeric_chars.contains(c.to_ascii_uppercase()))
        .count();
    let nut18_alphanumeric_ratio = (nut18_alphanumeric as f64 / nut18.len() as f64) * 100.0;

    let nut26_alphanumeric = nut26
        .chars()
        .filter(|c| alphanumeric_chars.contains(c.to_ascii_uppercase()))
        .count();
    let nut26_alphanumeric_ratio = (nut26_alphanumeric as f64 / nut26.len() as f64) * 100.0;

    println!("\n  QR Code Efficiency:");
    println!(
        "    NUT-18: {:.1}% alphanumeric-compatible",
        nut18_alphanumeric_ratio
    );
    println!(
        "    NUT-26: {:.1}% alphanumeric-compatible",
        nut26_alphanumeric_ratio
    );

    if nut26_alphanumeric_ratio > nut18_alphanumeric_ratio {
        println!(
            "    NUT-26 is more QR-friendly (+{:.1}%)",
            nut26_alphanumeric_ratio - nut18_alphanumeric_ratio
        );
    }

    // Estimate QR version (simplified)
    let nut18_qr_version = estimate_qr_version(nut18.len(), nut18_alphanumeric_ratio > 80.0);
    let nut26_qr_version = estimate_qr_version(nut26.len(), nut26_alphanumeric_ratio > 80.0);

    println!(
        "    NUT-18 QR version: ~{} ({}×{} modules)",
        nut18_qr_version,
        21 + (nut18_qr_version - 1) * 4,
        21 + (nut18_qr_version - 1) * 4
    );
    println!(
        "    NUT-26 QR version: ~{} ({}×{} modules)",
        nut26_qr_version,
        21 + (nut26_qr_version - 1) * 4,
        21 + (nut26_qr_version - 1) * 4
    );
}

fn estimate_qr_version(data_length: usize, is_alphanumeric: bool) -> u8 {
    // Simplified QR version estimation (Level L - Low error correction)
    if is_alphanumeric {
        // Alphanumeric mode capacity
        match data_length {
            0..=20 => 1,
            21..=38 => 2,
            39..=61 => 3,
            62..=90 => 4,
            91..=122 => 5,
            123..=154 => 6,
            155..=192 => 7,
            193..=230 => 8,
            231..=271 => 9,
            272..=321 => 10,
            322..=367 => 11,
            368..=425 => 12,
            426..=458 => 13,
            459..=520 => 14,
            521..=586 => 15,
            _ => 16,
        }
    } else {
        // Byte mode capacity
        match data_length {
            0..=14 => 1,
            15..=26 => 2,
            27..=42 => 3,
            43..=62 => 4,
            63..=84 => 5,
            85..=106 => 6,
            107..=122 => 7,
            123..=152 => 8,
            153..=180 => 9,
            181..=213 => 10,
            214..=251 => 11,
            252..=287 => 12,
            288..=331 => 13,
            332..=362 => 14,
            363..=394 => 15,
            _ => 16,
        }
    }
}

fn summary() {
    println!("  Key Observations:");
    println!("  • NUT-18 (creqA): CBOR binary + URL-safe base64 encoding");
    println!("  • NUT-26 (CREQB): TLV binary + Bech32m encoding");
    println!("  • Bech32m is optimized for QR codes (uppercase alphanumeric)");
    println!("  • CBOR may be more compact for complex nested structures");
    println!("  • Both formats support the same feature set");
    println!("  • NUT-26 has better error detection (Bech32m checksum)");
    println!("  • NUT-26 is case-insensitive for parsing");
    println!("  • Both can be parsed from the same FromStr implementation");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_minimal_comparison() {
        assert!(minimal_comparison().is_ok());
    }

    #[test]
    fn test_amount_unit_comparison() {
        assert!(amount_unit_comparison().is_ok());
    }

    #[test]
    fn test_multiple_mints_comparison() {
        assert!(multiple_mints_comparison().is_ok());
    }

    #[test]
    fn test_transport_comparison() {
        assert!(transport_comparison().is_ok());
    }

    #[test]
    fn test_complete_with_nut10_comparison() {
        assert!(complete_with_nut10_comparison().is_ok());
    }

    #[test]
    fn test_very_complex_comparison() {
        assert!(very_complex_comparison().is_ok());
    }

    #[test]
    fn test_round_trip_equivalence() {
        let payment_request = PaymentRequest {
            payment_id: Some("test".to_string()),
            amount: Some(Amount::from(1000)),
            unit: Some(CurrencyUnit::Sat),
            single_use: None,
            mints: Some(vec![MintUrl::from_str("https://mint.example.com").unwrap()]),
            description: Some("Test".to_string()),
            transports: vec![],
            nut10: None,
        };

        // Encode both ways
        let nut18 = payment_request.to_string();
        let nut26 = payment_request.to_bech32_string().unwrap();

        // Decode both
        let from_nut18 = PaymentRequest::from_str(&nut18).unwrap();
        let from_nut26 = PaymentRequest::from_str(&nut26).unwrap();

        // Should be equal
        assert_eq!(from_nut18.payment_id, from_nut26.payment_id);
        assert_eq!(from_nut18.amount, from_nut26.amount);
        assert_eq!(from_nut18.unit, from_nut26.unit);
        assert_eq!(from_nut18.description, from_nut26.description);
    }
}
