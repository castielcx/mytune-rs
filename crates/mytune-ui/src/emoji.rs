//! Discord-style colour emoji: Twemoji 17.0.3 (github.com/jdecked/twemoji, graphics CC-BY 4.0) 72x72 PNGs, packed
//! into `assets/twemoji/twemoji.bin` with a sorted index (`twemoji.idx`, by the scratchpad packer). egui cannot
//! draw colour fonts, so emoji show as these pictures: in messages, reactions and the picker. A texture is made
//! the first time an emoji shows and kept (72x72, ~20 KB each).

use eframe::egui::{self, TextureHandle, TextureId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

static BLOB: &[u8] = include_bytes!("../assets/twemoji/twemoji.bin");
static INDEX: &str = include_str!("../assets/twemoji/twemoji.idx");

/// (name, offset, length), sorted by name as the file is.
fn index() -> &'static [(&'static str, usize, usize)] {
    static PARSED: OnceLock<Vec<(&'static str, usize, usize)>> = OnceLock::new();
    PARSED.get_or_init(|| {
        INDEX
            .lines()
            .filter_map(|l| {
                let mut it = l.split(' ');
                Some((it.next()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?))
            })
            .collect()
    })
}

/// Twemoji's file name for an emoji: its code points in lower-case hex joined by '-', without U+FE0F unless the
/// sequence joins with U+200D (Twemoji's own rule).
pub fn name(emoji: &str) -> String {
    use std::fmt::Write;
    let zwj = emoji.contains('\u{200D}');
    // one allocation: `split` asks this for up to 10 candidates per char, for every message, every frame
    let mut n = String::with_capacity(emoji.len() * 2);
    for c in emoji.chars().filter(|&c| zwj || c != '\u{FE0F}') {
        if !n.is_empty() {
            n.push('-');
        }
        let _ = write!(n, "{:x}", c as u32);
    }
    n
}

/// The PNG of an emoji, when Twemoji has one.
pub fn png(emoji: &str) -> Option<&'static [u8]> {
    let find = |n: &str| index().binary_search_by(|(k, _, _)| (*k).cmp(n)).ok().map(|i| index()[i]);
    let n = name(emoji);
    let (_, at, len) = find(&n).or_else(|| n.contains("-fe0f").then(|| find(&n.replace("-fe0f", ""))).flatten())?;
    BLOB.get(at..at + len)
}

thread_local! {
    static TEXTURES: RefCell<HashMap<String, Option<TextureHandle>>> = RefCell::new(HashMap::new());
}

/// The emoji's picture as a texture, made once.
pub fn texture(ctx: &egui::Context, emoji: &str) -> Option<TextureId> {
    make(ctx, emoji, false)
}

/// As `texture`, but at most 8 PNG decodes a frame: over that it gives None for a frame and asks again.
/// Only for fixed-size cells (the picker): in message text the fallback glyph has another size, so rows jumped.
pub fn texture_paced(ctx: &egui::Context, emoji: &str) -> Option<TextureId> {
    make(ctx, emoji, true)
}

fn make(ctx: &egui::Context, emoji: &str, paced: bool) -> Option<TextureId> {
    TEXTURES.with(|t| {
        let mut t = t.borrow_mut();
        if let Some(h) = t.get(emoji) {
            return h.as_ref().map(|h| h.id());
        }
        // The picker's first open decoded its whole table in one frame.
        thread_local!(static SPENT: std::cell::Cell<(u64, u32)> = const { std::cell::Cell::new((0, 0)) });
        let frame = ctx.cumulative_frame_nr();
        let spent = SPENT.with(|s| if s.get().0 == frame { s.get().1 } else { 0 });
        if paced && spent >= 8 {
            ctx.request_repaint();
            return None;
        }
        SPENT.with(|s| s.set((frame, spent + 1)));
        let made = png(emoji).and_then(|bytes| image::load_from_memory(bytes).ok()).map(|img| {
            let rgba = img.to_rgba8();
            let size = [rgba.width() as usize, rgba.height() as usize];
            ctx.load_texture(format!("emoji:{emoji}"), egui::ColorImage::from_rgba_unmultiplied(size, &rgba), crate::theme::SMOOTH)
        });
        let id = made.as_ref().map(|h| h.id());
        t.insert(emoji.to_owned(), made);
        id
    })
}

/// A piece of text: plain, or one emoji.
#[derive(Debug, PartialEq)]
pub enum Piece<'a> {
    Text(&'a str),
    Emoji(&'a str),
}

/// Could this char start an emoji? (Plain letters, digits and punctuation never do on their own.)
fn may_start(c: char, next: Option<char>) -> bool {
    (c as u32) >= 0x2000 || matches!(next, Some('\u{FE0F}' | '\u{20E3}'))
}

/// Splits text into plain runs and emoji, taking the longest sequence Twemoji has a picture for.
pub fn split(text: &str) -> Vec<Piece<'_>> {
    let mut out = Vec::new();
    let idx: Vec<(usize, char)> = text.char_indices().collect();
    let mut plain_from = 0;
    let mut i = 0;
    while i < idx.len() {
        let (at, c) = idx[i];
        if may_start(c, idx.get(i + 1).map(|x| x.1)) {
            // longest first: ZWJ families and flags run to ~10 code points
            let found = (1..=10.min(idx.len() - i)).rev().find_map(|len| {
                let end = idx.get(i + len).map_or(text.len(), |x| x.0);
                let cand = &text[at..end];
                png(cand).map(|_| (len, end))
            });
            if let Some((len, end)) = found {
                if plain_from < at {
                    out.push(Piece::Text(&text[plain_from..at]));
                }
                out.push(Piece::Emoji(&text[at..end]));
                plain_from = end;
                i += len;
                continue;
            }
        }
        i += 1;
    }
    if plain_from < text.len() {
        out.push(Piece::Text(&text[plain_from..]));
    }
    out
}

/// Quick check before splitting: no char an emoji could start with, no work.
pub fn has_emoji(text: &str) -> bool {
    text.chars().any(|c| (c as u32) >= 0x2000 || c == '\u{FE0F}' || c == '\u{20E3}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_twemoji() {
        assert_eq!(name("😀"), "1f600");
        assert_eq!(name("❤️"), "2764");
        assert_eq!(name("👨‍👩‍👧"), "1f468-200d-1f469-200d-1f467");
    }

    #[test]
    fn finds_pictures() {
        for e in ["😀", "❤️", "👍", "🙏", "💀", "👍🏽", "🇩🇪", "1️⃣"] {
            assert!(png(e).is_some_and(|b| b.starts_with(b"\x89PNG")), "{e}");
        }
        assert!(png("a").is_none());
    }

    #[test]
    fn splits_text_and_emoji() {
        assert_eq!(split("hi 👍🏽 ok"), vec![Piece::Text("hi "), Piece::Emoji("👍🏽"), Piece::Text(" ok")]);
        assert_eq!(split("❤️🇩🇪"), vec![Piece::Emoji("❤️"), Piece::Emoji("🇩🇪")]);
        assert_eq!(split("plain"), vec![Piece::Text("plain")]);
    }
}
