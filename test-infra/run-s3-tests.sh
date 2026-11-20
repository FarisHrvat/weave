#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd "$(dirname "$0")" && pwd)
project_dir=$(cd "$script_dir/.." && pwd)
test_dir=$(mktemp -d "${TMPDIR:-/tmp}/weave-s3-tests.XXXXXX")
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')
server_pid=
cleanup() {
  if [[ -n $server_pid ]]; then kill "$server_pid" 2>/dev/null || true; fi
  wait 2>/dev/null || true
  rm -rf "$test_dir"
}
trap cleanup EXIT

python3 "$script_dir/s3_server.py" --port "$port" >"$test_dir/server.log" 2>&1 &
server_pid=$!
if ! python3 - "$port" <<'PY'
import socket
import sys
import time

port = int(sys.argv[1])
for _ in range(50):
    try:
        with socket.create_connection(("127.0.0.1", port), timeout=0.2):
            break
    except OSError:
        time.sleep(0.1)
else:
    sys.exit("Local S3 test server did not start")
PY
then
  cat "$test_dir/server.log" >&2
  exit 1
fi

WEAVE_S3_TEST_PORT=$port cargo test --manifest-path "$project_dir/src-tauri/Cargo.toml" s3_integration -- --ignored
