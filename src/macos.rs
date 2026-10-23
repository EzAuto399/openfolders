#![allow(unexpected_cfgs)]
#![allow(deprecated)]

use cocoa::appkit::{NSApplication, NSApplicationActivationPolicy, NSColor, NSScreen};
use cocoa::base::{id, nil, BOOL, NO, YES};
use cocoa::foundation::{NSAutoreleasePool, NSPoint, NSRect, NSSize, NSString};
use objc::{class, msg_send, sel, sel_impl};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::{c_char, c_void};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunningAppInfo {
    pub path: String,
    pub bundle_id: String,
    pub pid: i32,
    pub is_active: bool,
    pub is_hidden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenPosition {
    pub mouse_x: f64,
    pub mouse_y_top_left: f64,
    pub screen_x: f64,
    pub screen_y: f64,
    pub screen_width: f64,
    pub screen_height: f64,
    pub dock_bottom_inset: f64,
}

/// An autoreleased NSString; the caller's autorelease pool frees it.
unsafe fn ns_str(s: &str) -> id {
    let s = NSString::alloc(nil).init_str(s);
    msg_send![s, autorelease]
}

pub fn set_accessory_activation_policy() {
    unsafe {
        let app: id = NSApplication::sharedApplication(nil);
        app.setActivationPolicy_(
            NSApplicationActivationPolicy::NSApplicationActivationPolicyAccessory,
        );
    }
}

pub fn activate_this_app() {
    unsafe {
        let app: id = NSApplication::sharedApplication(nil);
        // NSApplicationActivateIgnoringOtherApps = 1 << 1
        let _: () = msg_send![app, activateIgnoringOtherApps: YES];
    }
}

/// Adds a native glass backdrop behind the webview (NSGlassEffectView on macOS 26,
/// NSVisualEffectView before). `clear` picks the see-through style the Dock uses; panels use
/// the frostier regular style so text on them stays readable. Returns the view pointer.
pub fn install_row_backdrop(ns_window_ptr: *mut c_void, clear: bool) -> usize {
    unsafe {
        let ns_win: id = ns_window_ptr as id;
        let content: id = msg_send![ns_win, contentView];
        if content == nil {
            return 0;
        }
        let frame = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0));
        let view: id = if let Some(cls) = objc::runtime::Class::get("NSGlassEffectView") {
            let v: id = msg_send![cls, alloc];
            let v: id = msg_send![v, initWithFrame: frame];
            // NSGlassEffectViewStyleRegular (0) / Clear (1)
            let responds: BOOL = msg_send![v, respondsToSelector: sel!(setStyle:)];
            if responds == YES {
                let _: () = msg_send![v, setStyle: if clear { 1i64 } else { 0i64 }];
            }
            v
        } else {
            let v: id = msg_send![class!(NSVisualEffectView), alloc];
            let v: id = msg_send![v, initWithFrame: frame];
            // NSVisualEffectMaterialPopover, BehindWindow blending, always active
            let _: () = msg_send![v, setMaterial: 6i64];
            let _: () = msg_send![v, setBlendingMode: 0i64];
            let _: () = msg_send![v, setState: 1i64];
            let _: () = msg_send![v, setWantsLayer: YES];
            v
        };
        // NSWindowBelow = -1: sit underneath the webview
        let _: () = msg_send![content, addSubview: view positioned: -1i64 relativeTo: nil];
        view as usize
    }
}

/// Moves the glass backdrop to the row's frame (content-view coords, bottom-left origin).
pub fn set_row_backdrop_frame(view: usize, x: f64, y: f64, w: f64, h: f64, radius: f64) {
    if view == 0 {
        return;
    }
    unsafe {
        let v = view as id;
        let frame = NSRect::new(NSPoint::new(x, y), NSSize::new(w.max(0.0), h.max(0.0)));
        let _: () = msg_send![v, setFrame: frame];
        let responds: BOOL = msg_send![v, respondsToSelector: sel!(setCornerRadius:)];
        if responds == YES {
            let _: () = msg_send![v, setCornerRadius: radius];
        } else {
            let layer: id = msg_send![v, layer];
            if layer != nil {
                let _: () = msg_send![layer, setCornerRadius: radius];
                let _: () = msg_send![layer, setMasksToBounds: YES];
            }
        }
    }
}

// Private but long-stable Dock calls (used by the Dock's own preference pane); instant and
// need no Automation permission, unlike scripting System Events.
#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn CoreDockGetAutoHideEnabled() -> u8;
    fn CoreDockSetAutoHideEnabled(flag: u8);
}

