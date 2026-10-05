use std::fmt;

use super::{Error, FRAME_OVERHEAD, MAX_FRAGMENT_COUNT, MAX_FRAGMENT_LENGTH, MAX_MESSAGE_LENGTH};

const PREFIX: [u8; 4] = [0x4e, 0x46, 1, 0];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Metadata {
    count: usize,
    length: usize,
    size: usize,
    checksum: u32,
}

/// Encodes arbitrary bytes into nut-fountain version-1 binary frames.
///
/// Owns a copy of the input. Source frames precede deterministic XOR repair
/// frames, which can be generated until the 32-bit sequence is exhausted.
pub struct FountainEncoder {
    message: Vec<u8>,
    metadata: Metadata,
    sequence: u32,
}

impl FountainEncoder {
    /// Creates an encoder with a fixed payload size per frame, excluding the
    /// 24-byte header and checksum. Empty messages are supported.
    ///
    /// # Errors
    /// Rejects fragment sizes outside `1..=4096`, messages exceeding one MiB,
    /// and messages requiring more than 256 source fragments.
    pub fn new(message: &[u8], max_fragment_length: usize) -> Result<Self, Error> {
        if !(1..=MAX_FRAGMENT_LENGTH).contains(&max_fragment_length) {
            return Err(Error::InvalidFragmentLength {
                actual: max_fragment_length,
            });
        }
        if message.len() > MAX_MESSAGE_LENGTH {
            return Err(Error::MessageTooLarge {
                actual: message.len(),
            });
        }
        let count = message.len().div_ceil(max_fragment_length).max(1);
        if count > MAX_FRAGMENT_COUNT {
            return Err(Error::TooManyFragments { actual: count });
        }
        Ok(Self {
            message: message.to_vec(),
            metadata: Metadata {
                count,
                length: message.len(),
                size: max_fragment_length,
                checksum: crc32(message),
            },
            sequence: 0,
        })
    }

    /// Returns the next complete binary frame.
    ///
    /// # Errors
    /// Returns [`Error::SequenceExhausted`] after sequence `u32::MAX`.
    pub fn next_part(&mut self) -> Result<Vec<u8>, Error> {
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or(Error::SequenceExhausted)?;
        let mut data = vec![0; self.metadata.size];
        for (i, selected) in coefficients(sequence, self.metadata.count)
            .iter()
            .enumerate()
        {
            if *selected != 0 {
                let start = i * self.metadata.size;
                let end = (start + self.metadata.size).min(self.message.len());
                xor(&mut data[..end - start], &self.message[start..end]);
            }
        }
        let mut frame = Vec::with_capacity(FRAME_OVERHEAD + data.len());
        frame.extend_from_slice(&PREFIX);
        frame.extend_from_slice(&sequence.to_be_bytes());
        frame.extend_from_slice(&(self.metadata.count as u32).to_be_bytes());
        frame.extend_from_slice(&(self.metadata.length as u32).to_be_bytes());
        frame.extend_from_slice(&self.metadata.checksum.to_be_bytes());
        frame.extend_from_slice(&data);
        frame.extend_from_slice(&crc32(&frame).to_be_bytes());
        self.sequence = sequence;
        Ok(frame)
    }

    /// Returns the number of frames emitted so far.
    pub fn current_index(&self) -> u32 {
        self.sequence
    }

    /// Returns the number of source fragments.
    pub fn fragment_count(&self) -> usize {
        self.metadata.count
    }

    /// Returns whether one source frame contains the whole message.
    pub fn is_single_fragment(&self) -> bool {
        self.fragment_count() == 1
    }
}

impl fmt::Debug for FountainEncoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FountainEncoder")
            .field("current_index", &self.current_index())
            .field("fragment_count", &self.fragment_count())
            .finish()
    }
}

struct Equation {
    coefficients: Vec<u8>,
    data: Vec<u8>,
}

/// Incremental decoder for nut-fountain version-1 binary frames.
///
/// Retains at most 256 independent equations, each with at most 4096 payload
/// bytes. Rank measures progress; duplicates require no additional storage.
/// Input is copied, and completed bytes are exposed only after validation.
#[derive(Default)]
pub struct FountainDecoder {
    metadata: Option<Metadata>,
    rows: Vec<Option<Equation>>,
    rank: usize,
    decoded: Option<Vec<u8>>,
}

