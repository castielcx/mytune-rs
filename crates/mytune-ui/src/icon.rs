//! The MyTune icon: a white "M" with rounded corners on a transparent background. Drawn at run time for the
//! window, taskbar button and tray; `ico()` writes the same picture into `assets/MyTune.ico` (exe, Explorer,
//! Task Manager, installer).

/// The M in a 256 x 256 design space (same polygon as the C# code).
const LETTER: [(f32, f32); 13] = [
    (65.0, 187.0), (65.0, 69.0), (90.0, 69.0), (128.0, 123.0), (166.0, 69.0), (191.0, 69.0),
    (191.0, 187.0), (167.0, 187.0), (167.0, 119.0), (133.0, 157.0), (123.0, 157.0), (89.0, 119.0), (89.0, 187.0),
];

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

/// RGBA, `px` x `px`, 4 x 4 supersampling: a white M with rounded corners on a transparent background,
/// scaled to fill the square.
pub fn render(px: u32) -> Vec<u8> {
    const ZOOM: f32 = 1.4;
    let scale = 256.0 / px as f32;
    let mut out = Vec::with_capacity((px * px * 4) as usize);
    for y in 0..px {
        for x in 0..px {
            let mut hit = 0u32;
            for sy in 0..4 {
                for sx in 0..4 {
                    let p = ((x as f32 + (sx as f32 + 0.5) / 4.0) * scale, (y as f32 + (sy as f32 + 0.5) / 4.0) * scale);
                    let q = (128.0 + (p.0 - 128.0) / ZOOM, 128.0 + (p.1 - 128.0) / ZOOM);
                    if inside_poly(q) || dist_to_edge(q) <= 3.0 {
                        hit += 1;
                    }
                }
            }
            // straight (non-premultiplied) alpha, which is what the window and tray APIs take
            out.extend_from_slice(&[255, 255, 255, (hit * 255 / 16) as u8]);
        }
    }
    out
}

/// BMP-framed .ico (16 to 256 px) of the same M, for the exe resource and the installer.
pub fn ico() -> Vec<u8> {
    let sizes = [16u32, 24, 32, 48, 64, 128, 256];
    let frames: Vec<Vec<u8>> = sizes.iter().map(|&n| {
        let rgba = render(n);
        let mut d = Vec::new();
        d.extend_from_slice(&40u32.to_le_bytes()); // BITMAPINFOHEADER
        d.extend_from_slice(&n.to_le_bytes());
        d.extend_from_slice(&(n * 2).to_le_bytes()); // image + AND mask
        d.extend_from_slice(&1u16.to_le_bytes());
        d.extend_from_slice(&32u16.to_le_bytes());
        d.extend_from_slice(&[0u8; 24]); // compression, size, resolution, colours
        for row in (0..n).rev() {
            for col in 0..n {
                let i = ((row * n + col) * 4) as usize;
                d.extend_from_slice(&[rgba[i + 2], rgba[i + 1], rgba[i], rgba[i + 3]]);
            }
        }
        d.extend(std::iter::repeat(0u8).take((n.div_ceil(32) * 4 * n) as usize)); // AND mask
        d
    }).collect();
    let mut out = vec![0, 0, 1, 0];
    out.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut off = 6 + 16 * sizes.len() as u32;
    for (n, f) in sizes.iter().zip(&frames) {
        out.extend_from_slice(&[(*n % 256) as u8, (*n % 256) as u8, 0, 0, 1, 0, 32, 0]);
        out.extend_from_slice(&(f.len() as u32).to_le_bytes());
        out.extend_from_slice(&off.to_le_bytes());
        off += f.len() as u32;
    }
    for f in frames {
        out.extend(f);
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
    fn white_m_on_transparent() {
        let img = render(64);
        assert_eq!(img.len(), 64 * 64 * 4);
        assert_eq!(px(&img, 64, 0, 0)[3], 0, "the corner is transparent");
        assert_eq!(px(&img, 64, 32, 4)[3], 0, "above the letter is transparent");
        assert_eq!(px(&img, 64, 14, 32), [255, 255, 255, 255], "the left stem is solid white");
    }

    /// Run once to rewrite `assets/MyTune.ico`: `cargo test -p mytune-ui --lib icon::tests::write_ico -- --ignored`
    #[test]
    #[ignore]
    fn write_ico() {
        std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/MyTune.ico"), ico()).unwrap();
    }
}
