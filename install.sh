#!/usr/bin/env bash
# Builds OpenFolders, installs it to ~/Applications/OpenFolders, starts it and
# launches it at login. Run it again any time to update.
set -euo pipefail
cd "$(dirname "$0")"

APP="$HOME/Applications/OpenFolders/OpenFolders.app"
AGENT="$HOME/Library/LaunchAgents/dev.openfolders.plist"

if [[ "$(uname)" != "Darwin" ]]; then
  echo "OpenFolders only runs on macOS."
  exit 1
fi

if ! command -v cargo >/dev/null 2>&1; then
  echo "OpenFolders is built with Rust, which isn't installed yet. Install it with:"
  echo
  echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
  echo
  echo "then open a new terminal and run ./install.sh again."
  exit 1
fi

echo "1/3  Building (first build takes about a minute)…"
cargo build --release --quiet

echo "2/3  Installing to ~/Applications/OpenFolders…"
launchctl bootout "gui/$(id -u)" "$AGENT" 2>/dev/null || true
pkill -x openfolders 2>/dev/null || true
sleep 0.5
./target/release/openfolders --build-bundles >/dev/null

# Sign with your own code-signing certificate if you have one, so macOS keeps the
# Accessibility permission across updates. Without one the app stays ad-hoc signed.
IDENTITY="${OPENFOLDERS_SIGN_IDENTITY:-$(security find-identity -v -p codesigning 2>/dev/null \
  | awk -F'"' '/Apple Development|Developer ID Application/ { print $2; exit }')}"
if [[ -n "$IDENTITY" ]]; then
  find "$APP" -exec xattr -c {} +
  codesign --force --sign "$IDENTITY" --identifier dev.openfolders.composer "$APP" 2>/dev/null \
    || echo "     (Couldn't sign with your certificate; continuing unsigned.)"
fi

echo "3/3  Starting OpenFolders and adding it to login items…"
mkdir -p "$(dirname "$AGENT")"
cat > "$AGENT" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>dev.openfolders</string>
  <key>ProgramArguments</key>
  <array>
    <string>$APP/Contents/MacOS/openfolders</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
  <key>ProcessType</key>
  <string>Interactive</string>
</dict>
</plist>
PLIST
launchctl bootstrap "gui/$(id -u)" "$AGENT"

cat <<'DONE'

OpenFolders is running.

  • macOS will ask to allow OpenFolders under Accessibility. Allow it: that's how
    it knows which Dock icon you're hovering.
  • Your folders are now in the Dock. Hover one to open it.
  • Right-click the row for Add Apps, Edit Folder, New Folder and Dock Settings.

To remove it: ./uninstall.sh
DONE
