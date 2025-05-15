//! Calculate Golomb-Coded Set filter

use bitvec::prelude::*;
use siphasher::sip::SipHasher24;
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::hash::Hasher;

type Result<T> = std::result::Result<T, GCSError>;

/// GCS Error Messages
#[derive(Debug)]
pub struct GCSError {
    message: String,
}

impl GCSError {
    fn new(message: &str) -> Self {
        GCSError {
            message: message.to_string(),
        }
    }
}

impl fmt::Display for GCSError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "GCS Error: {}", self.message)
    }
}

impl Error for GCSError {}

/// Hashes an item to a range using SipHash.
fn hash_to_range(item: &[u8], f: u64, key: &[u8]) -> u64 {
    let mut hasher = SipHasher24::new_with_key(&parse_key(key));
    hasher.write(item);
    let hash = hasher.finish();
    ((f as u128 * hash as u128) >> 64) as u64
}

/// Parses key to a form usable by SipHasher
fn parse_key(key: &[u8]) -> [u8; 16] {
    let mut result = [0u8; 16];
    for (i, &byte) in key.iter().take(16).enumerate() {
        result[i] = byte;
    }
    result
}

/// Creates a hashed set of items using a key and a multiplier.
fn create_hashed_set(items: &[Vec<u8>], key: &[u8], m: u32) -> Vec<u64> {
    let n = items.len() as u64;
    let f = n * m as u64;

    items.iter().map(|e| hash_to_range(e, f, key)).collect()
}

/// Golomb-encodes `x` into `stream` with remainder of `P` bits
fn golomb_encode(stream: &mut BitVec<u8, Msb0>, x: u64, p: u32) {
    let q = x >> p;
    let r = x & ((1 << p) - 1);

    // Append the quotient in unary coding
    for _ in 0..q {
        stream.push(true);
    }
    stream.push(false);

    // Append the remainder in binary coding
    for i in 0..p {
        stream.push(((r >> (p - 1 - i)) & 1) == 1);
    }
}

/// Decodes the first occurrence of a delta hash in `stream` starting from `offset`.
/// Returns the decoded delta and the new offset.
fn golomb_decode(stream: &BitVec<u8, Msb0>, offset: usize, p: u32) -> (u64, usize) {
    let mut q = 0;
    let mut current_offset = offset;

    while current_offset < stream.len() && stream[current_offset] {
        q += 1;
        current_offset += 1;
    }

    current_offset += 1; // Skip the 0 bit

    // Calculate the remainder directly from the BitVec slice
    let mut r = 0;
    for i in 0..p {
        if current_offset + (i as usize) < stream.len() && stream[current_offset + i as usize] {
            r |= 1 << (p - 1 - i);
        }
    }

    let x = (q << p) | r;
    (x, current_offset + p as usize)
}

/// GCS Filter
pub struct GCSFilter;

impl GCSFilter {
    /// Turns a list of entries into a Golomb-Coded Set of hashes.
    pub fn create(items: &[Vec<u8>], p: u32, m: u32, key: &[u8]) -> Result<Vec<u8>> {
        if (m as u64).checked_ilog2().unwrap_or(0) > 31 {
            return Err(GCSError::new("m parameter must be smaller than 2^32"));
        }
        if (items.len() as u64).checked_ilog2().unwrap_or(0) > 31 {
            return Err(GCSError::new(
                "number of elements must be smaller than 2^32",
            ));
        }

        let mut set_items = create_hashed_set(items, key, m);

        // Sort the items
        set_items.sort_unstable();

        let mut output_stream = BitVec::<u8, Msb0>::new();

        let mut last_value = 0;
        for &item in &set_items {
            let delta = item - last_value;
            golomb_encode(&mut output_stream, delta, p);
            last_value = item;
        }

        // Converts bitvec to bytes
        Ok(output_stream.into_vec())
    }

    /// Matches multiple target items against a Golomb-Coded Set.
    pub fn match_many(
        compressed_set: &[u8],
        targets: &[Vec<u8>],
        n: usize,
        p: u32,
        m: u32,
        key: &[u8],
    ) -> Result<HashMap<Vec<u8>, bool>> {
        if (m as u64).checked_ilog2().unwrap_or(0) > 31 {
            return Err(GCSError::new("m parameter must be smaller than 2^32"));
        }
        if (n as u64).checked_ilog2().unwrap_or(0) > 31 {
            return Err(GCSError::new(
                "number of elements must be smaller than 2^32",
            ));
        }

        let f = (n as u64) * (m as u64);

        // Check for uniqueness of targets
        let mut seen = HashMap::new();
        for target in targets {
            if seen.contains_key(target) {
                return Err(GCSError::new("match targets are not unique entries"));
            }
            seen.insert(target.clone(), false);
        }

        // Map targets to the same range as the set hashes
        let mut target_hashes: HashMap<u64, (Vec<u8>, bool)> = HashMap::new();
        for target in targets {
            let hash = hash_to_range(target, f, key);
            target_hashes.insert(hash, (target.clone(), false));
        }

        let input_stream = BitVec::<u8, Msb0>::from_vec(compressed_set.to_vec());

        let mut value = 0;
        let mut offset = 0;
        for _ in 0..n {
            if offset >= input_stream.len() {
                break; // Protect against malformed input
            }

            let (delta, new_offset) = golomb_decode(&input_stream, offset, p);
            offset = new_offset;
            value += delta;

            if let Some((target, _)) = target_hashes.get(&value) {
                target_hashes.insert(value, (target.clone(), true));
            }
        }

        let mut result = HashMap::new();
        for (_, (target, truth_value)) in target_hashes {
            result.insert(target, truth_value);
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::prelude::*;

    #[test]
    fn test_gcs_filter() {
        // Generate random data for testing
        let num_items = 1000;
        let item_size = 33; // 33 bytes
        let mut rng = rand::thread_rng();

        let mut items = Vec::new();
        for _ in 0..num_items {
            let mut item = vec![0u8; item_size];
            rng.fill_bytes(&mut item);
            items.push(item);
        }

        // Create a GCS filter with default parameters
        let p = 19;
        let m = 784931;
        let key = vec![0u8; 16];

        let gcs_filter = GCSFilter::create(&items, p, m, &key).unwrap();

        println!(
            "num_items = {}, item_size = {}, filter_size = {}",
            num_items,
            item_size,
            gcs_filter.len()
        );

        // Test set membership
        let results = GCSFilter::match_many(&gcs_filter, &items, num_items, p, m, &key).unwrap();

        // Assert all items are found in the filter
        for item in &items {
            assert!(results[item], "Item not found in the GCS filter");
        }

        // Test with a non-existent item
        let mut non_existent_item = vec![0u8; item_size];
        rng.fill_bytes(&mut non_existent_item);

        let results = GCSFilter::match_many(
            &gcs_filter,
            &[non_existent_item.clone()],
            num_items,
            p,
            m,
            &key,
        )
        .unwrap();

        assert!(
            !results[&non_existent_item],
            "Non-existent item was incorrectly found in the GCS filter"
        );
    }
}
