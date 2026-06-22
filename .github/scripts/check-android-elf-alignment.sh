#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "usage: $0 <shared-library.so>" >&2
  exit 2
fi

file="$1"
objdump="${ANDROID_NDK_HOME}/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-objdump"

load_segments="$("$objdump" -p "$file" | grep '^[[:space:]]*LOAD' || true)"
if [[ -z "$load_segments" ]]; then
  echo "no LOAD segments found in $file" >&2
  exit 1
fi

failed=0
while IFS= read -r line; do
  align="${line##*align }"
  exponent="${align#2**}"
  if [[ "$exponent" == "$align" || ! "$exponent" =~ ^[0-9]+$ ]]; then
    echo "could not parse LOAD segment alignment: $line" >&2
    failed=1
  elif (( exponent < 14 )); then
    echo "LOAD segment is not 16 KB aligned: $line" >&2
    failed=1
  fi
done <<< "$load_segments"

if (( failed )); then
  exit 1
fi

echo "$file has 16 KB ELF LOAD alignment"
