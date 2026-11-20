#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd "$(dirname "$0")" && pwd)
project_dir=$(cd "$script_dir/.." && pwd)
sshd=/usr/sbin/sshd
sftp_server=/usr/libexec/sftp-server

if [[ ! -x $sshd || ! -x $sftp_server ]]; then
  echo "The macOS OpenSSH server and SFTP server are required for this test." >&2
  exit 1
fi

test_dir=$(mktemp -d /private/tmp/weave-ssh-tests.XXXXXX)
server_pid=
cleanup() {
  if [[ -n $server_pid ]]; then
    kill "$server_pid" 2>/dev/null || true
    wait "$server_pid" 2>/dev/null || true
  fi
  rm -rf "$test_dir"
}
trap cleanup EXIT

ssh-keygen -q -t ed25519 -N '' -f "$test_dir/host" >/dev/null
ssh-keygen -q -t ed25519 -N '' -f "$test_dir/client" >/dev/null
cp "$test_dir/client.pub" "$test_dir/authorized_keys"

port=$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')
user_name=$(id -un)
cat > "$test_dir/sshd_config" <<EOF
Port $port
ListenAddress 127.0.0.1
HostKey $test_dir/host
AuthorizedKeysFile $test_dir/authorized_keys
PidFile $test_dir/sshd.pid
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
StrictModes no
PermitRootLogin no
AllowUsers $user_name
Subsystem sftp $sftp_server
EOF

host_key=$(cut -d' ' -f1,2 "$test_dir/host.pub")
printf '[127.0.0.1]:%s %s\n' "$port" "$host_key" > "$test_dir/known_hosts"

"$sshd" -D -e -f "$test_dir/sshd_config" > "$test_dir/sshd.log" 2>&1 &
server_pid=$!

ready=false
for _ in {1..50}; do
  if ssh -p "$port" -i "$test_dir/client" \
    -o "UserKnownHostsFile=$test_dir/known_hosts" \
    -o StrictHostKeyChecking=yes -o BatchMode=yes -o ConnectTimeout=1 \
    "$user_name@127.0.0.1" true 2>/dev/null; then
    ready=true
    break
  fi
  sleep 0.1
done
if [[ $ready != true ]]; then
  cat "$test_dir/sshd.log" >&2
  echo "The temporary local SSH server did not accept the test account." >&2
  exit 1
fi

WEAVE_SSH_TEST_PORT=$port \
WEAVE_SSH_TEST_KEY="$test_dir/client" \
WEAVE_SSH_TEST_KNOWN_HOSTS="$test_dir/known_hosts" \
WEAVE_SSH_TEST_USER="$user_name" \
  cargo test --manifest-path "$project_dir/src-tauri/Cargo.toml" \
    reuses_an_authenticated_ssh_master_for_sftp -- --ignored

WEAVE_SSH_TEST_PORT=$port \
WEAVE_SSH_TEST_KEY="$test_dir/client" \
WEAVE_SSH_TEST_KNOWN_HOSTS="$test_dir/known_hosts" \
WEAVE_SSH_TEST_USER="$user_name" \
  cargo test --manifest-path "$project_dir/src-tauri/Cargo.toml" \
    opens_and_stops_a_real_local_ssh_tunnel -- --ignored

WEAVE_SSH_TEST_PORT=$port \
WEAVE_SSH_TEST_KEY="$test_dir/client" \
WEAVE_SSH_TEST_KNOWN_HOSTS="$test_dir/known_hosts" \
WEAVE_SSH_TEST_USER="$user_name" \
  cargo test --manifest-path "$project_dir/src-tauri/Cargo.toml" \
    clones_pushes_fetches_and_pulls_over_ssh -- --ignored

WEAVE_SSH_TEST_PORT=$port \
WEAVE_SSH_TEST_KEY="$test_dir/client" \
WEAVE_SSH_TEST_KNOWN_HOSTS="$test_dir/known_hosts" \
WEAVE_SSH_TEST_USER="$user_name" \
  cargo test --manifest-path "$project_dir/src-tauri/Cargo.toml" \
    runs_a_script_on_a_real_ssh_host -- --ignored

WEAVE_SSH_TEST_PORT=$port \
WEAVE_SSH_TEST_KEY="$test_dir/client" \
WEAVE_SSH_TEST_KNOWN_HOSTS="$test_dir/known_hosts" \
WEAVE_SSH_TEST_USER="$user_name" \
  cargo test --manifest-path "$project_dir/src-tauri/Cargo.toml" \
    copies_to_and_from_real_ssh_with_scp_and_rsync -- --ignored

WEAVE_SSH_TEST_PORT=$port \
WEAVE_SSH_TEST_KEY="$test_dir/client" \
WEAVE_SSH_TEST_KNOWN_HOSTS="$test_dir/known_hosts" \
WEAVE_SSH_TEST_USER="$user_name" \
  cargo test --manifest-path "$project_dir/src-tauri/Cargo.toml" \
    copies_between_two_ssh_hosts_through_this_mac -- --ignored

WEAVE_SSH_TEST_PORT=$port \
WEAVE_SSH_TEST_KEY="$test_dir/client" \
WEAVE_SSH_TEST_KNOWN_HOSTS="$test_dir/known_hosts" \
WEAVE_SSH_TEST_USER="$user_name" \
  cargo test --manifest-path "$project_dir/src-tauri/Cargo.toml" \
    manages_repositories_on_a_real_ssh_host -- --ignored

# Signing in through Weave's own prompt, with the built app as ssh's helper.
cargo build --manifest-path "$project_dir/src-tauri/Cargo.toml"
target_dir=${CARGO_TARGET_DIR:-$project_dir/src-tauri/target}
WEAVE_ASKPASS_HELPER="$target_dir/debug/weave" \
  cargo test --manifest-path "$project_dir/src-tauri/Cargo.toml" \
    signs_in_through_the_app_prompt_with_a_real_ssh_server -- --ignored
