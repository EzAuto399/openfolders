#!/usr/bin/env bash
# Removes OpenFolders: stops it, takes its folders out of the Dock and puts their
# apps back, and deletes the app. Your settings stay in ~/.config/openfolders
# unless you pass --purge.
set -euo pipefail
cd "$(dirname "$0")"

APP_DIR="$HOME/Applications/OpenFolders"
AGENT="$HOME/Library/LaunchAgents/dev.openfolders.plist"
BIN="$APP_DIR/OpenFolders.app/Contents/MacOS/openfolders"
[[ -x "$BIN" ]] || BIN="./target/release/openfolders"

launchctl bootout "gui/$(id -u)" "$AGENT" 2>/dev/null || true
rm -f "$AGENT"
pkill -x openfolders 2>/dev/null || true
sleep 0.5

if [[ -x "$BIN" ]]; then
  "$BIN" --remove-from-dock
fi
rm -rf "$APP_DIR"

if [[ "${1:-}" == "--purge" ]]; then
  rm -rf "$HOME/.config/openfolders"
  echo "Removed OpenFolders and its settings."
else
  echo "Removed OpenFolders. Settings kept in ~/.config/openfolders (run with --purge to delete)."
fi
