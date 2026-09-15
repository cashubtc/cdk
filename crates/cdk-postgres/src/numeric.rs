//! Binary wire codec for the `numeric` values amount columns are declared as.
//!
//! `tokio-postgres` maps `numeric` onto nothing on its own, and the arbitrary
//! precision crates that fill the gap are a large dependency for the one shape
//! this driver needs: a non negative integer no wider than `u64`.

/// Digits of a postgres numeric are base 10000.
const NBASE: u64 = 10_000;

/// Sign word of a positive numeric. Every other value is negative, NaN or an
/// infinity, none of which an amount column can hold.
const SIGN_POSITIVE: u16 = 0x0000;

/// A numeric that is not an amount.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The wire representation was shorter than its own header claims
    #[error("Truncated numeric value")]
    Truncated,

    /// The value is negative, NaN or an infinity
    #[error("Numeric value is not a non-negative number")]
    NotANonNegativeNumber,

    /// The value has a fractional part, counted by the header's display scale
    #[error("Numeric value has {0} fractional digits")]
    NotAnInteger(u16),

    /// The header does not describe a value this codec can walk
    #[error("Malformed numeric value")]
    Malformed,

    /// The value is an integer but wider than `u64`
    #[error("Numeric value {0} does not fit in u64")]
    OutOfRange(String),
}

