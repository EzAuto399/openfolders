use image::{imageops::FilterType, ImageFormat, Rgba, RgbaImage};
use std::io::Cursor;

pub fn parse_hex_color(hex: &str) -> (u8, u8, u8) {
    let h = hex.trim_start_matches('#');
    // Byte slicing below needs ASCII; a hand-edited colour like "#ééé" would panic
    if h.len() >= 6 && h.is_ascii() {
        let r = u8::from_str_radix(&h[0..2], 16).unwrap_or(99);
        let g = u8::from_str_radix(&h[2..4], 16).unwrap_or(102);
        let b = u8::from_str_radix(&h[4..6], 16).unwrap_or(241);
        (r, g, b)
    } else {
        (99, 102, 241)
    }
}

/// Signed distance to a rounded rectangle centered at `(cx, cy)` with half-size `(hx, hy)` and corner radius `r`.
#[inline]
fn sd_rounded_box(px: f32, py: f32, cx: f32, cy: f32, hx: f32, hy: f32, r: f32) -> f32 {
    let qx = (px - cx).abs() - (hx - r);
    let qy = (py - cy).abs() - (hy - r);
    let out_dist = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    let in_dist = qx.max(qy).min(0.0);
    out_dist + in_dist - r
}

#[inline]
fn blend_pixel(dst: &mut Rgba<u8>, src: [u8; 4]) {
    let sa = src[3] as f32 / 255.0;
    if sa <= 0.001 {
        return;
    }
    let da = dst[3] as f32 / 255.0;
    let out_a = sa + da * (1.0 - sa);
    if out_a <= 0.001 {
        return;
    }
    for i in 0..3 {
        let sc = src[i] as f32 / 255.0;
        let dc = dst[i] as f32 / 255.0;
        let out_c = (sc * sa + dc * da * (1.0 - sa)) / out_a;
        dst[i] = (out_c.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    dst[3] = (out_a.clamp(0.0, 1.0) * 255.0).round() as u8;
}

/// Generates a 512x512 Retina iPhone-style 2x2 Folder Icon PNG from up to 4 app icon PNGs.
pub fn generate_folder_icon_png(app_pngs: &[Vec<u8>], accent_hex: &str) -> Vec<u8> {
    let size = 512u32;
    let mut canvas = RgbaImage::from_pixel(size, size, Rgba([0, 0, 0, 0]));
    let (ar, ag, ab) = parse_hex_color(accent_hex);

    // Standard macOS Dock squircle bounds: 428x428 centered at (256, 256), corner radius 98
    let cx = 256.0f32;
    let cy = 256.0f32;
    let half = 214.0f32;
    let radius = 98.0f32;

    // 1. Draw soft outer drop shadow + frosted glass folder squircle
    for y in 0..size {
        for x in 0..size {
            let px = x as f32 + 0.5;
            let py = y as f32 + 0.5;

            // Drop shadow shifted down by 10px
            let shadow_d = sd_rounded_box(px, py - 10.0, cx, cy, half - 2.0, half - 2.0, radius);
            if shadow_d < 22.0 {
                let s_alpha = ((22.0 - shadow_d) / 22.0).clamp(0.0, 1.0).powi(2) * 0.42;
                let p = canvas.get_pixel_mut(x, y);
                blend_pixel(p, [0, 0, 0, (s_alpha * 255.0) as u8]);
            }

            let d = sd_rounded_box(px, py, cx, cy, half, half, radius);
            if d < 1.0 {
                let edge_alpha = (0.5 - d).clamp(0.0, 1.0);
                // Vertical gradient factor (0 at top of squircle, 1 at bottom)
                let vy = ((py - (cy - half)) / (half * 2.0)).clamp(0.0, 1.0);
                // Radial glow from top-left
                let gx = (px - 140.0) / 360.0;
                let gy = (py - 120.0) / 360.0;
                let glow = (1.0 - (gx * gx + gy * gy).sqrt()).clamp(0.0, 1.0);

                // Dark frosted glass base tinted with accent color
                let base_r = 26.0 * (1.0 - vy * 0.35) + (ar as f32) * (0.28 + glow * 0.18);
                let base_g = 30.0 * (1.0 - vy * 0.35) + (ag as f32) * (0.28 + glow * 0.18);
                let base_b = 40.0 * (1.0 - vy * 0.35) + (ab as f32) * (0.28 + glow * 0.18);

                let mut r = base_r.clamp(0.0, 255.0);
                let mut g = base_g.clamp(0.0, 255.0);
                let mut b = base_b.clamp(0.0, 255.0);

                // Crisp inner glass rim border (2.5px thick)
                if d > -3.0 {
                    let border_t = ((d + 3.0) / 2.5).clamp(0.0, 1.0);
                    // Brighter specular highlight at the top edge
                    let spec = (1.0 - vy * 0.65) * 0.48 * border_t;
                    r = r * (1.0 - spec) + 255.0 * spec;
                    g = g * (1.0 - spec) + 255.0 * spec;
                    b = b * (1.0 - spec) + 255.0 * spec;
                }

                let p = canvas.get_pixel_mut(x, y);
                blend_pixel(
                    p,
                    [
                        r as u8,
                        g as u8,
                        b as u8,
                        (edge_alpha * 246.0).round() as u8,
                    ],
                );
            }
        }
    }

    // 2. Draw 2x2 mini-icon grid inside the folder squircle
    let slots: [(u32, u32); 4] = [(82, 82), (266, 82), (82, 266), (266, 266)];
    let icon_size = 164u32;

    for (idx, &(sx, sy)) in slots.iter().enumerate() {
        if let Some(png_bytes) = app_pngs.get(idx) {
            if let Ok(dyn_img) = image::load_from_memory(png_bytes) {
                let resized = dyn_img
                    .resize_exact(icon_size, icon_size, FilterType::Lanczos3)
                    .to_rgba8();

                // Subtle placeholder/shadow behind each mini-icon
                let scx = sx as f32 + icon_size as f32 * 0.5;
                let scy = sy as f32 + icon_size as f32 * 0.5;
                for yy in (sy.saturating_sub(8))..(sy + icon_size + 12).min(size) {
                    for xx in (sx.saturating_sub(8))..(sx + icon_size + 8).min(size) {
                        let sd = sd_rounded_box(
                            xx as f32 + 0.5,
                            yy as f32 + 0.5 - 4.0,
                            scx,
                            scy,
                            66.0,
                            66.0,
                            30.0,
                        );
                        if sd < 10.0 && sd > -4.0 {
                            let sa = ((10.0 - sd) / 10.0).clamp(0.0, 1.0) * 0.25;
                            blend_pixel(canvas.get_pixel_mut(xx, yy), [0, 0, 0, (sa * 255.0) as u8]);
                        }
                    }
                }

                // Composite the resized mini-icon
                for iy in 0..icon_size {
                    for ix in 0..icon_size {
                        let src_p = resized.get_pixel(ix, iy);
                        if src_p[3] > 0 {
                            let dst_p = canvas.get_pixel_mut(sx + ix, sy + iy);
                            blend_pixel(dst_p, src_p.0);
                        }
                    }
                }
                continue;
            }
        }

        // Empty slot subtle frosted mini-tile so even <4 apps looks like an iOS 2x2 grid
        let scx = sx as f32 + icon_size as f32 * 0.5;
        let scy = sy as f32 + icon_size as f32 * 0.5;
        for yy in sy..(sy + icon_size) {
            for xx in sx..(sx + icon_size) {
                let sd = sd_rounded_box(
                    xx as f32 + 0.5,
                    yy as f32 + 0.5,
                    scx,
                    scy,
                    62.0,
                    62.0,
                    28.0,
                );
                if sd < 1.0 {
                    let ea = (0.5 - sd).clamp(0.0, 1.0);
                    blend_pixel(
                        canvas.get_pixel_mut(xx, yy),
                        [255, 255, 255, (ea * 18.0) as u8],
                    );
                }
            }
        }
    }

    let mut out = Vec::new();
    let _ = canvas.write_to(&mut Cursor::new(&mut out), ImageFormat::Png);
    out
}

/// Encodes a 512x512 PNG into a valid multi-resolution macOS `.icns` file (`ic07`, `ic08`, `ic09`).
pub fn encode_icns(png_512: &[u8]) -> Vec<u8> {
    let mut chunks: Vec<([u8; 4], Vec<u8>)> = Vec::new();

    if let Ok(img) = image::load_from_memory(png_512) {
        for &(tag, dim) in &[(*b"ic07", 128u32), (*b"ic08", 256u32)] {
            let resized = img.resize_exact(dim, dim, FilterType::Lanczos3);
            let mut buf = Vec::new();
            if resized
                .write_to(&mut Cursor::new(&mut buf), ImageFormat::Png)
                .is_ok()
            {
                chunks.push((tag, buf));
            }
        }
    }
    chunks.push((*b"ic09", png_512.to_vec()));

    let total_len: u32 = 8 + chunks.iter().map(|(_, b)| 8 + b.len() as u32).sum::<u32>();
    let mut icns = Vec::with_capacity(total_len as usize);
    icns.extend_from_slice(b"icns");
    icns.extend_from_slice(&total_len.to_be_bytes());

    for (tag, data) in chunks {
        let chunk_len: u32 = 8 + data.len() as u32;
        icns.extend_from_slice(&tag);
        icns.extend_from_slice(&chunk_len.to_be_bytes());
        icns.extend_from_slice(&data);
    }

    icns
}
