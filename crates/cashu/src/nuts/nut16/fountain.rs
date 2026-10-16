//! NUT-16 binary fountain frames, sent as Base45 text in QR alphanumeric mode

use std::fmt;

use bitcoin::base64::engine::{general_purpose, GeneralPurpose};
use bitcoin::base64::{alphabet, Engine};

use super::Error;
use crate::nuts::nut00::{self, Token, TokenV4};

/// Default fragment size in bytes
///
/// A 207-byte frame is 311 Base45 characters, the alphanumeric capacity of a
/// version 10-M QR symbol.
pub const DEFAULT_FRAGMENT_SIZE: usize = 183;

/// `NF` magic, version 1, flags 0
const FRAME_PREFIX: [u8; 4] = [0x4e, 0x46, 0x01, 0x00];
const HEADER_LENGTH: usize = 20;
const FRAME_OVERHEAD: usize = 24;
const MAX_FRAGMENT_SIZE: usize = 4096;
const MAX_FRAGMENTS: usize = 1024;
const MAX_MESSAGE_LENGTH: usize = 1024 * 1024;
/// QR alphanumeric mode's 45 characters, in RFC 9285 order
const BASE45: &[u8; 45] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ $%*+-./:";
/// Base45 of the `NF` magic, whatever the version and flags
const TEXT_MAGIC: &str = "D+9";
/// Base45 lengths of the 25- and 4120-byte frame bounds
const MIN_TEXT_LENGTH: usize = 38;
const MAX_TEXT_LENGTH: usize = 6180;

/// CRC-32/ISO-HDLC lookup table
const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 == 1 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
};

fn crc32(bytes: &[u8]) -> u32 {
    !bytes.iter().fold(!0u32, |crc, &b| {
        CRC_TABLE[((crc ^ u32::from(b)) & 0xff) as usize] ^ (crc >> 8)
    })
}

/// NUT-16 `SELECT(q, N)`: the source fragments frame `sequence` combines, as a bitset
fn select_fragments(sequence: u32, count: usize) -> Vec<u64> {
    let mut bits = vec![0u64; count.div_ceil(64)];
    let index = sequence as usize - 1;
    if index < count {
        set_bit(&mut bits, index);
        return bits;
    }
    let mut state = sequence;
    let mut selected = false;
    for i in 0..count {
        state = state.wrapping_add(0x6d2b_79f5);
        let mut a = (state ^ (state >> 15)).wrapping_mul(state | 1);
        let b = (a ^ (a >> 7)).wrapping_mul(a | 61);
        a ^= a.wrapping_add(b);
        if (a ^ (a >> 14)) & 1 == 1 {
            set_bit(&mut bits, i);
            selected = true;
        }
    }
    if !selected {
        set_bit(&mut bits, index % count);
    }
    bits
}

fn set_bit(bits: &mut [u64], index: usize) {
    bits[index / 64] |= 1 << (index % 64);
}

fn has_bit(bits: &[u64], index: usize) -> bool {
    bits[index / 64] & (1 << (index % 64)) != 0
}

fn lowest_bit(bits: &[u64]) -> Option<usize> {
    bits.iter()
        .enumerate()
        .find(|(_, word)| **word != 0)
        .map(|(i, word)| i * 64 + word.trailing_zeros() as usize)
}

fn xor_into<T>(target: &mut [T], source: &[T])
where
    T: Copy + std::ops::BitXorAssign,
{
    for (t, s) in target.iter_mut().zip(source) {
        *t ^= *s;
    }
}

/// RFC 9285 Base45: two bytes to three QR alphanumeric characters
fn encode_base45(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(2) * 3);
    for chunk in bytes.chunks(2) {
        let mut n = chunk.iter().fold(0usize, |n, &b| n * 256 + usize::from(b));
        for _ in 0..=chunk.len() {
            out.push(char::from(BASE45[n % 45]));
            n /= 45;
        }
    }
    out
}

