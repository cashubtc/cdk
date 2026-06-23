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

fn make_result(
    blinded_secret: PublicKey,
    blinding_factor: SecretKey,
    secret: &Secret,
) -> *mut CdkBlindResult {
    let result = Box::new(CdkBlindResult {
        blinded_secret: CString::new(blinded_secret.to_string()).unwrap().into_raw(),
        blinding_factor: CString::new(blinding_factor.to_string())
            .unwrap()
            .into_raw(),
        secret: CString::new(secret.to_string()).unwrap().into_raw(),
    });
    Box::into_raw(result)
}

/// Create a random blinded message (ephemeral secret).
/// B_ = hash_to_curve(secret) + r*G
#[no_mangle]
pub extern "C" fn cdk_create_random_blinded_message(
    _amount: u64,
    _keyset_id: *const c_char,
) -> *mut CdkBlindResult {
    let secret = Secret::generate();

    let (blinded_secret, r) = match blind_message(&secret.to_bytes(), None) {
        Ok(res) => res,
        Err(_) => return ptr::null_mut(),
    };

    make_result(blinded_secret, r, &secret)
}

/// Create a P2PK blinded message locked to a public key.
#[no_mangle]
pub unsafe extern "C" fn cdk_create_p2pk_blinded_message(
    _amount: u64,
    _keyset_id: *const c_char,
    pubkey_hex: *const c_char,
    additional_pubkeys: *const *const c_char,
    additional_pubkeys_len: u32,
    num_sigs: u64,
    locktime: u64,
    refund_pubkeys: *const *const c_char,
    refund_pubkeys_len: u32,
    num_sigs_refund: u64,
    sig_flag_ptr: *const c_char,
) -> *mut CdkBlindResult {
    if pubkey_hex.is_null() {
        return ptr::null_mut();
    }

    let pubkey_str = match unsafe { CStr::from_ptr(pubkey_hex) }.to_str() {
        Ok(s) => s,
        Err(_) => return ptr::null_mut(),
    };

    let pubkey = match PublicKey::from_hex(pubkey_str) {
        Ok(pk) => pk,
        Err(_) => return ptr::null_mut(),
    };

    let add_pks = match parse_pubkey_array(additional_pubkeys, additional_pubkeys_len) {
        Ok(pks) => pks,
        Err(()) => return ptr::null_mut(),
    };
    let refund_pks = match parse_pubkey_array(refund_pubkeys, refund_pubkeys_len) {
        Ok(pks) => pks,
        Err(()) => return ptr::null_mut(),
    };

    let sig_flag = if sig_flag_ptr.is_null() {
        SigFlag::default()
    } else {
        match unsafe { CStr::from_ptr(sig_flag_ptr) }.to_str() {
            Ok("SigAll") => SigFlag::SigAll,
            Ok("SigInputs") => SigFlag::SigInputs,
            Ok(_) => return ptr::null_mut(),
            Err(_) => return ptr::null_mut(),
        }
    };

    let conditions = Conditions {
        locktime: if locktime > 0 { Some(locktime) } else { None },
        pubkeys: add_pks,
        refund_keys: refund_pks,
        num_sigs: if num_sigs > 1 { Some(num_sigs) } else { None },
        sig_flag,
        num_sigs_refund: if num_sigs_refund > 1 {
            Some(num_sigs_refund)
        } else {
            None
        },
    };

    let spending_conditions = SpendingConditions::P2PKConditions {
        data: pubkey,
        conditions: Some(conditions),
    };

    // NUT-10: encode spending conditions into the secret as JSON
    let nut10_secret: cashu::nuts::nut10::Secret = spending_conditions.into();
    let secret_json = match serde_json::to_string(&nut10_secret) {
        Ok(s) => s,
        Err(_) => return ptr::null_mut(),
    };
    let secret = Secret::new(secret_json);

    let (blinded_secret, r) = match blind_message(&secret.to_bytes(), None) {
        Ok(res) => res,
        Err(_) => return ptr::null_mut(),
    };

    make_result(blinded_secret, r, &secret)
}

