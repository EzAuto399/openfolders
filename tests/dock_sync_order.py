#!/usr/bin/env python3
"""Checks that syncing folders into the Dock keeps every pinned folder tile where the
user put it, instead of moving them all to the end in config order.

Runs the Python that src/config.rs embeds in sync_macos_dock(), against a throwaway
HOME, with a stubbed `killall` so the real Dock is never touched.

Usage: python3 tests/dock_sync_order.py
"""
import json
import os
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.parse

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
APPS_DIR = "/Users/test/Applications/OpenFolders"


def load_sync_script():
    src = open(os.path.join(ROOT, "src/config.rs"), encoding="utf8").read()
    m = re.search(r'let py_script = r#"(.*?)"#;', src, re.S)
    if not m:
        sys.exit("FAIL: couldn't find the embedded sync script in src/config.rs")
    return m.group(1)


def tile(path, label=None):
    return {
        "tile-type": "file-tile",
        "tile-data": {
            **({"file-label": label} if label is not None else {}),
            "file-data": {"_CFURLString": "file://" + urllib.parse.quote(path) + "/",
                          "_CFURLStringType": 15},
        },
    }


def write_plist(home, entries):
    p = os.path.join(home, "Library/Preferences/com.apple.dock.plist")
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "wb") as f:
        plistlib.dump({"persistent-apps": entries, "autohide": True}, f)
    return p


def run(script, home, killlog, payload):
    env = dict(os.environ, HOME=home, PATH=os.path.join(home, "bin") + ":" + os.environ["PATH"],
               KILLLOG=killlog)
    return subprocess.run([sys.executable, "-c", script, json.dumps(payload)],
                          env=env, capture_output=True, text=True)


def paths(plist_path):
    with open(plist_path, "rb") as f:
        dock = plistlib.load(f)
    out = []
    for e in dock["persistent-apps"]:
        url = e["tile-data"]["file-data"]["_CFURLString"]
        out.append(urllib.parse.unquote(urllib.parse.urlparse(url).path).rstrip("/"))
    return out, [e["tile-data"].get("file-label") for e in dock["persistent-apps"]]


def setup_home():
    home = tempfile.mkdtemp(prefix="of-dock-test-")
    bindir = os.path.join(home, "bin")
    os.makedirs(bindir)
    # The sync restarts the Dock when it changes something; record instead of doing it
    with open(os.path.join(bindir, "killall"), "w") as f:
        f.write('#!/bin/sh\necho "$@" >> "$KILLLOG"\nexit 0\n')
    os.chmod(os.path.join(bindir, "killall"), 0o755)
    return home


def main():
    script = load_sync_script()
    failures = []

    # 1. A pinned tile that is already correct is left alone: no write, no Dock restart
    home = setup_home()
    killlog = os.path.join(home, "killall.log")
    keep = os.path.join(APPS_DIR, "Keep.app")
    plist = write_plist(home, [tile("/Applications/One.app"), tile(keep, "\u200b"),
                               tile("/Applications/Two.app")])
    before = open(plist, "rb").read()
    r = run(script, home, killlog, {"pinned_bundles": [keep], "unpin_apps": [],
                                    "unpin_enabled": False, "repin_apps": []})
    after = open(plist, "rb").read()
    if r.returncode != 0:
        failures.append(f"unchanged sync exited {r.returncode}: {r.stderr.strip()}")
    if before != after:
        failures.append("unchanged sync rewrote the Dock settings anyway")
    if os.path.exists(killlog):
        failures.append("unchanged sync restarted the Dock")
    shutil.rmtree(home)

    # 2. Positions survive a sync: no tile moves, a gone folder loses its tile,
    #    a never-pinned folder lands at the end
    home = setup_home()
    killlog = os.path.join(home, "killall.log")
    keep = os.path.join(APPS_DIR, "Keep.app")
    gone = os.path.join(APPS_DIR, "Gone.app")
    new = os.path.join(APPS_DIR, "New.app")
    plist = write_plist(home, [tile("/Applications/One.app"), tile(keep, "Keep me"),
                               tile("/Applications/Two.app"), tile(gone, "\u200b")])
    r = run(script, home, killlog, {"pinned_bundles": [keep, new], "unpin_apps": [],
                                    "unpin_enabled": False, "repin_apps": []})
    if r.returncode != 0:
        failures.append(f"sync exited {r.returncode}: {r.stderr.strip()}")
    got, labels = paths(plist)
    want = ["/Applications/One.app", keep, "/Applications/Two.app", new]
    if got != want:
        failures.append(f"tile order changed:\n  got  {got}\n  want {want}")
    if labels[1] != "\u200b":
        failures.append(f"pinned tile label not blanked: {labels[1]!r}")
    if not os.path.exists(killlog):
        failures.append("sync with changes never restarted the Dock")
    shutil.rmtree(home)

    if failures:
        print("FAIL")
        for f in failures:
            print(" -", f)
        return 1
    print("PASS: folder tiles keep their Dock position across a sync")
    return 0


if __name__ == "__main__":
    sys.exit(main())