/// RFC 9285 Base45 decode, rejecting bad characters, lengths and out-of-range groups
fn decode_base45(text: &str) -> Result<Vec<u8>, Error> {
    let text = text.as_bytes();
    if text.len() % 3 == 1 {
        return Err(Error::InvalidFountainText("bad Base45 length"));
    }
    let overflow = |_| Error::InvalidFountainText("Base45 group out of range");
    let mut out = Vec::with_capacity(text.len() / 3 * 2 + 1);
    for group in text.chunks(3) {
        let mut n = 0usize;
        for c in group.iter().rev() {
            let value = BASE45
                .iter()
                .position(|b| b == c)
                .ok_or(Error::InvalidFountainText("unexpected Base45 character"))?;
            n = n * 45 + value;
        }
        match group.len() {
            3 => out.extend_from_slice(&u16::try_from(n).map_err(overflow)?.to_be_bytes()),
            _ => out.push(u8::try_from(n).map_err(overflow)?),
        }
    }
    Ok(out)
}

/// Dimensions every frame of one transfer shares
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Transfer {
    count: usize,
    length: usize,
    checksum: u32,
    size: usize,
}

fn read_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]])
}

/// Validates a frame and returns its sequence, transfer and payload
///
/// Nothing is allocated from header values until every check has passed.
fn parse_frame(frame: &[u8]) -> Result<(u32, Transfer, &[u8]), Error> {
    match frame.get(..4) {
        Some(prefix) if prefix == FRAME_PREFIX => {}
        Some(prefix) if prefix[..2] == FRAME_PREFIX[..2] => {
            return Err(Error::UnsupportedFountainFrame)
        }
        _ => return Err(Error::InvalidFountainFrame("not an NF frame")),
    }
    let size = frame
        .len()
        .checked_sub(FRAME_OVERHEAD)
        .filter(|size| (1..=MAX_FRAGMENT_SIZE).contains(size))
        .ok_or(Error::InvalidFountainFrame("frame length out of range"))?;
    let (body, checksum) = frame.split_at(HEADER_LENGTH + size);
    if crc32(body) != read_u32(checksum, 0) {
        return Err(Error::InvalidFountainFrame("frame checksum mismatch"));
    }
    let sequence = read_u32(body, 4);
    let transfer = Transfer {
        count: read_u32(body, 8) as usize,
        length: read_u32(body, 12) as usize,
        checksum: read_u32(body, 16),
        size,
    };
    if sequence == 0 {
        return Err(Error::InvalidFountainFrame("sequence 0"));
    }
    if !(1..=MAX_FRAGMENTS).contains(&transfer.count) {
        return Err(Error::InvalidFountainFrame("fragment count out of range"));
    }
    if transfer.length > MAX_MESSAGE_LENGTH {
        return Err(Error::InvalidFountainFrame("message length out of range"));
    }
    if transfer.count != transfer.length.div_ceil(size).max(1) {
        return Err(Error::InvalidFountainFrame(
            "fragment count does not match length and size",
        ));
    }
    Ok((sequence, transfer, &body[HEADER_LENGTH..]))
}

/// Splits a message into NUT-16 fountain frames for an animated QR code
///
/// Each [`next_frame`](Self::next_frame) is Base45 text for one alphanumeric
/// QR symbol. The first [`fragment_count`](Self::fragment_count) frames carry
/// the message in order; later ones mix fragments so a receiver can fill gaps.
pub struct FountainEncoder {
    fragments: Vec<u8>,
    header: [u8; HEADER_LENGTH],
    fragment_size: usize,
    fragment_count: usize,
    sequence: u32,
}

impl FountainEncoder {
    /// Creates an encoder for raw bytes; use [`Token::fountain_encoder`] for a token
    ///
    /// `fragment_size` is payload bytes per frame, 1 to 4096; each frame is
    /// `fragment_size + 24` bytes, 1.5 Base45 characters per byte.
    ///
    /// # Errors
    ///
    /// Returns an error if the message exceeds 1 MiB or needs more than 1024
    /// fragments at this size.
    pub fn new(message: &[u8], fragment_size: usize) -> Result<Self, Error> {
        if !(1..=MAX_FRAGMENT_SIZE).contains(&fragment_size) {
            return Err(Error::FountainEncoding(
                "fragment size must be 1 to 4096 bytes",
            ));
        }
        if message.len() > MAX_MESSAGE_LENGTH {
            return Err(Error::FountainEncoding("message exceeds 1 MiB"));
        }
        let fragment_count = message.len().div_ceil(fragment_size).max(1);
        if fragment_count > MAX_FRAGMENTS {
            return Err(Error::FountainEncoding(
                "message needs more than 1024 fragments; use a larger fragment size",
            ));
        }
        let mut fragments = vec![0; fragment_count * fragment_size];
        fragments[..message.len()].copy_from_slice(message);
        let mut header = [0; HEADER_LENGTH];
        header[..4].copy_from_slice(&FRAME_PREFIX);
        header[8..12].copy_from_slice(&(fragment_count as u32).to_be_bytes());
        header[12..16].copy_from_slice(&(message.len() as u32).to_be_bytes());
        header[16..20].copy_from_slice(&crc32(message).to_be_bytes());
        Ok(Self {
            fragments,
            header,
            fragment_size,
            fragment_count,
            sequence: 0,
        })
    }

