//! C API for React Native Nitro module.
//!
//! Exposes the DHKE blinding operations needed by the OutputDataCreator
//! HybridObject in C++ land.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::{ptr, slice};

use cashu::dhke::blind_message;
use cashu::nuts::nut01::SecretKey;
use cashu::nuts::nut02::Id;
use cashu::nuts::nut10::SpendingConditions;
use cashu::nuts::{Conditions, PublicKey, SigFlag};
use cashu::secret::Secret;

/// Result of a blinding operation, returned to C++.
#[repr(C)]
pub struct CdkBlindResult {
    pub blinded_secret: *mut c_char,
    pub blinding_factor: *mut c_char,
    pub secret: *mut c_char,
}

/// Free a CdkBlindResult allocated by this library.
#[no_mangle]
pub unsafe extern "C" fn cdk_blind_result_free(result: *mut CdkBlindResult) {
    if result.is_null() {
        return;
    }
    unsafe {
        let r = Box::from_raw(result);
        if !r.blinded_secret.is_null() {
            drop(CString::from_raw(r.blinded_secret));
        }
        if !r.blinding_factor.is_null() {
            drop(CString::from_raw(r.blinding_factor));
        }
        if !r.secret.is_null() {
            drop(CString::from_raw(r.secret));
        }
    }
}

/// Encode the three result strings into a heap `CdkBlindResult` value.
///
/// The hex and JSON encodings never contain an interior NUL byte, so
/// `CString::new` only fails on unexpectedly malformed input. Build all three
/// strings before taking ownership so a failure cannot leak an already
/// converted raw pointer.
fn encode_result(
    blinded_secret: &str,
    blinding_factor: &str,
    secret: &str,
) -> Result<CdkBlindResult, String> {
    match (
        CString::new(blinded_secret),
        CString::new(blinding_factor),
        CString::new(secret),
    ) {
        (Ok(a), Ok(b), Ok(c)) => Ok(CdkBlindResult {
            blinded_secret: a.into_raw(),
            blinding_factor: b.into_raw(),
            secret: c.into_raw(),
        }),
        _ => Err("failed to encode blinding result".to_string()),
    }
}

fn make_result(
    blinded_secret: PublicKey,
    blinding_factor: SecretKey,
    secret: &Secret,
) -> *mut CdkBlindResult {
    // Return null instead of panicking, which would unwind across the FFI
    // boundary and abort the process.
    match encode_result(
        &blinded_secret.to_string(),
        &blinding_factor.to_secret_hex(),
        &secret.to_string(),
    ) {
        Ok(result) => Box::into_raw(Box::new(result)),
        Err(_) => ptr::null_mut(),
    }
}

/// The secrets produced by a single blinding operation, before they are encoded
/// for the C boundary.
struct BlindParts {
    blinded_secret: PublicKey,
    blinding_factor: SecretKey,
    secret: Secret,
}

impl BlindParts {
    fn encode(&self) -> Result<CdkBlindResult, String> {
        encode_result(
            &self.blinded_secret.to_string(),
            &self.blinding_factor.to_secret_hex(),
            &self.secret.to_string(),
        )
    }
}

/// Blind an ephemeral random secret: B_ = hash_to_curve(secret) + r*G.
fn build_random_parts() -> Result<BlindParts, String> {
    let secret = Secret::generate();
    let (blinded_secret, blinding_factor) =
        blind_message(&secret.to_bytes(), None).map_err(|e| e.to_string())?;
    Ok(BlindParts {
        blinded_secret,
        blinding_factor,
        secret,
    })
}

/// Blind a NUT-10 P2PK secret locked to `pubkey`.
///
/// Each call draws a fresh nonce, so every output in a split gets a distinct
/// secret even when the spending conditions are identical.
fn build_p2pk_parts(
    pubkey: PublicKey,
    additional_pubkeys: Option<Vec<PublicKey>>,
    num_sigs: u64,
    locktime: u64,
    refund_pubkeys: Option<Vec<PublicKey>>,
    num_sigs_refund: u64,
    sig_flag: SigFlag,
) -> Result<BlindParts, String> {
    // num_sigs: 1 is the default and encoded implicitly (None). A value of 0
    // is an invalid request; pass it through so validation rejects it rather
    // than silently collapsing it to the default single-signature policy.
    // num_sigs_refund: both 0 and 1 mean the default single refund signature
    // (None); only 2+ encodes an explicit refund multisig threshold.
    let num_sigs = if num_sigs == 1 { None } else { Some(num_sigs) };
    let num_sigs_refund = if num_sigs_refund > 1 {
        Some(num_sigs_refund)
    } else {
        None
    };

    // Build through the validated constructor: rejects past locktimes and
    // signature counts that exceed the available keys.
    let conditions = Conditions::new(
        if locktime > 0 { Some(locktime) } else { None },
        additional_pubkeys,
        refund_pubkeys,
        num_sigs,
        Some(sig_flag),
        num_sigs_refund,
    )
    .map_err(|e| e.to_string())?;

    let spending_conditions = SpendingConditions::P2PKConditions {
        data: pubkey,
        conditions: Some(conditions),
    };

    let secret = Secret::try_from(spending_conditions).map_err(|e| e.to_string())?;

    let (blinded_secret, blinding_factor) =
        blind_message(&secret.to_bytes(), None).map_err(|e| e.to_string())?;
    Ok(BlindParts {
        blinded_secret,
        blinding_factor,
        secret,
    })
}

/// Blind a NUT-13 deterministic secret from seed + keyset id + counter.
fn build_deterministic_parts(seed: &[u8; 64], id: Id, counter: u32) -> Result<BlindParts, String> {
    let secret = Secret::from_seed(seed, id, counter).map_err(|e| e.to_string())?;
    let blinding_key = SecretKey::from_seed(seed, id, counter).map_err(|e| e.to_string())?;
    let (blinded_secret, blinding_factor) =
        blind_message(&secret.to_bytes(), Some(blinding_key)).map_err(|e| e.to_string())?;
    Ok(BlindParts {
        blinded_secret,
        blinding_factor,
        secret,
    })
}

/// Create a random blinded message (ephemeral secret).
/// B_ = hash_to_curve(secret) + r*G
#[no_mangle]
pub unsafe extern "C" fn cdk_create_random_blinded_message(
    _amount: u64,
    keyset_id: *const c_char,
) -> *mut CdkBlindResult {
    // The ephemeral secret does not depend on the keyset id, but validate it
    // for consistency with the P2PK and deterministic constructors.
    if unsafe { parse_keyset_id(keyset_id) }.is_none() {
        return ptr::null_mut();
    }

    match build_random_parts() {
        Ok(p) => make_result(p.blinded_secret, p.blinding_factor, &p.secret),
        Err(_) => ptr::null_mut(),
    }
}

