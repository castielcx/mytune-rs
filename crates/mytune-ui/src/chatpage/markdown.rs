//! Discord's message formatting: **bold**, *italic*, __underline__, ~~strike~~, `code`, ```code blocks```,
//! > quotes, # headings, -# small text, - lists, ||spoilers||, links (bare and [masked](url)), @mention pills,
//! server emoji and colour emoji. A message with none of these stays one selectable label (see `rich_body`).

use super::voice::{k_edge, k_raised};
use super::*;
use mytune_chat::yapfast_core::chat::ChatMessage;
use mytune_chat::yapfast_core::mention::Target;

const LINK: Color32 = Color32::from_rgb(0x6c, 0xb4, 0xff);

#[derive(Clone, Default, PartialEq)]
struct St {
    bold: bool,
    italic: Option<char>,
    underline: bool,
    strike: bool,
    code: bool,
    spoiler: bool,
    link: Option<String>,
}

/// Whether a message needs more than a plain label.
/// The text on one line without the markup marks (bold, italic, strike, spoiler, code, quote and heading
/// marks, `[text](url)` as its text), for previews such as the DM list.
pub fn plain(body: &str) -> String {
    let mut s = body.to_owned();
    // links: [text](url) -> text
    while let (Some(a), Some(b)) = (s.find('['), s.find("](")) {
        let Some(end) = s[b..].find(')').map(|e| b + e) else { break };
        if a >= b {
            break;
        }
        let text = s[a + 1..b].to_owned();
        s.replace_range(a..=end, &text);
    }
    for mark in ["```", "**", "__", "~~", "||", "`", "*"] {
        s = s.replace(mark, "");
    }
    let lines: Vec<&str> = s.lines().map(|l| l.trim_start().trim_start_matches(['#', '>', '-']).trim_start().trim_start_matches("# ")).filter(|l| !l.trim().is_empty()).collect();
    lines.join(" ")
}

pub fn has_markup(body: &str) -> bool {
    body.contains(['*', '_', '~', '|', '`', '[', '\\']) || body.contains("http") || body.lines().any(|l| matches!(l.trim_start().chars().next(), Some('>' | '#')) || l.trim_start().starts_with("- ") || l.trim_start().starts_with("-# "))
}

