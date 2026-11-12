//! The MyTune "M" icon in the accent colour, for the window, taskbar button and tray (C# `Services/AppIcon.cs`):
//! a rounded square with a diagonal gradient (the accent, and a deeper more saturated shade of it) and a white
//! "M" whose 6 px outline rounds the corners. Drawn at run time, so there is no icon file to ship.
//! The icon file inside the exe stays the blue `assets/MyTune.ico`, like in the C# app.

/// The M in a 256 x 256 design space (same polygon as the C# code).
const LETTER: [(f32, f32); 13] = [
    (65.0, 187.0), (65.0, 69.0), (90.0, 69.0), (128.0, 123.0), (166.0, 69.0), (191.0, 69.0),
    (191.0, 187.0), (167.0, 187.0), (167.0, 119.0), (133.0, 157.0), (123.0, 157.0), (89.0, 119.0), (89.0, 187.0),
];

fn hsv(c: [f32; 3]) -> (f32, f32, f32) {
    let (r, g, b) = (c[0] / 255.0, c[1] / 255.0, c[2] / 255.0);
    let (max, min) = (r.max(g).max(b), r.min(g).min(b));
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    (h, if max == 0.0 { 0.0 } else { d / max }, max)
}

fn from_hsv(h: f32, s: f32, v: f32) -> [f32; 3] {
    let i = (h * 6.0).floor();
    let (f, p, q, t) = (h * 6.0 - i, v * (1.0 - s), v * (1.0 - (h * 6.0 - i) * s), v * (1.0 - (1.0 - (h * 6.0 - i)) * s));
    let _ = f;
    let (r, g, b) = match i as i32 % 6 {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    };
    [r * 255.0, g * 255.0, b * 255.0]
}

/// (start, end) colours of the gradient for an accent like "#FF5A0A". "#60CDFF" (the old default) means orange.
pub fn gradient(hex: &str) -> ([f32; 3], [f32; 3]) {
    let h = hex.trim().trim_start_matches('#');
    let h = if h.len() != 6 || h.eq_ignore_ascii_case("60CDFF") { "FF5A0A" } else { h };
    let n = u32::from_str_radix(h, 16).unwrap_or(0xFF5A0A);
    let c = [((n >> 16) & 255) as f32, ((n >> 8) & 255) as f32, (n & 255) as f32];
    let (hue, s, v) = hsv(c);
    (c, from_hsv(hue, (s + 0.35).min(1.0), v * 0.78))
}

fn inside_poly(p: (f32, f32)) -> bool {
    let mut inside = false;
    let mut j = LETTER.len() - 1;
    for i in 0..LETTER.len() {
        let (a, b) = (LETTER[i], LETTER[j]);
        if (a.1 > p.1) != (b.1 > p.1) && p.0 < (b.0 - a.0) * (p.1 - a.1) / (b.1 - a.1) + a.0 {
            inside = !inside;
        }
        j = i;
    }
    inside
}

fn dist_to_edge(p: (f32, f32)) -> f32 {
    let mut best = f32::MAX;
    for i in 0..LETTER.len() {
        let (a, b) = (LETTER[i], LETTER[(i + 1) % LETTER.len()]);
        let (abx, aby, apx, apy) = (b.0 - a.0, b.1 - a.1, p.0 - a.0, p.1 - a.1);
        let t = ((apx * abx + apy * aby) / (abx * abx + aby * aby)).clamp(0.0, 1.0);
        best = best.min(((apx - t * abx).powi(2) + (apy - t * aby).powi(2)).sqrt());
    }
    best
}

/// Inside the 254 x 254 rounded square (corner radius 25) that starts at (1, 1).
fn inside_square(p: (f32, f32)) -> bool {
    let (x, y) = (p.0 - 1.0, p.1 - 1.0);
    if !(0.0..=254.0).contains(&x) || !(0.0..=254.0).contains(&y) {
        return false;
    }
    let (cx, cy) = (x.clamp(25.0, 229.0), y.clamp(25.0, 229.0));
    (x - cx).powi(2) + (y - cy).powi(2) <= 625.0
}

/// RGBA, `px` x `px`, 4 x 4 supersampling.
pub fn render(accent_hex: &str, px: u32) -> Vec<u8> {
    let (start, end) = gradient(accent_hex);
    let scale = 256.0 / px as f32;
    let mut out = Vec::with_capacity((px * px * 4) as usize);
    for y in 0..px {
        for x in 0..px {
            let (mut r, mut g, mut b, mut a) = (0.0, 0.0, 0.0, 0.0);
            for sy in 0..4 {
                for sx in 0..4 {
                    let p = ((x as f32 + (sx as f32 + 0.5) / 4.0) * scale, (y as f32 + (sy as f32 + 0.5) / 4.0) * scale);
                    if !inside_square(p) {
                        continue;
                    }
                    let t = ((p.0 + p.1) / 512.0).clamp(0.0, 1.0);
                    let white = inside_poly(p) || dist_to_edge(p) <= 3.0;
                    let c = if white { [255.0; 3] } else { [0, 1, 2].map(|i| start[i] + (end[i] - start[i]) * t) };
                    r += c[0];
                    g += c[1];
                    b += c[2];
                    a += 1.0;
                }
            }
            if a == 0.0 {
                out.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                // straight (non-premultiplied) alpha, which is what the window and tray APIs take
                out.extend_from_slice(&[(r / a) as u8, (g / a) as u8, (b / a) as u8, (a / 16.0 * 255.0) as u8]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(img: &[u8], n: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * n + x) * 4) as usize;
        [img[i], img[i + 1], img[i + 2], img[i + 3]]
    }

    #[test]
    fn m_on_a_rounded_gradient_square() {
        let img = render("#FF5A0A", 64);
        assert_eq!(img.len(), 64 * 64 * 4);
        assert_eq!(px(&img, 64, 0, 0)[3], 0, "the corner is transparent (rounded)");

        let stem = px(&img, 64, 20, 32); // x = 80, y = 128: inside the left stem of the M
        assert_eq!(stem, [255, 255, 255, 255], "the M is white");
        let top = px(&img, 64, 32, 6); // above the letter: gradient start side
        assert!(top[3] == 255 && top[0] > 200 && top[2] < 80, "orange near the top: {top:?}");
        let bottom = px(&img, 64, 56, 56);
        assert!(bottom[0] < top[0], "darker towards the bottom right");
    }

    #[test]
    fn old_blue_default_means_orange_and_bad_hex_too() {
        assert_eq!(gradient("#60CDFF").0, gradient("#FF5A0A").0);
        assert_eq!(gradient("nonsense").0, [255.0, 90.0, 10.0]);
        assert_eq!(gradient("#22D3EE").0, [34.0, 211.0, 238.0]);
    }
}
