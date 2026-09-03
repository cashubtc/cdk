#!/usr/bin/env bash
set -euo pipefail

root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)
profile=${1:-debug}

case "$profile" in
  debug)
    cargo_args=()
    target_dir=debug
    ;;
  release)
    cargo_args=(--release)
    target_dir=release
    ;;
  *)
    echo "usage: $0 [debug|release]" >&2
    exit 2
    ;;
esac

case "$(uname -s)" in
  Darwin) library_extension=dylib ;;
  Linux) library_extension=so ;;
  *)
    echo "unsupported operating system: $(uname -s)" >&2
    exit 2
    ;;
esac

output_dir=$(mktemp -d "${TMPDIR:-/tmp}/cdk-wallet-api.XXXXXX")
trap 'rm -rf "$output_dir"' EXIT

cd "$root"
cargo build -p cdk-ffi "${cargo_args[@]}"
cargo run -p cdk-ffi --bin uniffi-bindgen "${cargo_args[@]}" -- generate \
  --library "target/$target_dir/libcdk_ffi.$library_extension" \
  --language python \
  --out-dir "$output_dir" \
  --no-format
python3 crates/cdk-ffi/scripts/check-wallet-api.py \
  "$output_dir/cdk_ffi.py" \
  crates/cdk-ffi/wallet-api.manifest
