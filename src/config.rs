use crate::icon_gen::{encode_icns, generate_folder_icon_png};
use crate::macos::{extract_app_icon_png, normalize_app_path, set_custom_icon_on_path};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppEntry {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderGroup {
    pub id: String,
    pub name: String,
    pub accent: String,
    pub apps: Vec<AppEntry>,
    #[serde(default)]
    pub pinned_to_dock: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub folders: Vec<FolderGroup>,
    pub active_folder_id: String,
    #[serde(default = "default_true")]
    pub hide_on_blur: bool,
    /// Keep the Dock in sync with folders: apps inside a folder are unpinned from the Dock,
    /// and apps taken out of every folder are pinned back.
    #[serde(default)]
    pub keep_dock_clean: bool,
    /// The starter folders have been offered to the Dock at least once. install.sh runs
    /// --build-bundles (which writes config.json) before the app's first start, so the config
    /// file's existence can't tell a first run from an upgrade.
    #[serde(default)]
    pub first_run_pinning_done: bool,
}

fn default_true() -> bool {
    true
}

pub fn config_dir() -> PathBuf {
    let base = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
    let dir = base.join(".config").join("openfolders");
    let _ = fs::create_dir_all(&dir);
    dir
}

pub fn icon_cache_dir() -> PathBuf {
    let dir = config_dir().join("icon-cache");
    let _ = fs::create_dir_all(&dir);
    dir
}

pub fn folders_apps_dir() -> PathBuf {
    let base = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
    let dir = base.join("Applications").join("OpenFolders");
    let _ = fs::create_dir_all(&dir);
    dir
}

pub fn config_file_path() -> PathBuf {
    config_dir().join("config.json")
}

pub fn dock_backup_path() -> PathBuf {
    config_dir().join("com.apple.dock.backup.plist")
}

pub fn load_or_init_config() -> AppConfig {
    let path = config_file_path();
    if path.exists() {
        if let Ok(raw) = fs::read_to_string(&path) {
            if let Ok(cfg) = serde_json::from_str::<AppConfig>(&raw) {
                if !cfg.folders.is_empty() {
                    return cfg;
                }
            }
        }
        // Unreadable: keep a copy before starting over, so the user's folders aren't lost
        let _ = fs::copy(&path, path.with_extension("json.broken"));
    }

    let default_cfg = build_default_config();
    let _ = save_config(&default_cfg);
    default_cfg
}

pub fn save_config(cfg: &AppConfig) -> Result<(), String> {
    let path = config_file_path();
    let json = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    // Write a temp file and swap it in, so a crash mid-write never leaves half a file
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    fs::write(&tmp, json).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

fn make_existing_entries(candidates: &[(&str, &str)]) -> Vec<AppEntry> {
    let mut out = Vec::new();
    for &(name, path) in candidates {
        if Path::new(path).exists() {
            out.push(AppEntry {
                name: name.to_string(),
                path: normalize_app_path(path),
            });
        }
    }
    out
}

fn build_default_config() -> AppConfig {
    // Starter folders from common apps; only apps that are installed are added.
    let coding_apps = make_existing_entries(&[
        ("Xcode", "/Applications/Xcode.app"),
        ("Visual Studio Code", "/Applications/Visual Studio Code.app"),
        ("Cursor", "/Applications/Cursor.app"),
        ("Zed", "/Applications/Zed.app"),
        ("GitHub Desktop", "/Applications/GitHub Desktop.app"),
        ("iTerm", "/Applications/iTerm.app"),
        ("Ghostty", "/Applications/Ghostty.app"),
        ("Terminal", "/System/Applications/Utilities/Terminal.app"),
    ]);

    let ai_apps = make_existing_entries(&[
        ("Claude", "/Applications/Claude.app"),
        ("ChatGPT", "/Applications/ChatGPT.app"),
        ("Perplexity", "/Applications/Perplexity.app"),
        ("Ollama", "/Applications/Ollama.app"),
        ("LM Studio", "/Applications/LM Studio.app"),
        ("Obsidian", "/Applications/Obsidian.app"),
        ("Notion", "/Applications/Notion.app"),
    ]);

    let social_apps = make_existing_entries(&[
        ("Messages", "/System/Applications/Messages.app"),
        ("Slack", "/Applications/Slack.app"),
        ("Discord", "/Applications/Discord.app"),
        ("WhatsApp", "/Applications/WhatsApp.app"),
        ("Telegram", "/Applications/Telegram.app"),
        ("WeChat", "/Applications/WeChat.app"),
        ("LINE", "/Applications/LINE.app"),
    ]);

    let browsers_remote_apps = make_existing_entries(&[
        ("Safari", "/Applications/Safari.app"),
        ("Google Chrome", "/Applications/Google Chrome.app"),
        ("Arc", "/Applications/Arc.app"),
        ("Brave Browser", "/Applications/Brave Browser.app"),
        ("Firefox", "/Applications/Firefox.app"),
        (
            "Screen Sharing",
            "/System/Applications/Utilities/Screen Sharing.app",
        ),
        ("Tailscale", "/Applications/Tailscale.app"),
    ]);

    let folders = vec![
        FolderGroup {
            id: "coding".to_string(),
            name: "Coding & Dev".to_string(),
            accent: "#6366f1".to_string(),
            apps: coding_apps,
            pinned_to_dock: true,
        },
        FolderGroup {
            id: "ai-tools".to_string(),
            name: "AI & Agents".to_string(),
            accent: "#a855f7".to_string(),
            apps: ai_apps,
            pinned_to_dock: true,
        },
        FolderGroup {
            id: "social".to_string(),
            name: "Social & Chat".to_string(),
            accent: "#10b981".to_string(),
            apps: social_apps,
            pinned_to_dock: true,
        },
        FolderGroup {
            id: "browsers-remote".to_string(),
            name: "Web & Remote".to_string(),
            accent: "#0ea5e9".to_string(),
            apps: browsers_remote_apps,
            pinned_to_dock: true,
        },
    ];

    AppConfig {
        folders,
        active_folder_id: "coding".to_string(),
        hide_on_blur: true,
        keep_dock_clean: false,
        first_run_pinning_done: false,
    }
}

/// Scans `/Applications`, `/System/Applications`, `/System/Applications/Utilities`, and `~/Applications`.
pub fn scan_installed_apps() -> Vec<AppEntry> {
    let mut dirs_to_scan = vec![
        PathBuf::from("/Applications"),
        PathBuf::from("/Applications/Utilities"),
        PathBuf::from("/System/Applications"),
        PathBuf::from("/System/Applications/Utilities"),
    ];
    if let Some(home) = dirs::home_dir() {
        dirs_to_scan.push(home.join("Applications"));
    }

    let mut seen = HashSet::new();
    let mut apps = Vec::new();

    for dir in dirs_to_scan {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("app") {
                    let path_str = normalize_app_path(&path.to_string_lossy());
                    let stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("App")
                        .to_string();
                    if stem.contains("Uninstaller") || stem.starts_with("OpenFolders") {
                        continue;
                    }
                    if seen.insert(path_str.clone()) {
                        apps.push(AppEntry {
                            name: stem,
                            path: path_str,
                        });
                    }
                }
            }
        }
    }

    apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    apps
}

