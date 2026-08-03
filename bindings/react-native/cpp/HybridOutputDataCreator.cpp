#include "HybridOutputDataCreator.hpp"
#include <cstddef>
#include <cstdint>
#include <stdexcept>
#include <string>
#include <vector>

// C API provided by the cdk-nitro Rust crate. All amount splitting,
// validation, and the per-denomination loops live in Rust; this file only
// marshals values across the FFI boundary.
#include "cdk_nitro.h"

namespace margelo::nitro::cashudevkit {

namespace {

// Keyset key entries reduced to a plain array of denominations for the Rust
// splitter.
std::vector<double> denomsOf(const std::vector<KeyEntry>& keys) {
  std::vector<double> denoms;
  denoms.reserve(keys.size());
  for (const auto& key : keys) {
    denoms.push_back(key.amount);
  }
  return denoms;
}

// Pointer/length for an optional custom split.
const double* customPtr(const std::optional<std::vector<double>>& customSplit) {
  return customSplit.has_value() && !customSplit->empty() ? customSplit->data()
                                                          : nullptr;
}
size_t customLen(const std::optional<std::vector<double>>& customSplit) {
  return customSplit.has_value() ? customSplit->size() : 0;
}

// Optional list of pubkeys marshalled into C string pointers.
std::vector<const char*> cstrs(
    const std::optional<std::vector<std::string>>& pubkeys) {
  std::vector<const char*> out;
  if (pubkeys.has_value()) {
    out.reserve(pubkeys->size());
    for (const auto& s : pubkeys.value()) {
      out.push_back(s.c_str());
    }
  }
  return out;
}

OutputData outputOf(const CdkBlindResult& item, double amount,
                    const std::string& keysetId) {
  // OutputData is generated with an explicit constructor (not an aggregate),
  // so build it positionally rather than with designated initializers.
  return OutputData(amount, keysetId, std::string(item.blinded_secret),
                    std::string(item.blinding_factor),
                    std::string(item.secret));
}

// Turn a single-message result into OutputData, throwing on null, then free it.
// The Rust crypto ignores the amount, so it is applied here purely as the
// output label.
OutputData single(CdkBlindResult* res, double amount,
                  const std::string& keysetId, const char* failure) {
  if (!res) {
    throw std::runtime_error(failure);
  }
  OutputData out = outputOf(*res, amount, keysetId);
  cdk_blind_result_free(res);
  return out;
}

// Turn a Rust result list into OutputData, rethrowing its error, then free it.
std::vector<OutputData> collect(CdkBlindResultList* list,
                                const std::string& keysetId) {
  if (!list) {
    throw std::runtime_error("cdk-nitro returned no result");
  }
  if (list->error != nullptr) {
    std::string message(list->error);
    cdk_blind_result_list_free(list);
    throw std::runtime_error(message);
  }
  std::vector<OutputData> results;
  results.reserve(list->len);
  for (size_t i = 0; i < list->len; i++) {
    results.push_back(outputOf(list->items[i],
                               static_cast<double>(list->amounts[i]), keysetId));
  }
  cdk_blind_result_list_free(list);
  return results;
}

} // namespace

OutputData HybridOutputDataCreator::createSingleRandomData(
    double amount, const std::string& keysetId) {
  auto* res = cdk_create_random_blinded_message(0, keysetId.c_str());
  return single(res, amount, keysetId, "Failed to create random blinded message");
}

std::vector<OutputData> HybridOutputDataCreator::createRandomData(
    double amount,
    const std::string& keysetId,
    const std::vector<KeyEntry>& keys,
    const std::optional<std::vector<double>>& customSplit) {
  auto denoms = denomsOf(keys);
  auto* list = cdk_create_random_outputs(
    amount, keysetId.c_str(),
    denoms.empty() ? nullptr : denoms.data(), denoms.size(),
    customPtr(customSplit), customLen(customSplit));
  return collect(list, keysetId);
}

OutputData HybridOutputDataCreator::createSingleP2PKData(
    const P2PKOptions& p2pk,
    double amount,
    const std::string& keysetId) {
  auto addPubkeys = cstrs(p2pk.additionalPubkeys);
  auto refundPks = cstrs(p2pk.refundPubkeys);
  auto* res = cdk_create_p2pk_blinded_message(
    0,
    keysetId.c_str(),
    p2pk.pubkey.c_str(),
    addPubkeys.empty() ? nullptr : addPubkeys.data(),
    static_cast<uint32_t>(addPubkeys.size()),
    p2pk.numSigs.value_or(1),
    p2pk.locktime.value_or(0),
    refundPks.empty() ? nullptr : refundPks.data(),
    static_cast<uint32_t>(refundPks.size()),
    p2pk.numSigsRefund.value_or(0),
    p2pk.sigFlag.has_value() ? p2pk.sigFlag.value().c_str() : "SigInputs");
  return single(res, amount, keysetId, "Failed to create P2PK blinded message");
}

std::vector<OutputData> HybridOutputDataCreator::createP2PKData(
    const P2PKOptions& p2pk,
    double amount,
    const std::string& keysetId,
    const std::vector<KeyEntry>& keys,
    const std::optional<std::vector<double>>& customSplit) {
  auto denoms = denomsOf(keys);
  auto addPubkeys = cstrs(p2pk.additionalPubkeys);
  auto refundPks = cstrs(p2pk.refundPubkeys);
  auto* list = cdk_create_p2pk_outputs(
    amount,
    keysetId.c_str(),
    denoms.empty() ? nullptr : denoms.data(), denoms.size(),
    customPtr(customSplit), customLen(customSplit),
    p2pk.pubkey.c_str(),
    addPubkeys.empty() ? nullptr : addPubkeys.data(),
    static_cast<uint32_t>(addPubkeys.size()),
    p2pk.numSigs.value_or(1),
    p2pk.locktime.value_or(0),
    refundPks.empty() ? nullptr : refundPks.data(),
    static_cast<uint32_t>(refundPks.size()),
    p2pk.numSigsRefund.value_or(0),
    p2pk.sigFlag.has_value() ? p2pk.sigFlag.value().c_str() : "SigInputs");
  return collect(list, keysetId);
}

OutputData HybridOutputDataCreator::createSingleDeterministicData(
    double amount,
    const std::shared_ptr<ArrayBuffer>& seed,
    double counter,
    const std::string& keysetId) {
  auto* res = cdk_create_deterministic_blinded_message(
    0,
    keysetId.c_str(),
    seed->data(),
    static_cast<uint32_t>(seed->size()),
    counter);
  return single(res, amount, keysetId,
                "Failed to create deterministic blinded message");
}

std::vector<OutputData> HybridOutputDataCreator::createDeterministicData(
    double amount,
    const std::shared_ptr<ArrayBuffer>& seed,
    double counter,
    const std::string& keysetId,
    const std::vector<KeyEntry>& keys,
    const std::optional<std::vector<double>>& customSplit) {
  auto denoms = denomsOf(keys);
  auto* list = cdk_create_deterministic_outputs(
    amount,
    keysetId.c_str(),
    seed->data(),
    static_cast<uint32_t>(seed->size()),
    counter,
    denoms.empty() ? nullptr : denoms.data(), denoms.size(),
    customPtr(customSplit), customLen(customSplit));
  return collect(list, keysetId);
}

} // namespace margelo::nitro::cashudevkit
