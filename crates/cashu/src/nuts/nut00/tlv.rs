//! TLV records shared by transaction transcripts and nutroot leaves (NUT-00)
//!
//! `record = type (1 byte) || length (2 bytes, big-endian) || value`
//!
//! Integers encode as minimal big-endian bytes: zero is the empty string and
//! a leading zero byte is non-canonical. A field stream's record types
//! strictly ascend; a container stream groups containers by ascending type.

use thiserror::Error;

/// Largest value a record can carry
pub const MAX_RECORD_VALUE_LEN: usize = u16::MAX as usize;

/// TLV error
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum Error {
    /// A record value is longer than its length prefix can express
    #[error("TLV value of {0} bytes exceeds 65535")]
    ValueTooLong(usize),
    /// The stream ends inside a record
    #[error("Truncated TLV record")]
    Truncated,
    /// A field stream's record types do not strictly ascend
    #[error("TLV field type {current:#04x} does not ascend after {previous:#04x}")]
    NotAscending {
        /// Previous record type
        previous: u8,
        /// Offending record type
        current: u8,
    },
    /// An integer has a leading zero byte
    #[error("Non-minimal TLV integer")]
    NonMinimalInteger,
    /// An integer does not fit in 64 bits
    #[error("TLV integer longer than 8 bytes")]
    IntegerTooLong,
}

/// Append one record to `out`
pub fn write_record(out: &mut Vec<u8>, record_type: u8, value: &[u8]) -> Result<(), Error> {
    let len = u16::try_from(value.len()).map_err(|_| Error::ValueTooLong(value.len()))?;
    out.reserve(3 + value.len());
    out.push(record_type);
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value);
    Ok(())
}

/// Encode an integer as minimal big-endian bytes (zero is empty)
pub fn encode_uint(value: u64) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    let first = bytes.iter().position(|b| *b != 0).unwrap_or(bytes.len());
    bytes[first..].to_vec()
}

/// Decode a minimal big-endian integer, rejecting a leading zero byte
pub fn decode_uint(bytes: &[u8]) -> Result<u64, Error> {
    if bytes.len() > 8 {
        return Err(Error::IntegerTooLong);
    }
    if bytes.first() == Some(&0) {
        return Err(Error::NonMinimalInteger);
    }
    Ok(bytes
        .iter()
        .fold(0u64, |acc, byte| (acc << 8) | u64::from(*byte)))
}

/// One record borrowed from a stream
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record<'a> {
    /// Record type
    pub record_type: u8,
    /// Record value
    pub value: &'a [u8],
}

/// Read every record of a field stream, requiring strictly ascending types
///
/// The whole input must be consumed: trailing bytes are a truncated record.
pub fn read_field_stream(mut bytes: &[u8]) -> Result<Vec<Record<'_>>, Error> {
    let mut records: Vec<Record<'_>> = Vec::new();
    while !bytes.is_empty() {
        let (record, rest) = read_record(bytes)?;
        if let Some(previous) = records.last() {
            if record.record_type <= previous.record_type {
                return Err(Error::NotAscending {
                    previous: previous.record_type,
                    current: record.record_type,
                });
            }
        }
        records.push(record);
        bytes = rest;
    }
    Ok(records)
}

fn read_record(bytes: &[u8]) -> Result<(Record<'_>, &[u8]), Error> {
    let [record_type, len_hi, len_lo, rest @ ..] = bytes else {
        return Err(Error::Truncated);
    };
    let len = usize::from(u16::from_be_bytes([*len_hi, *len_lo]));
    if rest.len() < len {
        return Err(Error::Truncated);
    }
    let (value, rest) = rest.split_at(len);
    Ok((
        Record {
            record_type: *record_type,
            value,
        },
        rest,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uint_round_trip_is_minimal() {
        for (value, encoded) in [
            (0u64, vec![]),
            (1, vec![1]),
            (255, vec![0xff]),
            (256, vec![1, 0]),
            (1_755_561_600, vec![0x68, 0xa3, 0xbe, 0x80]),
            (u64::MAX, vec![0xff; 8]),
        ] {
            assert_eq!(encode_uint(value), encoded);
            assert_eq!(decode_uint(&encoded), Ok(value));
        }
        assert_eq!(decode_uint(&[0]), Err(Error::NonMinimalInteger));
        assert_eq!(decode_uint(&[0, 1]), Err(Error::NonMinimalInteger));
        assert_eq!(decode_uint(&[1; 9]), Err(Error::IntegerTooLong));
    }

    #[test]
    fn record_layout() {
        let mut out = Vec::new();
        write_record(&mut out, 0x04, &[0xaa; 33]).unwrap();
        assert_eq!(&out[..3], &[0x04, 0x00, 0x21]);
        assert_eq!(out.len(), 36);

        assert_eq!(
            write_record(&mut Vec::new(), 1, &vec![0; MAX_RECORD_VALUE_LEN + 1]),
            Err(Error::ValueTooLong(MAX_RECORD_VALUE_LEN + 1))
        );
        write_record(&mut Vec::new(), 1, &vec![0; MAX_RECORD_VALUE_LEN]).unwrap();
    }

    #[test]
    fn field_stream_rules() {
        let mut stream = Vec::new();
        write_record(&mut stream, 0x02, &[1]).unwrap();
        write_record(&mut stream, 0x04, &[]).unwrap();
        let records = read_field_stream(&stream).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].value, &[] as &[u8]);

        // repeated type
        let mut repeated = stream.clone();
        write_record(&mut repeated, 0x04, &[]).unwrap();
        assert_eq!(
            read_field_stream(&repeated),
            Err(Error::NotAscending {
                previous: 4,
                current: 4
            })
        );

        // descending type
        let mut descending = stream.clone();
        write_record(&mut descending, 0x03, &[]).unwrap();
        assert!(matches!(
            read_field_stream(&descending),
            Err(Error::NotAscending { .. })
        ));

        // truncated header and value
        assert_eq!(read_field_stream(&[0x02, 0x00]), Err(Error::Truncated));
        assert_eq!(
            read_field_stream(&[0x02, 0x00, 0x02, 0x01]),
            Err(Error::Truncated)
        );
        assert!(read_field_stream(&[]).unwrap().is_empty());
    }
}