/// The Dock's icon size in points, from its own settings. Unlike Accessibility frames this
/// needs no permission and ignores magnification.
pub fn dock_tile_size() -> Option<f64> {
    unsafe {
        let pool = NSAutoreleasePool::new(nil);
        let defaults: id = msg_send![class!(NSUserDefaults), standardUserDefaults];
        let domain: id = msg_send![defaults, persistentDomainForName: ns_str("com.apple.dock")];
        let mut size = None;
        if domain != nil {
            let v: id = msg_send![domain, objectForKey: ns_str("tilesize")];
            if v != nil {
                let d: f64 = msg_send![v, doubleValue];
                if d >= 16.0 {
                    size = Some(d);
                }
            }
        }
        let _: () = msg_send![pool, drain];
        size
    }
}

pub fn dock_autohide_enabled() -> bool {
    unsafe { CoreDockGetAutoHideEnabled() != 0 }
}

pub fn set_dock_autohide(enabled: bool) {
    unsafe { CoreDockSetAutoHideEnabled(enabled as u8) }
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXIsProcessTrustedWithOptions(options: id) -> bool;
}

/// Reading Dock icon positions needs Accessibility access. Returns whether it's granted;
/// with `prompt`, macOS shows its "allow in Privacy & Security" dialog if it isn't.
pub fn ensure_accessibility(prompt: bool) -> bool {
    unsafe {
        let pool = NSAutoreleasePool::new(nil);
        let key = ns_str("AXTrustedCheckOptionPrompt");
        let value: id = msg_send![class!(NSNumber), numberWithBool: prompt];
        let dict: id = msg_send![class!(NSDictionary), dictionaryWithObject: value forKey: key];
        let trusted = AXIsProcessTrustedWithOptions(dict);
        let _: () = msg_send![pool, drain];
        trusted
    }
}

/// PID of the app that currently owns keyboard focus (0 if unknown).
pub fn frontmost_app_pid() -> i32 {
    unsafe {
        let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
        let app: id = msg_send![workspace, frontmostApplication];
        if app == nil {
            return 0;
        }
        msg_send![app, processIdentifier]
    }
}

/// Hands focus back to the app the user was in before the hover popup took it.
pub fn activate_app_pid(pid: i32) {
    unsafe {
        let app: id = msg_send![
            class!(NSRunningApplication),
            runningApplicationWithProcessIdentifier: pid
        ];
        if app != nil {
            let _: BOOL = msg_send![app, activateWithOptions: 0u64];
        }
    }
}

pub fn configure_popover_window(ns_window_ptr: *mut c_void) {
    unsafe {
        let ns_win: id = ns_window_ptr as id;
        if ns_win == nil {
            return;
        }
        let _: () = msg_send![ns_win, setOpaque: NO];
        let clear: id = NSColor::clearColor(nil);
        let _: () = msg_send![ns_win, setBackgroundColor: clear];
        // Disable rectangular NSWindow shadow so CSS squircle shadow has zero box artifacts
        let _: () = msg_send![ns_win, setHasShadow: NO];
        // kCGStatusWindowLevel (25): above the Dock (kCGDockWindowLevel = 20) and its name
        // labels (drawn inside the Dock's own window), so the row covers the Dock's label
        // instead of colliding with it. Must stay below kCGDraggingWindowLevel (500):
        // AppKit does not deliver drag-session events to windows at/above the drag
        // overlay's level, which breaks in-row drag & drop (was NSScreenSaverWindowLevel).
        let _: () = msg_send![ns_win, setLevel: 25i64];
        // CanJoinAllSpaces (1 << 0) | Transient (1 << 3) | IgnoresCycle (1 << 6) | FullScreenAuxiliary (1 << 8)
        let behavior: u64 = (1 << 0) | (1 << 3) | (1 << 6) | (1 << 8);
        let _: () = msg_send![ns_win, setCollectionBehavior: behavior];
        let _: () = msg_send![ns_win, setMovableByWindowBackground: NO];
    }
}