/// Create a P2PK blinded message locked to a public key.
#[no_mangle]
pub unsafe extern "C" fn cdk_create_p2pk_blinded_message(
    _amount: u64,
    keyset_id: *const c_char,
    pubkey_hex: *const c_char,
    additional_pubkeys: *const *const c_char,
    additional_pubkeys_len: u32,
    num_sigs: f64,
    locktime: f64,
    refund_pubkeys: *const *const c_char,
    refund_pubkeys_len: u32,
    num_sigs_refund: f64,
    sig_flag_ptr: *const c_char,
) -> *mut CdkBlindResult {
    if pubkey_hex.is_null() {
        return ptr::null_mut();
    }

    let (num_sigs, locktime, num_sigs_refund) = match (
        to_u64(num_sigs, "numSigs"),
        to_u64(locktime, "locktime"),
        to_u64(num_sigs_refund, "numSigsRefund"),
    ) {
        (Ok(a), Ok(b), Ok(c)) => (a, b, c),
        _ => return ptr::null_mut(),
    };

    // Validate the keyset id for consistency with the other constructors,
    // even though the P2PK secret does not depend on it.
    if unsafe { parse_keyset_id(keyset_id) }.is_none() {
        return ptr::null_mut();
    }

    let pubkey = match unsafe { CStr::from_ptr(pubkey_hex) }
        .to_str()
        .ok()
        .and_then(|s| PublicKey::from_hex(s).ok())
    {
        Some(pk) => pk,
        None => return ptr::null_mut(),
    };

    let add_pks = match parse_pubkey_array(additional_pubkeys, additional_pubkeys_len) {
        Ok(pks) => pks,
        Err(()) => return ptr::null_mut(),
    };
    let refund_pks = match parse_pubkey_array(refund_pubkeys, refund_pubkeys_len) {
        Ok(pks) => pks,
        Err(()) => return ptr::null_mut(),
    };

    let sig_flag = match unsafe { parse_sig_flag(sig_flag_ptr) } {
        Ok(f) => f,
        Err(()) => return ptr::null_mut(),
    };

    match build_p2pk_parts(
        pubkey,
        add_pks,
        num_sigs,
        locktime,
        refund_pks,
        num_sigs_refund,
        sig_flag,
    ) {
        Ok(p) => make_result(p.blinded_secret, p.blinding_factor, &p.secret),
        Err(_) => ptr::null_mut(),
    }
}

/// Create a deterministic blinded message from seed + keyset_id + counter (NUT-13).
#[no_mangle]
pub unsafe extern "C" fn cdk_create_deterministic_blinded_message(
    _amount: u64,
    keyset_id: *const c_char,
    seed: *const u8,
    seed_len: u32,
    counter: f64,
) -> *mut CdkBlindResult {
    let id = match unsafe { parse_keyset_id(keyset_id) } {
        Some(id) => id,
        None => return ptr::null_mut(),
    };

    let counter = match to_u32(counter, "counter") {
        Ok(c) => c,
        Err(_) => return ptr::null_mut(),
    };

    // Validate pointer and length before creating the slice to avoid UB
    if seed.is_null() || seed_len != 64 {
        return ptr::null_mut();
    }

    let seed_slice = unsafe { slice::from_raw_parts(seed, seed_len as usize) };
    let seed_arr: &[u8; 64] = match seed_slice.try_into() {
        Ok(arr) => arr,
        Err(_) => return ptr::null_mut(),
    };

    match build_deterministic_parts(seed_arr, id, counter) {
        Ok(p) => make_result(p.blinded_secret, p.blinding_factor, &p.secret),
        Err(_) => ptr::null_mut(),
    }
}

/// Parse and validate a keyset id from a C string pointer.
///
/// Returns `None` for a null pointer, non-UTF-8 bytes, or a string that is not
/// a valid keyset id. Every constructor (single-message and list) validates the
/// keyset id the same way, even where the blinding math does not depend on it,
/// so a malformed id is rejected consistently.
unsafe fn parse_keyset_id(keyset_id: *const c_char) -> Option<Id> {
    if keyset_id.is_null() {
        return None;
    }
    let s = unsafe { CStr::from_ptr(keyset_id) }.to_str().ok()?;
    s.parse::<Id>().ok()
}

fn parse_pubkey_array(ptrs: *const *const c_char, len: u32) -> Result<Option<Vec<PublicKey>>, ()> {
    if ptrs.is_null() || len == 0 {
        return Ok(None);
    }
    let slice = unsafe { slice::from_raw_parts(ptrs, len as usize) };
    let mut pks = Vec::with_capacity(len as usize);
    for &p in slice {
        if p.is_null() {
            return Err(());
        }
        let s = unsafe { CStr::from_ptr(p) }.to_str().map_err(|_| ())?;
        let pk = PublicKey::from_hex(s).map_err(|_| ())?;
        pks.push(pk);
    }
    Ok(Some(pks))
}

/// Parse an optional signature flag from a C string.
///
/// A null pointer yields the default flag; `"SigAll"` / `"SigInputs"` map to
/// their variants; any other string is an error.
unsafe fn parse_sig_flag(ptr: *const c_char) -> Result<SigFlag, ()> {
    if ptr.is_null() {
        return Ok(SigFlag::default());
    }
    match unsafe { CStr::from_ptr(ptr) }.to_str() {
        Ok("SigAll") => Ok(SigFlag::SigAll),
        Ok("SigInputs") => Ok(SigFlag::SigInputs),
        _ => Err(()),
    }
}

// 2^64, the first value that is not representable as a u64.
const UINT64_CEIL: f64 = 18446744073709551616.0;

/// Validate a JavaScript number as an unsigned 64-bit integer.
///
/// Non-finite, negative, out-of-range, and non-integral values are rejected.
fn to_u64(v: f64, field: &str) -> Result<u64, String> {
    if !(0.0..UINT64_CEIL).contains(&v) || v != (v as u64) as f64 {
        return Err(format!("{field} is not a valid unsigned integer"));
    }
    Ok(v as u64)
}

/// Same validation constrained to the unsigned 32-bit range.
fn to_u32(v: f64, field: &str) -> Result<u32, String> {
    if !(0.0..=f64::from(u32::MAX)).contains(&v) || v != f64::from(v as u32) {
        return Err(format!("{field} is not a valid unsigned 32-bit integer"));
    }
    Ok(v as u32)
}