/// The styled runs of one line.
fn inline_runs(line: &str) -> Vec<(String, St)> {
    let c: Vec<char> = line.chars().collect();
    let (mut out, mut st, mut buf, mut i) = (Vec::new(), St::default(), String::new(), 0usize);
    let flush = |buf: &mut String, st: &St, out: &mut Vec<(String, St)>| {
        if !buf.is_empty() {
            out.push((std::mem::take(buf), st.clone()));
        }
    };
    let has = |from: usize, pat: &[char]| (from..c.len()).any(|k| c[k..].starts_with(pat));
    while i < c.len() {
        let ch = c[i];
        if ch == '\\' && i + 1 < c.len() && "*_~|`\\[]()#>-".contains(c[i + 1]) {
            buf.push(c[i + 1]);
            i += 2;
            continue;
        }
        if st.code {
            if ch == '`' {
                flush(&mut buf, &st, &mut out);
                st.code = false;
            } else {
                buf.push(ch);
            }
            i += 1;
            continue;
        }
        let two = |a: char| ch == a && i + 1 < c.len() && c[i + 1] == a;
        if two('*') && (st.bold || has(i + 2, &['*', '*'])) {
            flush(&mut buf, &st, &mut out);
            st.bold = !st.bold;
            i += 2;
            continue;
        }
        if two('_') && (st.underline || has(i + 2, &['_', '_'])) {
            flush(&mut buf, &st, &mut out);
            st.underline = !st.underline;
            i += 2;
            continue;
        }
        if two('~') && (st.strike || has(i + 2, &['~', '~'])) {
            flush(&mut buf, &st, &mut out);
            st.strike = !st.strike;
            i += 2;
            continue;
        }
        if two('|') && (st.spoiler || has(i + 2, &['|', '|'])) {
            flush(&mut buf, &st, &mut out);
            st.spoiler = !st.spoiler;
            i += 2;
            continue;
        }
        if ch == '`' && has(i + 1, &['`']) {
            flush(&mut buf, &st, &mut out);
            st.code = true;
            i += 1;
            continue;
        }
        if ch == '*' || ch == '_' {
            let prev_alnum = i > 0 && c[i - 1].is_alphanumeric();
            let prev_ws = i == 0 || c[i - 1].is_whitespace();
            let next_alnum = i + 1 < c.len() && c[i + 1].is_alphanumeric();
            let next_ws = i + 1 >= c.len() || c[i + 1].is_whitespace();
            if st.italic == Some(ch) && !prev_ws && !(ch == '_' && next_alnum) {
                flush(&mut buf, &st, &mut out);
                st.italic = None;
                i += 1;
                continue;
            }
            if st.italic.is_none() && !next_ws && !(ch == '_' && prev_alnum) && has(i + 1, &[ch]) {
                flush(&mut buf, &st, &mut out);
                st.italic = Some(ch);
                i += 1;
                continue;
            }
        }
        if ch == '[' {
            if let Some(j) = (i + 1..c.len()).find(|&j| c[j] == ']') {
                if c.get(j + 1) == Some(&'(') {
                    if let Some(k) = (j + 2..c.len()).find(|&k| c[k] == ')') {
                        let url: String = c[j + 2..k].iter().collect();
                        if url.starts_with("http://") || url.starts_with("https://") {
                            flush(&mut buf, &st, &mut out);
                            let mut ls = st.clone();
                            ls.link = Some(url);
                            out.push((c[i + 1..j].iter().collect(), ls));
                            i = k + 1;
                            continue;
                        }
                    }
                }
            }
        }
        buf.push(ch);
        i += 1;
    }
    flush(&mut buf, &st, &mut out);
    out
}

enum Block {
    Para(String),
    Code(String),
    Quote(Vec<String>),
    Heading(u8, String),
    Small(String),
    Bullet(String),
}

fn blocks(body: &str) -> Vec<Block> {
    let mut out = Vec::new();
    let lines: Vec<&str> = body.lines().collect();
    let mut i = 0;
    while i < lines.len() {
        let l = lines[i];
        let t = l.trim_start();
        if let Some(rest) = t.strip_prefix("```") {
            let mut code = Vec::new();
            let first = rest.trim_end_matches("```");
            // ```lang\n... : the first line is the language unless the fence closes on the same line
            if rest.ends_with("```") && !rest.is_empty() {
                code.push(first.to_owned());
                i += 1;
            } else {
                i += 1;
                while i < lines.len() && !lines[i].trim_start().starts_with("```") {
                    code.push(lines[i].to_owned());
                    i += 1;
                }
                i += 1;
            }
            out.push(Block::Code(code.join("\n")));
        } else if let Some(rest) = t.strip_prefix(">>> ") {
            let mut q = vec![rest.to_owned()];
            q.extend(lines[i + 1..].iter().map(|s| (*s).to_owned()));
            out.push(Block::Quote(q));
            break;
        } else if t.starts_with('>') {
            let mut q = Vec::new();
            while i < lines.len() && lines[i].trim_start().starts_with('>') {
                q.push(lines[i].trim_start().trim_start_matches('>').trim_start().to_owned());
                i += 1;
            }
            out.push(Block::Quote(q));
        } else {
            let (hashes, rest) = (t.chars().take_while(|c| *c == '#').count(), t.trim_start_matches('#'));
            if (1..=3).contains(&hashes) && rest.starts_with(' ') {
                out.push(Block::Heading(hashes as u8, rest.trim().to_owned()));
            } else if let Some(r) = t.strip_prefix("-# ") {
                out.push(Block::Small(r.to_owned()));
            } else if let Some(r) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
                out.push(Block::Bullet(r.to_owned()));
            } else {
                out.push(Block::Para(l.to_owned()));
            }
            i += 1;
        }
    }
    out
}

