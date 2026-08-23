#pragma once

#include "HybridOutputDataCreatorSpec.hpp"
#include <memory>

namespace margelo::nitro::cashudevkit {

using namespace margelo::nitro;

class HybridOutputDataCreator : public HybridOutputDataCreatorSpec {
public:
  HybridOutputDataCreator() : HybridObject(TAG) {}

  // Random outputs
  OutputData createSingleRandomData(double amount, const std::string& keysetId) override;
  std::vector<OutputData> createRandomData(
    double amount,
    const std::string& keysetId,
    const std::vector<KeyEntry>& keys,
    const std::optional<std::vector<double>>& customSplit) override;

  // P2PK outputs
  OutputData createSingleP2PKData(
    const P2PKOptions& p2pk,
    double amount,
    const std::string& keysetId) override;
  std::vector<OutputData> createP2PKData(
    const P2PKOptions& p2pk,
    double amount,
    const std::string& keysetId,
    const std::vector<KeyEntry>& keys,
    const std::optional<std::vector<double>>& customSplit) override;

  // Deterministic outputs
  OutputData createSingleDeterministicData(
    double amount,
    const std::shared_ptr<ArrayBuffer>& seed,
    double counter,
    const std::string& keysetId) override;
  std::vector<OutputData> createDeterministicData(
    double amount,
    const std::shared_ptr<ArrayBuffer>& seed,
    double counter,
    const std::string& keysetId,
    const std::vector<KeyEntry>& keys,
    const std::optional<std::vector<double>>& customSplit) override;

private:
  static constexpr auto TAG = "OutputDataCreator";
};

} // namespace margelo::nitro::cashudevkit
