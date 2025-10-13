//! NUT-26: Bech32m encoding for payment requests  
//!
//! This module provides bech32m encoding and decoding functionality for Cashu payment requests,
//! implementing the CREQ-B format using TLV (Tag-Length-Value) encoding as specified in NUT-26.

use std::str::FromStr;

use bitcoin::bech32::{self, Bech32, Bech32m, Hrp};

use super::Error;
use crate::mint_url::MintUrl;
use crate::nuts::nut10::Kind;
use crate::nuts::nut18::{Nut10SecretRequest, PaymentRequest, Transport, TransportType};
use crate::nuts::CurrencyUnit;
use crate::Amount;

/// Human-readable part for CREQ-B bech32m encoding
pub const CREQ_B_HRP: &str = "creqb";

/// Unit representation for TLV encoding
#[derive(Debug, Clone, PartialEq, Eq)]
enum TlvUnit {
    Sat,
    Custom(String),
}

impl From<CurrencyUnit> for TlvUnit {
    fn from(unit: CurrencyUnit) -> Self {
        match unit {
            CurrencyUnit::Sat => TlvUnit::Sat,
            CurrencyUnit::Msat => TlvUnit::Custom("msat".to_string()),
            CurrencyUnit::Usd => TlvUnit::Custom("usd".to_string()),
            CurrencyUnit::Eur => TlvUnit::Custom("eur".to_string()),
            CurrencyUnit::Custom(c) => TlvUnit::Custom(c),
            CurrencyUnit::Auth => TlvUnit::Custom("auth".to_string()),
        }
    }
}

impl From<TlvUnit> for CurrencyUnit {
    fn from(unit: TlvUnit) -> Self {
        match unit {
            TlvUnit::Sat => CurrencyUnit::Sat,
            TlvUnit::Custom(s) => match s.as_str() {
                "msat" => CurrencyUnit::Msat,
                "usd" => CurrencyUnit::Usd,
                "eur" => CurrencyUnit::Eur,
                _ => CurrencyUnit::Sat, // default fallback
            },
        }
    }
}

/// TLV reader helper for parsing binary TLV data
struct TlvReader<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> TlvReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    fn read_tlv(&mut self) -> Result<Option<(u8, Vec<u8>)>, &'static str> {
        if self.position + 3 > self.data.len() {
            return Ok(None);
        }

        let tag = self.data[self.position];
        let len = u16::from_be_bytes([self.data[self.position + 1], self.data[self.position + 2]])
            as usize;
        self.position += 3;

        if self.position + len > self.data.len() {
            return Err("TLV value extends beyond buffer");
        }

        let value = self.data[self.position..self.position + len].to_vec();
        self.position += len;

        Ok(Some((tag, value)))
    }
}

/// TLV writer helper for creating binary TLV data
struct TlvWriter {
    data: Vec<u8>,
}

impl TlvWriter {
    fn new() -> Self {
        Self { data: Vec::new() }
    }

    fn write_tlv(&mut self, tag: u8, value: &[u8]) {
        self.data.push(tag);
        let len = value.len() as u16;
        self.data.extend_from_slice(&len.to_be_bytes());
        self.data.extend_from_slice(value);
    }

    fn into_bytes(self) -> Vec<u8> {
        self.data
    }
}

/// CREQ-B encoding and decoding implementation
impl PaymentRequest {
    /// Encodes a payment request to CREQB1 bech32m format.
    ///
    /// This function serializes a payment request according to the NUT-26 specification
    /// and encodes it using the bech32m encoding scheme with the "creqb" human-readable
    /// part (HRP). The output is always uppercase for optimal QR code compatibility.
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing:
    /// * `Ok(String)` - The bech32m-encoded payment request string in uppercase
    /// * `Err(Error)` - If serialization or encoding fails
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// * The payment request cannot be serialized to TLV format
    /// * The bech32m encoding process fails
    ///
    /// # Specification
    ///
    /// See [NUT-26](https://github.com/cashubtc/nuts/blob/main/26.md) for the complete
    /// specification of the CREQB1 payment request format.
    ///
    /// # Examples
    ///
    /// ```
    /// use cashu::nuts::nut18::PaymentRequest;
    /// use cashu::{Amount, MintUrl};
    /// use std::str::FromStr;
    ///
    /// let payment_request = PaymentRequest {
    ///     payment_id: Some("test123".to_string()),
    ///     amount: Some(Amount::from(1000)),
    ///     unit: Some(cashu::nuts::CurrencyUnit::Sat),
    ///     single_use: None,
    ///     mints: Some(vec![MintUrl::from_str("https://mint.example.com")?]),
    ///     description: None,
    ///     transports: vec![],
    ///     nut10: None,
    /// };
    ///
    /// let encoded = payment_request.to_bech32_string()?;
    /// assert!(encoded.starts_with("CREQB1"));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn to_bech32_string(&self) -> Result<String, Error> {
        let tlv_bytes = self.encode_tlv()?;
        let hrp = Hrp::parse(CREQ_B_HRP).map_err(|_| Error::InvalidPrefix)?;