/// Read a required array of amounts, validating each as a u64.
unsafe fn read_u64_array(ptr: *const f64, len: usize, field: &str) -> Result<Vec<u64>, String> {
    if ptr.is_null() || len == 0 {
        return Ok(Vec::new());
    }
    let slice = unsafe { slice::from_raw_parts(ptr, len) };
    slice.iter().map(|&v| to_u64(v, field)).collect()
}

/// Read an optional custom split, validating each entry as a u64.
unsafe fn read_opt_u64_array(
    ptr: *const f64,
    len: usize,
    field: &str,
) -> Result<Option<Vec<u64>>, String> {
    if ptr.is_null() || len == 0 {
        return Ok(None);
    }
    let slice = unsafe { slice::from_raw_parts(ptr, len) };
    let values: Result<Vec<u64>, String> = slice.iter().map(|&v| to_u64(v, field)).collect();
    Ok(Some(values?))
}

/// Split `amount` into denominations, matching cashu-ts `splitAmount`.
///
/// With no custom split, the amount is filled greedily from the keyset
/// denominations, largest first. A custom split is honored as follows:
/// an all-zero split of a zero amount is returned verbatim (explicit zero /
/// blank outputs for restore); otherwise zero entries are ignored, the positive
/// entries must each be a keyset denomination and must not exceed `amount`, and
/// any shortfall is filled from the keyset. An exact positive split is returned
/// in the caller's order. Ordering therefore matches cashu-ts: custom positives
/// first, then the greedy fill in descending denomination order.
fn compute_split(
    amount: u64,
    denoms: &[u64],
    custom_split: Option<&[u64]>,
) -> Result<Vec<u64>, String> {
    if let Some(split) = custom_split {
        let total_all = split
            .iter()
            .try_fold(0u64, |acc, &v| acc.checked_add(v))
            .ok_or_else(|| "Split total overflows".to_string())?;

        // Explicit zero-total outputs (restore / NUT-08 blank outputs).
        if amount == 0 && total_all == 0 {
            return Ok(split.to_vec());
        }

        // Zero entries are ignored for a positive amount.
        let positive: Vec<u64> = split.iter().copied().filter(|&v| v != 0).collect();
        let total_positive: u64 = positive.iter().sum();

        if total_positive > amount {
            return Err(format!(
                "Split is greater than total amount: {total_positive} > {amount}"
            ));
        }
        if positive.iter().any(|p| !denoms.contains(p)) {
            return Err(
                "Provided amount preferences do not match the amounts of the mint keyset."
                    .to_string(),
            );
        }
        if total_positive == amount {
            return Ok(positive);
        }

        let mut result = positive;
        fill_from_keyset(amount - total_positive, denoms, &mut result)?;
        return Ok(result);
    }

    let mut result = Vec::new();
    fill_from_keyset(amount, denoms, &mut result)?;
    Ok(result)
}

/// Greedily fill `remaining` from the keyset denominations, largest first,
/// appending to `out`. Mirrors the denomination fill in cashu-ts `splitAmount`:
/// zero-valued denominations are skipped (so a zero denomination can never
/// divide by zero), an empty keyset is rejected, and a value that cannot be
/// represented is reported rather than silently truncated.
fn fill_from_keyset(mut remaining: u64, denoms: &[u64], out: &mut Vec<u64>) -> Result<(), String> {
    if remaining == 0 {
        return Ok(());
    }
    if denoms.is_empty() {
        return Err("Cannot split amount, keyset is inactive or contains no keys".to_string());
    }

    let mut sorted = denoms.to_vec();
    sorted.sort_unstable_by(|a, b| b.cmp(a)); // descending

    for d in sorted {
        if d == 0 {
            continue;
        }
        let count = remaining / d;
        for _ in 0..count {
            out.push(d);
        }
        remaining -= d * count;
        if remaining == 0 {
            break;
        }
    }

    if remaining != 0 {
        return Err(format!("Unable to split remaining amount: {remaining}"));
    }
    Ok(())
}

/// A list of blinding results returned to C++.
///
/// On success `error` is null, `items` points to `len` results, and `amounts`
/// points to the matching `len` denominations (so the caller can label each
/// output without recomputing the split). On failure `items` and `amounts` are
/// null, `len` is zero, and `error` carries an owned message. In all cases the
/// struct must be released with `cdk_blind_result_list_free`.
#[repr(C)]
pub struct CdkBlindResultList {
    pub items: *mut CdkBlindResult,
    pub amounts: *mut u64,
    pub len: usize,
    pub error: *mut c_char,
}

/// Free the three heap strings owned by a single result.
fn free_result_fields(item: &CdkBlindResult) {
    unsafe {
        if !item.blinded_secret.is_null() {
            drop(CString::from_raw(item.blinded_secret));
        }
        if !item.blinding_factor.is_null() {
            drop(CString::from_raw(item.blinding_factor));
        }
        if !item.secret.is_null() {
            drop(CString::from_raw(item.secret));
        }
    }
}

/// Free a CdkBlindResultList allocated by this library.
#[no_mangle]
pub unsafe extern "C" fn cdk_blind_result_list_free(list: *mut CdkBlindResultList) {
    if list.is_null() {
        return;
    }
    let list = unsafe { Box::from_raw(list) };
    if !list.error.is_null() {
        drop(unsafe { CString::from_raw(list.error) });
    }
    if !list.items.is_null() && list.len > 0 {
        let items = unsafe { Box::from_raw(ptr::slice_from_raw_parts_mut(list.items, list.len)) };
        for item in items.iter() {
            free_result_fields(item);
        }
    }
    if !list.amounts.is_null() && list.len > 0 {
        drop(unsafe { Box::from_raw(ptr::slice_from_raw_parts_mut(list.amounts, list.len)) });
    }
}

/// Encode a sequence of blinding parts, freeing any already-encoded results if
/// a later encoding fails so an error path never leaks C strings.
fn encode_all(parts: Vec<BlindParts>) -> Result<Vec<CdkBlindResult>, String> {
    let mut items: Vec<CdkBlindResult> = Vec::with_capacity(parts.len());
    for p in parts {
        match p.encode() {
            Ok(item) => items.push(item),
            Err(e) => {
                for item in &items {
                    free_result_fields(item);
                }
                return Err(e);
            }
        }
    }
    Ok(items)
}

