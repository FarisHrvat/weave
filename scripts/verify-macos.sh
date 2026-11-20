#!/usr/bin/env bash
set -euo pipefail

project_dir=$(cd "$(dirname "$0")/.." && pwd)
cd "$project_dir"

bash scripts/test-macos.sh

bash scripts/build-macos-installer.sh
