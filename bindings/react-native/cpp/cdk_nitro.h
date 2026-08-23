#pragma once

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct {
  char* blinded_secret;  // hex-encoded compressed public key (B_)
  char* blinding_factor; // hex-encoded secret key (r)
  char* secret;          // serialized secret string
} CdkBlindResult;

/// A set of blinding results, or an error.
///
/// On success `error` is NULL, `items` points to `len` results, and `amounts`
/// points to the matching `len` denominations so each output can be labeled
/// without recomputing the split. On failure `items` and `amounts` are NULL,
/// `len` is 0, and `error` holds an owned message. Always release with
/// cdk_blind_result_list_free.
typedef struct {
  CdkBlindResult* items;
  uint64_t*       amounts;
  size_t          len;
  char*           error;
} CdkBlindResultList;

/// Free a CdkBlindResult allocated by the Rust library.
void cdk_blind_result_free(CdkBlindResult* result);

/// Free a CdkBlindResultList allocated by the Rust library.
void cdk_blind_result_list_free(CdkBlindResultList* list);

/// Create a random blinded message with an ephemeral secret.
/// Returns NULL on error.
CdkBlindResult* cdk_create_random_blinded_message(
  uint64_t amount,
  const char* keyset_id
);

/// Create a P2PK blinded message locked to a public key.
/// Returns NULL on error.
CdkBlindResult* cdk_create_p2pk_blinded_message(
  uint64_t amount,
  const char* keyset_id,
  const char* pubkey_hex,
  const char* const* additional_pubkeys,
  uint32_t additional_pubkeys_len,
  double num_sigs,
  double locktime,
  const char* const* refund_pubkeys,
  uint32_t refund_pubkeys_len,
  double num_sigs_refund,
  const char* sig_flag
);

/// Create a deterministic blinded message from a BIP32 seed + counter.
/// Returns NULL on error.
CdkBlindResult* cdk_create_deterministic_blinded_message(
  uint64_t amount,
  const char* keyset_id,
  const uint8_t* seed,
  uint32_t seed_len,
  double counter
);

/// Create a full set of random blinded messages for an amount.
///
/// The amount is split entirely in Rust: a custom split when `custom_split` is
/// non-NULL (any shortfall filled greedily from `denoms`), otherwise greedily
/// over `denoms`. For a positive amount, an empty `denoms` list is rejected
/// (matching cashu-ts) via the list's `error` field.
/// Never returns NULL; check the list's `error` field.
CdkBlindResultList* cdk_create_random_outputs(
  double amount,
  const char* keyset_id,
  const double* denoms,
  size_t denoms_len,
  const double* custom_split,
  size_t custom_split_len
);

/// Create a full set of P2PK blinded messages for an amount.
/// Never returns NULL; check the list's `error` field.
CdkBlindResultList* cdk_create_p2pk_outputs(
  double amount,
  const char* keyset_id,
  const double* denoms,
  size_t denoms_len,
  const double* custom_split,
  size_t custom_split_len,
  const char* pubkey_hex,
  const char* const* additional_pubkeys,
  uint32_t additional_pubkeys_len,
  double num_sigs,
  double locktime,
  const char* const* refund_pubkeys,
  uint32_t refund_pubkeys_len,
  double num_sigs_refund,
  const char* sig_flag
);

/// Create a full set of deterministic blinded messages for an amount.
///
/// Output `i` uses `counter + i`. Never returns NULL; check the list's `error`
/// field.
CdkBlindResultList* cdk_create_deterministic_outputs(
  double amount,
  const char* keyset_id,
  const uint8_t* seed,
  uint32_t seed_len,
  double counter,
  const double* denoms,
  size_t denoms_len,
  const double* custom_split,
  size_t custom_split_len
);

#ifdef __cplusplus
}
#endif