/// Gets or caches the 256x256 PNG icon for an app.
pub fn get_cached_app_icon(app_path: &str, mem_cache: &mut HashMap<String, Vec<u8>>) -> Option<Vec<u8>> {
    let norm = normalize_app_path(app_path);
    if let Some(bytes) = mem_cache.get(&norm) {
        return Some(bytes.clone());
    }

    let safe_key: String = norm
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    let disk_path = icon_cache_dir().join(format!("{}.png", safe_key));
    if disk_path.exists() {
        if let Ok(bytes) = fs::read(&disk_path) {
            if !bytes.is_empty() {
                mem_cache.insert(norm, bytes.clone());
                return Some(bytes);
            }
        }
    }

    if let Some(png) = extract_app_icon_png(&norm, 256) {
        let _ = fs::write(&disk_path, &png);
        mem_cache.insert(norm, png.clone());
        return Some(png);
    }

    None
}

/// Generates the 512x512 2x2 iPhone folder icon PNG for a given folder.
pub fn generate_folder_preview_png(
    folder: &FolderGroup,
    mem_cache: &mut HashMap<String, Vec<u8>>,
) -> Vec<u8> {
    let mut app_pngs = Vec::new();
    for app in folder.apps.iter().take(4) {
        if let Some(png) = get_cached_app_icon(&app.path, mem_cache) {
            app_pngs.push(png);
        }
    }
    generate_folder_icon_png(&app_pngs, &folder.accent)
}