        // Always emit uppercase for QR compatibility
        let encoded =
            bech32::encode_upper::<Bech32m>(hrp, &tlv_bytes).map_err(|_| Error::InvalidPrefix)?;
        Ok(encoded)
    }

    /// Decodes a payment request from CREQB1 bech32m format.
    ///
    /// This function takes a bech32m-encoded payment request string (case-insensitive)
    /// with the "creqb" human-readable part and deserializes it back into a
    /// payment request according to the NUT-26 specification.
    ///
    /// # Arguments
    ///
    /// * `s` - The bech32m-encoded payment request string (case-insensitive)
    ///
    /// # Returns
    ///
    /// Returns a `Result` containing:
    /// * `Ok(PaymentRequest)` - The decoded payment request
    /// * `Err(Error)` - If decoding or deserialization fails
    ///
    /// # Errors
    ///
    /// This function will return an error if:
    /// * The input string is not valid bech32m encoding
    /// * The human-readable part is not "creqb" (case-insensitive)
    /// * The decoded data cannot be deserialized into a valid payment request
    /// * The TLV structure is malformed
    ///
    /// # Specification
    ///
    /// See [NUT-26](https://github.com/cashubtc/nuts/blob/main/26.md) for the complete
    /// specification of the CREQB1 payment request format.
    ///
    /// # Examples
    ///
    /// ```
    /// use cashu::nuts::nut18::PaymentRequest;
    ///
    /// let encoded = "CREQB1QYQQWAR9WD6RZV3NQ5QPS6R5W3C8XW309AKKJMN59EJHSCTDWPKX2TNRDAKS4U8XXF";
    /// let payment_request = PaymentRequest::from_bech32_string(encoded)?;
    /// assert_eq!(payment_request.payment_id, Some("test123".to_string()));
    /// # Ok::<(), cashu::nuts::nut26::Error>(())
    /// ```
    pub fn from_bech32_string(s: &str) -> Result<Self, Error> {
        let (hrp, data) = bech32::decode(s).map_err(|_| Error::InvalidPrefix)?;
        if !hrp.as_str().eq_ignore_ascii_case(CREQ_B_HRP) {
            return Err(Error::InvalidPrefix);
        }

        Self::from_bech32_bytes(&data)
    }

    /// Decode from TLV bytes
    fn from_bech32_bytes(bytes: &[u8]) -> Result<PaymentRequest, Error> {
        let mut reader = TlvReader::new(bytes);

        let mut id: Option<String> = None;
        let mut amount: Option<Amount> = None;
        let mut unit: Option<CurrencyUnit> = None;
        let mut single_use: Option<bool> = None;
        let mut mints: Vec<MintUrl> = Vec::new();
        let mut description: Option<String> = None;
        let mut transports: Vec<Transport> = Vec::new();
        let mut nut10: Option<Nut10SecretRequest> = None;

        while let Some((tag, value)) = reader.read_tlv().map_err(|_| Error::InvalidPrefix)? {
            match tag {
                0x01 => {
                    // id: string
                    id = Some(String::from_utf8(value).map_err(|_| Error::InvalidPrefix)?);
                }
                0x02 => {
                    // amount: u64
                    if value.len() != 8 {
                        return Err(Error::InvalidPrefix);
                    }
                    let amount_val = u64::from_be_bytes([
                        value[0], value[1], value[2], value[3], value[4], value[5], value[6],
                        value[7],
                    ]);
                    amount = Some(Amount::from(amount_val));
                }
                0x03 => {
                    // unit: u8 or string
                    if value.len() == 1 && value[0] == 0 {
                        unit = Some(CurrencyUnit::Sat);
                    } else {
                        let unit_str =
                            String::from_utf8(value).map_err(|_| Error::InvalidPrefix)?;
                        unit = Some(TlvUnit::Custom(unit_str).into());
                    }
                }
                0x04 => {
                    // single_use: u8 (0 or 1)
                    if !value.is_empty() {
                        single_use = Some(value[0] != 0);
                    }
                }
                0x05 => {
                    // mint: string (repeatable)
                    let mint_str = String::from_utf8(value).map_err(|_| Error::InvalidPrefix)?;
                    let mint_url =
                        MintUrl::from_str(&mint_str).map_err(|_| Error::InvalidPrefix)?;
                    mints.push(mint_url);
                }
                0x06 => {
                    // description: string
                    description = Some(String::from_utf8(value).map_err(|_| Error::InvalidPrefix)?);
                }
                0x07 => {
                    // transport: sub-TLV (repeatable)
                    let transport = Self::decode_transport(&value)?;
                    transports.push(transport);
                }
                0x08 => {
                    // nut10: sub-TLV
                    nut10 = Some(Self::decode_nut10(&value)?);
                }
                _ => {
                    // Unknown tags are ignored
                }
            }
        }

        // Set default unit if amount is present but unit is missing
        if amount.is_some() && unit.is_none() {
            unit = Some(CurrencyUnit::Sat);
        }

        Ok(PaymentRequest {
            payment_id: id,
            amount,
            unit,
            single_use,
            mints: if mints.is_empty() { None } else { Some(mints) },
            description,
            transports,
            nut10,
        })
    }

    /// Encode to TLV bytes
    fn encode_tlv(&self) -> Result<Vec<u8>, Error> {
        let mut writer = TlvWriter::new();

        // 0x01 id: string
        if let Some(ref id) = self.payment_id {
            writer.write_tlv(0x01, id.as_bytes());
        }

        // 0x02 amount: u64
        if let Some(amount) = self.amount {
            let amount_bytes = (amount.to_u64()).to_be_bytes();
            writer.write_tlv(0x02, &amount_bytes);
        }

        // 0x03 unit: u8 or string
        if let Some(ref unit) = self.unit {
            let tlv_unit = TlvUnit::from(unit.clone());
            match tlv_unit {
                TlvUnit::Sat => writer.write_tlv(0x03, &[0]),
                TlvUnit::Custom(s) => writer.write_tlv(0x03, s.as_bytes()),
            }
        }

        // 0x04 single_use: u8 (0 or 1)
        if let Some(single_use) = self.single_use {
            writer.write_tlv(0x04, &[if single_use { 1 } else { 0 }]);
        }

        // 0x05 mint: string (repeatable)
        if let Some(ref mints) = self.mints {
            for mint in mints {
                writer.write_tlv(0x05, mint.to_string().as_bytes());
            }
        }

        // 0x06 description: string
        if let Some(ref description) = self.description {
            writer.write_tlv(0x06, description.as_bytes());
        }

        // 0x07 transport: sub-TLV (repeatable, order = priority)
        for transport in &self.transports {
            let transport_bytes = Self::encode_transport(transport)?;
            writer.write_tlv(0x07, &transport_bytes);
        }

        // 0x08 nut10: sub-TLV
        if let Some(ref nut10) = self.nut10 {
            let nut10_bytes = Self::encode_nut10(nut10)?;
            writer.write_tlv(0x08, &nut10_bytes);
        }

        Ok(writer.into_bytes())
    }

    /// Decode transport sub-TLV
    fn decode_transport(bytes: &[u8]) -> Result<Transport, Error> {
        let mut reader = TlvReader::new(bytes);

        let mut kind: Option<u8> = None;
        let mut pubkey: Option<Vec<u8>> = None;
        let mut nips: Vec<u16> = Vec::new();
        let mut relays: Vec<String> = Vec::new();
        let mut tags: Vec<(String, Vec<String>)> = Vec::new();
        let mut http_target: Option<String> = None;

        while let Some((tag, value)) = reader.read_tlv().map_err(|_| Error::InvalidPrefix)? {
            match tag {
                0x01 => {
                    // kind: u8
                    if value.len() != 1 {
                        return Err(Error::InvalidPrefix);
                    }
                    kind = Some(value[0]);
                }
                0x02 => {
                    // target: bytes (interpretation depends on kind)
                    match kind {
                        Some(1) => {
                            // nostr: 32-byte x-only pubkey
                            if value.len() != 32 {
                                return Err(Error::InvalidPrefix);
                            }
                            pubkey = Some(value);
                        }
                        Some(2) => {
                            // http_post: UTF-8 URL string
                            http_target =
                                Some(String::from_utf8(value).map_err(|_| Error::InvalidPrefix)?);
                        }
                        Some(0) | None => {
                            // in_band: empty
                        }
                        _ => return Err(Error::InvalidPrefix),
                    }
                }
                0x03 => {
                    // nips: sequence of u16 values (only for nostr)
                    if value.len() % 2 != 0 {
                        return Err(Error::InvalidPrefix);
                    }
                    for chunk in value.chunks_exact(2) {
                        nips.push(u16::from_be_bytes([chunk[0], chunk[1]]));
                    }
                }
                0x04 => {
                    // relay: string (repeatable; only for nostr)
                    let relay = String::from_utf8(value).map_err(|_| Error::InvalidPrefix)?;
                    relays.push(relay);
                }
                0x05 => {
                    // tag_tuple: generic tuple (repeatable)
                    let tag_tuple = Self::decode_tag_tuple(&value)?;
                    tags.push(tag_tuple);
                }
                _ => {
                    // Unknown sub-TLV tags are ignored
                }
            }
        }

        let transport_type = match kind.unwrap_or(0) {
            1 => TransportType::Nostr,
            2 => TransportType::HttpPost,
            0 => TransportType::Nostr, // Default for in_band
            _ => return Err(Error::InvalidPrefix),
        };

        // Build the target string based on transport type
        let target = match transport_type {
            TransportType::Nostr => {
                // Build nprofile if we have relays, otherwise just npub
                if let Some(pk) = pubkey {
                    if relays.is_empty() {
                        // Just encode as npub
                        Self::encode_npub(&pk)?
                    } else {
                        // Encode as nprofile with relays
                        Self::encode_nprofile(&pk, &relays)?
                    }
                } else {
                    return Err(Error::InvalidPrefix);
                }
            }
            TransportType::HttpPost => http_target.ok_or(Error::InvalidPrefix)?,
        };

        // Convert nips to tag format for compatibility with existing Transport
        let mut final_tags = tags;
        for nip in nips {
            final_tags.push(("n".to_string(), vec![nip.to_string()]));
        }

        // Add relays as tags for compatibility
        for relay in relays {
            final_tags.push(("relay".to_string(), vec![relay]));
        }

        Ok(Transport {
            _type: transport_type,
            target,
            tags: if final_tags.is_empty() {
                None
            } else {
                Some(
                    final_tags
                        .into_iter()
                        .map(|(k, v)| {
                            let mut result = vec![k];
                            result.extend(v);
                            result
                        })
                        .collect(),
                )
            },
        })
    }

    /// Encode transport to sub-TLV
    fn encode_transport(transport: &Transport) -> Result<Vec<u8>, Error> {
        let mut writer = TlvWriter::new();

        // 0x01 kind: u8
        let kind = match transport._type {
            TransportType::Nostr => 1u8,
            TransportType::HttpPost => 2u8,
        };
        writer.write_tlv(0x01, &[kind]);

        // 0x02 target: bytes
        match transport._type {
            TransportType::Nostr => {
                // For nostr, extract the pubkey from npub or nprofile
                let (pubkey, relays) = if transport.target.starts_with("npub") {
                    // Decode npub bech32 to get 32-byte pubkey
                    let pubkey = Self::decode_npub(&transport.target)?;
                    (pubkey, Vec::new())
                } else if transport.target.starts_with("nprofile") {
                    // Decode nprofile to extract pubkey and relays
                    Self::decode_nprofile(&transport.target)?
                } else {
                    return Err(Error::InvalidPrefix);
                };

                // Write the 32-byte pubkey
                writer.write_tlv(0x02, &pubkey);

                // Extract NIPs and other tags from the tags field
                if let Some(ref tags) = transport.tags {
                    let mut nips = Vec::new();
                    let mut relays_from_tags = Vec::new();
                    let mut other_tags = Vec::new();

                    for tag in tags {
                        if tag.is_empty() {
                            continue;
                        }
                        if tag[0] == "n" && tag.len() >= 2 {
                            if let Ok(nip) = tag[1].parse::<u16>() {
                                nips.push(nip);
                            }
                        } else if tag[0] == "relay" && tag.len() >= 2 {
                            relays_from_tags.push(tag[1].clone());
                        } else {
                            other_tags.push(tag.clone());
                        }
                    }

                    // 0x03 nips: sequence of u16 values (only for nostr)
                    if !nips.is_empty() {
                        let mut nip_bytes = Vec::new();
                        for nip in nips {
                            nip_bytes.extend_from_slice(&nip.to_be_bytes());
                        }
                        writer.write_tlv(0x03, &nip_bytes);
                    }

                    // 0x04 relay: string (repeatable)
                    // Use relays from nprofile first, then from tags
                    for relay in relays.iter().chain(relays_from_tags.iter()) {
                        writer.write_tlv(0x04, relay.as_bytes());
                    }

                    // 0x05 tag_tuple: generic tuple (repeatable)
                    for tag in other_tags {
                        let tag_bytes = Self::encode_tag_tuple(&tag)?;
                        writer.write_tlv(0x05, &tag_bytes);
                    }
                } else if !relays.is_empty() {
                    // Even if there are no tags, write relays from nprofile
                    for relay in relays {
                        writer.write_tlv(0x04, relay.as_bytes());
                    }
                }
            }
            TransportType::HttpPost => {
                writer.write_tlv(0x02, transport.target.as_bytes());

                // Extract tags if any
                if let Some(ref tags) = transport.tags {
                    for tag in tags {
                        if !tag.is_empty() {
                            let tag_bytes = Self::encode_tag_tuple(tag)?;
                            writer.write_tlv(0x05, &tag_bytes);
                        }
                    }
                }
            }
        }

        Ok(writer.into_bytes())
    }

    /// Decode NUT-10 sub-TLV
    fn decode_nut10(bytes: &[u8]) -> Result<Nut10SecretRequest, Error> {
        let mut reader = TlvReader::new(bytes);

        let mut kind: Option<u16> = None;
        let mut data: Option<Vec<u8>> = None;
        let mut tags: Vec<(String, Vec<String>)> = Vec::new();

        while let Some((tag, value)) = reader.read_tlv().map_err(|_| Error::InvalidPrefix)? {
            match tag {
                0x01 => {
                    // kind: u16
                    if value.len() != 2 {
                        return Err(Error::InvalidPrefix);
                    }
                    kind = Some(u16::from_be_bytes([value[0], value[1]]));
                }
                0x02 => {
                    // data: bytes
                    data = Some(value);
                }
                0x03 | 0x05 => {
                    // tag_tuple: generic tuple (repeatable)
                    let tag_tuple = Self::decode_tag_tuple(&value)?;
                    tags.push(tag_tuple);
                }
                _ => {
                    // Unknown tags are ignored
                }
            }
        }

        let kind_val = kind.ok_or(Error::InvalidPrefix)?;
        let data_val = data.unwrap_or_default();

        // Convert kind u16 to Kind enum
        let data_str = String::from_utf8(data_val).map_err(|_| Error::InvalidUtf8)?;

        // Map kind value to Kind enum, error on unknown kinds
        let kind_enum = match kind_val {
            0 => Kind::P2PK,
            1 => Kind::HTLC,
            _ => return Err(Error::UnknownKind(kind_val)),
        };

        Ok(Nut10SecretRequest::new(
            kind_enum,
            &data_str,
            if tags.is_empty() {
                None
            } else {
                Some(
                    tags.into_iter()
                        .map(|(k, v)| {
                            let mut result = vec![k];
                            result.extend(v);
                            result
                        })
                        .collect::<Vec<_>>(),
                )
            },
        ))
    }

    /// Encode NUT-10 to sub-TLV
    fn encode_nut10(nut10: &Nut10SecretRequest) -> Result<Vec<u8>, Error> {
        let mut writer = TlvWriter::new();

        // 0x01 kind: u16
        let kind_val = match nut10.kind {
            Kind::P2PK => 0u16,
            Kind::HTLC => 1u16,
        };
        writer.write_tlv(0x01, &kind_val.to_be_bytes());

        // 0x02 data: bytes
        writer.write_tlv(0x02, nut10.data.as_bytes());

        // 0x03 tag_tuple: generic tuple (repeatable)
        if let Some(ref tags) = nut10.tags {
            for tag in tags {
                let tag_bytes = Self::encode_tag_tuple(tag)?;
                writer.write_tlv(0x03, &tag_bytes);
            }
        }

        Ok(writer.into_bytes())
    }

    /// Decode tag tuple
    fn decode_tag_tuple(bytes: &[u8]) -> Result<(String, Vec<String>), Error> {
        if bytes.is_empty() {
            return Err(Error::InvalidPrefix);
        }

        let key_len = bytes[0] as usize;
        if bytes.len() < 1 + key_len {
            return Err(Error::InvalidPrefix);
        }

        let key =
            String::from_utf8(bytes[1..1 + key_len].to_vec()).map_err(|_| Error::InvalidPrefix)?;

        let mut values = Vec::new();
        let mut pos = 1 + key_len;

        while pos < bytes.len() {
            let val_len = bytes[pos] as usize;
            pos += 1;

            if pos + val_len > bytes.len() {
                return Err(Error::InvalidPrefix);
            }

            let value = String::from_utf8(bytes[pos..pos + val_len].to_vec())
                .map_err(|_| Error::InvalidPrefix)?;
            values.push(value);
            pos += val_len;
        }

        Ok((key, values))
    }

    /// Encode tag tuple
    fn encode_tag_tuple(tag: &[String]) -> Result<Vec<u8>, Error> {
        if tag.is_empty() {
            return Err(Error::InvalidPrefix);
        }

        let mut bytes = Vec::new();

        // Key length + key
        let key = &tag[0];
        bytes.push(key.len() as u8);
        bytes.extend_from_slice(key.as_bytes());

        // Values
        for value in &tag[1..] {
            bytes.push(value.len() as u8);
            bytes.extend_from_slice(value.as_bytes());
        }

        Ok(bytes)
    }

    /// Decode npub bech32 string to 32-byte pubkey
    fn decode_npub(npub: &str) -> Result<Vec<u8>, Error> {
        let (hrp, data) = bech32::decode(npub).map_err(|_| Error::InvalidPrefix)?;
        if hrp.as_str() != "npub" {
            return Err(Error::InvalidPrefix);
        }
        if data.len() != 32 {
            return Err(Error::InvalidPrefix);
        }
        Ok(data)
    }

    /// Encode 32-byte pubkey to npub bech32 string
    fn encode_npub(pubkey: &[u8]) -> Result<String, Error> {
        if pubkey.len() != 32 {
            return Err(Error::InvalidPrefix);
        }
        let hrp = Hrp::parse("npub").map_err(|_| Error::InvalidPrefix)?;
        bech32::encode::<Bech32>(hrp, pubkey).map_err(|_| Error::InvalidPrefix)
    }

    /// Decode nprofile bech32 string to (pubkey, relays)
    /// NIP-19 nprofile TLV format:
    /// - Type 0: 32-byte pubkey (required, only one)
    /// - Type 1: relay URL string (optional, repeatable)
    fn decode_nprofile(nprofile: &str) -> Result<(Vec<u8>, Vec<String>), Error> {
        let (hrp, data) = bech32::decode(nprofile).map_err(|_| Error::InvalidPrefix)?;
        if hrp.as_str() != "nprofile" {
            return Err(Error::InvalidPrefix);
        }

        // Parse NIP-19 TLV format (Type: 1 byte, Length: 1 byte, Value: variable)
        let mut pos = 0;
        let mut pubkey: Option<Vec<u8>> = None;
        let mut relays: Vec<String> = Vec::new();

        while pos < data.len() {
            if pos + 2 > data.len() {
                break; // Not enough data for type + length
            }

            let tag = data[pos];
            let len = data[pos + 1] as usize;
            pos += 2;

            if pos + len > data.len() {
                return Err(Error::InvalidPrefix);
            }

            let value = &data[pos..pos + len];
            pos += len;

            match tag {
                0 => {
                    // pubkey: 32 bytes
                    if value.len() != 32 {
                        return Err(Error::InvalidPrefix);
                    }
                    pubkey = Some(value.to_vec());
                }
                1 => {
                    // relay: UTF-8 string
                    let relay =
                        String::from_utf8(value.to_vec()).map_err(|_| Error::InvalidPrefix)?;
                    relays.push(relay);
                }
                _ => {
                    // Unknown TLV types are ignored per NIP-19
                }
            }
        }

        let pubkey = pubkey.ok_or(Error::InvalidPrefix)?;
        Ok((pubkey, relays))
    }

    /// Encode pubkey and relays to nprofile bech32 string
    /// NIP-19 nprofile TLV format (Type: 1 byte, Length: 1 byte, Value: variable)
    fn encode_nprofile(pubkey: &[u8], relays: &[String]) -> Result<String, Error> {
        if pubkey.len() != 32 {
            return Err(Error::InvalidPrefix);
        }

        let mut tlv_bytes = Vec::new();

        // Type 0: pubkey (32 bytes) - Length must fit in 1 byte
        tlv_bytes.push(0); // type
        tlv_bytes.push(32); // length
        tlv_bytes.extend_from_slice(pubkey);

        // Type 1: relays (repeatable) - Length must fit in 1 byte
        for relay in relays {
            if relay.len() > 255 {
                return Err(Error::InvalidPrefix); // Relay URL too long for NIP-19
            }
            tlv_bytes.push(1); // type
            tlv_bytes.push(relay.len() as u8); // length
            tlv_bytes.extend_from_slice(relay.as_bytes());
        }

        let hrp = Hrp::parse("nprofile").map_err(|_| Error::InvalidPrefix)?;
        bech32::encode::<Bech32>(hrp, &tlv_bytes).map_err(|_| Error::InvalidPrefix)
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::nuts::nut10::Kind;
    use crate::util::hex;
    use crate::TransportType;

    #[test]
    fn test_bech32_basic_round_trip() {
        let transport = Transport {
            _type: TransportType::HttpPost,
            target: "https://api.example.com/payment".to_string(),
            tags: None,
        };

        let payment_request = PaymentRequest {
            payment_id: Some("test123".to_string()),
            amount: Some(Amount::from(100)),
            unit: Some(CurrencyUnit::Sat),
            single_use: Some(true),
            mints: Some(vec![MintUrl::from_str("https://mint.example.com").unwrap()]),
            description: Some("Test payment".to_string()),
            transports: vec![transport],
            nut10: None,
        };

        let encoded = payment_request
            .to_bech32_string()
            .expect("encoding should work");

        // Verify it starts with CREQB1
        assert!(encoded.starts_with("CREQB1"));

        // Round-trip test
        let decoded = PaymentRequest::from_bech32_string(&encoded).expect("decoding should work");
        assert_eq!(decoded.payment_id, payment_request.payment_id);
        assert_eq!(decoded.amount, payment_request.amount);
        assert_eq!(decoded.unit, payment_request.unit);
        assert_eq!(decoded.single_use, payment_request.single_use);
        assert_eq!(decoded.description, payment_request.description);
    }

    #[test]
    fn test_bech32_minimal() {
        let payment_request = PaymentRequest {
            payment_id: Some("minimal".to_string()),
            amount: None,
            unit: Some(CurrencyUnit::Sat),
            single_use: None,
            mints: Some(vec![MintUrl::from_str("https://mint.example.com").unwrap()]),
            description: None,
            transports: vec![],
            nut10: None,
        };

        let encoded = payment_request
            .to_bech32_string()
            .expect("encoding should work");

        let decoded = PaymentRequest::from_bech32_string(&encoded).expect("decoding should work");
        assert_eq!(decoded.payment_id, payment_request.payment_id);
        assert_eq!(decoded.mints, payment_request.mints);
    }

    #[test]
    fn test_bech32_with_nut10() {
        let nut10 = Nut10SecretRequest::new(
            Kind::P2PK,
            "026562efcfadc8e86d44da6a8adf80633d974302e62c850774db1fb36ff4cc7198",
            Some(vec![vec!["timeout".to_string(), "3600".to_string()]]),
        );

        let payment_request = PaymentRequest {
            payment_id: Some("nut10test".to_string()),
            amount: Some(Amount::from(500)),
            unit: Some(CurrencyUnit::Sat),
            single_use: None,
            mints: Some(vec![MintUrl::from_str("https://mint.example.com").unwrap()]),
            description: Some("P2PK locked payment".to_string()),
            transports: vec![],
            nut10: Some(nut10.clone()),
        };

        let encoded = payment_request
            .to_bech32_string()
            .expect("encoding should work");

        let decoded = PaymentRequest::from_bech32_string(&encoded).expect("decoding should work");
        assert_eq!(decoded.nut10.as_ref().unwrap().kind, nut10.kind);
        assert_eq!(decoded.nut10.as_ref().unwrap().data, nut10.data);
    }

    #[test]
    fn test_parse_creq_param_bech32() {
        let payment_request = PaymentRequest {
            payment_id: Some("test123".to_string()),
            amount: Some(Amount::from(100)),
            unit: Some(CurrencyUnit::Sat),
            single_use: None,
            mints: Some(vec![MintUrl::from_str("https://mint.example.com").unwrap()]),
            description: None,
            transports: vec![],
            nut10: None,
        };

        let encoded = payment_request
            .to_bech32_string()
            .expect("encoding should work");

        let decoded_payment_request =
            PaymentRequest::from_bech32_string(&encoded).expect("should parse bech32");
        assert_eq!(
            decoded_payment_request.payment_id,
            payment_request.payment_id
        );
    }

    #[test]
    fn test_from_bech32_string_errors_on_wrong_encoding() {
        // Test that from_bech32_string errors if given a non-CREQ-B string
        let legacy_creq = "creqApWF0gaNhdGVub3N0cmFheKlucHJvZmlsZTFxeTI4d3VtbjhnaGo3dW45ZDNzaGp0bnl2OWtoMnVld2Q5aHN6OW1od2RlbjV0ZTB3ZmprY2N0ZTljdXJ4dmVuOWVlaHFjdHJ2NWhzenJ0aHdkZW41dGUwZGVoaHh0bnZkYWtxcWd5ZGFxeTdjdXJrNDM5eWtwdGt5c3Y3dWRoZGh1NjhzdWNtMjk1YWtxZWZkZWhrZjBkNDk1Y3d1bmw1YWeBgmFuYjE3YWloYjdhOTAxNzZhYQphdWNzYXRhbYF4Imh0dHBzOi8vbm9mZWVzLnRlc3RudXQuY2FzaHUuc3BhY2U=";

        // Should error because it's not bech32m encoded
        assert!(PaymentRequest::from_bech32_string(legacy_creq).is_err());

        // Test with a string that's not CREQ-B
        assert!(PaymentRequest::from_bech32_string("not_a_creq").is_err());

        // Test with wrong HRP (npub instead of creqb)
        let pubkey_hex = "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d";
        let pubkey_bytes = hex::decode(pubkey_hex).unwrap();
        let npub = PaymentRequest::encode_npub(&pubkey_bytes).expect("should encode npub");
        assert!(PaymentRequest::from_bech32_string(&npub).is_err());
    }

    #[test]
    fn test_unit_encoding_bech32() {
        // Test default sat unit
        let payment_request = PaymentRequest {
            payment_id: Some("unit_test".to_string()),
            amount: Some(Amount::from(100)),
            unit: Some(CurrencyUnit::Sat),
            single_use: None,
            mints: Some(vec![MintUrl::from_str("https://mint.example.com").unwrap()]),
            description: None,
            transports: vec![],
            nut10: None,
        };

        let encoded = payment_request
            .to_bech32_string()
            .expect("encoding should work");

        let decoded = PaymentRequest::from_bech32_string(&encoded).expect("decoding should work");
        assert_eq!(decoded.unit, Some(CurrencyUnit::Sat));

        // Test custom unit
        let payment_request_usd = PaymentRequest {
            payment_id: Some("unit_test_usd".to_string()),
            amount: Some(Amount::from(100)),
            unit: Some(CurrencyUnit::Usd),
            single_use: None,
            mints: Some(vec![MintUrl::from_str("https://mint.example.com").unwrap()]),
            description: None,
            transports: vec![],
            nut10: None,
        };

        let encoded_usd = payment_request_usd
            .to_bech32_string()
            .expect("encoding should work");

        let decoded_usd =
            PaymentRequest::from_bech32_string(&encoded_usd).expect("decoding should work");
        assert_eq!(decoded_usd.unit, Some(CurrencyUnit::Usd));
    }

    #[test]
    fn test_npub_encoding_decoding() {
        // Test vector: a known 32-byte pubkey
        let pubkey_hex = "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d";
        let pubkey_bytes = hex::decode(pubkey_hex).unwrap();

        // Encode to npub
        let npub = PaymentRequest::encode_npub(&pubkey_bytes).expect("should encode npub");
        assert!(npub.starts_with("npub"));

        // Decode back
        let decoded = PaymentRequest::decode_npub(&npub).expect("should decode npub");
        assert_eq!(decoded, pubkey_bytes);
    }

    #[test]
    fn test_nprofile_encoding_decoding() {
        let pubkey_hex = "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d";
        let pubkey_bytes = hex::decode(pubkey_hex).unwrap();
        let relays = vec![
            "wss://relay.example.com".to_string(),
            "wss://another-relay.example.com".to_string(),
        ];

        // Encode to nprofile
        let nprofile = PaymentRequest::encode_nprofile(&pubkey_bytes, &relays)
            .expect("should encode nprofile");
        assert!(nprofile.starts_with("nprofile"));

        // Decode back
        let (decoded_pubkey, decoded_relays) =
            PaymentRequest::decode_nprofile(&nprofile).expect("should decode nprofile");
        assert_eq!(decoded_pubkey, pubkey_bytes);
        assert_eq!(decoded_relays, relays);
    }

    #[test]
    fn test_nostr_transport_with_npub() {
        // Create a payment request with nostr transport using npub
        let pubkey_hex = "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d";
        let pubkey_bytes = hex::decode(pubkey_hex).unwrap();
        let npub = PaymentRequest::encode_npub(&pubkey_bytes).expect("encode npub");

        let transport = Transport {
            _type: TransportType::Nostr,
            target: npub.clone(),
            tags: Some(vec![vec!["n".to_string(), "17".to_string()]]),
        };

        let payment_request = PaymentRequest {
            payment_id: Some("nostr_test".to_string()),
            amount: Some(Amount::from(1000)),
            unit: Some(CurrencyUnit::Sat),
            single_use: None,
            mints: Some(vec![MintUrl::from_str("https://mint.example.com").unwrap()]),
            description: Some("Nostr payment".to_string()),
            transports: vec![transport],
            nut10: None,
        };

        let encoded = payment_request
            .to_bech32_string()
            .expect("encoding should work");

        let decoded = PaymentRequest::from_bech32_string(&encoded).expect("decoding should work");

        assert_eq!(decoded.payment_id, payment_request.payment_id);
        assert_eq!(decoded.transports.len(), 1);
        assert_eq!(decoded.transports[0]._type, TransportType::Nostr);
        assert!(decoded.transports[0].target.starts_with("npub"));

        // Check that NIP-17 tag was preserved
        let tags = decoded.transports[0].tags.as_ref().unwrap();
        assert!(tags
            .iter()
            .any(|t| t.len() >= 2 && t[0] == "n" && t[1] == "17"));
    }

    #[test]
    fn test_nostr_transport_with_nprofile() {
        // Create a payment request with nostr transport using nprofile
        let pubkey_hex = "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d";
        let pubkey_bytes = hex::decode(pubkey_hex).unwrap();
        let relays = vec!["wss://relay.example.com".to_string()];
        let nprofile =
            PaymentRequest::encode_nprofile(&pubkey_bytes, &relays).expect("encode nprofile");

        let transport = Transport {
            _type: TransportType::Nostr,
            target: nprofile.clone(),
            tags: Some(vec![vec!["n".to_string(), "17".to_string()]]),
        };

        let payment_request = PaymentRequest {
            payment_id: Some("nprofile_test".to_string()),
            amount: Some(Amount::from(2100)),
            unit: Some(CurrencyUnit::Sat),
            single_use: None,
            mints: Some(vec![MintUrl::from_str("https://mint.example.com").unwrap()]),
            description: Some("Nostr payment with relays".to_string()),
            transports: vec![transport],
            nut10: None,
        };

        let encoded = payment_request
            .to_bech32_string()
            .expect("encoding should work");

        let decoded = PaymentRequest::from_bech32_string(&encoded).expect("decoding should work");

        assert_eq!(decoded.payment_id, payment_request.payment_id);
        assert_eq!(decoded.transports.len(), 1);
        assert_eq!(decoded.transports[0]._type, TransportType::Nostr);

        // Should be encoded back as nprofile since it has relays
        assert!(decoded.transports[0].target.starts_with("nprofile"));

        // Check that relay was preserved in tags
        let tags = decoded.transports[0].tags.as_ref().unwrap();
        assert!(tags
            .iter()
            .any(|t| t.len() >= 2 && t[0] == "relay" && t[1] == "wss://relay.example.com"));
    }

    #[test]
    fn test_spec_example_nostr_transport() {
        // Test a complete example as specified in the spec:
        // Payment request with nostr transport, NIP-17, pubkey, and one relay
        let pubkey_hex = "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d";
        let pubkey_bytes = hex::decode(pubkey_hex).unwrap();
        let relays = vec!["wss://relay.damus.io".to_string()];
        let nprofile =
            PaymentRequest::encode_nprofile(&pubkey_bytes, &relays).expect("encode nprofile");

        let transport = Transport {
            _type: TransportType::Nostr,
            target: nprofile,
            tags: Some(vec![vec!["n".to_string(), "17".to_string()]]),
        };

        let payment_request = PaymentRequest {
            payment_id: Some("spec_example".to_string()),
            amount: Some(Amount::from(10)),
            unit: Some(CurrencyUnit::Sat),
            single_use: Some(true),
            mints: Some(vec![MintUrl::from_str("https://mint.example.com").unwrap()]),
            description: Some("Coffee".to_string()),
            transports: vec![transport],
            nut10: None,
        };

        // Encode and decode
        let encoded = payment_request
            .to_bech32_string()
            .expect("encoding should work");

        println!("Spec example encoded: {}", encoded);

        let decoded = PaymentRequest::from_bech32_string(&encoded).expect("decoding should work");

        // Verify round-trip
        assert_eq!(decoded.payment_id, Some("spec_example".to_string()));
        assert_eq!(decoded.amount, Some(Amount::from(10)));
        assert_eq!(decoded.unit, Some(CurrencyUnit::Sat));
        assert_eq!(decoded.single_use, Some(true));
        assert_eq!(decoded.description, Some("Coffee".to_string()));
        assert_eq!(decoded.transports.len(), 1);
        assert_eq!(decoded.transports[0]._type, TransportType::Nostr);

        // Verify relay and NIP are preserved
        let tags = decoded.transports[0].tags.as_ref().unwrap();
        assert!(tags
            .iter()
            .any(|t| t.len() >= 2 && t[0] == "n" && t[1] == "17"));
        assert!(tags
            .iter()
            .any(|t| t.len() >= 2 && t[0] == "relay" && t[1] == "wss://relay.damus.io"));
    }

    #[test]
    fn test_decode_valid_bech32_with_nostr_pubkeys_and_mints() {
        // First, create a payment request with multiple mints and nostr transports with different pubkeys
        let pubkey1_hex = "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d";
        let pubkey1_bytes = hex::decode(pubkey1_hex).unwrap();
        let npub1 = PaymentRequest::encode_npub(&pubkey1_bytes).expect("encode npub1");

        let pubkey2_hex = "79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
        let pubkey2_bytes = hex::decode(pubkey2_hex).unwrap();
        let relays2 = vec![
            "wss://relay.damus.io".to_string(),
            "wss://nos.lol".to_string(),
        ];
        let nprofile2 =
            PaymentRequest::encode_nprofile(&pubkey2_bytes, &relays2).expect("encode nprofile2");

        let transport1 = Transport {
            _type: TransportType::Nostr,
            target: npub1.clone(),
            tags: Some(vec![vec!["n".to_string(), "17".to_string()]]),
        };

        let transport2 = Transport {
            _type: TransportType::Nostr,
            target: nprofile2.clone(),
            tags: Some(vec![
                vec!["n".to_string(), "17".to_string()],
                vec!["n".to_string(), "44".to_string()],
            ]),
        };

        let payment_request = PaymentRequest {
            payment_id: Some("multi_test".to_string()),
            amount: Some(Amount::from(5000)),
            unit: Some(CurrencyUnit::Sat),
            single_use: Some(false),
            mints: Some(vec![
                MintUrl::from_str("https://mint1.example.com").unwrap(),
                MintUrl::from_str("https://mint2.example.com").unwrap(),
                MintUrl::from_str("https://testnut.cashu.space").unwrap(),
            ]),
            description: Some("Payment with multiple transports and mints".to_string()),
            transports: vec![transport1, transport2],
            nut10: None,
        };

        // Encode to bech32 string
        let encoded = payment_request
            .to_bech32_string()
            .expect("encoding should work");

        println!("Encoded payment request: {}", encoded);

        // Now decode the bech32 string and verify contents
        let decoded = PaymentRequest::from_bech32_string(&encoded)
            .expect("should decode valid bech32 string");

        // Verify basic fields
        assert_eq!(decoded.payment_id, Some("multi_test".to_string()));
        assert_eq!(decoded.amount, Some(Amount::from(5000)));
        assert_eq!(decoded.unit, Some(CurrencyUnit::Sat));
        assert_eq!(decoded.single_use, Some(false));
        assert_eq!(
            decoded.description,
            Some("Payment with multiple transports and mints".to_string())
        );

        // Verify mints
        let mints = decoded.mints.as_ref().expect("should have mints");
        assert_eq!(mints.len(), 3);

        // MintUrl normalizes URLs and may add trailing slashes
        let mint_strings: Vec<String> = mints.iter().map(|m| m.to_string()).collect();
        assert!(
            mint_strings[0] == "https://mint1.example.com/"
                || mint_strings[0] == "https://mint1.example.com"
        );
        assert!(
            mint_strings[1] == "https://mint2.example.com/"
                || mint_strings[1] == "https://mint2.example.com"
        );
        assert!(
            mint_strings[2] == "https://testnut.cashu.space/"
                || mint_strings[2] == "https://testnut.cashu.space"
        );

        // Verify transports
        assert_eq!(decoded.transports.len(), 2);

        // Verify first transport (npub)
        let transport1_decoded = &decoded.transports[0];
        assert_eq!(transport1_decoded._type, TransportType::Nostr);
        assert!(transport1_decoded.target.starts_with("npub"));

        // Decode the npub to verify the pubkey
        let decoded_pubkey1 =
            PaymentRequest::decode_npub(&transport1_decoded.target).expect("should decode npub");
        assert_eq!(decoded_pubkey1, pubkey1_bytes);

        // Verify NIP-17 tag
        let tags1 = transport1_decoded.tags.as_ref().unwrap();
        assert!(tags1
            .iter()
            .any(|t| t.len() >= 2 && t[0] == "n" && t[1] == "17"));

        // Verify second transport (nprofile)
        let transport2_decoded = &decoded.transports[1];
        assert_eq!(transport2_decoded._type, TransportType::Nostr);
        assert!(transport2_decoded.target.starts_with("nprofile"));

        // Decode the nprofile to verify the pubkey and relays
        let (decoded_pubkey2, decoded_relays2) =
            PaymentRequest::decode_nprofile(&transport2_decoded.target)
                .expect("should decode nprofile");
        assert_eq!(decoded_pubkey2, pubkey2_bytes);
        assert_eq!(decoded_relays2, relays2);

        // Verify tags include both NIPs and relays
        let tags2 = transport2_decoded.tags.as_ref().unwrap();
        assert!(tags2
            .iter()
            .any(|t| t.len() >= 2 && t[0] == "n" && t[1] == "17"));
        assert!(tags2
            .iter()
            .any(|t| t.len() >= 2 && t[0] == "n" && t[1] == "44"));
        assert!(tags2
            .iter()
            .any(|t| t.len() >= 2 && t[0] == "relay" && t[1] == "wss://relay.damus.io"));
        assert!(tags2
            .iter()
            .any(|t| t.len() >= 2 && t[0] == "relay" && t[1] == "wss://nos.lol"));
    }

    // Test vectors from NUT-26 specification
    // https://github.com/cashubtc/nuts/blob/main/tests/26-tests.md
    #[test]
    fn test_basic_payment_request() {
        // Basic payment request with required fields
        let json = r#"{
            "i": "b7a90176",
            "a": 10,
            "u": "sat",
            "m": ["https://8333.space:3338"],
            "t": [
                {
                    "t": "nostr",
                    "a": "nprofile1qy28wumn8ghj7un9d3shjtnyv9kh2uewd9hsz9mhwden5te0wfjkccte9curxven9eehqctrv5hszrthwden5te0dehhxtnvdakqqgydaqy7curk439ykptkysv7udhdhu68sucm295akqefdehkf0d495cwunl5",
                    "g": [["n", "17"]]
                }
            ]
        }"#;

        let expected_encoded = "CREQB1QYQQSC3HVYUNQVFHXCPQQZQQQQQQQQQQQQ9QXQQPQQZSQ9MGW368QUE69UHNSVENXVH8XURPVDJN5VENXVUQWQRDQYQQZQGZQQSGM6QFA3C8DTZ2FVZHVFQEACMWM0E50PE3K5TFMVPJJMN0VJ7M2TGRQQPQQYGYQQ28WUMN8GHJ7UN9D3SHJTNYV9KH2UEWD9HSGQQHWAEHXW309AEX2MRP0YHRSVENXVH8XURPVDJJ7PQQP4MHXUE69UHKUMMN9EKX7MQ8402ZW";

        // Parse the JSON into a PaymentRequest
        let payment_request: PaymentRequest = serde_json::from_str(json).unwrap();
        let payment_request_cloned = payment_request.clone();

        // Verify the payment request fields
        assert_eq!(
            payment_request_cloned.payment_id.as_ref().unwrap(),
            "b7a90176"
        );
        assert_eq!(payment_request_cloned.amount.unwrap(), Amount::from(10));
        assert_eq!(payment_request_cloned.unit.unwrap(), CurrencyUnit::Sat);
        assert_eq!(
            payment_request_cloned.mints.unwrap(),
            vec![MintUrl::from_str("https://8333.space:3338").unwrap()]
        );

        let transport = payment_request.transports.first().unwrap();
        assert_eq!(transport._type, TransportType::Nostr);
        assert_eq!(transport.target, "nprofile1qy28wumn8ghj7un9d3shjtnyv9kh2uewd9hsz9mhwden5te0wfjkccte9curxven9eehqctrv5hszrthwden5te0dehhxtnvdakqqgydaqy7curk439ykptkysv7udhdhu68sucm295akqefdehkf0d495cwunl5");
        assert_eq!(
            transport.tags,
            Some(vec![vec!["n".to_string(), "17".to_string()]])
        );

        // Test bech32m encoding (CREQ-B format) - this is what NUT-26 is about
        let encoded = payment_request
            .to_bech32_string()
            .expect("Failed to encode to bech32");

        // Verify it starts with CREQB1 (uppercase because we use encode_upper)
        assert!(encoded == expected_encoded);

        // Test round-trip via bech32 format
        let decoded = PaymentRequest::from_bech32_string(&encoded).unwrap();

        // Verify decoded fields match original (but not the exact nprofile string,
        // as the decoded nprofile may include relays that were embedded in the original)
        assert_eq!(decoded.payment_id.as_ref().unwrap(), "b7a90176");
        assert_eq!(decoded.amount.unwrap(), Amount::from(10));
        assert_eq!(decoded.unit.unwrap(), CurrencyUnit::Sat);
        assert_eq!(
            decoded.mints.unwrap(),
            vec![MintUrl::from_str("https://8333.space:3338").unwrap()]
        );

        // Verify transport type and that it has the NIP-17 tag
        assert_eq!(decoded.transports.len(), 1);
        assert_eq!(decoded.transports[0]._type, TransportType::Nostr);
        let tags = decoded.transports[0].tags.as_ref().unwrap();
        assert!(tags
            .iter()
            .any(|t| t.len() >= 2 && t[0] == "n" && t[1] == "17"));

        // Verify the pubkey is preserved (decode both nprofiles and compare pubkeys)
        let (original_pubkey, _) = PaymentRequest::decode_nprofile(&transport.target).unwrap();
        let (decoded_pubkey, _) =
            PaymentRequest::decode_nprofile(&decoded.transports[0].target).unwrap();
        assert_eq!(original_pubkey, decoded_pubkey);
    }

    #[test]
    fn test_nostr_transport_payment_request() {
        // Nostr transport payment request with multiple mints
        let json = r#"{
            "i": "f92a51b8",
            "a": 100,
            "u": "sat",
            "m": ["https://mint1.example.com", "https://mint2.example.com"],
            "t": [
                {
                    "t": "nostr",
                    "a": "npub1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqzqujme",
                    "g": [["n", "17"], ["n", "9735"]]
                }
            ]
        }"#;

        let expected_encoded = "CREQB1QYQQSE3EXFSN2VTZ8QPQQZQQQQQQQQQQQPJQXQQPQQZSQXTGW368QUE69UHK66TWWSCJUETCV9KHQMR99E3K7MG9QQVKSAR5WPEN5TE0D45KUAPJ9EJHSCTDWPKX2TNRDAKSWQPWQYQQZQGZQQSQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQRQQZQQYFXQU6YHAEU";

        // Parse the JSON into a PaymentRequest
        let payment_request: PaymentRequest = serde_json::from_str(json).unwrap();
        let payment_request_cloned = payment_request.clone();

        // Verify the payment request fields
        assert_eq!(
            payment_request_cloned.payment_id.as_ref().unwrap(),
            "f92a51b8"
        );
        assert_eq!(payment_request_cloned.amount.unwrap(), Amount::from(100));
        assert_eq!(payment_request_cloned.unit.unwrap(), CurrencyUnit::Sat);
        assert_eq!(
            payment_request_cloned.mints.unwrap(),
            vec![
                MintUrl::from_str("https://mint1.example.com").unwrap(),
                MintUrl::from_str("https://mint2.example.com").unwrap()
            ]
        );

        let transport = payment_request_cloned.transports.first().unwrap();
        assert_eq!(transport._type, TransportType::Nostr);
        assert_eq!(
            transport.target,
            "npub1qqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqzqujme"
        );
        assert_eq!(
            transport.tags,
            Some(vec![
                vec!["n".to_string(), "17".to_string()],
                vec!["n".to_string(), "9735".to_string()]
            ])
        );

        // Test round-trip serialization
        let encoded = payment_request.to_bech32_string().unwrap();
        assert_eq!(encoded, expected_encoded);
        let decoded = PaymentRequest::from_str(&encoded).unwrap();
        assert_eq!(payment_request, decoded);

        // Test decoding the expected encoded string
        assert_eq!(decoded.payment_id.as_ref().unwrap(), "f92a51b8");
    }

    #[test]
    fn test_minimal_payment_request() {
        // Minimal payment request with only required fields
        let json = r#"{
            "i": "7f4a2b39",
            "u": "sat",
            "m": ["https://mint.example.com"]
        }"#;

        let expected_encoded =
            "CREQB1QYQQSDMXX3SNYC3N8YPSQQGQQ5QPS6R5W3C8XW309AKKJMN59EJHSCTDWPKX2TNRDAKSYP0LHG";

        // Parse the JSON into a PaymentRequest
        let payment_request: PaymentRequest = serde_json::from_str(json).unwrap();
        let payment_request_cloned = payment_request.clone();

        // Verify the payment request fields
        assert_eq!(
            payment_request_cloned.payment_id.as_ref().unwrap(),
            "7f4a2b39"
        );
        assert_eq!(payment_request_cloned.amount, None);
        assert_eq!(payment_request_cloned.unit.unwrap(), CurrencyUnit::Sat);
        assert_eq!(
            payment_request_cloned.mints.unwrap(),
            vec![MintUrl::from_str("https://mint.example.com").unwrap()]
        );
        assert_eq!(payment_request_cloned.transports, vec![]);

        // Test round-trip serialization
        let encoded = payment_request.to_bech32_string().unwrap();
        assert_eq!(encoded, expected_encoded);
        let decoded = PaymentRequest::from_bech32_string(&encoded).unwrap();
        assert_eq!(payment_request, decoded);

        // Test decoding the expected encoded string
        let decoded_from_spec = PaymentRequest::from_bech32_string(expected_encoded).unwrap();
        assert_eq!(decoded_from_spec.payment_id.as_ref().unwrap(), "7f4a2b39");
    }

    #[test]
    fn test_nut10_locking_payment_request() {
        // Payment request with NUT-10 P2PK locking
        let json = r#"{
            "i": "c9e45d2a",
            "a": 500,
            "u": "sat",
            "m": ["https://mint.example.com"],
            "nut10": {
                "k": "P2PK",
                "d": "02c3b5bb27e361457c92d93d78dd73d3d53732110b2cfe8b50fbc0abc615e9c331",
                "t": [["timeout", "3600"]]
            }
        }"#;

        let expected_encoded = "CREQB1QYQQSCEEV56R2EPJVYPQQZQQQQQQQQQQQ86QXQQPQQZSQXRGW368QUE69UHK66TWWSHX27RPD4CXCEFWVDHK6ZQQTGQSQQSQQQPQQS3SXF3NXC34VF3RYDM9XVMRZDP4XA3NJVNY8YEKGDECV3JRWVMYXDJR2VEHXVERZVFSVGEXXEN98P3R2VRXVF3NQCTZVVMRZDT9893NXVE3QVQQ6PM5D9KK2MM4WSZRXD3SXQQ5SCYD";

        // Parse the JSON into a PaymentRequest
        let payment_request: PaymentRequest = serde_json::from_str(json).unwrap();
        let payment_request_cloned = payment_request.clone();

        // Verify the payment request fields
        assert_eq!(
            payment_request_cloned.payment_id.as_ref().unwrap(),
            "c9e45d2a"
        );
        assert_eq!(payment_request_cloned.amount.unwrap(), Amount::from(500));
        assert_eq!(payment_request_cloned.unit.unwrap(), CurrencyUnit::Sat);
        assert_eq!(
            payment_request_cloned.mints.unwrap(),
            vec![MintUrl::from_str("https://mint.example.com").unwrap()]
        );

        // Test NUT-10 locking
        let nut10 = payment_request_cloned.nut10.unwrap();
        assert_eq!(nut10.kind, Kind::P2PK);
        assert_eq!(
            nut10.data,
            "02c3b5bb27e361457c92d93d78dd73d3d53732110b2cfe8b50fbc0abc615e9c331"
        );
        assert_eq!(
            nut10.tags,
            Some(vec![vec!["timeout".to_string(), "3600".to_string()]])
        );

        // Test round-trip serialization
        let encoded = payment_request.to_bech32_string().unwrap();
        assert_eq!(encoded, expected_encoded);
        let decoded = PaymentRequest::from_str(&encoded).unwrap();
        assert_eq!(payment_request, decoded);

        // Test decoding the expected encoded string
        let decoded_from_spec = PaymentRequest::from_str(expected_encoded).unwrap();
        assert_eq!(decoded_from_spec.payment_id.as_ref().unwrap(), "c9e45d2a");
    }

    #[test]
    fn test_nut26_example() {
        // Payment request with NUT-10 P2PK locking
        let json = r#"{
  "i": "demo123",
  "a": 1000,
  "u": "sat",
  "s": true,
  "m": ["https://mint.example.com"],
  "d": "Coffee payment"
}"#;

        let expected_encoded = "CREQB1QYQQWER9D4HNZV3NQGQQSQQQQQQQQQQRAQPSQQGQQSQQZQG9QQVXSAR5WPEN5TE0D45KUAPWV4UXZMTSD3JJUCM0D5RQQRJRDANXVET9YPCXZ7TDV4H8GXHR3TQ";

        // Parse the JSON into a PaymentRequest
        let payment_request: PaymentRequest = serde_json::from_str(json).unwrap();

        let encoded = payment_request.to_bech32_string().unwrap();

        assert_eq!(expected_encoded, encoded);
    }
}
