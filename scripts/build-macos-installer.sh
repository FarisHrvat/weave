#!/usr/bin/env bash
set -euo pipefail

project_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
app_path="$project_dir/src-tauri/target/release/bundle/macos/Weave.app"
image_path="$project_dir/Weave-macOS-arm64.dmg"
stage_dir="$(mktemp -d /private/tmp/weave-installer.XXXXXX)"
trap 'rm -rf "$stage_dir"' EXIT

cd "$project_dir"
npm run tauri build -- --bundles app
codesign --force --deep --sign - "$app_path"
codesign --verify --deep --strict "$app_path"

ditto "$app_path" "$stage_dir/Weave.app"
ln -s /Applications "$stage_dir/Applications"
hdiutil create -volname Weave -srcfolder "$stage_dir" -ov -format UDZO "$image_path"
hdiutil verify "$image_path"

printf 'Installer ready: %s\n' "$image_path"