pub fn sanitize_bundle_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-' || *c == '&' || *c == '_')
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "Folder".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Builds or updates the standalone macOS `.app` bundle for a folder (with its auto-generated 2x2 iPhone icon)
/// AND the main `OpenFolders.app` bundle in `~/Applications/OpenFolders/`.
/// Dock edits rewrite one settings file and bundle builds share temp names: each takes a turn,
/// so two at once can't drop each other's changes. Always take DOCK_EDIT before BUNDLE_BUILD.
static DOCK_EDIT: std::sync::Mutex<()> = std::sync::Mutex::new(());
static BUNDLE_BUILD: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn dock_edit_turn() -> std::sync::MutexGuard<'static, ()> {
    DOCK_EDIT.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn build_folder_app_bundles(
    cfg: &AppConfig,
    mem_cache: &mut HashMap<String, Vec<u8>>,
) -> Result<Vec<PathBuf>, String> {
    let _turn = BUNDLE_BUILD.lock().unwrap_or_else(|e| e.into_inner());
    let current_exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let out_dir = folders_apps_dir();
    let mut created_bundles = Vec::new();

    // 1. Build the main composer bundle: ~/Applications/OpenFolders/OpenFolders.app
    // Skipped while running from it: install.sh may have signed it, and rewriting it would
    // break that signature and with it the Accessibility permission
    let main_bundle = out_dir.join("OpenFolders.app");
    let running_from_main = current_exe.starts_with(&main_bundle);
    if let Some(first_folder) = cfg.folders.first().filter(|_| !running_from_main) {
        let main_png = generate_folder_preview_png(first_folder, mem_cache);
        create_single_app_bundle(
            &main_bundle,
            "dev.openfolders.composer",
            None,
            &main_png,
            &current_exe,
        )?;
    }

    // 2. Build an `.app` bundle for each folder with its live 2x2 mini-icon grid
    for folder in &cfg.folders {
        let safe_name = sanitize_bundle_name(&folder.name);
        let bundle_path = out_dir.join(format!("{}.app", safe_name));
        let png_512 = generate_folder_preview_png(folder, mem_cache);
        let bundle_id = format!("dev.openfolders.folder.{}", folder.id);
        create_single_app_bundle(
            &bundle_path,
            &bundle_id,
            Some(&folder.id),
            &png_512,
            &current_exe,
        )?;
        if folder.pinned_to_dock {
            created_bundles.push(bundle_path);
        }
    }

    // Remove bundles of folders that were renamed or deleted (only ones OpenFolders made)
    let current: std::collections::HashSet<String> = cfg
        .folders
        .iter()
        .map(|f| format!("{}.app", sanitize_bundle_name(&f.name)))
        .collect();
    if let Ok(entries) = fs::read_dir(&out_dir) {
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let ours = e.path().join("Contents/Resources/folder_id.txt").exists();
            if ours && name.ends_with(".app") && name != "OpenFolders.app" && !current.contains(&name) {
                let _ = fs::remove_dir_all(e.path());
            }
        }
    }

    Ok(created_bundles)
}

