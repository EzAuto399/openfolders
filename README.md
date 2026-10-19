# OpenFolders

**iPhone-style folders for the Mac Dock. Hover a folder, get its apps in a row right above it.**

<!-- demo video goes here -->

## Why

Too many apps in my Dock annoy me. Not having enough of them there annoys me too. I like my Dock clean and thin, not a long strip of icons.

My iPhone solved this years ago with folders, so I brought that design to the Mac. Your Dock stays the real macOS Dock, now holding a few folder icons. Each folder shows a 2×2 preview of the apps inside, like on iPhone. Hover one and its apps slide up in a second row that looks and feels like part of the Dock.

## What it does

- **Folders in your real Dock.** It doesn't replace the Dock; it extends it. Folder icons show a 2×2 preview of their apps.
- **Hover to open.** A row of the folder's apps appears just above the folder icon, in the same Liquid Glass as the Dock. Move along the Dock to switch folders.
- **Keep Dock clean (optional).** Apps inside folders leave the Dock. Take an app out of a folder and it goes back.
- **Drag and drop.** Drag apps from Finder onto a row to add them. Drag one off the row to take it out. Drag inside the row to reorder.
- **Works with auto-hide.** While a folder row is open the Dock stays up, then hides again as usual.
- **Small and local.** A ~2.5 MB native Rust app. No network access, no accounts, no tracking.

## Install

Paste this into Terminal:

```bash
git clone https://github.com/EzAuto399/openfolders.git && cd openfolders && ./install.sh
```

OpenFolders builds from source in about a minute. If you don't have [Rust](https://rustup.rs) yet, the installer offers to install it; if macOS lacks its command line tools, it opens Apple's installer for them. Built and tested on macOS 26 Tahoe; older versions should work but are untested.

The installer puts OpenFolders in `~/Applications/OpenFolders`, starts it and adds it to your login items. On first run it pins four starter folders to your Dock. They're filled with common apps you already have (developer tools, AI apps, chat apps, browsers), and you can change them all.

Then **allow OpenFolders under Accessibility** when macOS asks. It needs this to know which Dock icon you're hovering. Until you do, clicking a folder still opens it, and the row shows a **Turn on hover** button that takes you to the right setting.

To update later, run `git pull && ./install.sh` in the same folder.

## Use

| To | Do |
|---|---|
| Open a folder | Hover its icon in the Dock |
| Open an app | Click it in the row |
| Add apps | `+` in the row, or drag apps from Finder onto the row |
| Remove an app | Drag it off the row, or right-click → Remove from Folder |
| Reorder | Drag apps within the row |
| Rename, recolor or delete a folder | `⋯` → Edit Folder |
| New folder | `⋯` → New Folder |
| Keep the Dock clean | `⋯` → Dock Settings → Keep Dock clean |
| Close | Move the pointer away, or press Esc |

Right-click an app in the row to Hide, Quit, Force Quit, Move to another folder or Show in Finder.

## Uninstall

```bash
./uninstall.sh            # removes the app and its folders from the Dock and puts your apps back
./uninstall.sh --purge    # also deletes your folder settings
```

To put the Dock back exactly as it was before you installed OpenFolders, run `~/Applications/OpenFolders/OpenFolders.app/Contents/MacOS/openfolders --restore-dock` (it's backed up on first change).

## How it works

- Each folder is a tiny app bundle in `~/Applications/OpenFolders` with a generated 2×2 icon. That's what sits in your Dock.
- A background process reads where the Dock's icons are through the macOS Accessibility API. When you hover a folder icon, it shows the row: a transparent window over the Dock with native glass behind it.
- Folder tiles are pinned with an invisible label, so the Dock doesn't draw its own name tag over the row; OpenFolders shows the folder name beside the row instead.
- To keep an auto-hiding Dock up while you use a row, OpenFolders turns auto-hide off for that moment through the same private call the Dock's own settings use, and turns it back on when the row closes, even if OpenFolders quits.
- Settings live in `~/.config/openfolders/config.json`.

## Good to know

- **Running apps still show in the Dock.** macOS always shows running apps there, even with Keep Dock clean on.
- **Hover does nothing?** Check System Settings → Privacy & Security → Accessibility. Remove OpenFolders with **−**, then add `~/Applications/OpenFolders/OpenFolders.app` back with **+**. If you have an Apple developer certificate, the installer signs with it and this survives updates; otherwise each update is a new app to macOS and needs this again.
- **Bottom Dock, main display.** A Dock on the left or right side, and multiple displays, aren't supported yet.
- Changing which apps are pinned restarts the Dock for a moment. OpenFolders only does this after a row closes.

## Alternatives

macOS has no app folders for the Dock itself: Dock stacks open as a grid or fan on click and live on the right side of the Dock, and macOS 26 removed Launchpad folders. Several paid apps add folders to the Dock, such as [DockPops](https://dockpops.com/), [TinyFolder](https://tinyfolder.app/) and [ActiveDock](https://noteifyapp.com/activedock/), and [Dockish](https://dev.to/appish/how-to-make-app-folders-in-the-mac-dock-group-apps-like-on-iphone-ohg) replaces the Dock. OpenFolders is free and open source, opens on hover, and keeps your real Dock.

## Contributing

Issues and pull requests are welcome. The code is small:

| File | What's in it |
|---|---|
| `src/main.rs` | Hover tracking, the popup window, the local UI server |
| `src/macos.rs` | AppKit, Accessibility and Dock calls |
| `src/config.rs` | Folders, app bundles, Dock syncing |
| `src/icon_gen.rs` | The 2×2 folder icons |
| `src/ui.html` | The row, the app picker and the menus |

Build with `cargo build --release`; run `./target/release/openfolders --help` for the command-line options.

## License

[MIT](LICENSE)