/// Create a deterministic blinded message from seed + keyset_id + counter (NUT-13).
#[no_mangle]
pub unsafe extern "C" fn cdk_create_deterministic_blinded_message(
    _amount: u64,
    keyset_id: *const c_char,
    seed: *const u8,
    seed_len: u32,
    counter: u32,
) -> *mut CdkBlindResult {
    if keyset_id.is_null() {
        return ptr::null_mut();
    }

    let keyset_id_str = match unsafe { CStr::from_ptr(keyset_id) }.to_str() {
        Ok(s) => s,
        Err(_) => return ptr::null_mut(),
    };

    let id = match keyset_id_str.parse::<Id>() {
        Ok(id) => id,
        Err(_) => return ptr::null_mut(),
    };

    // Validate pointer and length before creating the slice to avoid UB
    if seed.is_null() || seed_len != 64 {
        return ptr::null_mut();
    }

    let seed_slice = unsafe { slice::from_raw_parts(seed, seed_len as usize) };
    let seed_arr: &[u8; 64] = seed_slice.try_into().unwrap();

    // Derive secret and blinding factor deterministically
    let secret = match Secret::from_seed(seed_arr, id, counter) {
        Ok(s) => s,
        Err(_) => return ptr::null_mut(),
    };

    let blinding_key = match SecretKey::from_seed(seed_arr, id, counter) {
        Ok(k) => k,
        Err(_) => return ptr::null_mut(),
    };

    let (blinded_secret, r) = match blind_message(&secret.to_bytes(), Some(blinding_key)) {
        Ok(res) => res,
        Err(_) => return ptr::null_mut(),
    };

    make_result(blinded_secret, r, &secret)
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

// ---------------------------------------------------------------------------
// Helper: read a CdkBlindResult field as a &str (for tests)
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
        let res = cdk_create_random_blinded_message(64, kid.as_ptr());
        assert!(!res.is_null());
        unsafe { cdk_blind_result_free(res) };
    }

    #[test]
    fn random_blinded_message_fields_are_valid_hex() {
        let kid = keyset_id_cstr();
        let res = cdk_create_random_blinded_message(1, kid.as_ptr());
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
        let r1 = cdk_create_random_blinded_message(1, kid.as_ptr());
        let r2 = cdk_create_random_blinded_message(1, kid.as_ptr());
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
                1,
                0,
                ptr::null(),
                0,
                0,
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
                1,
                0,
                ptr::null(),
                0,
                0,
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
                1,
                0,
                ptr::null(),
                0,
                0,
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

        let res = unsafe {
            cdk_create_p2pk_blinded_message(
                64,
                kid.as_ptr(),
                pk.as_ptr(),
                add_pks_ptrs.as_ptr(),
                1,
                2,          // num_sigs
                1700000000, // locktime
                ptr::null(),
                0,
                0,
                sig_flag.as_ptr(),
            )
        };
        assert!(!res.is_null());

        unsafe {
            let secret = read_cstr((*res).secret);
            assert!(secret.contains("P2PK"));
            // The secret should encode the conditions
            assert!(
                secret.contains("1700000000"),
                "locktime should be in secret"
            );

            cdk_blind_result_free(res);
        }
    }

    #[test]
    fn p2pk_num_sigs_refund_appears_in_secret() {
        let kid = keyset_id_cstr();
        let pk = pubkey_cstr();
        let sig_flag = CString::new("SigInputs").unwrap();

        let refund_pk = pubkey_cstr();
        let refund_pks_ptrs = [refund_pk.as_ptr()];

        let res = unsafe {
            cdk_create_p2pk_blinded_message(
                1,
                kid.as_ptr(),
                pk.as_ptr(),
                ptr::null(),
                0,
                1,
                0,
                refund_pks_ptrs.as_ptr(),
                1,
                2, // num_sigs_refund
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
                1,
                0,
                ptr::null(),
                0,
                0,
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
                1,
                0,
                ptr::null(),
                0,
                0,
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
                1,
                0,
                ptr::null(),
                0,
                0,
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
                2,
                0,
                ptr::null(),
                0,
                0,
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
                1,
                0,
                refund_pks_ptrs.as_ptr(),
                1,
                0,
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
            cdk_create_deterministic_blinded_message(1, kid.as_ptr(), seed.as_ptr(), 64, 0)
        };
        assert!(!res.is_null());
        unsafe { cdk_blind_result_free(res) };
    }

    #[test]
    fn deterministic_same_inputs_produce_same_outputs() {
        let kid = keyset_id_cstr();
        let seed = [42u8; 64];

        let r1 = unsafe {
            cdk_create_deterministic_blinded_message(1, kid.as_ptr(), seed.as_ptr(), 64, 0)
        };
        let r2 = unsafe {
            cdk_create_deterministic_blinded_message(1, kid.as_ptr(), seed.as_ptr(), 64, 0)
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
            cdk_create_deterministic_blinded_message(1, kid.as_ptr(), seed.as_ptr(), 64, 0)
        };
        let r2 = unsafe {
            cdk_create_deterministic_blinded_message(1, kid.as_ptr(), seed.as_ptr(), 64, 1)
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
            cdk_create_deterministic_blinded_message(1, kid.as_ptr(), seed_a.as_ptr(), 64, 0)
        };
        let r2 = unsafe {
            cdk_create_deterministic_blinded_message(1, kid.as_ptr(), seed_b.as_ptr(), 64, 0)
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
            cdk_create_deterministic_blinded_message(1, kid.as_ptr(), short_seed.as_ptr(), 32, 0)
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
}