pub fn get_mouse_and_screen() -> ScreenPosition {
    unsafe {
        let _pool = NSAutoreleasePool::new(nil);
        let mouse: NSPoint = msg_send![class!(NSEvent), mouseLocation];

        let screens: id = NSScreen::screens(nil);
        let count: usize = msg_send![screens, count];
        let mut target_screen: id = NSScreen::mainScreen(nil);

        for i in 0..count {
            let scr: id = msg_send![screens, objectAtIndex: i];
            let frame: NSRect = NSScreen::frame(scr);
            if mouse.x >= frame.origin.x
                && mouse.x <= frame.origin.x + frame.size.width
                && mouse.y >= frame.origin.y
                && mouse.y <= frame.origin.y + frame.size.height
            {
                target_screen = scr;
                break;
            }
        }

        if target_screen == nil {
            return ScreenPosition {
                mouse_x: mouse.x,
                mouse_y_top_left: 800.0 - mouse.y,
                screen_x: 0.0,
                screen_y: 0.0,
                screen_width: 1440.0,
                screen_height: 900.0,
                dock_bottom_inset: 80.0,
            };
        }

        let frame: NSRect = NSScreen::frame(target_screen);
        let visible: NSRect = NSScreen::visibleFrame(target_screen);

        // On macOS Cocoa, origin (0,0) is bottom-left of primary screen.
        // visible.origin.y - frame.origin.y is the bottom Dock height!
        let dock_bottom_inset = (visible.origin.y - frame.origin.y).max(76.0);
        let mouse_y_top_left = (frame.origin.y + frame.size.height) - mouse.y;

        ScreenPosition {
            mouse_x: mouse.x,
            mouse_y_top_left,
            screen_x: frame.origin.x,
            screen_y: frame.origin.y,
            screen_width: frame.size.width,
            screen_height: frame.size.height,
            dock_bottom_inset,
        }
    }
}

/// Extract an application's native macOS icon as PNG bytes using `NSWorkspace.sharedWorkspace.iconForFile:`.
/// Works for all `.app` bundles including macOS 26 Tahoe system apps with `Assets.car` icons.
pub fn extract_app_icon_png(app_path: &str, pixel_size: u32) -> Option<Vec<u8>> {
    let resolved_path = std::fs::canonicalize(app_path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| app_path.to_string());
    unsafe {
        let pool = NSAutoreleasePool::new(nil);
        let result = (|| -> Option<Vec<u8>> {
            let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
            if workspace == nil {
                return None;
            }
            let ns_path = ns_str(&resolved_path);
            let icon: id = msg_send![workspace, iconForFile: ns_path];
            if icon == nil {
                return None;
            }

            let target_size = NSSize::new(pixel_size as f64, pixel_size as f64);
            let _: () = msg_send![icon, setSize: target_size];

            // Create an explicit NSBitmapImageRep of exact pixel dimensions so Retina scaling never skews size
            let color_space_name = ns_str("NSCalibratedRGBColorSpace");
            let rep: id = msg_send![class!(NSBitmapImageRep), alloc];
            let rep: id = msg_send![
                rep,
                initWithBitmapDataPlanes: std::ptr::null_mut::<*mut u8>()
                pixelsWide: pixel_size as isize
                pixelsHigh: pixel_size as isize
                bitsPerSample: 8isize
                samplesPerPixel: 4isize
                hasAlpha: YES
                isPlanar: NO
                colorSpaceName: color_space_name
                bytesPerRow: 0isize
                bitsPerPixel: 0isize
            ];
            if rep == nil {
                return None;
            }
            let rep: id = msg_send![rep, autorelease];
            let _: () = msg_send![rep, setSize: target_size];

            let gfx_ctx: id =
                msg_send![class!(NSGraphicsContext), graphicsContextWithBitmapImageRep: rep];
            if gfx_ctx == nil {
                return None;
            }

            let _: () = msg_send![class!(NSGraphicsContext), saveGraphicsState];
            let _: () = msg_send![class!(NSGraphicsContext), setCurrentContext: gfx_ctx];
            let _: () = msg_send![gfx_ctx, setImageInterpolation: 3usize]; // NSImageInterpolationHigh

            let dest_rect = NSRect::new(NSPoint::new(0.0, 0.0), target_size);
            let zero_rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0));
            // NSCompositingOperationSourceOver = 2
            let _: () = msg_send![
                icon,
                drawInRect: dest_rect
                fromRect: zero_rect
                operation: 2usize
                fraction: 1.0f64
            ];

            let _: () = msg_send![class!(NSGraphicsContext), restoreGraphicsState];

            let empty_dict: id = msg_send![class!(NSDictionary), dictionary];
            // NSBitmapImageFileTypePNG = 4
            let png_data: id = msg_send![
                rep,
                representationUsingType: 4usize
                properties: empty_dict
            ];
            if png_data == nil {
                return None;
            }

            let length: usize = msg_send![png_data, length];
            let bytes_ptr: *const u8 = msg_send![png_data, bytes];
            if bytes_ptr.is_null() || length == 0 {
                return None;
            }

            let slice = std::slice::from_raw_parts(bytes_ptr, length);
            Some(slice.to_vec())
        })();
        let _: () = msg_send![pool, drain];
        result
    }
}

