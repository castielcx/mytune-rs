#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Chat, Dashboard, Music, Optimize, Games, Vpn, Mouse, Display, Vibrance, Crosshair, Sound,
    Debloat, Customize, Tools, Diagnostics, Bios, Restore, Settings,
}

pub struct Item {
    pub page: Page,
    pub label: &'static str,
    pub tip: &'static str,
    /// Segoe Fluent Icons code point.
    pub glyph: &'static str,
}

/// Sidebar icon of a page (Restore shares Settings').
pub fn glyph_of(page: Page) -> &'static str {
    let page = if page == Page::Restore { Page::Settings } else { page };
    GROUPS.iter().flat_map(|g| g.items).chain(std::iter::once(&SETTINGS)).find(|i| i.page == page).map_or("", |i| i.glyph)
}

pub struct Group {
    pub title: Option<&'static str>,
    pub items: &'static [Item],
}

macro_rules! it { ($p:ident, $l:expr, $t:expr, $g:expr) => { Item { page: Page::$p, label: $l, tip: $t, glyph: $g } }; }

pub const GROUPS: &[Group] = &[
    Group { title: None, items: &[
        it!(Dashboard, "Home", "Status of your PC and the one-click setup", "\u{E80F}"),
        it!(Chat, "Discord", "Your Discord chats (Yapfast)", "\u{E8BD}"),
        it!(Music, "Music", "What is playing on this PC, with play, pause and skip", "\u{E8D6}"),
    ] },
    Group { title: Some("Gaming"), items: &[
        it!(Optimize, "Optimize", "Windows settings that make games run smoother", "\u{E945}"),
        it!(Games, "Game Hub", "Your games, with settings for each one", "\u{E7FC}"),
        it!(Vpn, "Internet", "Test and repair your connection", "\u{E839}"),
    ] },
    Group { title: Some("Devices"), items: &[
        it!(Mouse, "Mouse", "Mouse acceleration and polling rate test", "\u{E962}"),
        it!(Display, "Display", "Resolution and refresh rate", "\u{E7F4}"),
        it!(Vibrance, "Vibrance", "Stronger colours, brightness and brighter shadows for games", "\u{E790}"),
        it!(Crosshair, "Crosshair", "An on-screen crosshair over your game, switched on and off with a key", "\u{E710}"),
        it!(Sound, "Sound", "Volume for each app, and which speakers or headset plays it", "\u{E767}"),
    ] },
    Group { title: Some("Windows"), items: &[
        it!(Debloat, "Debloat", "Remove apps and background extras you do not need", "\u{E74D}"),
        it!(Customize, "Customize", "How Windows looks and behaves", "\u{E771}"),
        it!(Tools, "Tools", "Clean up junk files and manage startup programs", "\u{E90F}"),
    ] },
    Group { title: Some("Check and undo"), items: &[
        it!(Diagnostics, "Diagnostics", "Benchmark, background programs and driver checks", "\u{E9D9}"),
        it!(Bios, "BIOS", "See which BIOS settings hold your PC back. MyTune only reads them.", "\u{E950}"),
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