    /// Creates an encoder for a `cashuB` string, keeping its CBOR bytes as NUT-16 requires
    ///
    /// # Errors
    ///
    /// Returns an error if `token` is not a valid V4 token string.
    pub fn from_token_str(token: &str, fragment_size: usize) -> Result<Self, Error> {
        let body = token
            .strip_prefix("cashuB")
            .ok_or(nut00::Error::UnsupportedToken)?;
        let config = general_purpose::GeneralPurposeConfig::new()
            .with_decode_padding_mode(bitcoin::base64::engine::DecodePaddingMode::Indifferent);
        let mut message = b"crawB".to_vec();
        message.extend(
            GeneralPurpose::new(&alphabet::URL_SAFE, config)
                .decode(body)
                .map_err(nut00::Error::from)?,
        );
        TokenV4::try_from(&message)?;
        Self::new(&message, fragment_size)
    }

    /// Returns the next frame as QR text; loop the display until the receiver completes
    ///
    /// # Errors
    ///
    /// Returns an error after 2^32 - 1 frames; start a new encoder.
    pub fn next_frame(&mut self) -> Result<String, Error> {
        self.sequence = self.sequence.checked_add(1).ok_or(Error::FountainEncoding(
            "sequence exhausted; start a new encoder",
        ))?;
        let size = self.fragment_size;
        let mut frame = Vec::with_capacity(size + FRAME_OVERHEAD);
        frame.extend_from_slice(&self.header);
        frame[4..8].copy_from_slice(&self.sequence.to_be_bytes());
        frame.resize(HEADER_LENGTH + size, 0);
        let selected = select_fragments(self.sequence, self.fragment_count);
        for (i, fragment) in self.fragments.chunks_exact(size).enumerate() {
            if has_bit(&selected, i) {
                xor_into(&mut frame[HEADER_LENGTH..], fragment);
            }
        }
        let checksum = crc32(&frame);
        frame.extend_from_slice(&checksum.to_be_bytes());
        Ok(encode_base45(&frame))
    }

    /// Returns the number of source fragments; a receiver needs at least this many frames
    pub fn fragment_count(&self) -> usize {
        self.fragment_count
    }

    /// Returns whether the message fits one frame, which may be shown as a static QR
    pub fn is_single_fragment(&self) -> bool {
        self.fragment_count == 1
    }
}

impl fmt::Debug for FountainEncoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FountainEncoder")
            .field("sequence", &self.sequence)
            .field("fragment_count", &self.fragment_count)
            .field("fragment_size", &self.fragment_size)
            .finish()
    }
}

impl Token {
    /// Creates a [`FountainEncoder`] for this token's raw V4 bytes (`crawB`)
    ///
    /// V3 tokens are converted to V4. To keep a `cashuB` string's exact bytes,
    /// use [`FountainEncoder::from_token_str`].
    ///
    /// # Errors
    ///
    /// Returns an error if the token cannot be serialized or framed.
    pub fn fountain_encoder(&self, fragment_size: usize) -> Result<FountainEncoder, Error> {
        let raw = match self {
            Self::TokenV3(token) => TokenV4::try_from(token.clone())?.to_raw_bytes()?,
            Self::TokenV4(token) => token.to_raw_bytes()?,
        };
        FountainEncoder::new(&raw, fragment_size)
    }
}

/// Reassembles a message from NUT-16 fountain frames, in any order and with gaps
///
/// A reassembly that fails verification resets the decoder.
#[derive(Default)]
pub struct FountainDecoder {
    transfer: Option<Transfer>,
    /// Reduced equations indexed by pivot; a row has bits only at or above its pivot
    rows: Vec<Option<(Vec<u64>, Vec<u8>)>>,
    rank: usize,
    message: Option<Vec<u8>>,
}