/// Wrap encoded results and their denominations in a success list.
fn list_ok(items: Vec<CdkBlindResult>, amounts: Vec<u64>) -> *mut CdkBlindResultList {
    let len = items.len();
    let items = Box::into_raw(items.into_boxed_slice()) as *mut CdkBlindResult;
    let amounts = Box::into_raw(amounts.into_boxed_slice()) as *mut u64;
    Box::into_raw(Box::new(CdkBlindResultList {
        items,
        amounts,
        len,
        error: ptr::null_mut(),
    }))
}

/// Wrap an owned message in an error list.
fn list_err(message: &str) -> *mut CdkBlindResultList {
    // Messages are ASCII with no interior NUL; if that ever fails, leave the
    // error pointer null (an empty len-0 list) rather than panicking across the
    // FFI boundary.
    let error = match CString::new(message) {
        Ok(c) => c.into_raw(),
        Err(_) => ptr::null_mut(),
    };
    Box::into_raw(Box::new(CdkBlindResultList {
        items: ptr::null_mut(),
        amounts: ptr::null_mut(),
        len: 0,
        error,
    }))
}

/// Encode built parts and pair them with their denominations, or report the
/// first error as an error list.
fn finish(built: Result<(Vec<BlindParts>, Vec<u64>), String>) -> *mut CdkBlindResultList {
    match built {
        Ok((parts, amounts)) => match encode_all(parts) {
            Ok(items) => list_ok(items, amounts),
            Err(e) => list_err(&e),
        },
        Err(e) => list_err(&e),
    }
}

/// Create a full set of random blinded messages for `amount`.
///
/// The amount is split (custom split or keyset denominations) entirely in Rust;
/// the caller only marshals inputs and outputs.
#[no_mangle]
pub unsafe extern "C" fn cdk_create_random_outputs(
    amount: f64,
    keyset_id: *const c_char,
    denoms: *const f64,
    denoms_len: usize,
    custom_split: *const f64,
    custom_split_len: usize,
) -> *mut CdkBlindResultList {
    finish((|| -> Result<(Vec<BlindParts>, Vec<u64>), String> {
        if unsafe { parse_keyset_id(keyset_id) }.is_none() {
            return Err("invalid keyset id".to_string());
        }
        let amount = to_u64(amount, "amount")?;
        let denoms = unsafe { read_u64_array(denoms, denoms_len, "Keyset denomination") }?;
        let custom = unsafe {
            read_opt_u64_array(custom_split, custom_split_len, "Custom split denomination")
        }?;
        let split = compute_split(amount, &denoms, custom.as_deref())?;
        let parts = split
            .iter()
            .map(|_| build_random_parts())
            .collect::<Result<Vec<_>, _>>()?;
        Ok((parts, split))
    })())
}