fn create_single_app_bundle(
    bundle_path: &Path,
    bundle_id: &str,
    folder_id: Option<&str>,
    png_512: &[u8],
    exe_source: &Path,
) -> Result<(), String> {
    let contents = bundle_path.join("Contents");
    let macos_dir = contents.join("MacOS");
    let resources_dir = contents.join("Resources");

    fs::create_dir_all(&macos_dir).map_err(|e| e.to_string())?;
    fs::create_dir_all(&resources_dir).map_err(|e| e.to_string())?;

    // Copy or update the executable stub
    let target_exe = macos_dir.join("openfolders");
    // Only overwrite if source != target
    if exe_source != target_exe {
        let tmp_exe = macos_dir.join("openfolders.tmp");
        if fs::copy(exe_source, &tmp_exe).is_ok() {
            if let Ok(meta) = fs::metadata(&tmp_exe) {
                let mut perms = meta.permissions();
                perms.set_mode(0o755);
                let _ = fs::set_permissions(&tmp_exe, perms);
            }
            // The source may be signed as part of OpenFolders.app, which ties its signature to
            // that bundle's Info.plist and stops this copy from launching. Re-sign it on its own
            // (outside any bundle) so it carries a plain ad-hoc signature.
            let loose = std::env::temp_dir().join(format!("openfolders-{}", std::process::id()));
            if fs::rename(&tmp_exe, &loose).is_ok() {
                let _ = Command::new("/usr/bin/codesign")
                    .args(["--force", "--sign", "-"])
                    .arg(&loose)
                    .output();
                if fs::copy(&loose, &tmp_exe).is_ok() {
                    let _ = fs::set_permissions(&tmp_exe, fs::Permissions::from_mode(0o755));
                }
                let _ = fs::remove_file(&loose);
            }
            let _ = fs::rename(&tmp_exe, &target_exe);
        }
    }

    // Write folder_id.txt if this bundle targets a specific folder
    let folder_id_file = resources_dir.join("folder_id.txt");
    if let Some(fid) = folder_id {
        fs::write(&folder_id_file, fid).map_err(|e| e.to_string())?;
    } else if folder_id_file.exists() {
        let _ = fs::remove_file(&folder_id_file);
    }

    // Write AppIcon.icns
    let icns_bytes = encode_icns(png_512);
    let icns_path = resources_dir.join("AppIcon.icns");
    fs::write(&icns_path, &icns_bytes).map_err(|e| e.to_string())?;

    // Write Info.plist with LSUIElement=true so clicking in Dock triggers immediately without bouncing.
    // The Dock reads these two name keys for a tile's hover label: a zero-width space keeps it
    // blank, like the tile's file-label, so only OpenFolders' own label beside the row shows the
    // title. The real name stays in the bundle's file name (Finder, Spotlight and TCC use that).
    let display_name = "\u{200b}";
    let bundle_id = &xml_escape(bundle_id);
    let info_plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleDevelopmentRegion</key>
    <string>en</string>
    <key>CFBundleDisplayName</key>
    <string>{display_name}</string>
    <key>CFBundleExecutable</key>
    <string>openfolders</string>
    <key>CFBundleIconFile</key>
    <string>AppIcon.icns</string>
    <key>CFBundleIdentifier</key>
    <string>{bundle_id}</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleName</key>
    <string>{display_name}</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleShortVersionString</key>
    <string>1.0</string>
    <key>CFBundleVersion</key>
    <string>1</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
"#
    );
    fs::write(contents.join("Info.plist"), info_plist).map_err(|e| e.to_string())?;

    // Set the live NSWorkspace custom icon on the .app bundle so macOS Dock immediately shows the 2x2 grid
    let _ = set_custom_icon_on_path(&bundle_path.to_string_lossy(), png_512);

    Ok(())
}

