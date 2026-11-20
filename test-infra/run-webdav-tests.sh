#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd "$(dirname "$0")" && pwd)
project_dir=$(cd "$script_dir/.." && pwd)
test_dir=$(mktemp -d "${TMPDIR:-/tmp}/weave-webdav-tests.XXXXXX")
port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')
tls_port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')
while [[ $tls_port == "$port" ]]; do tls_port=$(python3 -c 'import socket; s=socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()'); done
server_pid=
tls_pid=
cleanup() {
  if [[ -n $server_pid ]]; then kill "$server_pid" 2>/dev/null || true; fi
  if [[ -n $tls_pid ]]; then kill "$tls_pid" 2>/dev/null || true; fi
  wait 2>/dev/null || true
  rm -rf "$test_dir"
}
trap cleanup EXIT

python3 "$script_dir/webdav_server.py" --root "$test_dir/files" --port "$port" >"$test_dir/server.log" 2>&1 &
server_pid=$!
openssl req -x509 -newkey rsa:2048 -nodes -keyout "$test_dir/ca-key.pem" -out "$test_dir/ca.pem" -days 1 \
  -subj '/CN=Weave WebDAV Test CA' -addext 'basicConstraints=critical,CA:TRUE' \
  -addext 'keyUsage=critical,keyCertSign,cRLSign' >/dev/null 2>&1
openssl req -newkey rsa:2048 -nodes -keyout "$test_dir/key.pem" -out "$test_dir/server.csr" \
  -subj /CN=localhost >/dev/null 2>&1
cat > "$test_dir/server.ext" <<EOF
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
extendedKeyUsage=serverAuth
subjectAltName=DNS:localhost,IP:127.0.0.1
EOF
openssl x509 -req -in "$test_dir/server.csr" -CA "$test_dir/ca.pem" -CAkey "$test_dir/ca-key.pem" \
  -CAcreateserial -out "$test_dir/cert.pem" -days 1 -extfile "$test_dir/server.ext" >/dev/null 2>&1
python3 "$script_dir/webdav_server.py" --root "$test_dir/tls-files" --port "$tls_port" \
  --cert "$test_dir/cert.pem" --key "$test_dir/key.pem" >"$test_dir/tls.log" 2>&1 &
tls_pid=$!
if ! python3 - "$port" "$tls_port" <<'PY'
import socket
import sys
import time

for port in map(int, sys.argv[1:]):
    for _ in range(50):
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=0.2):
                break
        except OSError:
            time.sleep(0.1)
    else:
        sys.exit(f"Local WebDAV test server on port {port} did not start")
PY
then
  cat "$test_dir/server.log" "$test_dir/tls.log" >&2
  exit 1
fi

WEAVE_WEBDAV_TEST_PORT=$port WEAVE_WEBDAV_TLS_TEST_PORT=$tls_port \
WEAVE_WEBDAV_TLS_TEST_CERT="$test_dir/cert.pem" WEAVE_WEBDAV_TLS_TEST_CA="$test_dir/ca.pem" \
  cargo test --manifest-path "$project_dir/src-tauri/Cargo.toml" webdav -- --ignored