impl FountainDecoder {
    /// Accepts a complete frame. Returns whether it adds independent information.
    ///
    /// # Errors
    /// Invalid frames and foreign transfers are rejected without changing state,
    /// including after completion. On reconstruction failure, only the newly
    /// inserted equation is discarded, matching the TypeScript implementation.
    pub fn receive(&mut self, part: &[u8]) -> Result<bool, Error> {
        let (metadata, sequence, data) = parse_frame(part)?;
        if self.metadata.is_some_and(|current| current != metadata) {
            return Err(Error::InconsistentMessage);
        }
        if self.complete() {
            return Ok(false);
        }
        if self.metadata.is_none() {
            self.rows = (0..metadata.count).map(|_| None).collect();
            self.metadata = Some(metadata);
        }
        let mut equation = Equation {
            coefficients: coefficients(sequence, metadata.count),
            data: data.to_vec(),
        };
        for pivot in 0..metadata.count {
            if equation.coefficients[pivot] == 0 {
                continue;
            }
            match &self.rows[pivot] {
                Some(row) => {
                    xor(&mut equation.coefficients, &row.coefficients);
                    xor(&mut equation.data, &row.data);
                }
                None => {
                    self.rows[pivot] = Some(equation);
                    self.rank += 1;
                    if self.rank == metadata.count {
                        let mut message = self.recover(metadata);
                        if crc32(&message[..metadata.length]) != metadata.checksum
                            || message[metadata.length..].iter().any(|byte| *byte != 0)
                        {
                            self.rows[pivot] = None;
                            self.rank -= 1;
                            return Err(Error::MessageIntegrityMismatch);
                        }
                        message.truncate(metadata.length);
                        self.decoded = Some(message);
                    }
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    fn recover(&self, metadata: Metadata) -> Vec<u8> {
        let mut message = vec![0; metadata.count * metadata.size];
        for (i, row) in self.rows.iter().enumerate().rev() {
            // Called only at full rank, when every pivot has a row.
            let row = row.as_ref().expect("full rank has every pivot");
            let mut fragment = row.data.clone();
            for (j, coefficient) in row.coefficients.iter().enumerate().skip(i + 1) {
                if *coefficient != 0 {
                    xor(
                        &mut fragment,
                        &message[j * metadata.size..(j + 1) * metadata.size],
                    );
                }
            }
            message[i * metadata.size..(i + 1) * metadata.size].copy_from_slice(&fragment);
        }
        message
    }

    /// Returns whether reconstruction and integrity validation have completed.
    pub fn complete(&self) -> bool {
        self.decoded.is_some()
    }

    /// Returns the exact unpadded message, or `None` until completion.
    pub fn message(&self) -> Option<&[u8]> {
        self.decoded.as_deref()
    }

    /// Returns the source fragment count, or zero before receiving a valid frame.
    pub fn fragment_count(&self) -> usize {
        self.metadata.map_or(0, |metadata| metadata.count)
    }

    /// Returns the number of independent equations retained.
    pub fn independent_fragment_count(&self) -> usize {
        self.rank
    }

    /// Returns how many individual source fragments can be recovered, or `None`
    /// before receiving a valid frame. Mixed equations may increase rank without
    /// resolving individual fragments.
    pub fn resolved_fragment_count(&self) -> Option<usize> {
        let metadata = self.metadata?;
        // Reduce coefficients to echelon form in both directions. Rank alone
        // does not say which source fragments are individually recoverable.
        // Four words suffice for the protocol's maximum of 256 coefficients.
        let mut reduced = vec![[0u64; 4]; metadata.count];
        for (i, row) in self.rows.iter().enumerate() {
            if let Some(row) = row {
                for (j, coefficient) in row.coefficients.iter().enumerate() {
                    reduced[i][j / 64] |= u64::from(*coefficient) << (j % 64);
                }
            }
        }
        for pivot in (0..metadata.count).rev() {
            if self.rows[pivot].is_some() {
                let source = reduced[pivot];
                for row in &mut reduced[..pivot] {
                    if row[pivot / 64] & (1 << (pivot % 64)) != 0 {
                        for (target, source) in row.iter_mut().zip(source) {
                            *target ^= source;
                        }
                    }
                }
            }
        }
        Some(
            reduced
                .iter()
                .filter(|row| row.iter().map(|word| word.count_ones()).sum::<u32>() == 1)
                .count(),
        )
    }

    /// Returns the retained rank divided by the source fragment count, or zero
    /// before a transfer. One means the message passed integrity validation.
    pub fn progress(&self) -> f64 {
        self.metadata
            .map_or(0.0, |metadata| self.rank as f64 / metadata.count as f64)
    }

    /// Clears the message identity, equations, and completed result.
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

impl fmt::Debug for FountainDecoder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FountainDecoder")
            .field("complete", &self.complete())
            .field("fragment_count", &self.fragment_count())
            .field(
                "independent_fragment_count",
                &self.independent_fragment_count(),
            )
            .finish()
    }
}

fn parse_frame(part: &[u8]) -> Result<(Metadata, u32, &[u8]), Error> {
    if !(FRAME_OVERHEAD + 1..=FRAME_OVERHEAD + MAX_FRAGMENT_LENGTH).contains(&part.len()) {
        return Err(Error::InvalidFrameLength { actual: part.len() });
    }
    if part[..4] != PREFIX {
        return Err(Error::UnsupportedFormat);
    }
    let read_u32 = |offset: usize| {
        u32::from_be_bytes([
            part[offset],
            part[offset + 1],
            part[offset + 2],
            part[offset + 3],
        ])
    };
    let sequence = read_u32(4);
    let metadata = Metadata {
        count: read_u32(8) as usize,
        length: read_u32(12) as usize,
        size: part.len() - FRAME_OVERHEAD,
        checksum: read_u32(16),
    };
    if sequence == 0
        || !(1..=MAX_FRAGMENT_COUNT).contains(&metadata.count)
        || metadata.length > MAX_MESSAGE_LENGTH
        || metadata.count != metadata.length.div_ceil(metadata.size).max(1)
    {
        return Err(Error::InvalidMetadata);
    }
    if crc32(&part[..part.len() - 4]) != read_u32(part.len() - 4) {
        return Err(Error::FrameChecksumMismatch);
    }
    Ok((metadata, sequence, &part[20..part.len() - 4]))
}

fn coefficients(sequence: u32, count: usize) -> Vec<u8> {
    let mut bits = vec![0; count];
    if sequence as usize <= count {
        bits[sequence as usize - 1] = 1;
        return bits;
    }
    let mut state = sequence;
    for bit in &mut bits {
        state = state.wrapping_add(0x6d2b79f5);
        let mut word = (state ^ (state >> 15)).wrapping_mul(state | 1);
        word ^= word.wrapping_add((word ^ (word >> 7)).wrapping_mul(word | 61));
        *bit = ((word ^ (word >> 14)) & 1) as u8;
    }
    if bits.iter().all(|bit| *bit == 0) {
        bits[(sequence as usize - 1) % count] = 1;
    }
    bits
}

fn xor(target: &mut [u8], source: &[u8]) {
    for (target, source) in target.iter_mut().zip(source) {
        *target ^= source;
    }
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320u32.wrapping_mul(crc & 1));
        }
    }
    crc ^ u32::MAX
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_exhaustion_does_not_wrap_or_reset() {
        let mut encoder = FountainEncoder::new(&[1, 2, 3], 3).expect("encoder");
        encoder.sequence = u32::MAX - 1;
        let frame = encoder.next_part().expect("last frame");
        assert_eq!(encoder.current_index(), u32::MAX);
        let mut decoder = FountainDecoder::default();
        decoder.receive(&frame).expect("last sequence is valid");
        assert_eq!(decoder.message(), Some([1, 2, 3].as_slice()));
        for _ in 0..2 {
            assert!(matches!(encoder.next_part(), Err(Error::SequenceExhausted)));
            assert_eq!(encoder.current_index(), u32::MAX);
        }
    }
}
