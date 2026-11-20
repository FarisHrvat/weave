#!/usr/bin/env bash
set -euo pipefail

project_dir=$(cd "$(dirname "$0")/.." && pwd)
cd "$project_dir"

npm run format:check
npm test

cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml generates_a_key_signs_commits_and_trusts_imported_keys -- --ignored

bash test-infra/run-protocol-tests.sh
bash test-infra/run-webdav-tests.sh
bash test-infra/run-s3-tests.sh
bash test-infra/run-azure-tests.sh
bash test-infra/run-ssh-tests.sh
bash test-infra/run-cryptomator-tests.sh
