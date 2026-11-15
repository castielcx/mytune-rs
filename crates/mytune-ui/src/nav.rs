#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Chat, Dashboard, Optimize, Games, Vpn, Mouse, Display, Sound,
    Debloat, Customize, Tools, Diagnostics, Restore, Settings,
}

pub struct Item {
    pub page: Page,
    pub label: &'static str,
    pub tip: &'static str,
    /// Segoe Fluent Icons code point.
    pub glyph: &'static str,
}

pub struct Group {
    pub title: Option<&'static str>,
    pub items: &'static [Item],
}

macro_rules! it { ($p:ident, $l:expr, $t:expr, $g:expr) => { Item { page: Page::$p, label: $l, tip: $t, glyph: $g } }; }

pub const GROUPS: &[Group] = &[
    Group { title: None, items: &[
        it!(Chat, "Discord", "Your Discord chats (Yapfast)", "\u{E8BD}"),
        it!(Dashboard, "Home", "Status of your PC and the one-click setup", "\u{E80F}"),
    ] },
    Group { title: Some("GAMING"), items: &[
        it!(Optimize, "Optimize", "Windows settings that make games run smoother", "\u{E945}"),
        it!(Games, "Game Hub", "Your games, with settings for each one", "\u{E7FC}"),
        it!(Vpn, "Internet", "Test and repair your connection", "\u{E839}"),
    ] },
    Group { title: Some("DEVICES"), items: &[
        it!(Mouse, "Mouse", "Mouse acceleration and polling rate test", "\u{E962}"),
        it!(Display, "Display", "Refresh rate, colours and resolution", "\u{E7F4}"),
        it!(Sound, "Sound", "Volume for each app, and which speakers or headset plays it", "\u{E767}"),
    ] },
    Group { title: Some("WINDOWS"), items: &[
        it!(Debloat, "Debloat", "Remove apps and background extras you do not need", "\u{E74D}"),
        it!(Customize, "Customize", "How Windows looks and behaves", "\u{E771}"),
        it!(Tools, "Tools", "Clean up junk files and manage startup programs", "\u{E90F}"),
    ] },
    Group { title: Some("CHECK AND UNDO"), items: &[
        it!(Diagnostics, "Diagnostics", "Benchmark, background programs and driver checks", "\u{E9D9}"),
    ] },
];

pub const SETTINGS: Item = it!(Settings, "Settings", "Autostart, game booster, backup, updates, and Restore to undo changes", "\u{E713}");

impl Page {
    pub fn all() -> impl Iterator<Item = &'static Item> {
        GROUPS.iter().flat_map(|g| g.items.iter()).chain(std::iter::once(&SETTINGS))
    }
    pub fn label(self) -> &'static str {
        Page::all().find(|i| i.page == self).map(|i| i.label).unwrap_or("")
    }
}