/// Create a full set of P2PK blinded messages for `amount`.
#[no_mangle]
pub unsafe extern "C" fn cdk_create_p2pk_outputs(
    amount: f64,
    keyset_id: *const c_char,
    denoms: *const f64,
    denoms_len: usize,
    custom_split: *const f64,
    custom_split_len: usize,
    pubkey_hex: *const c_char,
    additional_pubkeys: *const *const c_char,
    additional_pubkeys_len: u32,
    num_sigs: f64,
    locktime: f64,
    refund_pubkeys: *const *const c_char,
    refund_pubkeys_len: u32,
    num_sigs_refund: f64,
    sig_flag_ptr: *const c_char,
) -> *mut CdkBlindResultList {
    finish((|| -> Result<(Vec<BlindParts>, Vec<u64>), String> {
        if pubkey_hex.is_null() {
            return Err("missing pubkey".to_string());
        }
        if unsafe { parse_keyset_id(keyset_id) }.is_none() {
            return Err("invalid keyset id".to_string());
        }
        let amount = to_u64(amount, "amount")?;
        let num_sigs = to_u64(num_sigs, "numSigs")?;
        let locktime = to_u64(locktime, "locktime")?;
        let num_sigs_refund = to_u64(num_sigs_refund, "numSigsRefund")?;
        let pubkey_str = unsafe { CStr::from_ptr(pubkey_hex) }
            .to_str()
            .map_err(|_| "invalid pubkey".to_string())?;
        let pubkey = PublicKey::from_hex(pubkey_str).map_err(|e| e.to_string())?;
        let add_pks = parse_pubkey_array(additional_pubkeys, additional_pubkeys_len)
            .map_err(|()| "invalid additional pubkey".to_string())?;
        let refund_pks = parse_pubkey_array(refund_pubkeys, refund_pubkeys_len)
            .map_err(|()| "invalid refund pubkey".to_string())?;
        let sig_flag =
            unsafe { parse_sig_flag(sig_flag_ptr) }.map_err(|()| "invalid sig flag".to_string())?;
        let denoms = unsafe { read_u64_array(denoms, denoms_len, "Keyset denomination") }?;
        let custom = unsafe {
            read_opt_u64_array(custom_split, custom_split_len, "Custom split denomination")
        }?;
        let split = compute_split(amount, &denoms, custom.as_deref())?;
        let parts = split
            .iter()
            .map(|_| {
                build_p2pk_parts(
                    pubkey,
                    add_pks.clone(),
                    num_sigs,
                    locktime,
                    refund_pks.clone(),
                    num_sigs_refund,
                    sig_flag,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((parts, split))
    })())
}

/// Create a full set of deterministic blinded messages for `amount`.
///
/// The per-output counter is incremented in Rust: output `i` uses
/// `counter + i`.
#[no_mangle]
pub unsafe extern "C" fn cdk_create_deterministic_outputs(
    amount: f64,
    keyset_id: *const c_char,
    seed: *const u8,
    seed_len: u32,
    counter: f64,
    denoms: *const f64,
    denoms_len: usize,
    custom_split: *const f64,
    custom_split_len: usize,
) -> *mut CdkBlindResultList {
    finish((|| -> Result<(Vec<BlindParts>, Vec<u64>), String> {
        let id =
            unsafe { parse_keyset_id(keyset_id) }.ok_or_else(|| "invalid keyset id".to_string())?;
        let amount = to_u64(amount, "amount")?;
        let counter = to_u32(counter, "counter")?;
        if seed.is_null() || seed_len != 64 {
            return Err("seed must be 64 bytes".to_string());
        }
        let seed_slice = unsafe { slice::from_raw_parts(seed, seed_len as usize) };
        let seed_arr: &[u8; 64] = seed_slice
            .try_into()
            .map_err(|_| "seed must be 64 bytes".to_string())?;
        let denoms = unsafe { read_u64_array(denoms, denoms_len, "Keyset denomination") }?;
        let custom = unsafe {
            read_opt_u64_array(custom_split, custom_split_len, "Custom split denomination")
        }?;
        let split = compute_split(amount, &denoms, custom.as_deref())?;
        let parts = split
            .iter()
            .enumerate()
            .map(|(i, _)| {
                let c = counter
                    .checked_add(i as u32)
                    .ok_or_else(|| "counter overflow".to_string())?;
                build_deterministic_parts(seed_arr, id, c)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok((parts, split))
    })())
}

// ---------------------------------------------------------------------------
// Helper: read a CdkBlindResult field as an owned String (for tests)
// ---------------------------------------------------------------------------
#[cfg(test)]
unsafe fn read_cstr(ptr: *const c_char) -> String {
    CStr::from_ptr(ptr).to_str().unwrap().to_owned()
}

#[cfg(test)]
mod tests {
    use std::ffi::CString;

    use super::*;

    const TEST_KEYSET_ID: &str = "009a1f293253e41e";
    // A well-known test pubkey (compressed, 33 bytes hex)
    const TEST_PUBKEY: &str = "02a1633cafcc01ebfb6d78e39f687a1f0995c62fc95f51ead10a02ee0be551b5dc";
    // A second distinct valid pubkey, for multisig/refund tests.
    const TEST_PUBKEY_2: &str =
        "02a4ed09e9b22c0563f2043593902973d040054ff03be93c990264177d65123982";

    fn keyset_id_cstr() -> CString {
        CString::new(TEST_KEYSET_ID).unwrap()
    }

    fn pubkey_cstr() -> CString {
        CString::new(TEST_PUBKEY).unwrap()
    }

    // --------------------------------------------------
    // Random blinded messages
    // --------------------------------------------------

    #[test]
    fn random_blinded_message_returns_non_null() {
        let kid = keyset_id_cstr();
        let res = unsafe { cdk_create_random_blinded_message(64, kid.as_ptr()) };
        assert!(!res.is_null());
        unsafe { cdk_blind_result_free(res) };
    }

    #[test]
    fn random_blinded_message_fields_are_valid_hex() {
        let kid = keyset_id_cstr();
        let res = unsafe { cdk_create_random_blinded_message(1, kid.as_ptr()) };
        assert!(!res.is_null());

        unsafe {
            let bs = read_cstr((*res).blinded_secret);
            let bf = read_cstr((*res).blinding_factor);
            let secret = read_cstr((*res).secret);

            // blinded_secret is a compressed pubkey (02/03 + 64 hex chars = 66)
            assert!(bs.len() == 66, "blinded_secret len: {}", bs.len());
            assert!(bs.starts_with("02") || bs.starts_with("03"));

            // blinding_factor is a 32-byte secret key (64 hex chars)
            assert_eq!(bf.len(), 64, "blinding_factor len: {}", bf.len());

            // secret should be non-empty
            assert!(!secret.is_empty());

            cdk_blind_result_free(res);
        }
    }

    #[test]
    fn random_blinded_messages_are_unique() {
        let kid = keyset_id_cstr();
        let r1 = unsafe { cdk_create_random_blinded_message(1, kid.as_ptr()) };
        let r2 = unsafe { cdk_create_random_blinded_message(1, kid.as_ptr()) };
        assert!(!r1.is_null());
        assert!(!r2.is_null());

        unsafe {
            let s1 = read_cstr((*r1).secret);
            let s2 = read_cstr((*r2).secret);
            assert_ne!(s1, s2, "two random secrets must differ");

            let bs1 = read_cstr((*r1).blinded_secret);
            let bs2 = read_cstr((*r2).blinded_secret);
            assert_ne!(bs1, bs2, "two random blinded secrets must differ");

            cdk_blind_result_free(r1);
            cdk_blind_result_free(r2);
        }
    }

    // --------------------------------------------------
    // P2PK blinded messages
    // --------------------------------------------------

    #[test]
    fn p2pk_blinded_message_returns_non_null() {
        let kid = keyset_id_cstr();
        let pk = pubkey_cstr();
        let sig_flag = CString::new("SigInputs").unwrap();

        let res = unsafe {
            cdk_create_p2pk_blinded_message(
                64,
                kid.as_ptr(),
                pk.as_ptr(),
                ptr::null(),
                0,
                1.0,
                0.0,
                ptr::null(),
                0,
                0.0,
                sig_flag.as_ptr(),
            )
        };
        assert!(!res.is_null());
        unsafe { cdk_blind_result_free(res) };
    }

    #[test]
    fn p2pk_secret_contains_spending_conditions() {
        let kid = keyset_id_cstr();
        let pk = pubkey_cstr();
        let sig_flag = CString::new("SigInputs").unwrap();

        let res = unsafe {
            cdk_create_p2pk_blinded_message(
                1,
                kid.as_ptr(),
                pk.as_ptr(),
                ptr::null(),
                0,
                1.0,
                0.0,
                ptr::null(),
                0,
                0.0,
                sig_flag.as_ptr(),
            )
        };
        assert!(!res.is_null());

        unsafe {
            let secret = read_cstr((*res).secret);
            // NUT-10 P2PK secret is JSON: ["P2PK", { "nonce": ..., "data": "<pubkey>", ... }]
            assert!(secret.contains("P2PK"), "secret should contain P2PK kind");
            assert!(
                secret.contains(TEST_PUBKEY),
                "secret should embed the recipient pubkey"
            );

            cdk_blind_result_free(res);
        }
    }

    #[test]
    fn p2pk_with_invalid_pubkey_returns_null() {
        let kid = keyset_id_cstr();
        let bad_pk = CString::new("not_a_pubkey").unwrap();
        let sig_flag = CString::new("SigInputs").unwrap();

        let res = unsafe {
            cdk_create_p2pk_blinded_message(
                1,
                kid.as_ptr(),
                bad_pk.as_ptr(),
                ptr::null(),
                0,
                1.0,
                0.0,
                ptr::null(),
                0,
                0.0,
                sig_flag.as_ptr(),
            )
        };
        assert!(res.is_null(), "invalid pubkey should return null");
    }

    #[test]
    fn p2pk_with_locktime_and_multisig() {
        let kid = keyset_id_cstr();
        let pk = pubkey_cstr();
        let sig_flag = CString::new("SigAll").unwrap();

        // Use the same key as an additional pubkey for simplicity
        let add_pk = pubkey_cstr();
        let add_pks_ptrs = [add_pk.as_ptr()];

        // Far-future locktime; the validated constructor rejects past ones.
        let res = unsafe {
            cdk_create_p2pk_blinded_message(
                64,
                kid.as_ptr(),
                pk.as_ptr(),
                add_pks_ptrs.as_ptr(),
                1,
                2.0,          // num_sigs
                4102444800.0, // locktime (2100-01-01)
                ptr::null(),
                0,
                0.0,
                sig_flag.as_ptr(),
            )
        };
        assert!(!res.is_null());

        unsafe {
            let secret = read_cstr((*res).secret);
            assert!(secret.contains("P2PK"));
            // The secret should encode the conditions
            assert!(
                secret.contains("4102444800"),
                "locktime should be in secret"
            );

            cdk_blind_result_free(res);
        }
    }

    #[test]
    fn p2pk_past_locktime_returns_null() {
        let kid = keyset_id_cstr();
        let pk = pubkey_cstr();
        let sig_flag = CString::new("SigInputs").unwrap();

        // A refund key is required for a locktime to be a meaningful condition.
        let refund_pk = pubkey_cstr();
        let refund_pks_ptrs = [refund_pk.as_ptr()];

        let res = unsafe {
            cdk_create_p2pk_blinded_message(
                1,
                kid.as_ptr(),
                pk.as_ptr(),
                ptr::null(),
                0,
                1.0,
                1_000_000_000.0, // locktime in the past (2001)
                refund_pks_ptrs.as_ptr(),
                1,
                0.0,
                sig_flag.as_ptr(),
            )
        };
        assert!(res.is_null(), "past locktime must be rejected");
    }

    #[test]
    fn p2pk_zero_num_sigs_returns_null() {
        let kid = keyset_id_cstr();
        let pk = pubkey_cstr();
        let sig_flag = CString::new("SigInputs").unwrap();

        let res = unsafe {
            cdk_create_p2pk_blinded_message(
                1,
                kid.as_ptr(),
                pk.as_ptr(),
                ptr::null(),
                0,
                0.0, // num_sigs = 0 is invalid, not the default
                0.0,
                ptr::null(),
                0,
                0.0,
                sig_flag.as_ptr(),
            )
        };
        assert!(
            res.is_null(),
            "num_sigs = 0 must be rejected, not silently defaulted"
        );
    }

    #[test]
    fn p2pk_num_sigs_exceeding_keys_returns_null() {
        let kid = keyset_id_cstr();
        let pk = pubkey_cstr();
        let sig_flag = CString::new("SigInputs").unwrap();

        // Require 2 signatures with only the single primary key available.
        let res = unsafe {
            cdk_create_p2pk_blinded_message(
                1,
                kid.as_ptr(),
                pk.as_ptr(),
                ptr::null(),
                0,
                2.0, // num_sigs exceeds the one available key
                0.0,
                ptr::null(),
                0,
                0.0,
                sig_flag.as_ptr(),
            )
        };
        assert!(
            res.is_null(),
            "num_sigs greater than available keys must be rejected"
        );
    }

    #[test]
    fn p2pk_num_sigs_refund_appears_in_secret() {
        let kid = keyset_id_cstr();
        let pk = pubkey_cstr();
        let sig_flag = CString::new("SigInputs").unwrap();

        // Two distinct refund keys so that requiring 2 refund signatures is
        // feasible under the multisig validation.
        let refund_pk1 = pubkey_cstr();
        let refund_pk2 = CString::new(TEST_PUBKEY_2).unwrap();
        let refund_pks_ptrs = [refund_pk1.as_ptr(), refund_pk2.as_ptr()];

        let res = unsafe {
            cdk_create_p2pk_blinded_message(
                1,
                kid.as_ptr(),
                pk.as_ptr(),
                ptr::null(),
                0,
                1.0,
                0.0,
                refund_pks_ptrs.as_ptr(),
                2,
                2.0, // num_sigs_refund
                sig_flag.as_ptr(),
            )
        };
        assert!(!res.is_null());

        unsafe {
            let secret = read_cstr((*res).secret);
            assert!(
                secret.contains("n_sigs_refund") && secret.contains("\"2\""),
                "num_sigs_refund should appear in the secret, got: {}",
                secret
            );
            cdk_blind_result_free(res);
        }
    }

    #[test]
    fn p2pk_sigall_flag_appears_in_secret() {
        let kid = keyset_id_cstr();
        let pk = pubkey_cstr();
        let sig_flag = CString::new("SigAll").unwrap();

        let res = unsafe {
            cdk_create_p2pk_blinded_message(
                1,
                kid.as_ptr(),
                pk.as_ptr(),
                ptr::null(),
                0,
                1.0,
                0.0,
                ptr::null(),
                0,
                0.0,
                sig_flag.as_ptr(),
            )
        };
        assert!(!res.is_null());

        unsafe {
            let secret = read_cstr((*res).secret);
            assert!(
                secret.contains("SIG_ALL"),
                "SigAll flag must be serialized as SIG_ALL in the NUT-10 secret, got: {}",
                secret
            );
            cdk_blind_result_free(res);
        }
    }

    #[test]
    fn p2pk_null_sig_flag_defaults_to_sig_inputs() {
        let kid = keyset_id_cstr();
        let pk = pubkey_cstr();

        let res = unsafe {
            cdk_create_p2pk_blinded_message(
                1,
                kid.as_ptr(),
                pk.as_ptr(),
                ptr::null(),
                0,
                1.0,
                0.0,
                ptr::null(),
                0,
                0.0,
                ptr::null(),
            )
        };
        assert!(!res.is_null());

        unsafe {
            let secret = read_cstr((*res).secret);
            assert!(
                secret.contains("SIG_INPUTS"),
                "null sig_flag must default to SIG_INPUTS, got: {}",
                secret
            );
            cdk_blind_result_free(res);
        }
    }

    #[test]
    fn p2pk_unknown_sig_flag_returns_null() {
        let kid = keyset_id_cstr();
        let pk = pubkey_cstr();
        let bad_flag = CString::new("SigNone").unwrap();

        let res = unsafe {
            cdk_create_p2pk_blinded_message(
                1,
                kid.as_ptr(),
                pk.as_ptr(),
                ptr::null(),
                0,
                1.0,
                0.0,
                ptr::null(),
                0,
                0.0,
                bad_flag.as_ptr(),
            )
        };
        assert!(res.is_null(), "unknown sig_flag should return null");
    }

    #[test]
    fn p2pk_malformed_additional_pubkey_returns_null() {
        let kid = keyset_id_cstr();
        let pk = pubkey_cstr();
        let sig_flag = CString::new("SigInputs").unwrap();
        let bad_pk = CString::new("not_a_pubkey").unwrap();
        let add_pks_ptrs = [bad_pk.as_ptr()];

        let res = unsafe {
            cdk_create_p2pk_blinded_message(
                1,
                kid.as_ptr(),
                pk.as_ptr(),
                add_pks_ptrs.as_ptr(),
                1,
                2.0,
                0.0,
                ptr::null(),
                0,
                0.0,
                sig_flag.as_ptr(),
            )
        };
        assert!(
            res.is_null(),
            "malformed additional pubkey must fail, not be silently dropped"
        );
    }

    #[test]
    fn p2pk_malformed_refund_pubkey_returns_null() {
        let kid = keyset_id_cstr();
        let pk = pubkey_cstr();
        let sig_flag = CString::new("SigInputs").unwrap();
        let bad_pk = CString::new("not_a_pubkey").unwrap();
        let refund_pks_ptrs = [bad_pk.as_ptr()];

        let res = unsafe {
            cdk_create_p2pk_blinded_message(
                1,
                kid.as_ptr(),
                pk.as_ptr(),
                ptr::null(),
                0,
                1.0,
                0.0,
                refund_pks_ptrs.as_ptr(),
                1,
                0.0,
                sig_flag.as_ptr(),
            )
        };
        assert!(
            res.is_null(),
            "malformed refund pubkey must fail, not be silently dropped"
        );
    }

    // --------------------------------------------------
    // Deterministic blinded messages (NUT-13)
    // --------------------------------------------------

    #[test]
    fn deterministic_blinded_message_returns_non_null() {
        let kid = keyset_id_cstr();
        let seed = [0u8; 64];

        let res = unsafe {
            cdk_create_deterministic_blinded_message(1, kid.as_ptr(), seed.as_ptr(), 64, 0.0)
        };
        assert!(!res.is_null());
        unsafe { cdk_blind_result_free(res) };
    }

    #[test]
    fn deterministic_same_inputs_produce_same_outputs() {
        let kid = keyset_id_cstr();
        let seed = [42u8; 64];

        let r1 = unsafe {
            cdk_create_deterministic_blinded_message(1, kid.as_ptr(), seed.as_ptr(), 64, 0.0)
        };
        let r2 = unsafe {
            cdk_create_deterministic_blinded_message(1, kid.as_ptr(), seed.as_ptr(), 64, 0.0)
        };
        assert!(!r1.is_null());
        assert!(!r2.is_null());

        unsafe {
            assert_eq!(read_cstr((*r1).secret), read_cstr((*r2).secret));
            assert_eq!(
                read_cstr((*r1).blinded_secret),
                read_cstr((*r2).blinded_secret)
            );
            assert_eq!(
                read_cstr((*r1).blinding_factor),
                read_cstr((*r2).blinding_factor)
            );

            cdk_blind_result_free(r1);
            cdk_blind_result_free(r2);
        }
    }

    #[test]
    fn deterministic_different_counters_produce_different_outputs() {
        let kid = keyset_id_cstr();
        let seed = [42u8; 64];

        let r1 = unsafe {
            cdk_create_deterministic_blinded_message(1, kid.as_ptr(), seed.as_ptr(), 64, 0.0)
        };
        let r2 = unsafe {
            cdk_create_deterministic_blinded_message(1, kid.as_ptr(), seed.as_ptr(), 64, 1.0)
        };
        assert!(!r1.is_null());
        assert!(!r2.is_null());

        unsafe {
            assert_ne!(
                read_cstr((*r1).secret),
                read_cstr((*r2).secret),
                "different counters must produce different secrets"
            );

            cdk_blind_result_free(r1);
            cdk_blind_result_free(r2);
        }
    }

    #[test]
    fn deterministic_different_seeds_produce_different_outputs() {
        let kid = keyset_id_cstr();
        let seed_a = [1u8; 64];
        let seed_b = [2u8; 64];

        let r1 = unsafe {
            cdk_create_deterministic_blinded_message(1, kid.as_ptr(), seed_a.as_ptr(), 64, 0.0)
        };
        let r2 = unsafe {
            cdk_create_deterministic_blinded_message(1, kid.as_ptr(), seed_b.as_ptr(), 64, 0.0)
        };
        assert!(!r1.is_null());
        assert!(!r2.is_null());

        unsafe {
            assert_ne!(
                read_cstr((*r1).secret),
                read_cstr((*r2).secret),
                "different seeds must produce different secrets"
            );

            cdk_blind_result_free(r1);
            cdk_blind_result_free(r2);
        }
    }

    #[test]
    fn deterministic_wrong_seed_length_returns_null() {
        let kid = keyset_id_cstr();
        let short_seed = [0u8; 32]; // NUT-13 requires 64 bytes

        let res = unsafe {
            cdk_create_deterministic_blinded_message(1, kid.as_ptr(), short_seed.as_ptr(), 32, 0.0)
        };
        assert!(res.is_null(), "seed != 64 bytes should return null");
    }

    // --------------------------------------------------
    // Free safety
    // --------------------------------------------------

    #[test]
    fn free_null_is_safe() {
        unsafe { cdk_blind_result_free(ptr::null_mut()) };
        // Should not crash
    }

    #[test]
    fn list_free_null_is_safe() {
        unsafe { cdk_blind_result_list_free(ptr::null_mut()) };
        // Should not crash
    }

    // --------------------------------------------------
    // Split logic
    // --------------------------------------------------

    // Standard power-of-two keyset used across split tests.
    const DENOMS: [u64; 11] = [1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024];

    #[test]
    fn split_fills_greedy_largest_first() {
        // 13 = 8 + 4 + 1, returned largest-first.
        let split = compute_split(13, &DENOMS, None).unwrap();
        assert_eq!(split, vec![8, 4, 1]);
    }

    #[test]
    fn split_without_keyset_is_rejected() {
        // cashu-ts throws when the keyset has no keys to fill from.
        let err = compute_split(13, &[], None).unwrap_err();
        assert_eq!(
            err,
            "Cannot split amount, keyset is inactive or contains no keys"
        );
    }

    #[test]
    fn split_reports_unrepresentable_remainder() {
        // Only a denomination of 3 available; 10 leaves a remainder of 1.
        let err = compute_split(10, &[3], None).unwrap_err();
        assert_eq!(err, "Unable to split remaining amount: 1");
    }

    #[test]
    fn split_skips_zero_denomination_without_panicking() {
        // A zero denomination is skipped, not divided by; here it leaves the
        // whole amount unrepresentable rather than panicking across the FFI.
        let err = compute_split(10, &[0], None).unwrap_err();
        assert_eq!(err, "Unable to split remaining amount: 10");

        // With a usable denomination present, the zero is simply ignored.
        let split = compute_split(10, &[0, 2], None).unwrap();
        assert_eq!(split, vec![2, 2, 2, 2, 2]);
    }

    #[test]
    fn exact_custom_split_is_returned_in_caller_order() {
        let custom = [8u64, 2];
        let split = compute_split(10, &DENOMS, Some(&custom)).unwrap();
        assert_eq!(split, vec![8, 2]);
    }

    #[test]
    fn partial_custom_split_is_filled_from_keyset() {
        // Positive entries kept first, remainder (10 - 2 = 8) filled greedily.
        let custom = [2u64];
        let split = compute_split(10, &DENOMS, Some(&custom)).unwrap();
        assert_eq!(split, vec![2, 8]);
    }

    #[test]
    fn zero_entries_in_custom_split_are_ignored_for_positive_amount() {
        let custom = [2u64, 0, 8];
        let split = compute_split(10, &DENOMS, Some(&custom)).unwrap();
        assert_eq!(split, vec![2, 8]);
    }

    #[test]
    fn zero_total_custom_split_returns_verbatim() {
        // Restore / NUT-08 blank outputs: amount 0 with an all-zero split.
        assert_eq!(compute_split(0, &DENOMS, Some(&[0])).unwrap(), vec![0]);
        assert_eq!(
            compute_split(0, &DENOMS, Some(&[0, 0, 0])).unwrap(),
            vec![0, 0, 0]
        );
    }

    #[test]
    fn custom_split_greater_than_amount_is_rejected() {
        let err = compute_split(10, &DENOMS, Some(&[8, 8])).unwrap_err();
        assert_eq!(err, "Split is greater than total amount: 16 > 10");
    }

    #[test]
    fn custom_split_denomination_not_in_keyset_is_rejected() {
        // 3 is not a keyset denomination.
        let err = compute_split(10, &DENOMS, Some(&[3])).unwrap_err();
        assert_eq!(
            err,
            "Provided amount preferences do not match the amounts of the mint keyset."
        );
    }

    #[test]
    fn to_u64_rejects_non_integers() {
        assert!(to_u64(1.5, "amount").is_err());
        assert!(to_u64(-1.0, "amount").is_err());
        assert!(to_u64(f64::NAN, "amount").is_err());
        assert!(to_u64(f64::INFINITY, "amount").is_err());
        assert_eq!(to_u64(42.0, "amount").unwrap(), 42);
    }

    // --------------------------------------------------
    // List constructors
    // --------------------------------------------------

    #[test]
    fn random_outputs_splits_and_returns_list() {
        let kid = keyset_id_cstr();
        let denoms = [1.0f64, 2.0, 4.0, 8.0];
        let list = unsafe {
            cdk_create_random_outputs(13.0, kid.as_ptr(), denoms.as_ptr(), 4, ptr::null(), 0)
        };
        assert!(!list.is_null());
        unsafe {
            assert!((*list).error.is_null());
            // 13 = 8 + 4 + 1
            assert_eq!((*list).len, 3);
            cdk_blind_result_list_free(list);
        }
    }

    #[test]
    fn random_outputs_reports_error_on_bad_split() {
        let kid = keyset_id_cstr();
        let denoms = [3.0f64];
        let list = unsafe {
            cdk_create_random_outputs(10.0, kid.as_ptr(), denoms.as_ptr(), 1, ptr::null(), 0)
        };
        assert!(!list.is_null());
        unsafe {
            assert_eq!((*list).len, 0);
            assert!(!(*list).error.is_null());
            let msg = read_cstr((*list).error);
            assert_eq!(msg, "Unable to split remaining amount: 1");
            cdk_blind_result_list_free(list);
        }
    }

    #[test]
    fn deterministic_outputs_increment_counter_per_output() {
        let kid = keyset_id_cstr();
        let seed = [7u8; 64];
        // Custom split into three outputs; each should use counter 5, 6, 7.
        let denoms = [1.0f64, 2.0, 4.0, 8.0];
        let custom = [1.0f64, 1.0, 1.0];
        let list = unsafe {
            cdk_create_deterministic_outputs(
                3.0,
                kid.as_ptr(),
                seed.as_ptr(),
                64,
                5.0,
                denoms.as_ptr(),
                4,
                custom.as_ptr(),
                3,
            )
        };
        assert!(!list.is_null());
        unsafe {
            assert!((*list).error.is_null());
            assert_eq!((*list).len, 3);

            // Recompute the expected secret for counter 6 (the second output)
            // and confirm it matches, proving the per-output increment.
            let id: Id = TEST_KEYSET_ID.parse().unwrap();
            let expected = Secret::from_seed(&seed, id, 6).unwrap().to_string();
            let second = read_cstr((*(*list).items.add(1)).secret);
            assert_eq!(second, expected);

            cdk_blind_result_list_free(list);
        }
    }

    #[test]
    fn random_outputs_skips_zero_denomination_without_panicking() {
        // A zero-valued keyset denomination must be skipped, not divided by, so
        // the call returns a clean error instead of aborting the process.
        let kid = keyset_id_cstr();
        let denoms = [0.0f64];
        let list = unsafe {
            cdk_create_random_outputs(10.0, kid.as_ptr(), denoms.as_ptr(), 1, ptr::null(), 0)
        };
        assert!(!list.is_null());
        unsafe {
            assert_eq!((*list).len, 0);
            assert!(!(*list).error.is_null());
            let msg = read_cstr((*list).error);
            assert_eq!(msg, "Unable to split remaining amount: 10");
            cdk_blind_result_list_free(list);
        }
    }

    #[test]
    fn p2pk_outputs_rejects_non_integral_num_sigs() {
        // A non-integer numeric option now fails validation in Rust instead of
        // reaching an out-of-range cast in C++.
        let kid = keyset_id_cstr();
        let pk = pubkey_cstr();
        let sig_flag = CString::new("SigInputs").unwrap();
        let list = unsafe {
            cdk_create_p2pk_outputs(
                1.0,
                kid.as_ptr(),
                ptr::null(),
                0,
                ptr::null(),
                0,
                pk.as_ptr(),
                ptr::null(),
                0,
                1.5, // num_sigs is not an integer
                0.0,
                ptr::null(),
                0,
                0.0,
                sig_flag.as_ptr(),
            )
        };
        assert!(!list.is_null());
        unsafe {
            assert_eq!((*list).len, 0);
            assert!(!(*list).error.is_null());
            let msg = read_cstr((*list).error);
            assert_eq!(msg, "numSigs is not a valid unsigned integer");
            cdk_blind_result_list_free(list);
        }
    }
}