/// Syncs pinned folders into `com.apple.dock` (`persistent-apps`), optionally unpins grouped
/// apps, and pins `repin_apps` (apps just taken out of every folder) back. Restarts the Dock
/// only if its contents actually changed.
pub fn sync_macos_dock(
    cfg: &AppConfig,
    unpin_grouped_apps: bool,
    repin_apps: &[String],
    mem_cache: &mut HashMap<String, Vec<u8>>,
) -> Result<String, String> {
    let _turn = dock_edit_turn();
    let pinned_bundles = build_folder_app_bundles(cfg, mem_cache)?;
    let backup = dock_backup_path();

    // Create a one-time backup of the user's original com.apple.dock.plist before modifying
    if !backup.exists() {
        if let Some(home) = dirs::home_dir() {
            let dock_plist = home.join("Library/Preferences/com.apple.dock.plist");
            if dock_plist.exists() {
                let _ = fs::copy(&dock_plist, &backup);
            }
        }
    }

    let mut grouped_app_paths: Vec<String> = Vec::new();
    if unpin_grouped_apps {
        for folder in &cfg.folders {
            for app in &folder.apps {
                grouped_app_paths.push(normalize_app_path(&app.path));
            }
        }
    }

    let pinned_bundle_paths: Vec<String> = pinned_bundles
        .iter()
        .map(|p| normalize_app_path(&p.to_string_lossy()))
        .collect();

    let payload = serde_json::json!({
        "pinned_bundles": pinned_bundle_paths,
        "unpin_apps": grouped_app_paths,
        "unpin_enabled": unpin_grouped_apps,
        "repin_apps": repin_apps.iter().map(|p| normalize_app_path(p)).collect::<Vec<_>>(),
    });

    let py_script = r#"
import sys, json, os, plistlib, urllib.parse, subprocess, copy

def keep_autohide(dock):
    # While a row holds an auto-hiding Dock open this file says auto-hide is off;
    # never save that, or the Dock would stay up for good
    real = os.environ.get("OPENFOLDERS_AUTOHIDE")
    if real is not None:
        dock["autohide"] = real == "1"
    return dock

data = json.loads(sys.argv[1])
pinned_bundles = data["pinned_bundles"]
unpin_apps = set(data["unpin_apps"])
unpin_enabled = data["unpin_enabled"]

plist_path = os.path.expanduser("~/Library/Preferences/com.apple.dock.plist")
with open(plist_path, "rb") as f:
    dock = plistlib.load(f)

apps = dock.get("persistent-apps", [])
new_apps = []
# The folder tiles the config still wants, by bundle path
pinned_set = {b.rstrip("/") for b in pinned_bundles}
kept = set()

def get_app_path(entry):
    try:
        url = entry["tile-data"]["file-data"]["_CFURLString"]
        parsed = urllib.parse.unquote(urllib.parse.urlparse(url).path).rstrip("/")
        return parsed
    except Exception:
        return ""

for entry in apps:
    p = get_app_path(entry)
    if "/Applications/OpenFolders/" in p:
        key = p.rstrip("/")
        # Keep every tile where the user put it: drop only tiles of folders that
        # are gone and duplicates left behind by older versions
        if key not in pinned_set or key in kept:
            continue
        kept.add(key)
        # Deep copy: the label fix must count as a change against the original
        entry = copy.deepcopy(entry)
        entry.setdefault("tile-data", {})["file-label"] = "\u200b"
        new_apps.append(entry)
        continue
    if unpin_enabled and p in unpin_apps:
        continue
    new_apps.append(entry)

# Apps taken out of every folder go back to the Dock, ahead of any new folder tile
present = {get_app_path(e) for e in new_apps}
for app in data.get("repin_apps", []):
    if app in present or (unpin_enabled and app in unpin_apps) or not os.path.isdir(app):
        continue
    new_apps.append({
        "tile-type": "file-tile",
        "tile-data": {
            "file-label": os.path.splitext(os.path.basename(app))[0],
            "file-data": {"_CFURLString": "file://" + urllib.parse.quote(app) + "/", "_CFURLStringType": 15},
        },
    })

fresh = False
for bpath in pinned_bundles:
    # Already pinned: it kept its own place in the Dock above. Only a folder the
    # Dock has never seen gets a tile, appended at the end like any new pin.
    if bpath.rstrip("/") in present:
        continue
    fresh = True
    label = os.path.splitext(os.path.basename(bpath))[0]
    url_str = "file://" + urllib.parse.quote(bpath) + "/"
    tile = {
        "tile-type": "file-tile",
        "tile-data": {
            # OpenFolders shows the folder name beside the row instead
            "file-label": "\u200b",
            "file-data": {
                "_CFURLString": url_str,
                "_CFURLStringType": 15
            }
        }
    }
    new_apps.append(tile)

def tile_key(e):
    td = e.get("tile-data", {})
    return (get_app_path(e), td.get("file-label"), e.get("tile-type"))

# Nothing to change: leave the Dock alone (restarting it flickers)
if [tile_key(e) for e in apps] == [tile_key(e) for e in new_apps]:
    sys.exit(0)

dock["persistent-apps"] = new_apps
with open(plist_path, "wb") as f:
    plistlib.dump(keep_autohide(dock), f)

subprocess.run(["killall", "cfprefsd"], stderr=subprocess.DEVNULL)
subprocess.run(["killall", "Dock"], stderr=subprocess.DEVNULL)

# New tiles get their name filled in by the Dock once it restarts; blank it again in place
if fresh:
    import time
    time.sleep(3)
    subprocess.run(["killall", "cfprefsd"], stderr=subprocess.DEVNULL)
    with open(plist_path, "rb") as f:
        dock = plistlib.load(f)
    changed = False
    for e in dock.get("persistent-apps", []):
        td = e.get("tile-data", {})
        if "/Applications/OpenFolders/" in get_app_path(e) and td.get("file-label") != "\u200b":
            td["file-label"] = "\u200b"
            changed = True
    if changed:
        with open(plist_path, "wb") as f:
            plistlib.dump(keep_autohide(dock), f)
        subprocess.run(["killall", "cfprefsd"], stderr=subprocess.DEVNULL)
        subprocess.run(["killall", "Dock"], stderr=subprocess.DEVNULL)
"#;

    let status = dock_python()
        .arg("-c")
        .arg(py_script)
        .arg(payload.to_string())
        .status()
        .map_err(|e| format!("Failed to update Dock plist: {}", e))?;

    if status.success() {
        Ok("Synced folders to macOS Dock!".to_string())
    } else {
        Err("Dock plist update returned non-zero status".to_string())
    }
}

