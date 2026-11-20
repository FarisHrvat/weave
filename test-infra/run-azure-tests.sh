#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd "$(dirname "$0")" && pwd)
project_dir=$(cd "$script_dir/.." && pwd)
azurite="$project_dir/node_modules/.bin/azurite-blob"
if [[ ! -x $azurite ]]; then
  echo "Azurite is missing. Run npm install first." >&2
  exit 1
fi
test_dir=$(mktemp -d "${TMPDIR:-/tmp}/weave-azure-tests.XXXXXX")
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')
server_pid=
cleanup() {
  if [[ -n $server_pid ]]; then kill "$server_pid" 2>/dev/null || true; fi
  wait 2>/dev/null || true
  rm -rf "$test_dir"
}
trap cleanup EXIT

"$azurite" --blobHost 127.0.0.1 --blobPort "$port" \
  --skipApiVersionCheck --location "$test_dir" >"$test_dir/server.log" 2>&1 &
server_pid=$!
for _ in $(seq 50); do
  if grep -q "successfully listens" "$test_dir/server.log" 2>/dev/null; then break; fi
  sleep 0.2
done
if ! grep -q "successfully listens" "$test_dir/server.log"; then
  cat "$test_dir/server.log" >&2
  exit 1
fi

WEAVE_AZURE_TEST_PORT=$port cargo test --manifest-path "$project_dir/src-tauri/Cargo.toml" azure_integration -- --ignored