/// Set a custom PNG icon directly on a file or `.app` bundle via `NSWorkspace.setIcon:forFile:options:`.
pub fn set_custom_icon_on_path(target_path: &str, png_bytes: &[u8]) -> bool {
    unsafe {
        let pool = NSAutoreleasePool::new(nil);
        let ok = (|| -> bool {
            let data: id = msg_send![
                class!(NSData),
                dataWithBytes: png_bytes.as_ptr() as *const c_void
                length: png_bytes.len()
            ];
            if data == nil {
                return false;
            }
            let image: id = msg_send![class!(NSImage), alloc];
            let image: id = msg_send![image, initWithData: data];
            if image == nil {
                return false;
            }
            let image: id = msg_send![image, autorelease];
            let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
            let ns_path = ns_str(target_path);
            let success: BOOL = msg_send![
                workspace,
                setIcon: image
                forFile: ns_path
                options: 0usize
            ];
            success == YES
        })();
        let _: () = msg_send![pool, drain];
        ok
    }
}

unsafe fn nsstring_to_string(ns_str: id) -> String {
    if ns_str == nil {
        return String::new();
    }
    let utf8: *const c_char = msg_send![ns_str, UTF8String];
    if utf8.is_null() {
        return String::new();
    }
    CStr::from_ptr(utf8).to_string_lossy().into_owned()
}

pub fn normalize_app_path(p: &str) -> String {
    p.trim_end_matches('/').to_string()
}

/// Query all running macOS applications via `NSWorkspace.runningApplications`.
pub fn get_running_apps() -> HashMap<String, RunningAppInfo> {
    let mut map = HashMap::new();
    unsafe {
        let pool = NSAutoreleasePool::new(nil);
        let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
        if workspace != nil {
            let running: id = msg_send![workspace, runningApplications];
            let count: usize = msg_send![running, count];
            for i in 0..count {
                let app: id = msg_send![running, objectAtIndex: i];
                if app == nil {
                    continue;
                }
                let is_terminated: BOOL = msg_send![app, isTerminated];
                if is_terminated == YES {
                    continue;
                }
                let url: id = msg_send![app, bundleURL];
                if url == nil {
                    continue;
                }
                let path_ns: id = msg_send![url, path];
                let raw_path = nsstring_to_string(path_ns);
                if raw_path.is_empty() {
                    continue;
                }
                let norm_path = normalize_app_path(&raw_path);
                let bundle_id_ns: id = msg_send![app, bundleIdentifier];
                let bundle_id = nsstring_to_string(bundle_id_ns);
                let pid: i32 = msg_send![app, processIdentifier];
                let is_active: BOOL = msg_send![app, isActive];
                let is_hidden: BOOL = msg_send![app, isHidden];

                map.insert(
                    norm_path.clone(),
                    RunningAppInfo {
                        path: norm_path,
                        bundle_id,
                        pid,
                        is_active: is_active == YES,
                        is_hidden: is_hidden == YES,
                    },
                );
            }
        }
        let _: () = msg_send![pool, drain];
    }
    map
}

