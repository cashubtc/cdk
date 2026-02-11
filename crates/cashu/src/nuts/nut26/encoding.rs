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
                "auth" => CurrencyUnit::Auth,
                _ => CurrencyUnit::Custom(s), // preserve unknown units
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

    fn read_tlv(&mut self) -> Result<Option<(u8, Vec<u8>)>, Error> {
        if self.position + 3 > self.data.len() {
            return Ok(None);
        }

        let tag = self.data[self.position];
        let len = u16::from_be_bytes([self.data[self.position + 1], self.data[self.position + 2]])
            as usize;
        self.position += 3;

        if self.position + len > self.data.len() {
            return Err(Error::InvalidLength);
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
    /// use std::str::FromStr;
    ///
    /// use cashu::nuts::nut18::PaymentRequest;
    /// use cashu::{Amount, MintUrl};
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
        let hrp = Hrp::parse(CREQ_B_HRP).map_err(|_| Error::InvalidStructure)?;

        // Always emit uppercase for QR compatibility
        let encoded = bech32::encode_upper::<Bech32m>(hrp, &tlv_bytes)
            .map_err(|_| Error::InvalidStructure)?;
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
        let (hrp, data) = bech32::decode(s).map_err(|e| Error::Bech32Error(e))?;
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

        while let Some((tag, value)) = reader.read_tlv()? {
            match tag {
                0x01 => {
                    // id: string
                    if id.is_some() {
                        return Err(Error::InvalidStructure);
                    }
                    id = Some(String::from_utf8(value).map_err(|_| Error::InvalidUtf8)?);
                }
                0x02 => {
                    // amount: u64
                    if amount.is_some() {
                        return Err(Error::InvalidStructure);
                    }
                    if value.len() != 8 {
                        return Err(Error::InvalidLength);
                    }
                    let amount_val = u64::from_be_bytes([
                        value[0], value[1], value[2], value[3], value[4], value[5], value[6],
                        value[7],
                    ]);
                    amount = Some(Amount::from(amount_val));
                }
                0x03 => {
                    // unit: u8 or string
                    if unit.is_some() {
                        return Err(Error::InvalidStructure);
                    }
                    if value.len() == 1 && value[0] == 0 {
                        unit = Some(CurrencyUnit::Sat);
                    } else {
                        let unit_str = String::from_utf8(value).map_err(|_| Error::InvalidUtf8)?;
                        unit = Some(TlvUnit::Custom(unit_str).into());
                    }
                }
                0x04 => {
                    // single_use: u8 (0 or 1)
                    if single_use.is_some() {
                        return Err(Error::InvalidStructure);
                    }
                    if !value.is_empty() {
                        single_use = Some(value[0] != 0);
                    }
                }
                0x05 => {
                    // mint: string (repeatable)
                    let mint_str = String::from_utf8(value).map_err(|_| Error::InvalidUtf8)?;
                    let mint_url =
                        MintUrl::from_str(&mint_str).map_err(|_| Error::InvalidStructure)?;
                    mints.push(mint_url);
                }
                0x06 => {
                    // description: string
                    if description.is_some() {
                        return Err(Error::InvalidStructure);
                    }
                    description = Some(String::from_utf8(value).map_err(|_| Error::InvalidUtf8)?);
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
        // In-band transports are represented by the absence of a transport tag (NUT-18 semantics)
        for transport in &self.transports {
            if transport._type == TransportType::InBand {
                // Skip in-band transports - absence of transport tag means in-band
                continue;
            }
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
        let mut tags: Vec<(String, Vec<String>)> = Vec::new();
        let mut http_target: Option<String> = None;

        while let Some((tag, value)) = reader.read_tlv()? {
            match tag {
                0x01 => {
                    // kind: u8
                    if kind.is_some() {
                        return Err(Error::InvalidStructure);
                    }
                    if value.len() != 1 {
                        return Err(Error::InvalidLength);
                    }
                    kind = Some(value[0]);
                }
                0x02 => {
                    // target: bytes (interpretation depends on kind)
                    match kind {
                        Some(0x00) => {
                            // nostr: 32-byte x-only pubkey
                            if value.len() != 32 {
                                return Err(Error::InvalidLength);
                            }
                            pubkey = Some(value);
                        }
                        Some(0x01) => {
                            // http_post: UTF-8 URL string
                            http_target =
                                Some(String::from_utf8(value).map_err(|_| Error::InvalidUtf8)?);
                        }
                        None => {
                            // kind should always be present if there's a target
                        }
                        _ => return Err(Error::InvalidStructure),
                    }
                }
                0x03 => {
                    // tag_tuple: generic tuple (repeatable)
                    let tag_tuple = Self::decode_tag_tuple(&value)?;
                    tags.push(tag_tuple);
                }
                _ => {
                    // Unknown sub-TLV tags are ignored
                }
            }
        }

        // In-band transport is represented by absence of transport tag (0x07)
        // If we're here, we have a transport tag, so it must be nostr or http_post
        let transport_type = match kind.ok_or(Error::InvalidStructure)? {
            0x00 => TransportType::Nostr,
            0x01 => TransportType::HttpPost,
            _ => return Err(Error::InvalidStructure),
        };

        // Extract relays from "r" tag tuples for Nostr transport
        let relays: Vec<String> = tags
            .iter()
            .filter(|(k, _)| k == "r")
            .flat_map(|(_, v)| v.clone())
            .collect();

        // Build the target string based on transport type
        let target = match transport_type {
            TransportType::Nostr => {
                // Always use nprofile (with empty relay list if no relays)
                if let Some(pk) = pubkey {
                    Self::encode_nprofile(&pk, &relays)?
                } else {
                    return Err(Error::InvalidStructure);
                }
            }
            TransportType::HttpPost => http_target.ok_or(Error::InvalidStructure)?,
            TransportType::InBand => {
                // This case should not be reachable since InBand is not decoded from transport tag
                unreachable!("InBand transport should not be decoded from transport tag")
            }
        };

        // Keep tags as-is per NUT-26 spec (no "r" to "relay" conversion)
        // "r" tags are part of the transport encoding and should be preserved
        let final_tags: Vec<(String, Vec<String>)> = tags;

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
        // Note: InBand transports should not reach here (filtered out in encode_tlv)
        // but we handle it defensively
        let kind = match transport._type {
            TransportType::InBand => {
                // In-band is represented by absence of transport tag, not by encoding
                return Err(Error::InvalidStructure);
            }
            TransportType::Nostr => 0x00u8,
            TransportType::HttpPost => 0x01u8,
        };
        writer.write_tlv(0x01, &[kind]);

        // 0x02 target: bytes
        // Note: InBand already returned error above, so only Nostr and HttpPost reach here
        match transport._type {
            TransportType::Nostr => {
                // For nostr, decode nprofile to extract pubkey and relays
                let (pubkey, relays) = Self::decode_nprofile(&transport.target)?;

                // Write the 32-byte pubkey
                writer.write_tlv(0x02, &pubkey);

                // Collect all relays (from nprofile and from "relay" tags)
                let mut all_relays = relays;

                // Extract NIPs and other tags from the tags field
                if let Some(ref tags) = transport.tags {
                    for tag in tags {
                        if tag.is_empty() {
                            continue;
                        }
                        if tag[0] == "n" && tag.len() >= 2 {
                            // Encode NIPs as tag tuples with key "n"
                            let tag_bytes = Self::encode_tag_tuple(tag)?;
                            writer.write_tlv(0x03, &tag_bytes);
                        } else if tag[0] == "relay" && tag.len() >= 2 {
                            // Collect relays from tags to encode as "r" tag tuples
                            all_relays.push(tag[1].clone());
                        } else {
                            // Other tags as generic tag tuples
                            let tag_bytes = Self::encode_tag_tuple(tag)?;
                            writer.write_tlv(0x03, &tag_bytes);
                        }
                    }
                }

                // 0x03 tag_tuple: encode relays as tag tuples with key "r"
                for relay in all_relays {
                    let relay_tag = vec!["r".to_string(), relay];
                    let tag_bytes = Self::encode_tag_tuple(&relay_tag)?;
                    writer.write_tlv(0x03, &tag_bytes);
                }
            }
            TransportType::HttpPost => {
                writer.write_tlv(0x02, transport.target.as_bytes());

                // 0x03 tag_tuple: generic tuple (repeatable)
                if let Some(ref tags) = transport.tags {
                    for tag in tags {
                        if !tag.is_empty() {
                            let tag_bytes = Self::encode_tag_tuple(tag)?;
                            writer.write_tlv(0x03, &tag_bytes);
                        }
                    }
                }
            }
            TransportType::InBand => {
                // This case is unreachable since we return early with error for InBand
                unreachable!("InBand transport should not reach target encoding")
            }
        }

        Ok(writer.into_bytes())
    }

    /// Decode NUT-10 sub-TLV
    fn decode_nut10(bytes: &[u8]) -> Result<Nut10SecretRequest, Error> {
        let mut reader = TlvReader::new(bytes);

        let mut kind: Option<u8> = None;
        let mut data: Option<Vec<u8>> = None;
        let mut tags: Vec<(String, Vec<String>)> = Vec::new();

        while let Some((tag, value)) = reader.read_tlv()? {
            match tag {
                0x01 => {
                    // kind: u8
                    if kind.is_some() {
                        return Err(Error::InvalidStructure);
                    }
                    if value.len() != 1 {
                        return Err(Error::InvalidLength);
                    }
                    kind = Some(value[0]);
                }
                0x02 => {
                    // data: bytes
                    if data.is_some() {
                        return Err(Error::InvalidStructure);
                    }
                    data = Some(value);
                }
                0x03 => {
                    // tag_tuple: generic tuple (repeatable)
                    let tag_tuple = Self::decode_tag_tuple(&value)?;
                    tags.push(tag_tuple);
                }
                _ => {
                    // Unknown tags are ignored
                }
            }
        }

        let kind_val = kind.ok_or(Error::InvalidStructure)?;
        let data_val = data.unwrap_or_default();

        // Convert kind u8 to Kind enum
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

        // 0x01 kind: u8
        let kind_val = match nut10.kind {
            Kind::P2PK => 0u8,
            Kind::HTLC => 1u8,
        };
        writer.write_tlv(0x01, &[kind_val]);

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
            return Err(Error::InvalidLength);
        }

        let key_len = bytes[0] as usize;
        if bytes.len() < 1 + key_len {
            return Err(Error::InvalidLength);
        }

        let key =
            String::from_utf8(bytes[1..1 + key_len].to_vec()).map_err(|_| Error::InvalidUtf8)?;

        let mut values = Vec::new();
        let mut pos = 1 + key_len;

        while pos < bytes.len() {
            let val_len = bytes[pos] as usize;
            pos += 1;

            if pos + val_len > bytes.len() {
                return Err(Error::InvalidLength);
            }

            let value = String::from_utf8(bytes[pos..pos + val_len].to_vec())
                .map_err(|_| Error::InvalidUtf8)?;
            values.push(value);
            pos += val_len;
        }

        Ok((key, values))
    }

    /// Encode tag tuple
    fn encode_tag_tuple(tag: &[String]) -> Result<Vec<u8>, Error> {
        if tag.is_empty() {
            return Err(Error::InvalidStructure);
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

    /// Decode nprofile bech32 string to (pubkey, relays)
    /// NIP-19 nprofile TLV format:
    /// - Type 0: 32-byte pubkey (required, only one)
    /// - Type 1: relay URL string (optional, repeatable)
    fn decode_nprofile(nprofile: &str) -> Result<(Vec<u8>, Vec<String>), Error> {
        let (hrp, data) = bech32::decode(nprofile).map_err(|e| Error::Bech32Error(e))?;
        if hrp.as_str() != "nprofile" {
            return Err(Error::InvalidStructure);
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
                return Err(Error::InvalidLength);
            }

            let value = &data[pos..pos + len];
            pos += len;

            match tag {
                0 => {
                    // pubkey: 32 bytes
                    if value.len() != 32 {
                        return Err(Error::InvalidLength);
                    }
                    pubkey = Some(value.to_vec());
                }
                1 => {
                    // relay: UTF-8 string
                    let relay =
                        String::from_utf8(value.to_vec()).map_err(|_| Error::InvalidUtf8)?;
                    relays.push(relay);
                }
                _ => {
                    // Unknown TLV types are ignored per NIP-19
                }
            }
        }

        let pubkey = pubkey.ok_or(Error::InvalidStructure)?;
        Ok((pubkey, relays))
    }

    /// Encode pubkey and relays to nprofile bech32 string
    /// NIP-19 nprofile TLV format (Type: 1 byte, Length: 1 byte, Value: variable)
    fn encode_nprofile(pubkey: &[u8], relays: &[String]) -> Result<String, Error> {
        if pubkey.len() != 32 {
            return Err(Error::InvalidLength);
        }

        let mut tlv_bytes = Vec::new();

        // Type 0: pubkey (32 bytes) - Length must fit in 1 byte
        tlv_bytes.push(0); // type
        tlv_bytes.push(32); // length
        tlv_bytes.extend_from_slice(pubkey);

        // Type 1: relays (repeatable) - Length must fit in 1 byte
        for relay in relays {
            if relay.len() > 255 {
                return Err(Error::TagTooLong); // Relay URL too long for NIP-19
            }
            tlv_bytes.push(1); // type
            tlv_bytes.push(relay.len() as u8); // length
            tlv_bytes.extend_from_slice(relay.as_bytes());
        }

        let hrp = Hrp::parse("nprofile").map_err(|_| Error::InvalidStructure)?;
        bech32::encode::<Bech32>(hrp, &tlv_bytes).map_err(|_| Error::InvalidStructure)
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

        // Test with wrong HRP (nprofile instead of creqb)
        let pubkey_hex = "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d";
        let pubkey_bytes = hex::decode(pubkey_hex).unwrap();
        let nprofile =
            PaymentRequest::encode_nprofile(&pubkey_bytes, &[]).expect("should encode nprofile");
        assert!(PaymentRequest::from_bech32_string(&nprofile).is_err());
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
    fn test_nprofile_no_relays() {
        // Test vector: a known 32-byte pubkey
        let pubkey_hex = "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d";
        let pubkey_bytes = hex::decode(pubkey_hex).unwrap();

        // Encode to nprofile with empty relay list
        let nprofile =
            PaymentRequest::encode_nprofile(&pubkey_bytes, &[]).expect("should encode nprofile");
        assert!(nprofile.starts_with("nprofile"));

        // Decode back
        let decoded = PaymentRequest::decode_nprofile(&nprofile).expect("should decode nprofile");
        assert_eq!(decoded.0, pubkey_bytes);
        assert!(decoded.1.is_empty());
    }

    #[test]
    fn test_nprofile_encoding_decoding() {
        use nostr_sdk::prelude::*;

        let keys = Keys::generate();
        let pubkey_bytes = keys.public_key().to_bytes().to_vec();
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
    fn test_nprofile_matches_nostr_crate() {
        use nostr_sdk::prelude::*;

        let keys = Keys::generate();
        let nostr_pubkey = keys.public_key();
        let pubkey_bytes = nostr_pubkey.to_bytes().to_vec();
        let relays = vec![
            "wss://relay.example.com".to_string(),
            "wss://relay.damus.io".to_string(),
        ];

        // Create nostr-sdk relay URLs
        let nostr_relays: Vec<RelayUrl> = relays
            .iter()
            .map(|r| RelayUrl::parse(r).expect("valid relay url"))
            .collect();

        // Test 1: Encode with our implementation, decode with nostr-sdk
        let our_nprofile = PaymentRequest::encode_nprofile(&pubkey_bytes, &relays)
            .expect("should encode nprofile");

        let nostr_decoded =
            Nip19Profile::from_bech32(&our_nprofile).expect("nostr-sdk should decode our nprofile");
        assert_eq!(nostr_decoded.public_key, nostr_pubkey);
        assert_eq!(nostr_decoded.relays.len(), relays.len());
        for (decoded_relay, expected_relay) in nostr_decoded.relays.iter().zip(nostr_relays.iter())
        {
            assert_eq!(decoded_relay, expected_relay);
        }

        // Test 2: Encode with nostr-sdk, decode with our implementation
        let nostr_profile = Nip19Profile::new(nostr_pubkey, nostr_relays.clone());
        let nostr_nprofile = nostr_profile.to_bech32().expect("should encode nprofile");

        let (our_decoded_pubkey, our_decoded_relays) =
            PaymentRequest::decode_nprofile(&nostr_nprofile)
                .expect("should decode nostr-sdk nprofile");
        assert_eq!(our_decoded_pubkey, pubkey_bytes);
        assert_eq!(our_decoded_relays.len(), relays.len());
        for (decoded_relay, expected_relay) in our_decoded_relays.iter().zip(relays.iter()) {
            assert_eq!(decoded_relay, expected_relay);
        }

        // Test 3: Both implementations produce identical bech32 strings
        assert_eq!(our_nprofile, nostr_nprofile);
    }

    #[test]
    fn test_nprofile_empty_relays_matches_nostr_crate() {
        use nostr_sdk::prelude::*;

        let keys = Keys::generate();
        let nostr_pubkey = keys.public_key();
        let pubkey_bytes = nostr_pubkey.to_bytes().to_vec();

        // Create nostr-sdk types with empty relays
        let nostr_relays: Vec<RelayUrl> = vec![];

        // Test with empty relays
        let our_nprofile =
            PaymentRequest::encode_nprofile(&pubkey_bytes, &[]).expect("should encode nprofile");

        let nostr_profile = Nip19Profile::new(nostr_pubkey, nostr_relays);
        let nostr_nprofile = nostr_profile.to_bech32().expect("should encode nprofile");

        // Verify both can decode each other's output
        let nostr_decoded =
            Nip19Profile::from_bech32(&our_nprofile).expect("nostr-sdk should decode our nprofile");
        assert_eq!(nostr_decoded.public_key, nostr_pubkey);
        assert!(nostr_decoded.relays.is_empty());

        let (our_decoded_pubkey, our_decoded_relays) =
            PaymentRequest::decode_nprofile(&nostr_nprofile)
                .expect("should decode nostr-sdk nprofile");
        assert_eq!(our_decoded_pubkey, pubkey_bytes);
        assert!(our_decoded_relays.is_empty());

        // Both should produce identical strings
        assert_eq!(our_nprofile, nostr_nprofile);
    }

    #[test]
    fn nut_18_payment_request() {
        use nostr_sdk::prelude::*;
        let nprofile = "nprofile1qy28wumn8ghj7un9d3shjtnyv9kh2uewd9hsz9mhwden5te0wfjkccte9curxven9eehqctrv5hszrthwden5te0dehhxtnvdakqqgydaqy7curk439ykptkysv7udhdhu68sucm295akqefdehkf0d495cwunl5";

        let nostr_decoded =
            Nip19Profile::from_bech32(&nprofile).expect("nostr-sdk should decode our nprofile");

        // Verify the decoded data can be re-encoded (round-trip works)
        let encoded = nostr_decoded.to_bech32().unwrap();

        // Re-decode to verify content is preserved (encoding may differ due to normalization)
        let re_decoded = Nip19Profile::from_bech32(&encoded)
            .expect("nostr-sdk should decode re-encoded nprofile");

        // Verify the semantic content is preserved
        assert_eq!(nostr_decoded.public_key, re_decoded.public_key);
        assert_eq!(nostr_decoded.relays.len(), re_decoded.relays.len());
    }

    #[test]
    fn test_nostr_transport_with_nprofile_no_relays() {
        // Create a payment request with nostr transport using nprofile with empty relay list
        let pubkey_hex = "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d";
        let pubkey_bytes = hex::decode(pubkey_hex).unwrap();
        let nprofile =
            PaymentRequest::encode_nprofile(&pubkey_bytes, &[]).expect("encode nprofile");

        let transport = Transport {
            _type: TransportType::Nostr,
            target: nprofile.clone(),
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
        assert!(decoded.transports[0].target.starts_with("nprofile"));

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

        // Check that relay was preserved in tags as "r" per NUT-26 spec
        let tags = decoded.transports[0].tags.as_ref().unwrap();
        assert!(tags
            .iter()
            .any(|t| t.len() >= 2 && t[0] == "r" && t[1] == "wss://relay.example.com"));
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
            .any(|t| t.len() >= 2 && t[0] == "r" && t[1] == "wss://relay.damus.io"));
    }

    #[test]
    fn test_decode_valid_bech32_with_nostr_pubkeys_and_mints() {
        // First, create a payment request with multiple mints and nostr transports with different pubkeys
        let pubkey1_hex = "3bf0c63fcb93463407af97a5e5ee64fa883d107ef9e558472c4eb9aaaefa459d";
        let pubkey1_bytes = hex::decode(pubkey1_hex).unwrap();
        // Use nprofile with empty relay list instead of npub
        let nprofile1 =
            PaymentRequest::encode_nprofile(&pubkey1_bytes, &[]).expect("encode nprofile1");

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
            target: nprofile1.clone(),
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

        // Verify first transport (nprofile with no relays)
        let transport1_decoded = &decoded.transports[0];
        assert_eq!(transport1_decoded._type, TransportType::Nostr);
        assert!(transport1_decoded.target.starts_with("nprofile"));

        // Decode the nprofile to verify the pubkey
        let (decoded_pubkey1, decoded_relays1) =
            PaymentRequest::decode_nprofile(&transport1_decoded.target)
                .expect("should decode nprofile");
        assert_eq!(decoded_pubkey1, pubkey1_bytes);
        assert!(decoded_relays1.is_empty());

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
            .any(|t| t.len() >= 2 && t[0] == "r" && t[1] == "wss://relay.damus.io"));
        assert!(tags2
            .iter()
            .any(|t| t.len() >= 2 && t[0] == "r" && t[1] == "wss://nos.lol"));
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
                    "a": "nprofile1qqsgm6qfa3c8dtz2fvzhvfqeacmwm0e50pe3k5tfmvpjjmn0vj7m2tgpz3mhxue69uhhyetvv9ujuerpd46hxtnfduq3wamnwvaz7tmjv4kxz7fw8qenxvewwdcxzcm99uqs6amnwvaz7tmwdaejumr0ds4ljh7n",
                    "g": [["n", "17"]]
                }
            ]
        }"#;

        let expected_encoded = "CREQB1QYQQSC3HVYUNQVFHXCPQQZQQQQQQQQQQQQ9QXQQPQQZSQ9MGW368QUE69UHNSVENXVH8XURPVDJN5VENXVUQWQREQYQQZQQZQQSGM6QFA3C8DTZ2FVZHVFQEACMWM0E50PE3K5TFMVPJJMN0VJ7M2TGRQQZSZMSZXYMSXQQHQ9EPGAMNWVAZ7TMJV4KXZ7FWV3SK6ATN9E5K7QCQRGQHY9MHWDEN5TE0WFJKCCTE9CURXVEN9EEHQCTRV5HSXQQSQ9EQ6AMNWVAZ7TMWDAEJUMR0DSRYDPGF";

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
        assert_eq!(transport.target, "nprofile1qqsgm6qfa3c8dtz2fvzhvfqeacmwm0e50pe3k5tfmvpjjmn0vj7m2tgpz3mhxue69uhhyetvv9ujuerpd46hxtnfduq3wamnwvaz7tmjv4kxz7fw8qenxvewwdcxzcm99uqs6amnwvaz7tmwdaejumr0ds4ljh7n");
        assert_eq!(
            transport.tags,
            Some(vec![vec!["n".to_string(), "17".to_string()]])
        );

        // Test bech32m encoding (CREQ-B format) - this is what NUT-26 is about
        let encoded = payment_request
            .to_bech32_string()
            .expect("Failed to encode to bech32");

        // Verify it starts with CREQB1 (uppercase because we use encode_upper)
        assert!(encoded.starts_with("CREQB1"));

        // Verify exact encoding matches expected
        assert_eq!(encoded, expected_encoded);

        // Test round-trip via bech32 format
        let decoded = PaymentRequest::from_bech32_string(&encoded).unwrap();

        // Verify decoded fields match original
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

        // Test decoding the expected encoded string
        let decoded_from_spec = PaymentRequest::from_bech32_string(expected_encoded).unwrap();
        assert_eq!(decoded_from_spec.payment_id.as_ref().unwrap(), "b7a90176");
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
                    "a": "nprofile1qqsqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq8uzqt",
                    "g": [["n", "17"], ["n", "9735"]]
                }
            ]
        }"#;

        let expected_encoded = "CREQB1QYQQSE3EXFSN2VTZ8QPQQZQQQQQQQQQQQPJQXQQPQQZSQXTGW368QUE69UHK66TWWSCJUETCV9KHQMR99E3K7MG9QQVKSAR5WPEN5TE0D45KUAPJ9EJHSCTDWPKX2TNRDAKSWQPEQYQQZQQZQQSQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQQRQQZSZMSZXYMSXQQ8Q9HQGWFHXV6SCAGZ48";

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
            "nprofile1qqsqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqqq8uzqt"
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

        // Verify exact encoding matches expected
        assert_eq!(encoded, expected_encoded);

        let decoded = PaymentRequest::from_str(&encoded).unwrap();
        assert_eq!(payment_request, decoded);

        // Test decoding the expected encoded string
        let decoded_from_spec = PaymentRequest::from_bech32_string(expected_encoded).unwrap();
        assert_eq!(decoded_from_spec.payment_id.as_ref().unwrap(), "f92a51b8");
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

        let expected_encoded = "CREQB1QYQQSCEEV56R2EPJVYPQQZQQQQQQQQQQQ86QXQQPQQZSQXRGW368QUE69UHK66TWWSHX27RPD4CXCEFWVDHK6ZQQTYQSQQGQQGQYYVPJVVEKYDTZVGERWEFNXCCNGDFHVVUNYEPEXDJRWWRYVSMNXEPNVS6NXDENXGCNZVRZXF3KVEFCVG6NQENZVVCXZCNRXCCN2EFEVVENXVGRQQXSWARFD4JK7AT5QSENVVPS2N5FAS";

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

    #[test]
    fn test_http_post_transport_kind_1() {
        // Test HTTP POST transport (kind=0x01) encoding and decoding
        let json = r#"{
            "i": "http_test",
            "a": 250,
            "u": "sat",
            "m": ["https://mint.example.com"],
            "t": [
                {
                    "t": "post",
                    "a": "https://api.example.com/v1/payment",
                    "g": [["custom", "value1", "value2"]]
                }
            ]
        }"#;

        // Note: The encoded string is generated by our implementation and verified via round-trip
        let expected_encoded = "CREQB1QYQQJ6R5W3C97AR9WD6QYQQGQQQQQQQQQQQ05QCQQYQQ2QQCDP68GURN8GHJ7MTFDE6ZUETCV9KHQMR99E3K7MG8QPQSZQQPQYPQQGNGW368QUE69UHKZURF9EJHSCTDWPKX2TNRDAKJ7A339ACXZ7TDV4H8GQCQZ5RXXATNW3HK6PNKV9K82EF3QEMXZMR4V5EQ9X3SJM";

        // Parse the JSON into a PaymentRequest
        let payment_request: PaymentRequest = serde_json::from_str(json).unwrap();

        let encoded = payment_request
            .to_bech32_string()
            .expect("encoding should work");

        // Verify exact encoding matches expected
        assert_eq!(encoded, expected_encoded);

        // Decode and verify round-trip
        let decoded = PaymentRequest::from_bech32_string(&encoded).expect("decoding should work");

        // Verify transport type is HTTP POST
        assert_eq!(decoded.transports.len(), 1);
        assert_eq!(decoded.transports[0]._type, TransportType::HttpPost);
        assert_eq!(
            decoded.transports[0].target,
            "https://api.example.com/v1/payment"
        );

        // Verify custom tags are preserved
        let tags = decoded.transports[0].tags.as_ref().unwrap();
        assert!(tags
            .iter()
            .any(|t| t.len() >= 3 && t[0] == "custom" && t[1] == "value1" && t[2] == "value2"));

        // Test decoding the expected encoded string
        let decoded_from_spec = PaymentRequest::from_bech32_string(expected_encoded).unwrap();
        assert_eq!(decoded_from_spec.payment_id.as_ref().unwrap(), "http_test");
    }

    #[test]
    fn test_relay_tag_extraction_from_nprofile() {
        // Test that relays are properly extracted from nprofile as "r" tags per NUT-26 spec
        let json = r#"{
            "i": "relay_test",
            "a": 100,
            "u": "sat",
            "m": ["https://mint.example.com"],
            "t": [
                {
                    "t": "nostr",
                    "a": "nprofile1qqsrhuxx8l9ex335q7he0f09aej04zpazpl0ne2cgukyawd24mayt8gprpmhxue69uhhyetvv9unztn90psk6urvv5hxxmmdqyv8wumn8ghj7un9d3shjv3wv4uxzmtsd3jjucm0d5q3samnwvaz7tmjv4kxz7fn9ejhsctdwpkx2tnrdaksxzjpjp"
                }
            ]
        }"#;

        let expected_encoded = "CREQB1QYQQ5UN9D3SHJHM5V4EHGQSQPQQQQQQQQQQQQEQRQQQSQPGQRP58GARSWVAZ7TMDD9H8GTN90PSK6URVV5HXXMMDQUQGZQGQQYQQYQPQ80CVV07TJDRRGPA0J7J7TMNYL2YR6YR7L8J4S3EVF6U64TH6GKWSXQQMQ9EPSAMNWVAZ7TMJV4KXZ7F39EJHSCTDWPKX2TNRDAKSXQQMQ9EPSAMNWVAZ7TMJV4KXZ7FJ9EJHSCTDWPKX2TNRDAKSXQQMQ9EPSAMNWVAZ7TMJV4KXZ7FN9EJHSCTDWPKX2TNRDAKSKRFDAR";

        // Parse the JSON into a PaymentRequest
        let payment_request: PaymentRequest = serde_json::from_str(json).unwrap();

        let encoded = payment_request
            .to_bech32_string()
            .expect("encoding should work");

        // Verify exact encoding matches expected
        assert_eq!(encoded, expected_encoded);

        // Decode and verify round-trip
        let decoded = PaymentRequest::from_bech32_string(&encoded).expect("decoding should work");

        // Verify relays were extracted as "r" tags per NUT-26 spec
        let tags = decoded.transports[0]
            .tags
            .as_ref()
            .expect("should have tags");

        // Check all three relays are present as "r" tags per NUT-26 spec
        let relay_tags: Vec<&Vec<String>> = tags
            .iter()
            .filter(|t| !t.is_empty() && t[0] == "r")
            .collect();
        assert_eq!(relay_tags.len(), 3);

        let relay_values: Vec<&str> = relay_tags
            .iter()
            .filter(|t| t.len() >= 2)
            .map(|t| t[1].as_str())
            .collect();
        // The nprofile has 3 relays embedded - verified by decode
        assert_eq!(relay_values.len(), 3);

        // Verify the nprofile is preserved (relays are encoded back into it)
        assert_eq!(
            "nprofile1qqsrhuxx8l9ex335q7he0f09aej04zpazpl0ne2cgukyawd24mayt8gprpmhxue69uhhyetvv9unztn90psk6urvv5hxxmmdqyv8wumn8ghj7un9d3shjv3wv4uxzmtsd3jjucm0d5q3samnwvaz7tmjv4kxz7fn9ejhsctdwpkx2tnrdaksxzjpjp",
            decoded.transports[0].target
        );

        // Also verify the nprofile contains the relays
        let (_, decoded_relays) =
            PaymentRequest::decode_nprofile(&decoded.transports[0].target).unwrap();
        assert_eq!(decoded_relays.len(), 3);

        // Test decoding the expected encoded string
        let decoded_from_spec = PaymentRequest::from_bech32_string(expected_encoded).unwrap();
        assert_eq!(decoded_from_spec.payment_id.as_ref().unwrap(), "relay_test");
    }

    #[test]
    fn test_description_field() {
        // Test description field (tag 0x06) encoding and decoding
        let expected_encoded = "CREQB1QYQQJER9WD347AR9WD6QYQQGQQQQQQQQQQQXGQCQQYQQ2QQCDP68GURN8GHJ7MTFDE6ZUETCV9KHQMR99E3K7MGXQQV9GETNWSS8QCTED4JKUAPQV3JHXCMJD9C8G6T0DCFLJJRX";

        let payment_request = PaymentRequest {
            payment_id: Some("desc_test".to_string()),
            amount: Some(Amount::from(100)),
            unit: Some(CurrencyUnit::Sat),
            single_use: None,
            mints: Some(vec![MintUrl::from_str("https://mint.example.com").unwrap()]),
            description: Some("Test payment description".to_string()),
            transports: vec![],
            nut10: None,
        };

        let encoded = payment_request
            .to_bech32_string()
            .expect("encoding should work");

        assert_eq!(encoded, expected_encoded);

        // Decode from the expected encoded string
        let decoded =
            PaymentRequest::from_bech32_string(expected_encoded).expect("decoding should work");

        assert_eq!(
            decoded.description,
            Some("Test payment description".to_string())
        );
        assert_eq!(decoded.payment_id, Some("desc_test".to_string()));
        assert_eq!(decoded.amount, Some(Amount::from(100)));
        assert_eq!(decoded.unit, Some(CurrencyUnit::Sat));
    }

    #[test]
    fn test_single_use_field_true() {
        // Test single_use field (tag 0x04) with value true
        let expected_encoded = "CREQB1QYQQ7UMFDENKCE2LW4EK2HM5WF6K2QSQPQQQQQQQQQQQQEQRQQQSQPQQQYQS2QQCDP68GURN8GHJ7MTFDE6ZUETCV9KHQMR99E3K7MGX0AYM7";

        let payment_request = PaymentRequest {
            payment_id: Some("single_use_true".to_string()),
            amount: Some(Amount::from(100)),
            unit: Some(CurrencyUnit::Sat),
            single_use: Some(true),
            mints: Some(vec![MintUrl::from_str("https://mint.example.com").unwrap()]),
            description: None,
            transports: vec![],
            nut10: None,
        };

        let encoded = payment_request
            .to_bech32_string()
            .expect("encoding should work");

        assert_eq!(encoded, expected_encoded);

        // Decode from the expected encoded string
        let decoded =
            PaymentRequest::from_bech32_string(expected_encoded).expect("decoding should work");

        assert_eq!(decoded.single_use, Some(true));
        assert_eq!(decoded.payment_id, Some("single_use_true".to_string()));
    }

    #[test]
    fn test_single_use_field_false() {
        // Test single_use field (tag 0x04) with value false
        let expected_encoded = "CREQB1QYQPQUMFDENKCE2LW4EK2HMXV9K8XEGZQQYQQQQQQQQQQQRYQVQQZQQYQQQSQPGQRP58GARSWVAZ7TMDD9H8GTN90PSK6URVV5HXXMMDQ40L90";

        let payment_request = PaymentRequest {
            payment_id: Some("single_use_false".to_string()),
            amount: Some(Amount::from(100)),
            unit: Some(CurrencyUnit::Sat),
            single_use: Some(false),
            mints: Some(vec![MintUrl::from_str("https://mint.example.com").unwrap()]),
            description: None,
            transports: vec![],
            nut10: None,
        };

        let encoded = payment_request
            .to_bech32_string()
            .expect("encoding should work");

        assert_eq!(encoded, expected_encoded);

        // Decode from the expected encoded string
        let decoded =
            PaymentRequest::from_bech32_string(expected_encoded).expect("decoding should work");

        assert_eq!(decoded.single_use, Some(false));
        assert_eq!(decoded.payment_id, Some("single_use_false".to_string()));
    }

    #[test]
    fn test_unit_msat() {
        // Test msat unit encoding (should be string, not 0x00)
        let expected_encoded = "CREQB1QYQQJATWD9697MTNV96QYQQGQQQQQQQQQQP7SQCQQ3KHXCT5Q5QPS6R5W3C8XW309AKKJMN59EJHSCTDWPKX2TNRDAKSYYMU95";

        let payment_request = PaymentRequest {
            payment_id: Some("unit_msat".to_string()),
            amount: Some(Amount::from(1000)),
            unit: Some(CurrencyUnit::Msat),
            single_use: None,
            mints: Some(vec![MintUrl::from_str("https://mint.example.com").unwrap()]),
            description: None,
            transports: vec![],
            nut10: None,
        };

        let encoded = payment_request
            .to_bech32_string()
            .expect("encoding should work");

        assert_eq!(encoded, expected_encoded);

        // Decode from the expected encoded string
        let decoded =
            PaymentRequest::from_bech32_string(expected_encoded).expect("decoding should work");

        assert_eq!(decoded.unit, Some(CurrencyUnit::Msat));
        assert_eq!(decoded.payment_id, Some("unit_msat".to_string()));
        assert_eq!(decoded.amount, Some(Amount::from(1000)));
    }

    #[test]
    fn test_unit_usd() {
        // Test usd unit encoding (should be string, not 0x00)
        let expected_encoded = "CREQB1QYQQSATWD9697ATNVSPQQZQQQQQQQQQQQ86QXQQRW4EKGPGQRP58GARSWVAZ7TMDD9H8GTN90PSK6URVV5HXXMMDEPCJYC";

        let payment_request = PaymentRequest {
            payment_id: Some("unit_usd".to_string()),
            amount: Some(Amount::from(500)),
            unit: Some(CurrencyUnit::Usd),
            single_use: None,
            mints: Some(vec![MintUrl::from_str("https://mint.example.com").unwrap()]),
            description: None,
            transports: vec![],
            nut10: None,
        };

        let encoded = payment_request
            .to_bech32_string()
            .expect("encoding should work");

        assert_eq!(encoded, expected_encoded);

        // Decode from the expected encoded string
        let decoded =
            PaymentRequest::from_bech32_string(expected_encoded).expect("decoding should work");

        assert_eq!(decoded.unit, Some(CurrencyUnit::Usd));
        assert_eq!(decoded.payment_id, Some("unit_usd".to_string()));
        assert_eq!(decoded.amount, Some(Amount::from(500)));
    }

    #[test]
    fn test_multiple_transports() {
        // Test payment request with multiple transport options (priority order)
        let json = r#"{
            "i": "multi_transport",
            "a": 500,
            "u": "sat",
            "m": ["https://mint.example.com"],
            "d": "Payment with multiple transports",
            "t": [
                {
                    "t": "nostr",
                    "a": "nprofile1qqsrhuxx8l9ex335q7he0f09aej04zpazpl0ne2cgukyawd24mayt8g2lcy6q",
                    "g": [["n", "17"]]
                },
                {
                    "t": "post",
                    "a": "https://api1.example.com/payment"
                },
                {
                    "t": "post",
                    "a": "https://api2.example.com/payment",
                    "g": [["priority", "backup"]]
                }
            ]
        }"#;

        let expected_encoded = "CREQB1QYQQ7MT4D36XJHM5WFSKUUMSDAE8GQSQPQQQQQQQQQQQRAQRQQQSQPGQRP58GARSWVAZ7TMDD9H8GTN90PSK6URVV5HXXMMDQCQZQ5RP09KK2MN5YPMKJARGYPKH2MR5D9CXCEFQW3EXZMNNWPHHYARNQUQZ7QGQQYQQYQPQ80CVV07TJDRRGPA0J7J7TMNYL2YR6YR7L8J4S3EVF6U64TH6GKWSXQQ9Q9HQYVFHQUQZWQGQQYQSYQPQDP68GURN8GHJ7CTSDYCJUETCV9KHQMR99E3K7MF0WPSHJMT9DE6QWQP6QYQQZQGZQQSXSAR5WPEN5TE0V9CXJV3WV4UXZMTSD3JJUCM0D5HHQCTED4JKUAQRQQGQSURJD9HHY6T50YRXYCTRDD6HQTSH7TP";

        // Parse the JSON into a PaymentRequest
        let payment_request: PaymentRequest = serde_json::from_str(json).unwrap();

        let encoded = payment_request
            .to_bech32_string()
            .expect("encoding should work");

        // Verify exact encoding matches expected
        assert_eq!(encoded, expected_encoded);

        // Decode from the encoded string
        let decoded = PaymentRequest::from_bech32_string(&encoded).expect("decoding should work");

        // Verify all three transports are preserved in order
        assert_eq!(decoded.transports.len(), 3);

        // First transport: Nostr
        assert_eq!(decoded.transports[0]._type, TransportType::Nostr);
        assert!(decoded.transports[0].target.starts_with("nprofile"));

        // Second transport: HTTP POST
        assert_eq!(decoded.transports[1]._type, TransportType::HttpPost);
        assert_eq!(
            decoded.transports[1].target,
            "https://api1.example.com/payment"
        );

        // Third transport: HTTP POST with tags
        assert_eq!(decoded.transports[2]._type, TransportType::HttpPost);
        assert_eq!(
            decoded.transports[2].target,
            "https://api2.example.com/payment"
        );
        let tags = decoded.transports[2].tags.as_ref().unwrap();
        assert!(tags
            .iter()
            .any(|t| t.len() >= 2 && t[0] == "priority" && t[1] == "backup"));

        // Test decoding the expected encoded string
        let decoded_from_spec = PaymentRequest::from_bech32_string(expected_encoded).unwrap();
        assert_eq!(
            decoded_from_spec.payment_id.as_ref().unwrap(),
            "multi_transport"
        );
    }

    #[test]
    fn test_minimal_transport_nostr_only_pubkey() {
        // Test minimal Nostr transport with just pubkey (no relays, no tags)
        let json = r#"{
            "i": "minimal_nostr",
            "u": "sat",
            "m": ["https://mint.example.com"],
            "t": [
                {
                    "t": "nostr",
                    "a": "nprofile1qqsrhuxx8l9ex335q7he0f09aej04zpazpl0ne2cgukyawd24mayt8g2lcy6q"
                }
            ]
        }"#;

        let expected_encoded = "CREQB1QYQQ6MTFDE5K6CTVTAHX7UM5WGPSQQGQQ5QPS6R5W3C8XW309AKKJMN59EJHSCTDWPKX2TNRDAKSWQP8QYQQZQQZQQSRHUXX8L9EX335Q7HE0F09AEJ04ZPAZPL0NE2CGUKYAWD24MAYT8G7QNXMQ";

        // Parse the JSON into a PaymentRequest
        let payment_request: PaymentRequest = serde_json::from_str(json).unwrap();

        let encoded = payment_request
            .to_bech32_string()
            .expect("encoding should work");

        // Verify exact encoding matches expected
        assert_eq!(encoded, expected_encoded);

        // Decode from the encoded string
        let decoded = PaymentRequest::from_bech32_string(&encoded).expect("decoding should work");

        assert_eq!(decoded.transports.len(), 1);
        assert_eq!(decoded.transports[0]._type, TransportType::Nostr);
        assert!(decoded.transports[0].target.starts_with("nprofile"));

        // Tags should be None for minimal transport
        assert!(decoded.transports[0].tags.is_none());

        // Test decoding the expected encoded string
        let decoded_from_spec = PaymentRequest::from_bech32_string(expected_encoded).unwrap();
        assert_eq!(
            decoded_from_spec.payment_id.as_ref().unwrap(),
            "minimal_nostr"
        );
    }

    #[test]
    fn test_minimal_transport_http_just_url() {
        // Test minimal HTTP POST transport with just URL (no tags)
        let json = r#"{
            "i": "minimal_http",
            "u": "sat",
            "m": ["https://mint.example.com"],
            "t": [
                {
                    "t": "post",
                    "a": "https://api.example.com"
                }
            ]
        }"#;

        let expected_encoded = "CREQB1QYQQCMTFDE5K6CTVTA58GARSQVQQZQQ9QQVXSAR5WPEN5TE0D45KUAPWV4UXZMTSD3JJUCM0D5RSQ8SPQQQSZQSQZA58GARSWVAZ7TMPWP5JUETCV9KHQMR99E3K7MG0TWYGX";

        // Parse the JSON into a PaymentRequest
        let payment_request: PaymentRequest = serde_json::from_str(json).unwrap();

        let encoded = payment_request
            .to_bech32_string()
            .expect("encoding should work");

        // Verify exact encoding matches expected
        assert_eq!(encoded, expected_encoded);

        // Decode and verify round-trip
        let decoded = PaymentRequest::from_bech32_string(&encoded).expect("decoding should work");

        assert_eq!(decoded.transports.len(), 1);
        assert_eq!(decoded.transports[0]._type, TransportType::HttpPost);
        assert_eq!(decoded.transports[0].target, "https://api.example.com");
        assert!(decoded.transports[0].tags.is_none());

        // Test decoding the expected encoded string
        let decoded_from_spec = PaymentRequest::from_bech32_string(expected_encoded).unwrap();
        assert_eq!(
            decoded_from_spec.payment_id.as_ref().unwrap(),
            "minimal_http"
        );
    }

    #[test]
    fn test_in_band_transport_implicit() {
        // Test in-band transport: absence of transport tag means in-band (NUT-18 semantics)
        // In-band transports are NOT encoded - they're represented by the absence of a transport tag

        let transport = Transport {
            _type: TransportType::InBand,
            target: String::new(), // In-band has no target
            tags: None,
        };

        let payment_request = PaymentRequest {
            payment_id: Some("in_band_test".to_string()),
            amount: Some(Amount::from(100)),
            unit: Some(CurrencyUnit::Sat),
            single_use: None,
            mints: Some(vec![MintUrl::from_str("https://mint.example.com").unwrap()]),
            description: None,
            transports: vec![transport],
            nut10: None,
        };

        let encoded = payment_request
            .to_bech32_string()
            .expect("encoding should work");

        // Decode the encoded string
        let decoded = PaymentRequest::from_bech32_string(&encoded).expect("decoding should work");

        // In-band transports are not encoded, so when decoded, transports should be empty
        // (absence of transport tag = in-band is implicit)
        assert_eq!(decoded.transports.len(), 0);
        assert_eq!(decoded.payment_id, Some("in_band_test".to_string()));
        assert_eq!(decoded.amount, Some(Amount::from(100)));
    }

    #[test]
    fn test_nut10_htlc_kind_1() {
        // Test NUT-10 HTLC (kind=1) encoding and decoding
        let json = r#"{
            "i": "htlc_test",
            "a": 1000,
            "u": "sat",
            "m": ["https://mint.example.com"],
            "d": "HTLC locked payment",
            "nut10": {
                "k": "HTLC",
                "d": "a]0e66820bfb412212cf7ab3deb0459ce282a1b04fda76ea6026a67e41ae26f3dc",
                "t": [
                    ["locktime", "1700000000"],
                    ["refund", "033281c37677ea273eb7183b783067f5244933ef78d8c3f15b1a77cb246099c26e"]
                ]
            }
        }"#;

        // Note: The encoded string is generated by our implementation and verified via round-trip
        let expected_encoded = "CREQB1QYQQJ6R5D3347AR9WD6QYQQGQQQQQQQQQQP7SQCQQYQQ2QQCDP68GURN8GHJ7MTFDE6ZUETCV9KHQMR99E3K7MGXQQF5S4ZVGVSXCMMRDDJKGGRSV9UK6ETWWSYQPTGPQQQSZQSQGFS46VR9XCMRSV3SVFNXYDP3XGERZVNRVCMKZC3NV3JKYVP5X5UKXEFJ8QEXZVTZXQ6XVERPXUMX2CFKXQERVCFKXAJNGVTPV5ERVE3NV33SXQQ5PPKX7CMTW35K6EG2XYMNQVPSXQCRQVPSQVQY5PNJV4N82MNYGGCRXVEJ8QCKXVEHXCMNWETPXGMNXETZXUCNSVMZXUURXVPKXANR2V35XSUNXVM9VCMNSEPCVVEKVVF4VGCKZDEHVD3RYDPKXQUNJCEJXEJS4EHJHC";

        // Parse the JSON into a PaymentRequest
        let payment_request: PaymentRequest = serde_json::from_str(json).unwrap();

        let encoded = payment_request
            .to_bech32_string()
            .expect("encoding should work");

        // Verify exact encoding matches expected
        assert_eq!(encoded, expected_encoded);

        // Decode from the encoded string and verify round-trip
        let decoded =
            PaymentRequest::from_bech32_string(&expected_encoded).expect("decoding should work");

        // Verify all top-level fields
        assert_eq!(decoded.payment_id, Some("htlc_test".to_string()));
        assert_eq!(decoded.amount, Some(Amount::from(1000)));
        assert_eq!(decoded.unit, Some(CurrencyUnit::Sat));
        assert_eq!(
            decoded.mints,
            Some(vec![MintUrl::from_str("https://mint.example.com").unwrap()])
        );
        assert_eq!(decoded.description, Some("HTLC locked payment".to_string()));

        // Verify NUT-10 fields
        let nut10 = decoded.nut10.as_ref().unwrap();
        assert_eq!(nut10.kind, Kind::HTLC);
        assert_eq!(
            nut10.data,
            "a]0e66820bfb412212cf7ab3deb0459ce282a1b04fda76ea6026a67e41ae26f3dc"
        );

        // Verify all tags with exact values
        let tags = nut10.tags.as_ref().unwrap();
        assert_eq!(tags.len(), 2);
        assert_eq!(
            tags[0],
            vec!["locktime".to_string(), "1700000000".to_string()]
        );
        assert_eq!(
            tags[1],
            vec![
                "refund".to_string(),
                "033281c37677ea273eb7183b783067f5244933ef78d8c3f15b1a77cb246099c26e".to_string()
            ]
        );
    }

    #[test]
    fn test_case_insensitive_decoding() {
        // Test that decoder accepts both lowercase and uppercase input
        // Note: Per BIP-173/BIP-350, mixed-case is NOT valid for bech32/bech32m
        // "Decoders MUST NOT accept strings where some characters are uppercase and some are lowercase"
        let payment_request = PaymentRequest {
            payment_id: Some("case_test".to_string()),
            amount: Some(Amount::from(100)),
            unit: Some(CurrencyUnit::Sat),
            single_use: None,
            mints: Some(vec![MintUrl::from_str("https://mint.example.com").unwrap()]),
            description: None,
            transports: vec![],
            nut10: None,
        };

        let uppercase = payment_request
            .to_bech32_string()
            .expect("encoding should work");

        // Convert to lowercase
        let lowercase = uppercase.to_lowercase();

        // Both uppercase and lowercase should decode successfully
        let decoded_upper =
            PaymentRequest::from_bech32_string(&uppercase).expect("uppercase should decode");
        let decoded_lower =
            PaymentRequest::from_bech32_string(&lowercase).expect("lowercase should decode");

        // Both should produce the same result
        assert_eq!(decoded_upper.payment_id, Some("case_test".to_string()));
        assert_eq!(decoded_lower.payment_id, Some("case_test".to_string()));

        assert_eq!(decoded_upper.amount, decoded_lower.amount);
        assert_eq!(decoded_upper.unit, decoded_lower.unit);
    }

    #[test]
    fn test_custom_currency_unit() {
        // Test that custom/unknown currency units are preserved
        let expected_encoded = "CREQB1QYQQKCM4WD6X7M2LW4HXJAQZQQYQQQQQQQQQQQRYQVQQXCN5VVZSQXRGW368QUE69UHK66TWWSHX27RPD4CXCEFWVDHK6PZHCW8";

        let payment_request = PaymentRequest {
            payment_id: Some("custom_unit".to_string()),
            amount: Some(Amount::from(100)),
            unit: Some(CurrencyUnit::Custom("btc".to_string())),
            single_use: None,
            mints: Some(vec![MintUrl::from_str("https://mint.example.com").unwrap()]),
            description: None,
            transports: vec![],
            nut10: None,
        };

        let encoded = payment_request
            .to_bech32_string()
            .expect("encoding should work");

        assert_eq!(encoded, expected_encoded);

        // Decode from the expected encoded string
        let decoded =
            PaymentRequest::from_bech32_string(expected_encoded).expect("decoding should work");

        assert_eq!(decoded.unit, Some(CurrencyUnit::Custom("btc".to_string())));
        assert_eq!(decoded.payment_id, Some("custom_unit".to_string()));
    }
}
