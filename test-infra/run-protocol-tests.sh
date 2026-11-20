#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd "$(dirname "$0")" && pwd)
project_dir=$(cd "$script_dir/.." && pwd)
python_bin=${WEAVE_TEST_PYTHON:-python3}

if ! "$python_bin" -c 'import pyftpdlib; import OpenSSL' 2>/dev/null; then
  echo "Install test-infra/requirements.txt into a Python environment and set WEAVE_TEST_PYTHON to its python executable." >&2
  exit 1
fi

test_dir=$(mktemp -d "${TMPDIR:-/tmp}/weave-protocol-tests.XXXXXX")
ftp_pid=
fxp_pid=
ftps_pid=
implicit_pid=
fxps_source_pid=
fxps_dest_pid=
client_cert_pid=
cleanup() {
  if [[ -n $ftp_pid ]]; then kill "$ftp_pid" 2>/dev/null || true; fi
  if [[ -n $fxp_pid ]]; then kill "$fxp_pid" 2>/dev/null || true; fi
  if [[ -n $ftps_pid ]]; then kill "$ftps_pid" 2>/dev/null || true; fi
  if [[ -n $implicit_pid ]]; then kill "$implicit_pid" 2>/dev/null || true; fi
  if [[ -n $fxps_source_pid ]]; then kill "$fxps_source_pid" 2>/dev/null || true; fi
  if [[ -n $fxps_dest_pid ]]; then kill "$fxps_dest_pid" 2>/dev/null || true; fi
  if [[ -n $client_cert_pid ]]; then kill "$client_cert_pid" 2>/dev/null || true; fi
  wait 2>/dev/null || true
  rm -rf "$test_dir"
}
trap cleanup EXIT

choose_port() {
  "$python_bin" -c 'import socket; sock = socket.socket(); sock.bind(("127.0.0.1", 0)); print(sock.getsockname()[1]); sock.close()'
}

ftp_port=$(choose_port)
fxp_port=$(choose_port)
while [[ $fxp_port == "$ftp_port" ]]; do fxp_port=$(choose_port); done
ftps_port=$(choose_port)
while [[ $ftps_port == "$ftp_port" ]]; do ftps_port=$(choose_port); done
implicit_port=$(choose_port)
while [[ $implicit_port == "$ftp_port" || $implicit_port == "$ftps_port" ]]; do implicit_port=$(choose_port); done
used_ports=" $ftp_port $fxp_port $ftps_port $implicit_port "
fxps_source_port=$(choose_port)
while [[ $used_ports == *" $fxps_source_port "* ]]; do fxps_source_port=$(choose_port); done
used_ports+="$fxps_source_port "
fxps_dest_port=$(choose_port)
while [[ $used_ports == *" $fxps_dest_port "* ]]; do fxps_dest_port=$(choose_port); done
used_ports+="$fxps_dest_port "
client_cert_port=$(choose_port)
while [[ $used_ports == *" $client_cert_port "* ]]; do client_cert_port=$(choose_port); done

openssl req -x509 -newkey rsa:2048 -nodes \
  -keyout "$test_dir/key.pem" -out "$test_dir/cert.pem" -days 1 \
  -subj /CN=localhost \
  -addext subjectAltName=DNS:localhost,IP:127.0.0.1 \
  -addext extendedKeyUsage=serverAuth \
  -addext keyUsage=digitalSignature,keyEncipherment \
  -addext basicConstraints=critical,CA:TRUE \
  >/dev/null 2>&1
cat "$test_dir/cert.pem" "$test_dir/key.pem" > "$test_dir/server.pem"
openssl req -x509 -newkey rsa:2048 -nodes -traditional \
  -keyout "$test_dir/client-key.pem" -out "$test_dir/client-cert.pem" -days 1 \
  -subj /CN=weave-client -addext extendedKeyUsage=clientAuth >/dev/null 2>&1 ||
  openssl req -x509 -newkey rsa:2048 -nodes \
    -keyout "$test_dir/client-key.pem" -out "$test_dir/client-cert.pem" -days 1 \
    -subj /CN=weave-client -addext extendedKeyUsage=clientAuth >/dev/null 2>&1