/// Puts back the invisible label on folder tiles if the Dock named them again, then
/// restarts the Dock. Returns true if anything changed.
pub fn hide_folder_tile_labels() -> bool {
    let _turn = dock_edit_turn();
    folder_tile_labels_script(false)
}

/// Whether any folder tile shows a label again, without touching the Dock.
pub fn folder_tile_labels_need_fix() -> bool {
    folder_tile_labels_script(true)
}

fn folder_tile_labels_script(check_only: bool) -> bool {
    let py = r#"
import os, plistlib, subprocess, sys

def keep_autohide(dock):
    # While a row holds an auto-hiding Dock open this file says auto-hide is off;
    # never save that, or the Dock would stay up for good
    real = os.environ.get("OPENFOLDERS_AUTOHIDE")
    if real is not None:
        dock["autohide"] = real == "1"
    return dock
p = os.path.expanduser("~/Library/Preferences/com.apple.dock.plist")
with open(p, "rb") as f:
    dock = plistlib.load(f)
changed = False
for e in dock.get("persistent-apps", []):
    td = e.get("tile-data", {})
    url = td.get("file-data", {}).get("_CFURLString", "")
    if "/Applications/OpenFolders/" in url and td.get("file-label") != "\u200b":
        td["file-label"] = "\u200b"
        changed = True
if not changed:
    sys.exit(1)
if sys.argv[1:] == ["check"]:
    sys.exit(0)
with open(p, "wb") as f:
    plistlib.dump(keep_autohide(dock), f)
subprocess.run(["killall", "cfprefsd"], stderr=subprocess.DEVNULL)
subprocess.run(["killall", "Dock"], stderr=subprocess.DEVNULL)
"#;
    let mut cmd = dock_python();
    cmd.arg("-c").arg(py);
    if check_only {
        cmd.arg("check");
    }
    cmd.status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Marker left while a row holds an auto-hiding Dock open (auto-hide is really on).
pub fn autohide_marker() -> PathBuf {
    config_dir().join("autohide-suspended")
}

/// Python for scripts that rewrite the Dock's settings file, told the user's real auto-hide
/// setting: the file says "off" while a row holds the Dock open.
fn dock_python() -> Command {
    let real = autohide_marker().exists() || crate::macos::dock_autohide_enabled();
    let mut c = Command::new("/usr/bin/python3");
    c.env("OPENFOLDERS_AUTOHIDE", if real { "1" } else { "0" });
    c
}

/// Restores the original macOS Dock layout from backup.
pub fn restore_macos_dock_backup() -> Result<String, String> {
    let _turn = dock_edit_turn();
    let backup = dock_backup_path();
    if !backup.exists() {
        return Err("No Dock backup found yet.".to_string());
    }
    let home = dirs::home_dir().ok_or("Could not find home dir")?;
    let dock_plist = home.join("Library/Preferences/com.apple.dock.plist");
    fs::copy(&backup, &dock_plist).map_err(|e| e.to_string())?;
    let _ = Command::new("killall").arg("cfprefsd").status();
    let _ = Command::new("killall").arg("Dock").status();
    Ok("Original macOS Dock restored!".to_string())
}

/// Pins one app to the end of the macOS Dock (used when an app is dragged out of a folder
/// and dropped on the Dock). No-op if it is already pinned.
pub fn pin_app_to_dock(app_path: &str) -> Result<(), String> {
    let _turn = dock_edit_turn();
    let py_script = r#"
import sys, os, plistlib, urllib.parse, subprocess

def keep_autohide(dock):
    # While a row holds an auto-hiding Dock open this file says auto-hide is off;
    # never save that, or the Dock would stay up for good
    real = os.environ.get("OPENFOLDERS_AUTOHIDE")
    if real is not None:
        dock["autohide"] = real == "1"
    return dock

app = sys.argv[1].rstrip("/")
plist_path = os.path.expanduser("~/Library/Preferences/com.apple.dock.plist")
with open(plist_path, "rb") as f:
    dock = plistlib.load(f)

apps = dock.get("persistent-apps", [])
for entry in apps:
    try:
        url = entry["tile-data"]["file-data"]["_CFURLString"]
        if urllib.parse.unquote(urllib.parse.urlparse(url).path).rstrip("/") == app:
            sys.exit(0)
    except Exception:
        pass

apps.append({
    "tile-type": "file-tile",
    "tile-data": {
        "file-label": os.path.splitext(os.path.basename(app))[0],
        "file-data": {"_CFURLString": "file://" + urllib.parse.quote(app) + "/", "_CFURLStringType": 15},
    },
})
dock["persistent-apps"] = apps
with open(plist_path, "wb") as f:
    plistlib.dump(keep_autohide(dock), f)
subprocess.run(["killall", "cfprefsd"], stderr=subprocess.DEVNULL)
subprocess.run(["killall", "Dock"], stderr=subprocess.DEVNULL)
"#;
    let status = dock_python()
        .arg("-c")
        .arg(py_script)
        .arg(normalize_app_path(app_path))
        .status()
        .map_err(|e| format!("Failed to update Dock plist: {}", e))?;
    if status.success() {
        Ok(())
    } else {
        Err("Dock plist update returned non-zero status".to_string())
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// Undoes OpenFolders in the Dock without touching anything else: removes the folder tiles
/// and pins every foldered app that isn't already in the Dock. Used by uninstall.
pub fn remove_from_dock(cfg: &AppConfig) -> Result<String, String> {
    let _turn = dock_edit_turn();
    let apps: Vec<String> = cfg
        .folders
        .iter()
        .flat_map(|f| f.apps.iter().map(|a| normalize_app_path(&a.path)))
        .collect();
    let py_script = r#"
import sys, json, os, plistlib, urllib.parse, subprocess

def keep_autohide(dock):
    # While a row holds an auto-hiding Dock open this file says auto-hide is off;
    # never save that, or the Dock would stay up for good
    real = os.environ.get("OPENFOLDERS_AUTOHIDE")
    if real is not None:
        dock["autohide"] = real == "1"
    return dock

apps = json.loads(sys.argv[1])
plist_path = os.path.expanduser("~/Library/Preferences/com.apple.dock.plist")
with open(plist_path, "rb") as f:
    dock = plistlib.load(f)

def get_app_path(entry):
    try:
        url = entry["tile-data"]["file-data"]["_CFURLString"]
        return urllib.parse.unquote(urllib.parse.urlparse(url).path).rstrip("/")
    except Exception:
        return ""

kept = [e for e in dock.get("persistent-apps", []) if "/Applications/OpenFolders/" not in get_app_path(e)]
present = {get_app_path(e) for e in kept}
for app in apps:
    if app in present or not os.path.isdir(app):
        continue
    present.add(app)
    kept.append({
        "tile-type": "file-tile",
        "tile-data": {
            "file-label": os.path.splitext(os.path.basename(app))[0],
            "file-data": {"_CFURLString": "file://" + urllib.parse.quote(app) + "/", "_CFURLStringType": 15},
        },
    })
dock["persistent-apps"] = kept
with open(plist_path, "wb") as f:
    plistlib.dump(keep_autohide(dock), f)
subprocess.run(["killall", "cfprefsd"], stderr=subprocess.DEVNULL)
subprocess.run(["killall", "Dock"], stderr=subprocess.DEVNULL)
"#;
    let status = dock_python()
        .arg("-c")
        .arg(py_script)
        .arg(serde_json::to_string(&apps).unwrap_or_default())
        .status()
        .map_err(|e| format!("Failed to update Dock plist: {}", e))?;
    if status.success() {
        Ok("Removed OpenFolders from the Dock and put your apps back.".to_string())
    } else {
        Err("Dock plist update returned non-zero status".to_string())
    }
}
