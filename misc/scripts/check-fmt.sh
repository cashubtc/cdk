#!/usr/bin/env bash

set -euo pipefail

flags=""

# Check if "check" is passed as an argument
if [[ "$#" -gt 0 && "$1" == "check" ]]; then
    flags="--check"
fi


# Check workspace crates
cargo fmt --all -- --config format_code_in_doc_comments=true $flags