/// A message body with its formatting.
#[allow(clippy::too_many_arguments)]
pub fn body(ui: &mut Ui, s: &AppState, m: &ChatMessage, size: f32, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    let key = egui::Id::new(("spoiler_open", &m.item));
    let mut open: bool = ctx.data(|d| d.get_temp(key)).unwrap_or(false);
    let before = open;
    let color = Color32::from_rgb(0xDD, 0xDF, 0xE5);
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 3.0;
        for b in blocks(&m.body) {
            match b {
                Block::Code(code) => {
                    egui::Frame::new().fill(k_rail()).stroke(egui::Stroke::new(1.0, k_edge())).corner_radius(10.0).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
                        ui.add(egui::Label::new(rt(&code, size - 1.0).monospace().color(color)).selectable(true));
                    });
                }
                Block::Quote(lines) => {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let (bar, _) = ui.allocate_exact_size(vec2(4.0, 0.0), Sense::hover());
                        let top = bar.top();
                        ui.vertical(|ui| {
                            for l in &lines {
                                line(ui, &ctx, s, m, l, size, color, 1.0, tex, asked, want, acts, &mut open);
                            }
                        });
                        let bottom = ui.min_rect().bottom();
                        ui.painter().rect_filled(egui::Rect::from_min_max(egui::pos2(bar.left(), top), egui::pos2(bar.left() + 4.0, bottom)), 2.0, FAINT);
                    });
                }
                Block::Heading(n, t) => line(ui, &ctx, s, m, &format!("**{t}**"), size, color, [1.6, 1.35, 1.15][n as usize - 1], tex, asked, want, acts, &mut open),
                Block::Small(t) => line(ui, &ctx, s, m, &t, size, MUTED, 0.8, tex, asked, want, acts, &mut open),
                Block::Bullet(t) => {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.add_space(6.0);
                        ui.label(rt("\u{2022}", size).color(SECONDARY));
                        ui.vertical(|ui| line(ui, &ctx, s, m, &t, size, color, 1.0, tex, asked, want, acts, &mut open));
                    });
                }
                Block::Para(t) => {
                    if t.trim().is_empty() {
                        ui.add_space(size * 0.5);
                    } else {
                        line(ui, &ctx, s, m, &t, size, color, 1.0, tex, asked, want, acts, &mut open);
                    }
                }
            }
        }
    });
    if open != before {
        ctx.data_mut(|d| d.insert_temp(key, open));
    }
}

/// One wrapped line of styled words, mentions and emoji.
#[allow(clippy::too_many_arguments)]
fn line(ui: &mut Ui, ctx: &egui::Context, s: &AppState, m: &ChatMessage, text: &str, size: f32, color: Color32, scale: f32, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>, open: &mut bool) {
    let size = size * scale;
    let runs = inline_runs(text);
    let only_emoji = runs.iter().all(|(t, st)| !st.code && crate::emoji::split(t).into_iter().all(|p| matches!(p, crate::emoji::Piece::Emoji(_)) || matches!(p, crate::emoji::Piece::Text(x) if x.trim().is_empty() || m.emoji.iter().any(|c| c.text == x.trim()))));
    let es = if only_emoji && runs.len() <= 27 { 44.0 } else { (size * 1.35).round() };
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
        for (t, st) in &runs {
            let mut rest: &str = t;
            while !rest.is_empty() {
                // the next mention or server emoji in this run
                let hit_m = m.mentions.iter().filter(|x| !x.text.is_empty()).filter_map(|x| rest.find(x.text.as_str()).map(|at| (at, x.text.len(), Some(x), None))).min_by_key(|h| h.0);
                let hit_e = m.emoji.iter().filter(|c| !c.text.is_empty()).filter_map(|c| rest.find(c.text.as_str()).map(|at| (at, c.text.len(), None, Some(c)))).min_by_key(|h| h.0);
                let hit = match (hit_m, hit_e) {
                    (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
                    (a, b) => a.or(b),
                };
                let (head, tail) = match &hit {
                    Some((at, len, _, _)) => (&rest[..*at], &rest[at + len..]),
                    None => (rest, ""),
                };
                pieces(ui, ctx, head, st, size, color, es, tex, asked, want, s, m, open);
                match hit {
                    Some((_, _, Some(x), _)) => {
                        let c = x.color.map(|c| Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)).unwrap_or(Color32::from_rgb(0xA6, 0xB3, 0xFF));
                        let r = ui.add(egui::Label::new(sb(&x.text, size).color(c).background_color(if crate::theme::p().black { Color32::TRANSPARENT } else { c.gamma_multiply(0.2) })).sense(Sense::click()));
                        if matches!(x.target, Target::User(_)) && r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            if let Target::User(u) = &x.target {
                                super::header::remember_profile_at(ctx);
                                acts.push(Action::ShowProfile { user: u.clone(), large: false });
                            }
                        }
                    }
                    Some((_, _, _, Some(c))) => match texture(ctx, tex, asked, s, &Some(c.picture.clone()), want) {
                        Some(t) => {
                            ui.add(egui::Image::new(egui::load::SizedTexture::new(t, vec2(es, es)))).on_hover_text(&c.text);
                        }
                        None => {
                            ui.label(rt(&c.text, size).color(color));
                        }
                    },
                    _ => {}
                }
                rest = tail;
            }
        }
    });
    if *open {
        // a revealed spoiler stays open for this message; nothing more to draw
    }
    let _ = acts;
}

