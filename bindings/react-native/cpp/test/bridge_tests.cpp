// Exercises the generated bridge against the real Rust library.
//
// The bridge deliberately has no React Native dependency, so everything that
// crosses the UniFFI ABI is testable here with a bare toolchain.

#include "CashuCryptoBridge.hpp"

#include <cstdint>
#include <cstdio>
#include <cstdlib>
#include <iostream>
#include <memory>
#include <optional>
#include <string>
#include <vector>

namespace bridge = cashucrypto::bridge;

namespace {

int failures = 0;
int checks = 0;

void check(bool condition, const std::string& what) {
  checks++;
  if (!condition) {
    failures++;
    std::cout << "  FAIL " << what << "\n";
  }
}

template <typename T>
void checkEqual(const T& actual, const T& expected, const std::string& what) {
  checks++;
  if (!(actual == expected)) {
    failures++;
    std::cout << "  FAIL " << what << "\n";
  }
}

std::string toHex(const std::vector<uint8_t>& bytes) {
  static const char* digits = "0123456789abcdef";
  std::string hex;
  for (uint8_t byte : bytes) {
    hex.push_back(digits[byte >> 4]);
    hex.push_back(digits[byte & 0x0f]);
  }
  return hex;
}

std::vector<uint8_t> fromHex(const std::string& hex) {
  std::vector<uint8_t> bytes;
  for (size_t i = 0; i + 1 < hex.size(); i += 2) {
    bytes.push_back(static_cast<uint8_t>(std::stoul(hex.substr(i, 2), nullptr, 16)));
  }
  return bytes;
}

const char* KEYSET = "009a1f293253e41e";

// Valid compressed secp256k1 points: the generator and twice the generator.
const char* PUBKEY_G = "0279be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798";
const char* PUBKEY_2G = "02c6047f9441ed7d6d3045406e95c07cd85c778e4b8cef3ca7abac09b95c709ee5";
// x is larger than the field prime, so no point exists.
const char* PUBKEY_INVALID = "02ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff";

// BIP39 seed for "half depart obvious quality work element tank gorilla view
// sugar picture humble", the NUT-13 test vector mnemonic.
const char* SEED_HEX =
    "dd44ee516b0647e80b488e8dcc56d736a148f15276bef588b37057476d4b2b25"
    "780d3688a32b37353d6995997842c0fd8b412475c891c16310471fbc86dcbda8";

std::vector<bridge::KeyEntry> powersOfTwoKeys(uint32_t count) {
  std::vector<bridge::KeyEntry> keys;
  for (uint32_t i = 0; i < count; i++) {
    bridge::KeyEntry entry;
    entry.amount = uint64_t{1} << i;
    entry.pubkey = (i % 2 == 0) ? PUBKEY_G : PUBKEY_2G;
    keys.push_back(entry);
  }
  return keys;
}

void testAbiVersion() {
  std::cout << "abi version and checksums\n";
  bool threw = false;
  try {
    bridge::assertAbiCompatible();
  } catch (const std::exception&) {
    threw = true;
  }
  check(!threw, "the loaded library matches the metadata the bindings came from");
}

void testBytes() {
  std::cout << "byte buffers\n";
  std::vector<uint8_t> abc = {'a', 'b', 'c'};
  checkEqual(toHex(bridge::sha256Digest(abc)),
             std::string("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
             "sha256 of abc");

  checkEqual(bridge::sha256Digest({}).size(), size_t{32}, "sha256 of an empty buffer");

  std::vector<uint8_t> zeros(32, 0);
  checkEqual(toHex(bridge::hashToCurve(zeros)),
             std::string("024cce997d3b518f739663b757deaec95bcd9473c30a14ac2fd04023a739d1a725"),
             "hash_to_curve NUT-00 vector");

  std::vector<uint8_t> large(4 * 1024 * 1024, 0x5a);
  checkEqual(bridge::sha256Digest(large).size(), size_t{32}, "sha256 of a 4 MiB buffer");
  checkEqual(bridge::sha256Digest(large), bridge::sha256Digest(large),
             "a large buffer hashes the same twice");
}

void testPrimitives() {
  std::cout << "primitives and 64 bit amounts\n";
  std::vector<uint64_t> denominations;
  for (uint32_t i = 0; i < 32; i++) {
    denominations.push_back(uint64_t{1} << i);
  }
  std::vector<uint64_t> expected = {1, 4, 8};
  checkEqual(bridge::splitAmount(13, denominations, std::nullopt), expected, "split of 13");

  // A value past 2^53 would be lossy as a JavaScript number; it must survive.
  std::vector<uint64_t> big = {uint64_t{1} << 62};
  checkEqual(bridge::splitAmount(uint64_t{1} << 62, big, std::nullopt), big,
             "u64 amounts keep their full range");

  std::vector<uint64_t> custom = {4, 2, 1, 1};
  auto split = bridge::splitAmount(8, denominations, custom);
  checkEqual(split.size(), size_t{4}, "a custom split is honoured");
}

void testRecordsAndOptionals() {
  std::cout << "records and optionals\n";
  auto output = bridge::createSingleRandomOutput(8, KEYSET);
  checkEqual(output.amount, uint64_t{8}, "amount round-trips");
  checkEqual(output.keysetId, std::string(KEYSET), "keyset id round-trips");
  checkEqual(output.blindedSecret.size(), size_t{66}, "B_ is a compressed point");
  checkEqual(output.blindingFactor.size(), size_t{64}, "r is 32 bytes of hex");
  check(!output.derivationIndex.has_value(), "a random output has no derivation index");

  auto seed = fromHex(SEED_HEX);
  auto derived = bridge::createSingleDeterministicOutput(8, seed, 3, KEYSET);
  check(derived.derivationIndex.has_value(), "a deterministic output carries its counter");
  checkEqual(derived.derivationIndex.value(), uint32_t{3}, "the counter is the one asked for");

  checkEqual(
      derived.secret,
      std::string("59284fd1650ea9fa17db2b3acf59ecd0f2d52ec3261dd4152785813ff27a33bf"),
      "NUT-13 vector for counter 3");

  std::vector<uint64_t> denominations;
  for (uint32_t i = 0; i < 32; i++) {
    denominations.push_back(uint64_t{1} << i);
  }
  auto amounts = bridge::splitAmount(31, denominations, std::nullopt);
  auto batch = bridge::createRandomOutputs(amounts, KEYSET);
  uint64_t total = 0;
  for (const auto& item : batch) {
    total += item.amount;
  }
  checkEqual(total, uint64_t{31}, "a batch adds up to the amount");
  checkEqual(batch.size(), size_t{5}, "31 splits into five powers of two");

  std::vector<uint64_t> ordered = {8, 1, 4};
  auto kept = bridge::createDeterministicOutputs(ordered, seed, 0, KEYSET);
  checkEqual(kept.size(), size_t{3}, "one output per requested denomination");
  checkEqual(kept[0].amount, uint64_t{8}, "the caller's order is preserved");
  checkEqual(kept[2].derivationIndex.value(), uint32_t{2}, "counters walk in that order");
}

void testEnums() {
  std::cout << "enums\n";
  bridge::P2pkOptions options;
  options.pubkey = PUBKEY_G;
  options.sigFlag = bridge::SigFlag::SigInputs;
  auto inputs = bridge::createSingleP2pkOutput(options, 8, KEYSET);
  check(inputs.secret.find("SIG_ALL") == std::string::npos,
        "SigInputs writes no sigflag tag");

  options.sigFlag = bridge::SigFlag::SigAll;
  auto all = bridge::createSingleP2pkOutput(options, 8, KEYSET);
  check(all.secret.find("SIG_ALL") != std::string::npos, "SigAll writes the tag");

  options.sigFlag = bridge::SigFlag::SigInputs;
  options.additionalPubkeys = std::vector<std::string>{PUBKEY_2G};
  options.numSigs = uint64_t{2};
  auto multisig = bridge::createSingleP2pkOutput(options, 8, KEYSET);
  check(multisig.secret.find("n_sigs") != std::string::npos,
        "optional record fields reach Rust");
}

void testStrings() {
  std::cout << "strings\n";
  auto keys = powersOfTwoKeys(4);
  auto id = bridge::keysetIdV1(keys);
  checkEqual(id.size(), size_t{16}, "a keyset id is sixteen hex characters");

  bridge::KeyEntry broken;
  broken.amount = 1;
  broken.pubkey = PUBKEY_INVALID;
  bool threw = false;
  try {
    bridge::keysetIdV1({broken});
  } catch (const bridge::CashuFfiError& error) {
    threw = true;
    check(error.kind() == bridge::CashuFfiErrorKind::InvalidPublicKey,
          "the variant identifies the bad key");
  }
  check(threw, "a point that is not on the curve is rejected");

  auto pair = bridge::blindMessage({'s', 'e', 'c'}, std::nullopt);
  checkEqual(pair.blindedSecret.size(), size_t{66}, "blindMessage returns a point");
  checkEqual(pair.blindingFactor.size(), size_t{64}, "blindMessage returns a scalar");

  auto factor = fromHex(pair.blindingFactor);
  auto again = bridge::blindMessage({'s', 'e', 'c'}, factor);
  checkEqual(again.blindedSecret, pair.blindedSecret,
            "an optional byte argument reaches Rust");
}

void testObjectLifecycle() {
  std::cout << "object lifecycle\n";
  auto seed = fromHex(SEED_HEX);
  auto factory = bridge::createDeterministicOutputFactory(seed, KEYSET);
  check(factory->handle() != 0, "the constructor returns a live handle");
  checkEqual(factory->keysetId(), std::string(KEYSET), "a method call reaches the object");

  auto direct = bridge::createSingleDeterministicOutput(16, seed, 9, KEYSET);
  auto viaObject = factory->singleOutput(16, 9);
  checkEqual(viaObject.secret, direct.secret, "the object derives the same secret");

  auto batch = factory->outputs({16, 8, 4, 2, 1}, 0);
  checkEqual(batch.size(), size_t{5}, "the object derives one output per amount");

  auto restored = factory->restoreBatch(2, 4);
  checkEqual(restored.size(), size_t{4}, "restore derives one output per counter asked for");
  checkEqual(restored[0].derivationIndex.value(), uint32_t{2}, "restore starts at the given counter");
  checkEqual(restored[3].derivationIndex.value(), uint32_t{5}, "restore walks the counter upward");

  factory->close();
  checkEqual(factory->handle(), uint64_t{0}, "close clears the handle");
  bool threw = false;
  try {
    factory->keysetId();
  } catch (const std::logic_error&) {
    threw = true;
  }
  check(threw, "a call after close is refused rather than crashing");

  factory->close();
  check(true, "close is safe to repeat");

  // Churn enough handles that a leaked Arc would be obvious to a leak checker
  // and a double free would abort the process here. Lowered under a sanitizer,
  // where the point is the memory checking rather than the volume.
  const char* churnEnv = std::getenv("BRIDGE_TEST_CHURN");
  const int churn = churnEnv != nullptr ? std::atoi(churnEnv) : 20000;
  for (int i = 0; i < churn; i++) {
    auto churn = bridge::createDeterministicOutputFactory(seed, KEYSET);
    if (churn->handle() == 0) {
      check(false, "handle churn produced a dead object");
      break;
    }
  }
  check(true, "the create and destroy cycles survive");
}

void testErrors() {
  std::cout << "errors\n";
  bool threw = false;
  try {
    bridge::createSingleRandomOutput(1, "not-a-keyset");
  } catch (const bridge::CashuFfiError& error) {
    threw = true;
    check(error.kind() == bridge::CashuFfiErrorKind::InvalidKeysetId,
          "the variant survives the crossing");
    std::string message = error.what();
    check(message.find("uniffi-nitro-error:") == 0, "the payload is tagged for the TS layer");
    check(message.find("\"kind\":\"InvalidKeysetId\"") != std::string::npos,
          "the payload names the variant");
    check(message.find("\"id\":\"not-a-keyset\"") != std::string::npos,
          "the payload keeps the structured field");
  }
  check(threw, "a bad keyset id raises");

  threw = false;
  try {
    bridge::createSingleDeterministicOutput(1, std::vector<uint8_t>(32, 0), 0, KEYSET);
  } catch (const bridge::CashuFfiError& error) {
    threw = true;
    check(error.kind() == bridge::CashuFfiErrorKind::InvalidSeedLength,
          "a numeric error field is reported");
    check(std::string(error.what()).find("\"length\":\"32\"") != std::string::npos,
          "a 64 bit error field survives as a decimal string");
  }
  check(threw, "a short seed raises");
}

void testDleq() {
  std::cout << "dleq\n";
  bridge::DleqProof proof;
  proof.e = std::string(64, '0');
  proof.s = std::string(64, '0');
  bool threw = false;
  try {
    bridge::verifyProofDleq("secret", PUBKEY_G, proof, std::string(64, '1'), PUBKEY_G);
  } catch (const bridge::CashuFfiError&) {
    threw = true;
  }
  check(threw, "a malformed dleq proof raises rather than returning false");
}

}  // namespace

int main() {
  testAbiVersion();
  testBytes();
  testPrimitives();
  testRecordsAndOptionals();
  testEnums();
  testStrings();
  testObjectLifecycle();
  testErrors();
  testDleq();

  std::cout << "\n" << (checks - failures) << "/" << checks << " checks passed\n";
  return failures == 0 ? 0 : 1;
}
