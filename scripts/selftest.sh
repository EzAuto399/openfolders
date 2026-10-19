#!/usr/bin/env bash
# Checks a live OpenFolders install: signatures, Accessibility, Dock tiles and where each
# folder's row opens. Rows flash open for a moment; the mouse isn't touched.
#   scripts/selftest.sh         check the installed app
#   scripts/selftest.sh --ui    also check that no panel is clipped (needs Node; first run
#                               installs a headless browser into tests/ui, about 100 MB)
set -euo pipefail
cd "$(dirname "$0")/.."

APP="$HOME/Applications/OpenFolders/OpenFolders.app"
if [[ ! -d "$APP" ]]; then
  echo "OpenFolders isn't installed. Run ./install.sh first."
  exit 1
fi

# Started through LaunchServices so macOS judges OpenFolders' own permissions, not this terminal's
OUT="$(mktemp -t openfolders-selftest)"
open -n -W --stdout "$OUT" --stderr "$OUT" -a "$APP" --args --self-test
cat "$OUT"
status=0
grep -q "^All checks passed" "$OUT" || status=1
rm -f "$OUT"

if [[ "${1:-}" == "--ui" ]]; then
  echo
  echo "Panels"
  PID="$(pgrep -f 'OpenFolders.app/Contents/MacOS/openfolders$' | head -1)"
  PORT="$(lsof -nP -a -p "$PID" -iTCP -sTCP:LISTEN 2>/dev/null | awk 'NR>1 { n = split($9, a, ":"); print a[n]; exit }')"
  if [[ -z "$PORT" ]]; then
    echo "  ✗ couldn't find OpenFolders' local UI server"
    exit 1
  fi
  if [[ ! -d tests/ui/node_modules ]]; then
    (cd tests/ui && npm install --silent && npx playwright install chromium-headless-shell >/dev/null)
  fi
  node tests/ui/layout.mjs "$PORT" || status=1
fi
exit $status
