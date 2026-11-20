#!/usr/bin/env bash
# Checks Weave's Cryptomator vaults against pycryptomator, an independent
# implementation. Set WEAVE_CRYPTOMATOR_PYTHON (or WEAVE_TEST_PYTHON)
# to a Python with pycryptomator.
set -euo pipefail

script_dir=$(cd "$(dirname "$0")" && pwd)
project_dir=$(cd "$script_dir/.." && pwd)
python=${WEAVE_CRYPTOMATOR_PYTHON:-${WEAVE_TEST_PYTHON:-python3}}
work=$(mktemp -d /private/tmp/weave-cryptomator.XXXXXX)
trap 'rm -rf "$work"' EXIT

"$python" -I "$script_dir/cryptomator_interop.py" write "$work"
WEAVE_CRYPTOMATOR_DIR=$work WEAVE_CRYPTOMATOR_STEP=read \
  cargo test --manifest-path "$project_dir/src-tauri/Cargo.toml" interoperates_with_another_implementation -- --ignored
WEAVE_CRYPTOMATOR_DIR=$work WEAVE_CRYPTOMATOR_STEP=write \
  cargo test --manifest-path "$project_dir/src-tauri/Cargo.toml" interoperates_with_another_implementation -- --ignored
"$python" -I "$script_dir/cryptomator_interop.py" read "$work"

"$python" -I "$script_dir/cryptomator_format7.py" create "$work/format7"
WEAVE_CRYPTOMATOR_DIR=$work \
  cargo test --manifest-path "$project_dir/src-tauri/Cargo.toml" reads_and_writes_format_7_vaults_like_the_reference -- --ignored
"$python" -I "$script_dir/cryptomator_format7.py" read "$work/format7"