cat "$test_dir/client-cert.pem" "$test_dir/client-key.pem" > "$test_dir/client.pem"

"$python_bin" "$script_dir/ftp_server.py" --root "$test_dir" --port "$ftp_port" \
  >"$test_dir/ftp.log" 2>&1 &
ftp_pid=$!
mkdir "$test_dir/fxp-dest"
"$python_bin" "$script_dir/ftp_server.py" --root "$test_dir/fxp-dest" --port "$fxp_port" \
  >"$test_dir/fxp.log" 2>&1 &
fxp_pid=$!
"$python_bin" "$script_dir/ftp_server.py" --root "$test_dir" --port "$ftps_port" \
  --cert "$test_dir/server.pem" >"$test_dir/ftps.log" 2>&1 &
ftps_pid=$!
"$python_bin" "$script_dir/ftp_server.py" --root "$test_dir" --port "$implicit_port" \
  --cert "$test_dir/server.pem" --implicit >"$test_dir/implicit.log" 2>&1 &
implicit_pid=$!
mkdir "$test_dir/fxps-dest"
"$python_bin" "$script_dir/ftp_server.py" --root "$test_dir" --port "$fxps_source_port" \
  --cert "$test_dir/server.pem" --clear-data >"$test_dir/fxps-source.log" 2>&1 &
fxps_source_pid=$!
"$python_bin" "$script_dir/ftp_server.py" --root "$test_dir/fxps-dest" --port "$fxps_dest_port" \
  --cert "$test_dir/server.pem" --clear-data >"$test_dir/fxps-dest.log" 2>&1 &
fxps_dest_pid=$!
"$python_bin" "$script_dir/ftp_server.py" --root "$test_dir" --port "$client_cert_port" \
  --cert "$test_dir/server.pem" --client-ca "$test_dir/client-cert.pem" >"$test_dir/client-cert.log" 2>&1 &
client_cert_pid=$!

if ! "$python_bin" - "$ftp_port" "$fxp_port" "$ftps_port" "$implicit_port" "$fxps_source_port" "$fxps_dest_port" "$client_cert_port" <<'PY'
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
        sys.exit(f"Local test server on port {port} did not start")
PY
then
  cat "$test_dir/ftp.log" "$test_dir/ftps.log" "$test_dir/implicit.log" >&2
  exit 1
fi

WEAVE_FTP_TEST_PORT=$ftp_port \
WEAVE_FTP_TEST_ROOT="$test_dir" \
WEAVE_FXP_TEST_PORT=$fxp_port \
WEAVE_FXP_TEST_ROOT="$test_dir/fxp-dest" \
WEAVE_FTPS_TEST_PORT=$ftps_port \
WEAVE_IMPLICIT_FTPS_TEST_PORT=$implicit_port \
WEAVE_FTPS_TEST_CERT="$test_dir/cert.pem" \
WEAVE_FXPS_SOURCE_PORT=$fxps_source_port \
WEAVE_FTPS_CLIENT_CERT_PORT=$client_cert_port \
WEAVE_FTPS_CLIENT_IDENTITY="$test_dir/client.pem" \
WEAVE_FXPS_DEST_PORT=$fxps_dest_port \
WEAVE_FXPS_DEST_ROOT="$test_dir/fxps-dest" \
  cargo test --manifest-path "$project_dir/src-tauri/Cargo.toml" -- --ignored \
    --skip webdav \
    --skip s3_integration \
    --skip azure_integration \
    --skip signs_in_through_the_app_prompt_with_a_real_ssh_server \
    --skip reuses_an_authenticated_ssh_master_for_sftp \
    --skip opens_and_stops_a_real_local_ssh_tunnel \
    --skip clones_pushes_fetches_and_pulls_over_ssh \
    --skip runs_a_script_on_a_real_ssh_host \
    --skip copies_to_and_from_real_ssh_with_scp_and_rsync \
    --skip copies_between_two_ssh_hosts_through_this_mac \
    --skip manages_repositories_on_a_real_ssh_host \
    --skip interoperates_with_another_implementation \
    --skip reads_and_writes_format_7_vaults_like_the_reference \
    --skip generates_a_key_signs_commits_and_trusts_imported_keys \
    --test-threads=1