impl FountainDecoder {
    /// Returns whether scanned QR text must go to this decoder rather than a token or UR parser
    ///
    /// True for any text starting `D+9`, even if [`receive`](Self::receive)
    /// then rejects it (eg an unsupported version).
    pub fn is_frame(text: &str) -> bool {
        text.starts_with(TEXT_MAGIC)
    }

    /// Adds one frame, as the exact scanned QR text (no trimming or case changes)
    ///
    /// Returns whether the frame added information; duplicates, redundant
    /// frames and frames after completion return `false`.
    ///
    /// # Errors
    ///
    /// Returns an error if the frame is invalid, belongs to another transfer
    /// (call [`reset`](Self::reset) to switch), or completes a message that
    /// fails verification.
    pub fn receive(&mut self, text: &str) -> Result<bool, Error> {
        if !(MIN_TEXT_LENGTH..=MAX_TEXT_LENGTH).contains(&text.len()) {
            return Err(Error::InvalidFountainText("text length out of range"));
        }
        let frame = decode_base45(text)?;
        let (sequence, transfer, payload) = parse_frame(&frame)?;
        match self.transfer {
            Some(active) if active != transfer => return Err(Error::FountainTransferMismatch),
            Some(_) => {}
            None => {
                self.transfer = Some(transfer);
                self.rows = vec![None; transfer.count];
            }
        }
        if self.message.is_some() {
            return Ok(false);
        }
        let mut coefficients = select_fragments(sequence, transfer.count);
        let mut data = payload.to_vec();
        while let Some(pivot) = lowest_bit(&coefficients) {
            match &self.rows[pivot] {
                Some((row_coefficients, row_data)) => {
                    xor_into(&mut coefficients, row_coefficients);
                    xor_into(&mut data, row_data);
                }
                None => {
                    self.rows[pivot] = Some((coefficients, data));
                    self.rank += 1;
                    if self.rank == transfer.count {
                        self.solve(transfer)?;
                    }
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn solve(&mut self, transfer: Transfer) -> Result<(), Error> {
        let Transfer {
            count,
            length,
            checksum,
            size,
        } = transfer;
        let mut fragments = vec![0u8; count * size];
        for i in (0..count).rev() {
            let Some((coefficients, data)) = &self.rows[i] else {
                continue;
            };
            let (head, solved) = fragments.split_at_mut((i + 1) * size);
            let fragment = &mut head[i * size..];
            fragment.copy_from_slice(data);
            for (j, source) in solved.chunks_exact(size).enumerate() {
                if has_bit(coefficients, i + 1 + j) {
                    xor_into(fragment, source);
                }
            }
        }
        if crc32(&fragments[..length]) != checksum || fragments[length..].iter().any(|&b| b != 0) {
            self.reset();
            return Err(Error::FountainVerificationFailed);
        }
        fragments.truncate(length);
        self.message = Some(fragments);
        self.rows = Vec::new();
        Ok(())
    }

    /// Returns whether the message is reassembled and verified
    pub fn complete(&self) -> bool {
        self.message.is_some()
    }

    /// Returns the reassembled message once [`complete`](Self::complete)
    pub fn message(&self) -> Option<&[u8]> {
        self.message.as_deref()
    }

    /// Returns the reassembled [`Token`] once [`complete`](Self::complete)
    ///
    /// # Errors
    ///
    /// Returns an error if the message is not a `crawB` V4 token.
    pub fn token(&self) -> Result<Option<Token>, Error> {
        Ok(self.message.as_ref().map(Token::try_from).transpose()?)
    }

    /// Returns the number of source fragments, `0` before the first frame
    pub fn fragment_count(&self) -> usize {
        self.transfer.map_or(0, |transfer| transfer.count)
    }

    /// Returns the number of fragments resolved so far, for progress indication
    pub fn resolved_fragment_count(&self) -> usize {
        self.rank
    }

    /// Discards all frames, ready for a new transfer
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

impl fmt::Debug for FountainDecoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FountainDecoder")
            .field("complete", &self.complete())
            .field("fragment_count", &self.fragment_count())
            .field("resolved_fragment_count", &self.rank)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use serde_json::Value;

    use super::*;
    use crate::util::hex;

    const VECTORS: &str = include_str!("vectors.json");

    fn vectors() -> Value {
        serde_json::from_str(VECTORS).expect("valid vectors")
    }

    fn str_of<'a>(v: &'a Value, key: &str) -> &'a str {
        v[key].as_str().expect("string field")
    }

    fn usize_of(v: &Value, key: &str) -> usize {
        v[key].as_u64().expect("integer field") as usize
    }

    fn bytes_of(v: &Value, key: &str) -> Vec<u8> {
        hex::decode(str_of(v, key)).expect("hex field")
    }

    fn indexes(bits: &[u64], count: usize) -> Vec<u64> {
        (0..count)
            .filter(|&i| has_bit(bits, i))
            .map(|i| i as u64)
            .collect()
    }

    fn expected_indexes(v: &Value) -> Vec<u64> {
        v["indexes"]
            .as_array()
            .expect("indexes")
            .iter()
            .map(|i| i.as_u64().expect("index"))
            .collect()
    }

    fn decode_all<'a, I>(frames: I) -> FountainDecoder
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut decoder = FountainDecoder::default();
        for frame in frames {
            decoder.receive(frame).expect("valid frame");
        }
        decoder
    }

    fn frame_text(transfer: &Value, sequence: u64) -> &str {
        let frame = transfer["frames"]
            .as_array()
            .expect("frames")
            .iter()
            .find(|f| f["sequence"].as_u64() == Some(sequence))
            .expect("listed sequence");
        str_of(frame, "qr_text")
    }

    fn transfer_named(vectors: &Value, name: &str) -> Value {
        vectors["transfers"]
            .as_array()
            .expect("transfers")
            .iter()
            .find(|t| t["name"] == name)
            .expect("named transfer")
            .clone()
    }

    #[test]
    fn crc32_check_values() {
        let v = vectors();
        let check = str_of(&v["checksum_check"], "input_ascii").as_bytes();
        assert_eq!(
            format!("{:08x}", crc32(check)),
            str_of(&v["checksum_check"], "crc32")
        );
        assert_eq!(crc32(&[]), 0);
    }

    #[test]
    fn selection_vectors() {
        for s in vectors()["selection_vectors"].as_array().expect("array") {
            let count = usize_of(s, "fragment_count");
            let sequence = s["sequence"].as_u64().expect("sequence") as u32;
            assert_eq!(
                indexes(&select_fragments(sequence, count), count),
                expected_indexes(s)
            );
        }
    }

    #[test]
    fn base45_vectors() {
        for b in vectors()["base45_vectors"].as_array().expect("array") {
            let bytes = bytes_of(b, "bytes_hex");
            assert_eq!(encode_base45(&bytes), str_of(b, "text"));
            assert_eq!(decode_base45(str_of(b, "text")).expect("valid"), bytes);
        }
    }

    #[test]
    fn transfer_vectors() {
        for t in vectors()["transfers"].as_array().expect("array") {
            let name = str_of(t, "name");
            let message = bytes_of(t, "message_hex");
            let count = usize_of(t, "fragment_count");
            let frames = t["frames"].as_array().expect("frames");

            let mut encoder =
                FountainEncoder::new(&message, usize_of(t, "fragment_size")).expect("encoder");
            assert_eq!(encoder.fragment_count(), count, "{name}");
            for f in frames {
                let sequence = f["sequence"].as_u64().expect("sequence") as u32;
                // The last vector is sequence 0xffffffff; jump the counter to it.
                encoder.sequence = sequence - 1;
                let text = encoder.next_frame().expect("frame");
                assert_eq!(text, str_of(f, "qr_text"), "{name} q={sequence}");

                let frame = decode_base45(&text).expect("base45");
                assert_eq!(frame, bytes_of(f, "frame_hex"));
                let (q, _, payload) = parse_frame(&frame).expect("valid frame");
                assert_eq!(q, sequence);
                assert_eq!(payload, bytes_of(f, "data_hex").as_slice());
                assert_eq!(
                    indexes(&select_fragments(q, count), count),
                    expected_indexes(f)
                );
            }

            let systematic = frames
                .iter()
                .filter(|f| f["sequence"].as_u64().expect("sequence") <= count as u64)
                .map(|f| str_of(f, "qr_text"));
            assert_eq!(decode_all(systematic).message(), Some(message.as_slice()));

            let repair = t["mixed_only_recovery_sequences"]
                .as_array()
                .expect("sequences")
                .iter()
                .map(|q| frame_text(t, q.as_u64().expect("sequence")));
            assert_eq!(decode_all(repair).message(), Some(message.as_slice()));

            let texts: Vec<&str> = frames.iter().map(|f| str_of(f, "qr_text")).collect();
            let shuffled = texts.iter().rev().chain(&texts).copied();
            assert_eq!(decode_all(shuffled).message(), Some(message.as_slice()));

            let is_token = Token::try_from(&message).is_ok();
            assert_eq!(is_token, t["scope"] == "token", "{name}");
        }
    }

    #[test]
    fn valid_frame_vectors() {
        for f in vectors()["valid_frames"].as_array().expect("array") {
            let frame = bytes_of(f, "frame_hex");
            assert_eq!(encode_base45(&frame), str_of(f, "qr_text"));
            let (sequence, transfer, payload) = parse_frame(&frame).expect("valid frame");
            assert_eq!(
                u64::from(sequence),
                f["sequence"].as_u64().expect("sequence")
            );
            assert_eq!(transfer.count, usize_of(f, "fragment_count"));
            assert_eq!(transfer.length, usize_of(f, "message_length"));
            assert_eq!(transfer.size, usize_of(f, "fragment_size"));
            assert_eq!(
                format!("{:08x}", transfer.checksum),
                str_of(f, "message_crc32")
            );
            assert_eq!(payload, bytes_of(f, "data_hex").as_slice());
            assert!(FountainDecoder::default()
                .receive(str_of(f, "qr_text"))
                .expect("accepted"));
        }
    }

    #[test]
    fn invalid_frame_vectors() {
        for f in vectors()["invalid_frames"].as_array().expect("array") {
            let name = str_of(f, "name");
            let mut decoder = FountainDecoder::default();
            let result = decoder.receive(str_of(f, "qr_text"));
            match str_of(f, "reject_at") {
                "token" => {
                    assert!(decoder.complete(), "{name}");
                    assert!(decoder.token().is_err(), "{name}");
                }
                reject_at => {
                    if reject_at == "frame" {
                        assert!(parse_frame(&bytes_of(f, "frame_hex")).is_err(), "{name}");
                    }
                    assert!(result.is_err(), "{name}");
                    assert_eq!(decoder.resolved_fragment_count(), 0, "{name}");
                    assert_eq!(decoder.fragment_count(), 0, "{name}");
                }
            }
        }
    }

    #[test]
    fn invalid_qr_text_vectors() {
        for q in vectors()["invalid_qr_text"].as_array().expect("array") {
            let mut decoder = FountainDecoder::default();
            assert!(decoder.receive(str_of(q, "text")).is_err());
            assert_eq!(decoder.fragment_count(), 0);
        }
    }

    #[test]
    fn routing_vectors() {
        for r in vectors()["routing_vectors"].as_array().expect("array") {
            assert_eq!(
                FountainDecoder::is_frame(str_of(r, "text")),
                r["parser"] == "fountain"
            );
        }
    }

    #[test]
    fn receiver_sequences() {
        for s in vectors()["receiver_sequences"].as_array().expect("array") {
            let mut decoder = FountainDecoder::default();
            for step in s["steps"].as_array().expect("steps") {
                if step["action"] == "reset" {
                    decoder.reset();
                    continue;
                }
                let result = decoder.receive(str_of(step, "qr_text"));
                match str_of(step, "result") {
                    "reject" => assert!(result.is_err()),
                    outcome => {
                        result.expect("accepted");
                        assert_eq!(decoder.complete(), outcome == "complete");
                    }
                }
            }
            assert_eq!(
                decoder.message().map(hex::encode),
                Some(str_of(s, "message_hex").to_string())
            );
        }
    }

    #[test]
    fn default_fragment_size_fits_version_10_m() {
        let mut encoder = FountainEncoder::new(&[0; 500], DEFAULT_FRAGMENT_SIZE).expect("encoder");
        assert_eq!(encoder.next_frame().expect("frame").len(), 311);
    }

    #[test]
    fn encoder_rejects_out_of_bounds_input() {
        assert!(FountainEncoder::new(&[0], 0).is_err());
        assert!(FountainEncoder::new(&[0], 4097).is_err());
        assert!(FountainEncoder::new(&vec![0; MAX_MESSAGE_LENGTH + 1], 4096).is_err());
        assert!(FountainEncoder::new(&[0; 1025], 1).is_err());
    }

    #[test]
    fn encoder_stops_at_the_last_sequence() {
        let mut encoder = FountainEncoder::new(&[0; 4], 1).expect("encoder");
        encoder.sequence = u32::MAX - 1;
        let frame = decode_base45(&encoder.next_frame().expect("frame")).expect("base45");
        assert_eq!(parse_frame(&frame).expect("valid").0, u32::MAX);
        assert!(encoder.next_frame().is_err());
    }

    #[test]
    fn token_str_keeps_its_bytes() {
        let v = vectors();
        let t = transfer_named(&v, "synthetic-v4-token");
        let mut encoder = FountainEncoder::from_token_str(
            str_of(&t, "token_text"),
            usize_of(&t, "fragment_size"),
        )
        .expect("encoder");
        assert_eq!(encoder.next_frame().expect("frame"), frame_text(&t, 1));
        assert!(FountainEncoder::from_token_str("cashuAeyJ0b2tlbiI6W119", 64).is_err());
        assert!(FountainEncoder::from_token_str("cashuBnotatoken", 64).is_err());
    }

    #[test]
    fn token_roundtrip() {
        let v = vectors();
        let t = transfer_named(&v, "synthetic-v4-token");
        let token = Token::from_str(str_of(&t, "token_text")).expect("token");
        let mut encoder = token.fountain_encoder(32).expect("encoder");
        let mut decoder = FountainDecoder::default();
        while !decoder.complete() {
            decoder
                .receive(&encoder.next_frame().expect("frame"))
                .expect("receive");
        }
        assert_eq!(decoder.token().expect("token"), Some(token));
    }

    #[test]
    fn decoder_progress_duplicates_and_completion() {
        let v = vectors();
        let t = transfer_named(&v, "padded-multipart");
        let mut decoder = FountainDecoder::default();
        assert_eq!(decoder.fragment_count(), 0);
        assert!(decoder.receive(frame_text(&t, 1)).expect("frame"));
        assert!(!decoder.receive(frame_text(&t, 1)).expect("duplicate"));
        assert_eq!(
            (decoder.resolved_fragment_count(), decoder.fragment_count()),
            (1, 4)
        );
        for q in 2..=4 {
            decoder.receive(frame_text(&t, q)).expect("frame");
        }
        assert!(decoder.complete());
        assert!(!decoder
            .receive(frame_text(&t, 5))
            .expect("after completion"));
    }

    #[test]
    fn decoder_refuses_another_transfer_until_reset() {
        let v = vectors();
        let active = transfer_named(&v, "padded-multipart");
        let other = transfer_named(&v, "canonical-four-fragments");
        let mut decoder = FountainDecoder::default();
        decoder.receive(frame_text(&active, 1)).expect("frame");
        assert!(matches!(
            decoder.receive(frame_text(&other, 1)),
            Err(Error::FountainTransferMismatch)
        ));
        decoder.reset();
        assert!(decoder.receive(frame_text(&other, 1)).expect("frame"));
    }

    #[test]
    fn decoder_resets_after_failed_verification() {
        let v = vectors();
        let bad = v["invalid_frames"]
            .as_array()
            .expect("array")
            .iter()
            .find(|f| f["name"] == "valid-frame-crc-wrong-message-crc")
            .expect("vector");
        let mut decoder = FountainDecoder::default();
        assert!(matches!(
            decoder.receive(str_of(bad, "qr_text")),
            Err(Error::FountainVerificationFailed)
        ));
        assert_eq!(decoder.fragment_count(), 0);
        let t = transfer_named(&v, "padded-multipart");
        assert!(decoder.receive(frame_text(&t, 1)).expect("frame"));
    }

    #[test]
    fn recovers_1024_fragments_from_repair_frames_alone() {
        let message: Vec<u8> = (0..1024 * 16 - 7)
            .map(|i: usize| (i * 131 + 7) as u8)
            .collect();
        let mut encoder = FountainEncoder::new(&message, 16).expect("encoder");
        encoder.sequence = 1024;
        let mut decoder = FountainDecoder::default();
        for _ in 0..1100 {
            if decoder.complete() {
                break;
            }
            decoder
                .receive(&encoder.next_frame().expect("frame"))
                .expect("receive");
        }
        assert_eq!(decoder.message(), Some(message.as_slice()));
    }
}