/// Execute a native action on an application (`launch`, `hide`, `unhide`, `quit`, `force_quit`, `reveal`).
pub fn perform_app_action(app_path: &str, action: &str) -> Result<String, String> {
    let norm_target = normalize_app_path(app_path);
    match action {
        "launch" => {
            // If running and hidden, unhide first, then open/activate
            unsafe {
                let pool = NSAutoreleasePool::new(nil);
                let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
                if workspace != nil {
                    let running: id = msg_send![workspace, runningApplications];
                    let count: usize = msg_send![running, count];
                    for i in 0..count {
                        let app: id = msg_send![running, objectAtIndex: i];
                        let url: id = msg_send![app, bundleURL];
                        if url != nil {
                            let path_ns: id = msg_send![url, path];
                            if normalize_app_path(&nsstring_to_string(path_ns)) == norm_target {
                                let _: BOOL = msg_send![app, unhide];
                                // NSApplicationActivateIgnoringOtherApps = 1 << 1
                                let _: BOOL = msg_send![app, activateWithOptions: 2usize];
                                break;
                            }
                        }
                    }
                }
                let _: () = msg_send![pool, drain];
            }
            Command::new("open")
                .arg("-a")
                .arg(&norm_target)
                .spawn()
                .map_err(|e| format!("Failed to open {}: {}", norm_target, e))?;
            Ok("Launched".to_string())
        }
        "reveal" => {
            Command::new("open")
                .arg("-R")
                .arg(&norm_target)
                .spawn()
                .map_err(|e| format!("Failed to reveal {}: {}", norm_target, e))?;
            Ok("Revealed in Finder".to_string())
        }
        "hide" | "unhide" | "quit" | "force_quit" => unsafe {
            let pool = NSAutoreleasePool::new(nil);
            let mut matched = false;
            let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
            if workspace != nil {
                let running: id = msg_send![workspace, runningApplications];
                let count: usize = msg_send![running, count];
                for i in 0..count {
                    let app: id = msg_send![running, objectAtIndex: i];
                    let url: id = msg_send![app, bundleURL];
                    if url == nil {
                        continue;
                    }
                    let path_ns: id = msg_send![url, path];
                    if normalize_app_path(&nsstring_to_string(path_ns)) == norm_target {
                        matched = true;
                        match action {
                            "hide" => {
                                let _: BOOL = msg_send![app, hide];
                            }
                            "unhide" => {
                                let _: BOOL = msg_send![app, unhide];
                                let _: BOOL = msg_send![app, activateWithOptions: 2usize];
                            }
                            "quit" => {
                                let _: BOOL = msg_send![app, terminate];
                            }
                            "force_quit" => {
                                let _: BOOL = msg_send![app, forceTerminate];
                            }
                            _ => {}
                        }
                    }
                }
            }
            let _: () = msg_send![pool, drain];
            if matched {
                Ok(format!("Executed {}", action))
            } else if Path::new(&norm_target).exists() {
                Err("App is not currently running".to_string())
            } else {
                Err("App path not found".to_string())
            }
        },
        _ => Err(format!("Unknown action: {}", action)),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DockIconRect {
    pub title: String,
    /// File path of the tile's app bundle (empty for folders/separators). Folder tiles are
    /// pinned with no label so the Dock draws no name tag, so they're matched by this.
    pub path: String,
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[link(name = "ApplicationServices", kind = "framework")]
extern "C" {
    fn AXUIElementCreateApplication(pid: i32) -> *const c_void;
    fn AXUIElementCopyAttributeValue(
        element: *const c_void,
        attribute: id,
        value: *mut *const c_void,
    ) -> i32;
    fn AXValueGetValue(value: *const c_void, the_type: u32, value_ptr: *mut c_void) -> bool;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(cf: *const c_void);
}

/// Reads the live screen coordinates `(x, y, width, height)` (top-left origin) of every icon in the macOS Dock
/// directly via `ApplicationServices` (`AXUIElement`) in < 0.2ms.
pub fn get_macos_dock_icon_rects() -> Vec<DockIconRect> {
    let mut out = Vec::new();
    unsafe {
        let pool = NSAutoreleasePool::new(nil);
        let workspace: id = msg_send![class!(NSWorkspace), sharedWorkspace];
        if workspace == nil {
            let _: () = msg_send![pool, drain];
            return out;
        }

        // Find PID of com.apple.dock
        let mut dock_pid: i32 = 0;
        let running: id = msg_send![workspace, runningApplications];
        let rcount: usize = msg_send![running, count];
        for i in 0..rcount {
            let app: id = msg_send![running, objectAtIndex: i];
            if app != nil {
                let bid: id = msg_send![app, bundleIdentifier];
                if nsstring_to_string(bid) == "com.apple.dock" {
                    dock_pid = msg_send![app, processIdentifier];
                    break;
                }
            }
        }

        if dock_pid <= 0 {
            let _: () = msg_send![pool, drain];
            return out;
        }

        let ax_dock = AXUIElementCreateApplication(dock_pid);
        if ax_dock.is_null() {
            let _: () = msg_send![pool, drain];
            return out;
        }

        let attr_children = ns_str("AXChildren");
        let attr_title = ns_str("AXTitle");
        let attr_pos = ns_str("AXPosition");
        let attr_size = ns_str("AXSize");
        let attr_url = ns_str("AXURL");

        let mut dock_children: *const c_void = std::ptr::null();
        if AXUIElementCopyAttributeValue(ax_dock, attr_children, &mut dock_children) == 0
            && !dock_children.is_null()
        {
            let top_arr: id = dock_children as id;
            let top_count: usize = msg_send![top_arr, count];
            if top_count > 0 {
                // First child of Dock is AXList containing all Dock tiles
                let ax_list: *const c_void = msg_send![top_arr, objectAtIndex: 0usize];
                let mut list_children: *const c_void = std::ptr::null();
                if !ax_list.is_null()
                    && AXUIElementCopyAttributeValue(ax_list, attr_children, &mut list_children)
                        == 0
                    && !list_children.is_null()
                {
                    let items_arr: id = list_children as id;
                    let item_count: usize = msg_send![items_arr, count];
                    for j in 0..item_count {
                        let item: *const c_void = msg_send![items_arr, objectAtIndex: j];
                        if item.is_null() {
                            continue;
                        }
                        let mut title_val: *const c_void = std::ptr::null();
                        let mut pos_val: *const c_void = std::ptr::null();
                        let mut size_val: *const c_void = std::ptr::null();

                        let title = if AXUIElementCopyAttributeValue(
                            item,
                            attr_title,
                            &mut title_val,
                        ) == 0
                            && !title_val.is_null()
                        {
                            let s = nsstring_to_string(title_val as id);
                            CFRelease(title_val);
                            s
                        } else {
                            String::new()
                        };

                        let mut url_val: *const c_void = std::ptr::null();
                        let path = if AXUIElementCopyAttributeValue(item, attr_url, &mut url_val) == 0
                            && !url_val.is_null()
                        {
                            // CFURL is toll-free bridged to NSURL
                            let p: id = msg_send![url_val as id, path];
                            let s = nsstring_to_string(p);
                            CFRelease(url_val);
                            s
                        } else {
                            String::new()
                        };

                        let mut pt = NSPoint::new(0.0, 0.0);
                        let mut sz = NSSize::new(0.0, 0.0);

                        if AXUIElementCopyAttributeValue(item, attr_pos, &mut pos_val) == 0
                            && !pos_val.is_null()
                        {
                            // kAXValueCGPointType = 1
                            AXValueGetValue(
                                pos_val,
                                1,
                                &mut pt as *mut NSPoint as *mut c_void,
                            );
                            CFRelease(pos_val);
                        }
                        if AXUIElementCopyAttributeValue(item, attr_size, &mut size_val) == 0
                            && !size_val.is_null()
                        {
                            // kAXValueCGSizeType = 2
                            AXValueGetValue(
                                size_val,
                                2,
                                &mut sz as *mut NSSize as *mut c_void,
                            );
                            CFRelease(size_val);
                        }

                        if (!title.is_empty() || !path.is_empty()) && sz.width > 0.0 {
                            out.push(DockIconRect {
                                title,
                                path,
                                x: pt.x,
                                y: pt.y,
                                width: sz.width,
                                height: sz.height,
                            });
                        }
                    }
                    CFRelease(list_children);
                }
            }
            CFRelease(dock_children);
        }

        CFRelease(ax_dock);
        let _: () = msg_send![pool, drain];
    }
    out
}


#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGWindowListCopyWindowInfo(option: u32, relative_to_window: u32) -> *const c_void;
}

/// On-screen windows of a process as (x, y, width, height), top-left origin.
pub fn onscreen_window_frames(pid: i32) -> Vec<(f64, f64, f64, f64)> {
    let mut out = Vec::new();
    unsafe {
        let pool = NSAutoreleasePool::new(nil);
        // kCGWindowListOptionOnScreenOnly, kCGNullWindowID
        let list = CGWindowListCopyWindowInfo(1, 0);
        if !list.is_null() {
            let arr = list as id;
            let count: usize = msg_send![arr, count];
            let key = |s: &str| ns_str(s);
            for i in 0..count {
                let info: id = msg_send![arr, objectAtIndex: i];
                let owner: id = msg_send![info, objectForKey: key("kCGWindowOwnerPID")];
                if owner == nil || { let p: i32 = msg_send![owner, intValue]; p } != pid {
                    continue;
                }
                let b: id = msg_send![info, objectForKey: key("kCGWindowBounds")];
                if b == nil {
                    continue;
                }
                let num = |k: &str| -> f64 {
                    let v: id = msg_send![b, objectForKey: key(k)];
                    if v == nil { 0.0 } else { msg_send![v, doubleValue] }
                };
                out.push((num("X"), num("Y"), num("Width"), num("Height")));
            }
            CFRelease(list);
        }
        let _: () = msg_send![pool, drain];
    }
    out
}