/// Text with colour emoji, in words.
#[allow(clippy::too_many_arguments)]
fn pieces(ui: &mut Ui, ctx: &egui::Context, text: &str, st: &St, size: f32, color: Color32, es: f32, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, s: &AppState, m: &ChatMessage, open: &mut bool) {
    let _ = (tex, asked, want, s, m);
    for p in crate::emoji::split(text) {
        match p {
            crate::emoji::Piece::Emoji(e) => match crate::emoji::texture(ctx, e) {
                Some(t) => {
                    ui.add(egui::Image::new(egui::load::SizedTexture::new(t, vec2(es, es)))).on_hover_text(e);
                }
                None => {
                    ui.label(rt(e, size).color(color));
                }
            },
            crate::emoji::Piece::Text(t) => {
                for w in t.split_inclusive(' ') {
                    word(ui, ctx, w, st, size, color, open);
                }
            }
        }
    }
}

fn word(ui: &mut Ui, ctx: &egui::Context, w: &str, st: &St, size: f32, color: Color32, open: &mut bool) {
    let bare = w.trim_end().trim_end_matches(['.', ',', '!', '?', ')', '>', ';', ':']);
    let url = st.link.clone().or_else(|| (bare.starts_with("http://") || bare.starts_with("https://")).then(|| bare.to_owned()));
    let hidden = st.spoiler && !*open;
    let mut t = if st.bold { sb(w, size) } else { rt(w, size) };
    t = t.color(if url.is_some() { LINK } else { color });
    if st.italic.is_some() {
        t = t.italics();
    }
    if st.underline || url.is_some() {
        t = t.underline();
    }
    if st.strike {
        t = t.strikethrough();
    }
    if st.code {
        t = t.monospace().background_color(if crate::theme::p().black { Color32::TRANSPARENT } else { k_raised() });
    }
    if hidden {
        // kept in the Black look too: a hidden spoiler has nothing else to show that it is there
        t = t.color(k_active()).background_color(k_active());
    } else if st.spoiler {
        t = t.background_color(if crate::theme::p().black { Color32::TRANSPARENT } else { k_raised() });
    }
    let clickable = url.is_some() || st.spoiler;
    let r = ui.add(egui::Label::new(t).sense(if clickable { Sense::click() } else { Sense::hover() }).selectable(!clickable));
    if clickable {
        let r = r.on_hover_cursor(egui::CursorIcon::PointingHand);
        if r.clicked() {
            if hidden {
                *open = true;
            } else if let Some(u) = url {
                super::extras::open_link(ctx, &u);
            }
        }
    }
}
