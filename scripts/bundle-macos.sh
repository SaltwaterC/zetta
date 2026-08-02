#!/usr/bin/env bash

set -euo pipefail

bundle_path=${1:?application bundle path is required}
version=${2:?application version is required}
icon_source="${ICON_SOURCE:-assets/icons/zetta-terminal-icon-512.png}"
plist_template="${PLIST_TEMPLATE:-resources/macos/Info.plist.in}"

command -v sips >/dev/null 2>&1 || {
    echo "sips is required to create the macOS application icon" >&2
    exit 1
}

resources_path="$bundle_path/Contents/Resources"
mkdir -p "$resources_path"

sips -s format icns "$icon_source" --out "$resources_path/Zetta.icns" >/dev/null
sed "s/@VERSION@/$version/g" "$plist_template" > "$bundle_path/Contents/Info.plist"
plutil -lint "$bundle_path/Contents/Info.plist" >/dev/null

echo "Created application bundle: $bundle_path"