/// Writes `value` in the binary format of [`Type::NUMERIC`], which a `u64`
/// never fills with more than five base 10000 groups.
///
/// The weight counts base 10000 places left of the point, so it is read before
/// trailing zero groups are dropped from the canonical form.
pub fn encode(value: u64, out: &mut tokio_postgres::types::private::BytesMut) {
    let mut digits: Vec<i16> = Vec::with_capacity(5);
    let mut rest = value;
    while rest > 0 {
        digits.push((rest % NBASE) as i16);
        rest /= NBASE;
    }
    digits.reverse();

    let weight = digits.len().saturating_sub(1) as i16;
    while digits.last() == Some(&0) {
        digits.pop();
    }

    let ndigits = digits.len() as i16;
    out.extend_from_slice(&ndigits.to_be_bytes());
    out.extend_from_slice(&weight.to_be_bytes());
    out.extend_from_slice(&SIGN_POSITIVE.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    for digit in digits {
        out.extend_from_slice(&digit.to_be_bytes());
    }
}

/// Reads a [`Type::NUMERIC`] in binary format, refusing anything an amount
/// column cannot legitimately hold.
///
/// The weight is the base 10000 place of the first digit, so the digits left of
/// the point are the ones it still reaches; the trailing zero groups the
/// canonical form dropped are scaled back on afterwards. Zero stays zero however
/// far the weight reaches, which matters because a header may claim a weight of
/// 32767.
pub fn decode(raw: &[u8]) -> Result<u64, Error> {
    let header = raw.get(..8).ok_or(Error::Truncated)?;
    let ndigits = i16::from_be_bytes([header[0], header[1]]);
    let weight = i16::from_be_bytes([header[2], header[3]]);
    let sign = u16::from_be_bytes([header[4], header[5]]);
    let dscale = u16::from_be_bytes([header[6], header[7]]);

    if sign != SIGN_POSITIVE {
        return Err(Error::NotANonNegativeNumber);
    }

    let ndigits = usize::try_from(ndigits).map_err(|_| Error::Truncated)?;
    let body = raw.get(8..8 + ndigits * 2).ok_or(Error::Truncated)?;
    let digit = |index: usize| i16::from_be_bytes([body[index * 2], body[index * 2 + 1]]);

    let places = i32::from(weight) + 1;
    let integral = usize::try_from(places.max(0))
        .map_err(|_| Error::Malformed)?
        .min(ndigits);

    let trailing = places - i32::try_from(integral).map_err(|_| Error::Malformed)?;

    for index in integral..ndigits {
        if digit(index) != 0 {
            return Err(Error::NotAnInteger(dscale));
        }
    }

    let decimal = || {
        let mut text = String::new();
        for index in 0..integral {
            if index == 0 {
                text.push_str(&digit(index).to_string());
            } else {
                text.push_str(&format!("{:04}", digit(index)));
            }
        }

        for _ in 0..trailing.max(0) {
            text.push_str("0000");
        }

        if text.is_empty() {
            text.push('0');
        }

        text
    };

    let mut value = 0u64;
    for index in 0..integral {
        let digit = u64::try_from(digit(index)).map_err(|_| Error::NotANonNegativeNumber)?;
        value = value
            .checked_mul(NBASE)
            .and_then(|value| value.checked_add(digit))
            .ok_or_else(|| Error::OutOfRange(decimal()))?;
    }

    if value != 0 {
        for _ in 0..trailing {
            value = value
                .checked_mul(NBASE)
                .ok_or_else(|| Error::OutOfRange(decimal()))?;
        }
    }

    Ok(value)
}

#[cfg(test)]
mod test {
    use tokio_postgres::types::private::BytesMut;

    use super::*;

    fn round_trip(value: u64) {
        let mut out = BytesMut::new();
        encode(value, &mut out);
        assert_eq!(decode(&out).expect("decodes"), value, "for {value}");
    }

    #[test]
    fn round_trips_the_whole_range() {
        for value in [
            0,
            1,
            9_999,
            10_000,
            10_001,
            1_000_000,
            u64::from(u32::MAX),
            i64::MAX as u64,
            u64::MAX - 1,
            u64::MAX,
        ] {
            round_trip(value);
        }
    }

    #[test]
    fn round_trips_values_with_trailing_zero_groups() {
        for value in [10_000, 100_000_000, 10_000_000_000_000_000] {
            round_trip(value);
        }
    }

    #[test]
    fn rejects_a_negative_value() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&1i16.to_be_bytes());
        raw.extend_from_slice(&0i16.to_be_bytes());
        raw.extend_from_slice(&0x4000u16.to_be_bytes());
        raw.extend_from_slice(&0u16.to_be_bytes());
        raw.extend_from_slice(&1i16.to_be_bytes());

        assert!(matches!(decode(&raw), Err(Error::NotANonNegativeNumber)));
    }

    /// The reported count is the header's display scale, so it is a number of
    /// fractional decimal digits rather than of base 10000 groups.
    #[test]
    fn rejects_a_fractional_value() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&2i16.to_be_bytes());
        raw.extend_from_slice(&0i16.to_be_bytes());
        raw.extend_from_slice(&SIGN_POSITIVE.to_be_bytes());
        raw.extend_from_slice(&4u16.to_be_bytes());
        raw.extend_from_slice(&1i16.to_be_bytes());
        raw.extend_from_slice(&5000i16.to_be_bytes());

        assert!(matches!(decode(&raw), Err(Error::NotAnInteger(4))));
    }

    /// A value below one has no digit left of the point at all, so the fraction
    /// is caught by the first digit rather than by a later one.
    #[test]
    fn rejects_a_value_below_one() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&1i16.to_be_bytes());
        raw.extend_from_slice(&(-1i16).to_be_bytes());
        raw.extend_from_slice(&SIGN_POSITIVE.to_be_bytes());
        raw.extend_from_slice(&1u16.to_be_bytes());
        raw.extend_from_slice(&5000i16.to_be_bytes());

        assert!(matches!(decode(&raw), Err(Error::NotAnInteger(1))));
    }

    /// The digits are `u64::MAX + 1` written out in base 10000.
    #[test]
    fn rejects_a_value_wider_than_u64() {
        let digits: [i16; 5] = [1844, 6744, 737, 955, 1616];
        let mut raw = Vec::new();
        raw.extend_from_slice(&5i16.to_be_bytes());
        raw.extend_from_slice(&4i16.to_be_bytes());
        raw.extend_from_slice(&SIGN_POSITIVE.to_be_bytes());
        raw.extend_from_slice(&0u16.to_be_bytes());
        for digit in digits {
            raw.extend_from_slice(&digit.to_be_bytes());
        }

        assert!(
            matches!(decode(&raw), Err(Error::OutOfRange(value)) if value == "18446744073709551616")
        );
    }

    /// A header may claim a weight the digits never reach. Zero stays zero
    /// however far it reaches, so the scale loop has nothing to do.
    #[test]
    fn decodes_a_zero_with_an_absurd_weight() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&0i16.to_be_bytes());
        raw.extend_from_slice(&i16::MAX.to_be_bytes());
        raw.extend_from_slice(&SIGN_POSITIVE.to_be_bytes());
        raw.extend_from_slice(&0u16.to_be_bytes());

        assert_eq!(decode(&raw).expect("decodes"), 0);
    }

    #[test]
    fn rejects_a_truncated_value() {
        assert!(matches!(decode(&[0, 1, 0, 0]), Err(Error::Truncated)));

        let mut raw = Vec::new();
        raw.extend_from_slice(&2i16.to_be_bytes());
        raw.extend_from_slice(&1i16.to_be_bytes());
        raw.extend_from_slice(&SIGN_POSITIVE.to_be_bytes());
        raw.extend_from_slice(&0u16.to_be_bytes());
        raw.extend_from_slice(&1i16.to_be_bytes());

        assert!(matches!(decode(&raw), Err(Error::Truncated)));
    }
}
