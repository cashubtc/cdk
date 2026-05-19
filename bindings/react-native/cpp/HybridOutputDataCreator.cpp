#include "HybridOutputDataCreator.hpp"
#include <algorithm>
#include <stdexcept>

// C API provided by the cdk-nitro Rust crate
#include "cdk_nitro.h"

namespace cashudevkit {

std::vector<uint64_t> HybridOutputDataCreator::splitAmount(
    uint64_t amount,
    const std::vector<KeyEntry>& keys,
    const std::optional<std::vector<double>>& customSplit) {

  if (customSplit.has_value()) {
    std::vector<uint64_t> result;
    for (double v : customSplit.value()) {
      result.push_back(static_cast<uint64_t>(v));
    }
    return result;
  }

  // Collect available denominations from keyset keys
  std::vector<uint64_t> denoms;
  for (const auto& key : keys) {
    denoms.push_back(static_cast<uint64_t>(key.amount));
  }
  std::sort(denoms.rbegin(), denoms.rend()); // descending

  // Greedy split into available denominations
  std::vector<uint64_t> result;
  uint64_t remaining = amount;
  for (uint64_t d : denoms) {
    while (remaining >= d) {
      result.push_back(d);
      remaining -= d;
    }
  }
  if (remaining > 0) {
    throw std::runtime_error("Cannot split amount with available denominations");
  }
  return result;
}

OutputData HybridOutputDataCreator::createSingleRandomData(
    double amount, const std::string& keysetId) {
  auto* res = cdk_create_random_blinded_message(
    static_cast<uint64_t>(amount), keysetId.c_str());
  if (!res) throw std::runtime_error("Failed to create random blinded message");

  OutputData out{
    .amount = amount,
    .keysetId = keysetId,
    .blindedSecret = std::string(res->blinded_secret),
    .blindingFactor = std::string(res->blinding_factor),
    .secret = std::string(res->secret),
  };
  cdk_blind_result_free(res);
  return out;
}

std::vector<OutputData> HybridOutputDataCreator::createRandomData(
    double amount,
    const std::string& keysetId,
    const std::vector<KeyEntry>& keys,
    const std::optional<std::vector<double>>& customSplit) {

  auto amounts = splitAmount(static_cast<uint64_t>(amount), keys, customSplit);
  std::vector<OutputData> results;
  results.reserve(amounts.size());
  for (uint64_t a : amounts) {
    results.push_back(createSingleRandomData(static_cast<double>(a), keysetId));
  }
  return results;
}

OutputData HybridOutputDataCreator::createSingleP2PKData(
    const P2PKOptions& p2pk,
    double amount,
    const std::string& keysetId) {

  // Build additional pubkeys array
  std::vector<const char*> addPubkeys;
  if (p2pk.additionalPubkeys.has_value()) {
    for (const auto& pk : p2pk.additionalPubkeys.value()) {
      addPubkeys.push_back(pk.c_str());
    }
  }

  std::vector<const char*> refundPks;
  if (p2pk.refundPubkeys.has_value()) {
    for (const auto& pk : p2pk.refundPubkeys.value()) {
      refundPks.push_back(pk.c_str());
    }
  }

  auto* res = cdk_create_p2pk_blinded_message(
    static_cast<uint64_t>(amount),
    keysetId.c_str(),
    p2pk.pubkey.c_str(),
    addPubkeys.empty() ? nullptr : addPubkeys.data(),
    static_cast<uint32_t>(addPubkeys.size()),
    p2pk.numSigs.value_or(1),
    p2pk.locktime.value_or(0),
    refundPks.empty() ? nullptr : refundPks.data(),
    static_cast<uint32_t>(refundPks.size()),
    p2pk.sigFlag.has_value() ? p2pk.sigFlag.value().c_str() : "SigInputs"
  );
  if (!res) throw std::runtime_error("Failed to create P2PK blinded message");

  OutputData out{
    .amount = amount,
    .keysetId = keysetId,
    .blindedSecret = std::string(res->blinded_secret),
    .blindingFactor = std::string(res->blinding_factor),
    .secret = std::string(res->secret),
  };
  cdk_blind_result_free(res);
  return out;
}

std::vector<OutputData> HybridOutputDataCreator::createP2PKData(
    const P2PKOptions& p2pk,
    double amount,
    const std::string& keysetId,
    const std::vector<KeyEntry>& keys,
    const std::optional<std::vector<double>>& customSplit) {

  auto amounts = splitAmount(static_cast<uint64_t>(amount), keys, customSplit);
  std::vector<OutputData> results;
  results.reserve(amounts.size());
  for (uint64_t a : amounts) {
    results.push_back(createSingleP2PKData(p2pk, static_cast<double>(a), keysetId));
  }
  return results;
}

OutputData HybridOutputDataCreator::createSingleDeterministicData(
    double amount,
    const std::shared_ptr<ArrayBuffer>& seed,
    double counter,
    const std::string& keysetId) {

  auto* res = cdk_create_deterministic_blinded_message(
    static_cast<uint64_t>(amount),
    keysetId.c_str(),
    seed->data(),
    static_cast<uint32_t>(seed->size()),
    static_cast<uint32_t>(counter)
  );
  if (!res) throw std::runtime_error("Failed to create deterministic blinded message");

  OutputData out{
    .amount = amount,
    .keysetId = keysetId,
    .blindedSecret = std::string(res->blinded_secret),
    .blindingFactor = std::string(res->blinding_factor),
    .secret = std::string(res->secret),
  };
  cdk_blind_result_free(res);
  return out;
}

std::vector<OutputData> HybridOutputDataCreator::createDeterministicData(
    double amount,
    const std::shared_ptr<ArrayBuffer>& seed,
    double counter,
    const std::string& keysetId,
    const std::vector<KeyEntry>& keys,
    const std::optional<std::vector<double>>& customSplit) {

  auto amounts = splitAmount(static_cast<uint64_t>(amount), keys, customSplit);
  std::vector<OutputData> results;
  results.reserve(amounts.size());
  for (uint32_t i = 0; i < amounts.size(); i++) {
    results.push_back(createSingleDeterministicData(
      static_cast<double>(amounts[i]), seed, counter + i, keysetId));
  }
  return results;
}

} // namespace cashudevkit
d->data(),
    static_cast<uint32_t>(seed->size()),
    static_cast<uint32_t>(counter)
  );
  if (!res) throw std::runtime_error("Failed to create deterministic blinded message");

  OutputData out{
    .amount = amount,
    .keysetId = keysetId,
    .blindedSecret = std::string(res->blinded_secret),
    .blindingFactor = std::string(res->blinding_factor),
    .secret = std::string(res->secret),
  };
  cdk_ffi_blind_result_free(res);
  return out;
}

std::vector<OutputData> HybridOutputDataCreator::createDeterministicData(
    double amount,
    const std::shared_ptr<ArrayBuffer>& seed,
    double counter,
    const std::string& keysetId,
    const std::vector<KeyEntry>& keys,
    const std::optional<std::vector<double>>& customSplit) {

  auto amounts = splitAmount(static_cast<uint64_t>(amount), keys, customSplit);
  std::vector<OutputData> results;
  results.reserve(amounts.size());
  for (uint32_t i = 0; i < amounts.size(); i++) {
    results.push_back(createSingleDeterministicData(
      static_cast<double>(amounts[i]), seed, counter + i, keysetId));
  }
  return results;
}

} // namespace cashudevkit
