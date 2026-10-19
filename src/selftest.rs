//! `openfolders --self-test`: checks a live install end to end without touching the mouse.
//! Run it through `scripts/selftest.sh`, which starts it the way macOS starts the app, so the
//! Accessibility check is honest (from a terminal it would borrow the terminal's permission).

use crate::config::{self, folders_apps_dir, load_or_init_config};
use crate::macos::{self, DockIconRect};
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

/// Must match main.rs: the row's bottom sits this far below the top of the Dock tile.
const ROW_DROP: f64 = 4.0;

struct Report {
    failed: usize,
}

impl Report {
    fn check(&mut self, ok: bool, what: &str, detail: impl AsRef<str>) {
        let detail = detail.as_ref();
        if ok {
            println!("  ✓ {what}");
        } else {
            self.failed += 1;
            println!("  ✗ {what}: {detail}");
        }
    }
}

fn folder_tile<'a>(rects: &'a [DockIconRect], name: &str) -> Option<&'a DockIconRect> {
    let stem = config::sanitize_bundle_name(name);
    rects.iter().find(|r| {
        r.path.contains("/OpenFolders/")
            && Path::new(&r.path).file_stem().map(|s| s.to_string_lossy() == stem.as_str()) == Some(true)
    })
}

fn main_instance_pid() -> Option<i32> {
    let out = Command::new("/usr/bin/pgrep")
        .args(["-f", "OpenFolders.app/Contents/MacOS/openfolders$"])
        .output()
        .ok()?;
    let me = std::process::id() as i32;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse::<i32>().ok())
        .find(|p| *p != me)
}

pub fn run(socket_path: &str) -> i32 {
    let mut r = Report { failed: 0 };
    let cfg = load_or_init_config();

    println!("Install");
    let app = folders_apps_dir().join("OpenFolders.app");
    let sig = Command::new("/usr/bin/codesign").arg("-v").arg(&app).output();
    r.check(
        sig.as_ref().map(|o| o.status.success()).unwrap_or(false),
        "OpenFolders.app signature is valid",
        sig.map(|o| String::from_utf8_lossy(&o.stderr).trim().to_string()).unwrap_or_default(),
    );
    for f in &cfg.folders {
        let bundle = folders_apps_dir().join(format!("{}.app", config::sanitize_bundle_name(&f.name)));
        let plist = bundle.join("Contents/Info.plist");
        let lint = Command::new("/usr/bin/plutil").arg("-lint").arg(&plist).output();
        r.check(
            lint.map(|o| o.status.success()).unwrap_or(false),
            &format!("\"{}\" bundle is well formed", f.name),
            format!("{} is missing or invalid; run ./install.sh", plist.display()),
        );
        // A copy of the signed main binary is tied to OpenFolders.app and won't launch
        let ident = Command::new("/usr/bin/codesign")
            .arg("-dv")
            .arg(bundle.join("Contents/MacOS/openfolders"))
            .output()
            .map(|o| String::from_utf8_lossy(&o.stderr).to_string())
            .unwrap_or_default();
        r.check(
            !ident.contains("Identifier=dev.openfolders.composer"),
            &format!("\"{}\" opens when clicked", f.name),
            "its executable carries OpenFolders.app's signature; run ./install.sh",
        );
    }

    println!("Permissions");
    let trusted = macos::ensure_accessibility(false);
    r.check(
        trusted,
        "Accessibility is on (needed for hover)",
        "System Settings → Privacy & Security → Accessibility: add OpenFolders.app and turn it on",
    );
    let rects = macos::get_macos_dock_icon_rects();
    r.check(!rects.is_empty(), "the Dock's icons can be read", "0 icons read; see Accessibility above");

    println!("Dock");
    for f in &cfg.folders {
        match folder_tile(&rects, &f.name) {
            Some(t) => {
                let visible_name = t.title.replace('\u{200b}', "");
                r.check(
                    visible_name.trim().is_empty(),
                    &format!("\"{}\" is in the Dock without a name tag", f.name),
                    format!("the Dock shows \"{}\" over its row", visible_name),
                );
            }
            None if !rects.is_empty() => r.check(
                false,
                &format!("\"{}\" is in the Dock", f.name),
                "not pinned; ⋯ → Dock Settings → Pin",
            ),
            None => {}
        }
    }

    println!("Row");
    let pid = main_instance_pid();
    r.check(pid.is_some(), "OpenFolders is running", "start it with ./install.sh");
    let (Some(pid), true) = (pid, !rects.is_empty()) else {
        return summary(r);
    };
    let screen = macos::get_mouse_and_screen();
    let tile = macos::dock_tile_size().map_or(48.0, |s| s + 2.0);
    for f in &cfg.folders {
        let Some(t) = folder_tile(&rects, &f.name) else { continue };
        let msg = serde_json::json!({ "folder_id": f.id, "screen_pos": screen });
        let sent = UnixStream::connect(socket_path)
            .and_then(|mut s| s.write_all(msg.to_string().as_bytes()))
            .is_ok();
        if !sent {
            r.check(false, &format!("\"{}\" opens", f.name), "couldn't reach the running app");
            continue;
        }
        // The row shows within a frame or two; it closes again once it sees no pointer on it
        let start = Instant::now();
        let mut frame = None;
        while start.elapsed() < Duration::from_millis(800) {
            std::thread::sleep(Duration::from_millis(30));
            frame = macos::onscreen_window_frames(pid).into_iter().find(|w| w.2 > 60.0 && w.3 > 30.0);
            if frame.is_some() {
                break;
            }
        }
        let Some((x, y, w, h)) = frame else {
            r.check(false, &format!("\"{}\" opens", f.name), "no row appeared within 0.8 s");
            continue;
        };
        let centre = t.x + t.width * 0.5;
        let near_edge = centre - w / 2.0 < screen.screen_x + 14.0
            || centre + w / 2.0 > screen.screen_x + screen.screen_width - 14.0;
        r.check(
            near_edge || (x + w / 2.0 - centre).abs() <= 2.0,
            &format!("\"{}\" row is centred over its folder", f.name),
            format!("row centre {:.0}, folder centre {:.0}", x + w / 2.0, centre),
        );
        // A hidden Dock reports its tiles off-screen; the row then sits where the revealed Dock will be
        let expected_bottom = if t.y < screen.screen_height - 8.0 {
            t.y + ROW_DROP
        } else {
            screen.screen_height - tile - 22.0 + ROW_DROP
        };
        r.check(
            (y + h - expected_bottom).abs() <= 3.0,
            &format!("\"{}\" row sits right above the Dock", f.name),
            format!("row bottom {:.0}, expected {:.0}", y + h, expected_bottom),
        );
        // Let it close before the next folder
        std::thread::sleep(Duration::from_millis(700));
    }
    summary(r)
}

fn summary(r: Report) -> i32 {
    println!();
    if r.failed == 0 {
        println!("All checks passed.");
        0
    } else {
        println!("{} check{} failed.", r.failed, if r.failed == 1 { "" } else { "s" });
        1
    }
}
