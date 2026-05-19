#pragma once

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct {
  char* blinded_secret;  // hex-encoded compressed public key (B_)
  char* blinding_factor; // hex-encoded secret key (r)
  char* secret;          // serialized secret string
} CdkBlindResult;

/// Free a CdkBlindResult allocated by the Rust library.
void cdk_blind_result_free(CdkBlindResult* result);

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
  uint64_t num_sigs,
  uint64_t locktime,
  const char* const* refund_pubkeys,
  uint32_t refund_pubkeys_len,
  uint64_t num_sigs_refund,
  const char* sig_flag
);

/// Create a deterministic blinded message from a BIP32 seed + counter.
/// Returns NULL on error.
CdkBlindResult* cdk_create_deterministic_blinded_message(
  uint64_t amount,
  const char* keyset_id,
  const uint8_t* seed,
  uint32_t seed_len,
  uint32_t counter
);

#ifdef __cplusplus
}
#endif
