//! Golomb-Rice coded sets.
//!
//! Follows the construction of [BIP-158] with two departures that cut the
//! primitives needed: elements are already SHA-256 digests over a
//! domain-separated preimage, so there is no SipHash step, and `M` is fixed to
//! `2^P` rather than `1.497137 * 2^P`, which makes this Rice coding with a
//! single parameter.
//!
//! [BIP-158]: https://github.com/bitcoin/bips/blob/master/bip-0158.mediawiki

use super::element::FilterElement;
use super::Error;

/// Smallest permitted Golomb-Rice parameter.
///
/// Decoding stops when fewer than `P + 1` bits remain, and a code is at least
/// `P + 1` bits long. Padding is at most 7 bits, so `P` below 7 would let
/// padding complete a code and inflate `N`.
pub const MIN_P: u8 = 7;

/// Largest Golomb-Rice parameter this implementation accepts.
///
/// Bounded below 64 so that `1 << p` and `64 - p` stay well defined. Real
/// filters use 28; anything near this bound is already absurd.
pub const MAX_P: u8 = 63;

fn check_p(p: u8) -> Result<(), Error> {
    if !(MIN_P..=MAX_P).contains(&p) {
        return Err(Error::InvalidParameter(p));
    }
    Ok(())
}

/// Position of an element in a filter holding `n` distinct elements.
///
/// The spec defines this as `(v * n * 2^p) >> 64`. Dividing by `2^64` after
/// multiplying by `2^p` is the same floor as shifting right by `64 - p`, which
/// keeps the product inside a `u128` for every `p` this module accepts.
fn position(element: &[u8; 32], n: u64, p: u8) -> u64 {
    let mut head = [0u8; 8];
    head.copy_from_slice(&element[..8]);
    let v = u64::from_be_bytes(head);

    let scaled = (v as u128) * (n as u128);
    (scaled >> (64 - p)) as u64
}

#[derive(Default)]
struct BitWriter {
    out: Vec<u8>,
    acc: u8,
    used: u8,
}

impl BitWriter {
    fn write_bit(&mut self, bit: bool) {
        self.acc = (self.acc << 1) | u8::from(bit);
        self.used += 1;
        if self.used == 8 {
            self.out.push(self.acc);
            self.acc = 0;
            self.used = 0;
        }
    }

    fn write_bits(&mut self, value: u64, nbits: u8) {
        for i in (0..nbits).rev() {
            self.write_bit((value >> i) & 1 == 1);
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if self.used > 0 {
            self.out.push(self.acc << (8 - self.used));
        }
        self.out
    }
}

struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        (self.data.len() * 8).saturating_sub(self.pos)
    }

    fn read_bit(&mut self) -> Result<bool, Error> {
        if self.pos >= self.data.len() * 8 {
            return Err(Error::UnexpectedEnd);
        }
        let byte = self.data[self.pos / 8];
        let bit = (byte >> (7 - (self.pos % 8))) & 1 == 1;
        self.pos += 1;
        Ok(bit)
    }

    fn read_bits(&mut self, nbits: u8) -> Result<u64, Error> {
        let mut value = 0u64;
        for _ in 0..nbits {
            value = (value << 1) | u64::from(self.read_bit()?);
        }
        Ok(value)
    }

    fn read_unary(&mut self) -> Result<u64, Error> {
        let mut count = 0u64;
        while self.read_bit()? {
            count = count.checked_add(1).ok_or(Error::PositionOverflow)?;
        }
        Ok(count)
    }
}

/// Encode a set of elements as a Golomb-Rice coded set.
///
/// Duplicate elements are removed, so `n` is recoverable by a decoder. Distinct
/// elements that land on the same position are kept, encoding as a zero delta.
pub fn encode(elements: &[FilterElement], p: u8) -> Result<Vec<u8>, Error> {
    check_p(p)?;

    let mut distinct: Vec<[u8; 32]> = elements.iter().map(|e| *e.as_bytes()).collect();
    distinct.sort_unstable();
    distinct.dedup();

    let n = distinct.len() as u64;

    let mut positions: Vec<u64> = distinct.iter().map(|bytes| position(bytes, n, p)).collect();
    positions.sort_unstable();

    let mut writer = BitWriter::default();
    let mut previous = 0u64;
    for pos in positions {
        let delta = pos - previous;
        previous = pos;

        let quotient = delta >> p;
        for _ in 0..quotient {
            writer.write_bit(true);
        }
        writer.write_bit(false);

        writer.write_bits(delta & ((1u64 << p) - 1), p);
    }

    Ok(writer.finish())
}

/// A filter whose positions have been read back out.
///
/// `n` is not transmitted; it is the number of values the decoder read, which
/// is why membership can only be tested after decoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedFilter {
    n: u64,
    p: u8,
    positions: Vec<u64>,
}

impl DecodedFilter {
    /// Decode a Golomb-Rice coded set.
    pub fn decode(data: &[u8], p: u8) -> Result<Self, Error> {
        check_p(p)?;

        let mut reader = BitReader::new(data);
        let mut positions = Vec::new();
        let mut previous = 0u64;

        while reader.remaining() > usize::from(p) {
            let quotient = reader.read_unary()?;
            let remainder = reader.read_bits(p)?;

            if quotient > (u64::MAX >> p) {
                return Err(Error::PositionOverflow);
            }
            let delta = (quotient << p)
                .checked_add(remainder)
                .ok_or(Error::PositionOverflow)?;

            previous = previous.checked_add(delta).ok_or(Error::PositionOverflow)?;
            positions.push(previous);
        }

        Ok(Self {
            n: positions.len() as u64,
            p,
            positions,
        })
    }

    /// Number of distinct elements the filter was built from.
    pub fn n(&self) -> u64 {
        self.n
    }

    /// Test an element against the filter.
    ///
    /// A `false` is conclusive; a `true` is a match with a `2^-p` chance of
    /// being spurious, so callers must confirm before acting on it.
    pub fn contains(&self, element: &FilterElement) -> bool {
        if self.n == 0 {
            return false;
        }
        let pos = position(element.as_bytes(), self.n, self.p);
        self.positions.binary_search(&pos).is_ok()
    }
}
