//! A game's in-game settings, read from and written into the game's own settings file. Each supported game has
//! a hand-written list of settings (`Setting`: key, label, kind, choices) with MyTune's own "competitive" value
//! for some of them (V-Sync, motion blur, depth of field and lens flare off; shadows, effects, post-processing
//! and foliage low). Resolution, window mode, sensitivity, keys and sound are never touched.
//!
//! Only the chosen keys change; every other byte of the file stays (line ends, encoding, read-only flag).
//! `ConfigTweak` (id `game:<id>:config`) copies the whole file to `%ProgramData%\MyTune\gameconfig` first and
//! records it in the backup's `Data` (`file<n>`, `backup<n>`, `after<n>` = hash of what was written, `procs`);
//! revert puts that exact file back. Saving again while applied keeps that first copy, so revert always goes
//! back to the file from before MyTune touched it. Both refuse while the game runs: it would overwrite the
//! file on exit.

use crate::backup::TweakBackup;
use crate::engine::Tweak;
use crate::error::{Error, Result};
use crate::procs::Procs;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Format {
    /// `[Section]` and `Key=Value` lines (Unreal).
    Ini,
    /// `key:value` lines, no sections (Minecraft).
    Colon,
    /// `<Key value="..." />` (GTA V settings.xml); only the attribute's value changes.
    Xml,
}

pub struct Edit {
    pub section: &'static str,
    pub key: &'static str,
    pub value: String,
    /// Write the key at the end of its section when the file lacks it (Unreal reads added keys there).
    pub add: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Group {
    Graphics,
    Performance,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Kind {
    /// On or off, in the file's own words. A value that is not `off` counts as on.
    Switch { on: &'static str, off: &'static str },
    /// (label, value in the file)
    Choice(&'static [(&'static str, &'static str)]),
    /// A number, written with `decimals` digits after the point.
    Number { min: f64, max: f64, decimals: usize },
}

/// One setting of a game that the settings dialog shows.
#[derive(Debug)]
pub struct Setting {
    pub section: &'static str,
    pub key: &'static str,
    pub label: &'static str,
    pub group: Group,
    pub kind: Kind,
    /// MyTune's competitive value (as written in the file); `None`: no opinion.
    pub best: Option<&'static str>,
    /// See `Edit::add`. Other settings show only when the file has the key.
    pub add: bool,
}

const G: Group = Group::Graphics;
const P: Group = Group::Performance;

/// A switch whose competitive value is off.
const fn sw(section: &'static str, key: &'static str, label: &'static str, group: Group, on: &'static str, off: &'static str) -> Setting {
    Setting { section, key, label, group, kind: Kind::Switch { on, off }, best: Some(off), add: false }
}
const fn pick(key: &'static str, label: &'static str, group: Group, choices: &'static [(&'static str, &'static str)], best: Option<&'static str>) -> Setting {
    Setting { section: "", key, label, group, kind: Kind::Choice(choices), best, add: false }
}
const fn num(section: &'static str, key: &'static str, label: &'static str, group: Group, max: f64, decimals: usize) -> Setting {
    Setting { section, key, label, group, kind: Kind::Number { min: 0.0, max, decimals }, best: None, add: false }
}
/// Unreal's quality levels. shortcut: the names are Unreal's defaults, a game's own menu may call them differently.
const UE_LEVELS: &[(&str, &str)] = &[("Low", "0"), ("Medium", "1"), ("High", "2"), ("Epic", "3"), ("Ultra", "4")];
/// An Unreal `sg.*` quality level.
const fn sg(key: &'static str, label: &'static str, best: Option<&'static str>) -> Setting {
    Setting { section: "ScalabilityGroups", key, label, group: G, kind: Kind::Choice(UE_LEVELS), best, add: true }
}

/// The edits that write every setting's competitive value.
fn best_edits(settings: &[Setting]) -> Vec<Edit> {
    settings.iter().filter_map(|s| Some(Edit { section: s.section, key: s.key, value: s.best?.to_string(), add: s.add })).collect()
}

pub struct Target {
    pub path: PathBuf,
    pub format: Format,
    pub settings: &'static [Setting],
}

pub struct Profile {
    pub targets: Vec<Target>,
    /// Process names besides the game's exe that hold the settings file.
    pub procs: &'static [&'static str],
}

// ---------------------------------------------------------------- editors

fn eol_len(line: &[u8]) -> usize {
    if line.ends_with(b"\r\n") {
        2
    } else {
        line.ends_with(b"\n") as usize
    }
}

/// Writes the section's missing `add` keys after its last line with text.
fn add_missing(section: &str, out: &mut Vec<u8>, tail: usize, seen: &mut [bool], edits: &[Edit], nl: &[u8]) {
    let mut at = tail;
    for (i, e) in edits.iter().enumerate() {
        if e.add && !seen[i] && !section.is_empty() && e.section.eq_ignore_ascii_case(section) {
            let mut line = Vec::new();
            if at > 0 && out[at - 1] != b'\n' {
                line.extend_from_slice(nl); // the file ended without a line end
            }
            line.extend_from_slice(format!("{}={}", e.key, e.value).as_bytes());
            line.extend_from_slice(nl);
            out.splice(at..at, line.iter().copied());
            at += line.len();
            seen[i] = true;
        }
    }
}

fn edit_lines(src: &[u8], edits: &[Edit], sep: u8, sections: bool) -> (Vec<u8>, usize) {
    let nl: &[u8] = if src.contains(&b'\n') && !src.windows(2).any(|w| w == b"\r\n") { b"\n" } else { b"\r\n" };
    let mut out = Vec::with_capacity(src.len() + 64);
    let mut seen = vec![false; edits.len()];
    let (mut section, mut tail) = (String::new(), 0);
    for line in src.split_inclusive(|&b| b == b'\n') {
        let body = line.trim_ascii();
        if sections && body.starts_with(b"[") && body.ends_with(b"]") {
            add_missing(&section, &mut out, tail, &mut seen, edits, nl);
            section = String::from_utf8_lossy(&body[1..body.len() - 1]).to_string();
            out.extend_from_slice(line);
            tail = out.len();
            continue;
        }
        let eq = line.iter().position(|&b| b == sep);
        let hit = eq.and_then(|eq| {
            let key = line[..eq].trim_ascii();
            edits.iter().position(|e| (!sections || e.section.eq_ignore_ascii_case(&section)) && e.key.as_bytes().eq_ignore_ascii_case(key))
        });
        match (hit, eq) {
            (Some(i), Some(eq)) => {
                seen[i] = true;
                out.extend_from_slice(&line[..=eq]);
                out.extend_from_slice(edits[i].value.as_bytes());
                out.extend_from_slice(&line[line.len() - eol_len(line)..]);
            }
            _ => out.extend_from_slice(line),
        }
        if !body.is_empty() {
            tail = out.len();
        }
    }
    add_missing(&section, &mut out, tail, &mut seen, edits, nl);
    (out, seen.iter().filter(|s| **s).count())
}

fn edit_xml(src: &[u8], edits: &[Edit]) -> (Vec<u8>, usize) {
    let (mut out, mut found) = (src.to_vec(), 0);
    for e in edits {
        let pat = format!("<{} value=\"", e.key);
        let Some(start) = out.windows(pat.len()).position(|w| w == pat.as_bytes()).map(|at| at + pat.len()) else { continue };
        let Some(len) = out[start..].iter().position(|&b| b == b'"') else { continue };
        found += 1;
        // GTA V Legacy writes switches as words, Enhanced as numbers: keep the file's own form
        let word = matches!(&out[start..start + len], b"true" | b"false");
        let new = match (word, e.value.as_str()) {
            (true, "0") => "false",
            (true, "1") => "true",
            (_, v) => v,
        };
        out.splice(start..start + len, new.bytes());
    }
    (out, found)
}

/// The file with the profile's values, and how many of the profile's keys it has (0: not a file we know).
/// shortcut: UTF-16 files are refused, not edited; no settings file of the supported games is UTF-16.
pub fn edit(format: Format, src: &[u8], edits: &[Edit]) -> Result<(Vec<u8>, usize)> {
    if src.starts_with(&[0xFF, 0xFE]) || src.starts_with(&[0xFE, 0xFF]) {
        return Err(Error::Plain("The game's settings file is in an encoding MyTune does not edit. Nothing changed.".into()));
    }
    Ok(match format {
        Format::Ini => edit_lines(src, edits, b'=', true),
        Format::Colon => edit_lines(src, edits, b':', false),
        Format::Xml => edit_xml(src, edits),
    })
}

fn read_lines(src: &[u8], settings: &[Setting], sep: u8, sections: bool) -> Vec<Option<String>> {
    let mut out = vec![None; settings.len()];
    let mut section = String::new();
    for line in src.split(|&b| b == b'\n') {
        let body = line.trim_ascii();
        if sections && body.starts_with(b"[") && body.ends_with(b"]") {
            section = String::from_utf8_lossy(&body[1..body.len() - 1]).to_string();
        } else if let Some(eq) = line.iter().position(|&b| b == sep) {
            let key = line[..eq].trim_ascii();
            if let Some(i) = settings.iter().position(|s| (!sections || s.section.eq_ignore_ascii_case(&section)) && s.key.as_bytes().eq_ignore_ascii_case(key)) {
                out[i] = Some(String::from_utf8_lossy(line[eq + 1..].trim_ascii()).to_string());
            }
        }
    }
    out
}

fn read_xml(src: &[u8], settings: &[Setting]) -> Vec<Option<String>> {
    let value = |s: &Setting| {
        let pat = format!("<{} value=\"", s.key);
        let start = src.windows(pat.len()).position(|w| w == pat.as_bytes())? + pat.len();
        let len = src[start..].iter().position(|&b| b == b'"')?;
        // GTA V Legacy writes switches as words: report them as Enhanced's numbers
        Some(match &src[start..start + len] {
            b"true" => "1".to_string(),
            b"false" => "0".to_string(),
            v => String::from_utf8_lossy(v).to_string(),
        })
    };
    settings.iter().map(value).collect()
}

/// The file's value of each setting, as written in the file; `None`: the file lacks the key (or is UTF-16,
/// which `edit` refuses).
pub fn read(format: Format, src: &[u8], settings: &[Setting]) -> Vec<Option<String>> {
    if src.starts_with(&[0xFF, 0xFE]) || src.starts_with(&[0xFE, 0xFF]) {
        return vec![None; settings.len()];
    }
    match format {
        Format::Ini => read_lines(src, settings, b'=', true),
        Format::Colon => read_lines(src, settings, b':', false),
        Format::Xml => read_xml(src, settings),
    }
}

// ---------------------------------------------------------------- profiles

/// Where ARC Raiders keeps what is not a quality level (seen in the file on disk).
const EMBARK: &str = "/Script/EmbarkUserSettings.EmbarkGameUserSettings";
const ARC: &[Setting] = &[
    sg("sg.ShadowQuality", "Shadows", Some("0")),
    sg("sg.PostProcessQuality", "Post-processing", Some("0")),
    sg("sg.EffectsQuality", "Effects", Some("0")),
    sg("sg.FoliageQuality", "Foliage", Some("0")),
    sg("sg.TextureQuality", "Textures", None),
    sg("sg.ViewDistanceQuality", "View distance", None),
    sg("sg.GlobalIlluminationQuality", "Global illumination", None),
    sg("sg.ReflectionQuality", "Reflections", None),
    sw(EMBARK, "MotionBlurMode", "Motion blur", G, "On", "Off"),
    sw(EMBARK, "bUseVSync", "V-Sync", P, "True", "False"),
    num(EMBARK, "FrameRateLimit", "FPS cap (0 = no cap)", P, 1000.0, 6),
];
const WD: &str = "/Script/WDGame.WDUserSettings";
const WARDOGS: &[Setting] = &[
    sg("sg.ShadowQuality", "Shadows", Some("0")),
    sg("sg.PostProcessQuality", "Post-processing", Some("0")),
    sg("sg.EffectsQuality", "Effects", Some("0")),
    sg("sg.FoliageQuality", "Foliage", Some("0")),
    sg("sg.TextureQuality", "Textures", None),
    sg("sg.ViewDistanceQuality", "View distance", None),
    sw(WD, "bUseMotionBlur", "Motion blur", G, "True", "False"),
    sw(WD, "bUseDepthOfField", "Depth of field", G, "True", "False"),
    sw(WD, "bUseLensFlare", "Lens flare", G, "True", "False"),
    sw(WD, "bUseBloom", "Bloom", G, "True", "False"),
    sw(WD, "bUseVSync", "V-Sync", P, "True", "False"),
    num(WD, "FrameRateLimit", "FPS cap (0 = no cap)", P, 1000.0, 6),
];
const RL: &str = "SystemSettings";
const ROCKET_LEAGUE: &[Setting] = &[
    sw(RL, "DynamicShadows", "Dynamic shadows", G, "True", "False"),
    sw(RL, "AmbientOcclusion", "Ambient occlusion", G, "True", "False"),
    sw(RL, "bAllowLightShafts", "Light shafts", G, "True", "False"),
    sw(RL, "Bloom", "Bloom", G, "True", "False"),
    sw(RL, "LensFlares", "Lens flares", G, "True", "False"),
    sw(RL, "DepthOfField", "Depth of field", G, "True", "False"),
    sw(RL, "MotionBlur", "Motion blur", G, "True", "False"),
    sw(RL, "UseVsync", "V-Sync", P, "True", "False"),
];
const GTA_3: &[(&str, &str)] = &[("Normal", "0"), ("High", "1"), ("Very High", "2")];
const GTA_4: &[(&str, &str)] = &[("Normal", "0"), ("High", "1"), ("Very High", "2"), ("Ultra", "3")];
/// Shadows 1 = Normal (0 turns them off, which the game's menu does not offer); the others' lowest is 0.
const GTA: &[Setting] = &[
    pick("ShadowQuality", "Shadows", G, &[("Normal", "1"), ("High", "2"), ("Very High", "3")], Some("1")),
    pick("ParticleQuality", "Particles", G, GTA_3, Some("0")),
    pick("GrassQuality", "Grass", G, GTA_4, Some("0")),
    pick("PostFX", "Post effects", G, GTA_4, Some("0")),
    sw("", "MotionBlurStrength", "Motion blur", G, "0.500000", "0.000000"),
    sw("", "DoF", "Depth of field", G, "1", "0"),
    pick("VSync", "V-Sync", P, &[("Off", "0"), ("On", "1"), ("Half", "2")], Some("0")),
];
const MC_JAVA: &[Setting] = &[
    sw("", "entityShadows", "Entity shadows", G, "true", "false"),
    pick("particles", "Particles", G, &[("All", "0"), ("Decreased", "1"), ("Minimal", "2")], Some("1")),
    num("", "renderDistance", "Render distance (chunks)", G, 32.0, 0),
    sw("", "enableVsync", "V-Sync", P, "true", "false"),
    num("", "maxFps", "FPS cap (260 = no cap)", P, 260.0, 0),
];
const MC_BEDROCK: &[Setting] = &[sw("", "gfx_vsync", "V-Sync (Bedrock)", P, "1", "0")];

/// The Documents folder, also when it was moved (OneDrive).
fn documents() -> PathBuf {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{FOLDERID_Documents, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    unsafe {
        if let Ok(p) = SHGetKnownFolderPath(&FOLDERID_Documents, KF_FLAG_DEFAULT, None) {
            let s = p.to_string().ok();
            CoTaskMemFree(Some(p.0 as *const _));
            if let Some(s) = s {
                return PathBuf::from(s);
            }
        }
    }
    env("USERPROFILE").join("Documents")
}

fn env(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_default())
}

/// The profile of a game from the `Games` list, by its title; `None`: no profile for this game yet.
pub fn profile_for(name: &str, exe_path: &str) -> Option<Profile> {
    profile_in(name, exe_path, &env("LOCALAPPDATA"), &env("APPDATA"), &documents())
}

fn profile_in(name: &str, exe_path: &str, local: &Path, roaming: &Path, docs: &Path) -> Option<Profile> {
    let one = |path: PathBuf, format, settings| Profile { targets: vec![Target { path, format, settings }], procs: &[] };
    Some(match crate::gamelib::title_key(name).as_str() {
        "arc raiders" => one(local.join("PioneerGame\\Saved\\Config\\WindowsClient\\GameUserSettings.ini"), Format::Ini, ARC),
        "wardogs" => one(local.join("Wardogs\\Saved\\Config\\WindowsClient\\GameUserSettings.ini"), Format::Ini, WARDOGS),
        "rocket league" => one(docs.join("My Games\\Rocket League\\TAGame\\Config\\TASystemSettings.ini"), Format::Ini, ROCKET_LEAGUE),
        "grand theft auto v" => {
            let folder = if exe_path.to_lowercase().contains("enhanced") { "GTAV Enhanced" } else { "GTA V" };
            one(docs.join("Rockstar Games").join(folder).join("settings.xml"), Format::Xml, GTA)
        }
        "minecraft" => {
            let options = "games\\com.mojang\\minecraftpe\\options.txt";
            let mut targets = vec![
                Target { path: roaming.join(".minecraft\\options.txt"), format: Format::Colon, settings: MC_JAVA },
                Target { path: local.join("Packages\\Microsoft.MinecraftUWP_8wekyb3d8bbwe\\LocalState").join(options), format: Format::Colon, settings: MC_BEDROCK },
            ];
            // newer Bedrock builds keep one folder per signed-in user
            for user in fs::read_dir(roaming.join("Minecraft Bedrock\\Users")).into_iter().flatten().flatten() {
                targets.push(Target { path: user.path().join(options), format: Format::Colon, settings: MC_BEDROCK });
            }
            Profile { targets, procs: &["javaw", "Minecraft.Windows"] }
        }
        _ => return None,
    })
}

// ---------------------------------------------------------------- the tweak

fn hash(bytes: &[u8]) -> String {
    format!("{:016x}", bytes.iter().fold(0xcbf29ce484222325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100000001b3))) // FNV-1a
}

/// Writes the file; a read-only file is written and is read-only again afterwards.
fn write_keeping_readonly(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let perm = fs::metadata(path).map(|m| m.permissions()).ok().filter(|p| p.readonly());
    if let Some(p) = &perm {
        let mut open = p.clone();
        #[allow(clippy::permissions_set_readonly_false)] // Windows: this is only the read-only attribute
        open.set_readonly(false);
        fs::set_permissions(path, open)?;
    }
    let r = fs::write(path, bytes);
    if let Some(p) = perm {
        let _ = fs::set_permissions(path, p);
    }
    r
}

fn slot(b: &TweakBackup, kind: &str, n: usize) -> Option<String> {
    b.data.get(&format!("{kind}{n}")).cloned()
}

/// True when a file was changed after MyTune wrote it (by the game's own menu, usually). Revert still puts
/// the copy from before the profile back; the caller says so.
pub fn changed_since_apply(b: &TweakBackup) -> bool {
    (0..).map_while(|n| Some((slot(b, "file", n)?, slot(b, "after", n)?))).any(|(file, after)| fs::read(file).is_ok_and(|now| hash(&now) != after))
}

/// Revert-only tweaks for the game profiles in the engine's journal (`Pending` in state.json), so
/// `Engine::recover` can put back a profile whose apply was cut off. A backup without `after<n>` is such a
/// cut-off apply: `changed_since_apply` says false for it and revert restores the copy as usual.
pub fn pending_reverts(store: &crate::store::Store) -> Vec<ConfigTweak> {
    let pending = store.top("Pending").and_then(|p| p.as_object()).cloned().unwrap_or_default();
    pending
        .iter()
        .filter(|(id, _)| id.starts_with("game:") && id.ends_with(":config"))
        .map(|(id, e)| ConfigTweak::for_revert(Arc::new(crate::procs::WinProcs), id, e["Title"].as_str().unwrap_or(id)))
        .collect()
}

pub struct ConfigTweak {
    pub id: String,
    pub title: String,
    /// `None`: a game without a profile, or a tweak built only to revert (revert needs the backup alone).
    pub profile: Option<Profile>,
    pub procs: Arc<dyn Procs>,
    /// Process names (no `.exe`) that mean the game is open.
    pub stems: Vec<String>,
    /// Where the copies of the original files go.
    pub backup_dir: PathBuf,
    /// What to write: (target, setting in the target's list, value as written in the file). `None`: every
    /// setting's competitive value.
    pub values: Option<Vec<(usize, usize, String)>>,
}

/// A setting of a game as its file has it now.
pub struct Item {
    pub target: usize,
    pub index: usize,
    pub setting: &'static Setting,
    /// `None`: the file lacks the key (only settings that may be added are listed then).
    pub value: Option<String>,
}

impl ConfigTweak {
    pub fn new(procs: Arc<dyn Procs>, game_id: &str, game_name: &str, exe_path: &str, state_dir: &Path) -> ConfigTweak {
        let profile = profile_for(game_name, exe_path);
        let mut stems: Vec<String> = Path::new(exe_path).file_stem().map(|s| s.to_string_lossy().to_string()).into_iter().collect();
        stems.extend(profile.iter().flat_map(|p| p.procs.iter().map(|s| s.to_string())));
        ConfigTweak {
            id: crate::pergame::game_tweak_id(game_id, "config"),
            title: format!("{game_name}: in-game settings"),
            profile,
            procs,
            stems,
            backup_dir: state_dir.join("gameconfig"),
            values: None,
        }
    }

    /// The same tweak, writing these values instead of the competitive ones.
    pub fn with_values(mut self, values: Vec<(usize, usize, String)>) -> ConfigTweak {
        self.values = Some(values);
        self
    }

    /// The game's settings as its files have them now, in the profile's order. Reads the files.
    pub fn settings(&self) -> Vec<Item> {
        let mut out = Vec::new();
        for (target, t) in self.profile.iter().flat_map(|p| &p.targets).enumerate() {
            let Ok(src) = fs::read(&t.path) else { continue };
            let values = read(t.format, &src, t.settings);
            if values.iter().all(Option::is_none) {
                continue; // not a file we know
            }
            out.extend(values.into_iter().enumerate().filter(|(i, v)| v.is_some() || t.settings[*i].add).map(|(index, value)| Item { target, index, setting: &t.settings[index], value }));
        }
        out
    }

    fn edits(&self, target: usize, t: &Target) -> Vec<Edit> {
        let Some(values) = &self.values else { return best_edits(t.settings) };
        // a value that would break the line or the attribute is dropped, whatever sent it
        let ok = |v: &str| !v.is_empty() && !v.contains(['\r', '\n', '"', '<', '>']);
        values.iter().filter(|v| v.0 == target && ok(&v.2)).filter_map(|v| t.settings.get(v.1).map(|s| Edit { section: s.section, key: s.key, value: v.2.clone(), add: s.add })).collect()
    }

    /// For reverting a stored backup by its id alone (Restore page).
    pub fn for_revert(procs: Arc<dyn Procs>, id: &str, title: &str) -> ConfigTweak {
        ConfigTweak { id: id.into(), title: title.into(), profile: None, procs, stems: vec![], backup_dir: PathBuf::new(), values: None }
    }

    fn not_running(&self, stems: &[String]) -> Result<()> {
        let list = self.procs.list()?;
        match stems.iter().any(|s| list.iter().any(|p| p.name.eq_ignore_ascii_case(s))) {
            true => Err(Error::Plain("The game is running and would overwrite its settings on exit. Close it first.".into())),
            false => Ok(()),
        }
    }

    /// The targets that exist and hold keys to write: (target, file now, file with the values).
    fn readable(&self) -> Result<Vec<(&Target, Vec<u8>, Vec<u8>)>> {
        let mut out = Vec::new();
        for (i, t) in self.profile.iter().flat_map(|p| &p.targets).enumerate() {
            let Ok(src) = fs::read(&t.path) else { continue }; // the game has not made this file (yet)
            let (new, found) = edit(t.format, &src, &self.edits(i, t))?;
            if found > 0 {
                out.push((t, src, new));
            }
        }
        Ok(out)
    }

    /// Everything before the write: refuses while the game runs, copies each file that will change and records
    /// the copy in `b`. Changes no game file. Returns (file, new content, slot) of the files to write.
    /// Running it again copies nothing twice: a file keeps its first copy, the true original.
    fn prepare(&self, b: &mut TweakBackup) -> Result<Vec<(PathBuf, Vec<u8>, usize)>> {
        if self.profile.is_none() {
            return Err(Error::Plain("No settings for this game yet.".into()));
        }
        self.not_running(&self.stems)?;
        let files = self.readable()?;
        if files.is_empty() {
            return Err(Error::Plain("The game's settings file was not found. Start the game once so it creates the file, then try again.".into()));
        }
        let mut out = Vec::new();
        for (t, src, new) in files {
            if src == new {
                continue;
            }
            let path = t.path.to_string_lossy().to_string();
            let n = (0..).find(|&n| slot(b, "file", n).map_or(true, |f| f == path)).unwrap_or(0);
            if slot(b, "file", n).is_none() {
                fs::create_dir_all(&self.backup_dir)?;
                let name = t.path.file_name().unwrap_or_default().to_string_lossy();
                let copy = self.backup_dir.join(format!("{}-{n}-{name}", self.id.replace(':', "-")));
                fs::write(&copy, &src)?;
                if fs::read(&copy)? != src {
                    return Err(Error::Plain("The copy of the game's settings file does not match the file. Nothing changed.".into()));
                }
                b.data.insert(format!("file{n}"), path);
                b.data.insert(format!("backup{n}"), copy.to_string_lossy().to_string());
            }
            b.data.insert("procs".into(), self.stems.join("|"));
            out.push((t.path.clone(), new, n));
        }
        Ok(out)
    }

    /// The game's settings files that exist on this PC.
    pub fn files(&self) -> Vec<PathBuf> {
        self.profile.iter().flat_map(|p| &p.targets).filter(|t| t.path.is_file()).map(|t| t.path.clone()).collect()
    }
}

impl Tweak for ConfigTweak {
    fn id(&self) -> &str {
        &self.id
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn is_applied(&self) -> Result<bool> {
        let files = self.readable()?;
        Ok(!files.is_empty() && files.iter().all(|(_, src, new)| src == new))
    }
    /// The engine saves this backup before `apply` runs, so a crash during the write still has the copy.
    fn capture(&self, b: &mut TweakBackup) -> Result<()> {
        self.prepare(b).map(|_| ())
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        for (path, new, n) in self.prepare(b)? {
            write_keeping_readonly(&path, &new)?;
            b.data.insert(format!("after{n}"), hash(&new));
        }
        Ok(())
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        let stems: Vec<String> = b.data.get("procs").map(|s| s.split('|').map(String::from).collect()).unwrap_or_default();
        self.not_running(&stems)?;
        let (mut fails, mut copies) = (Vec::new(), Vec::new());
        for n in 0.. {
            let (Some(file), Some(copy)) = (slot(b, "file", n), slot(b, "backup", n)) else { break };
            if !Path::new(&file).parent().is_some_and(Path::is_dir) {
                crate::log::info(&format!("revert {}: {file} has no folder any more, skipped", self.id));
            } else if let Err(e) = fs::read(&copy).and_then(|orig| write_keeping_readonly(Path::new(&file), &orig)) {
                fails.push(format!("{file}: {e}"));
            }
            copies.push(copy);
        }
        if !fails.is_empty() {
            return Err(Error::Revert(fails)); // the copies stay for another try
        }
        for c in copies {
            let _ = fs::remove_file(c);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procs::{Held, ProcInfo};
    use std::sync::Mutex;

    struct Fake(Mutex<Vec<&'static str>>);
    impl Procs for Fake {
        fn list(&self) -> Result<Vec<ProcInfo>> {
            Ok(self.0.lock().unwrap().iter().map(|n| ProcInfo { pid: 1, name: n.to_string() }).collect())
        }
        fn throttle(&self, _: u32) -> Result<Option<Held>> {
            Ok(None)
        }
        fn restore(&self, _: u32, _: &Held) -> Result<()> {
            Ok(())
        }
    }

    /// Lines of `b` that differ from `a`, and lines only `b` has; fails when anything else differs.
    fn changed_lines(a: &[u8], b: &[u8]) -> Vec<String> {
        let (a, b): (Vec<&[u8]>, Vec<&[u8]>) = (a.split_inclusive(|&c| c == b'\n').collect(), b.split_inclusive(|&c| c == b'\n').collect());
        let (mut i, mut out) = (0, Vec::new());
        for l in &b {
            if a.get(i) == Some(l) {
                i += 1;
            } else {
                // a changed line replaces a's line; an added one does not
                if a.get(i).is_some_and(|al| al.split(|&c| c == b'=' || c == b':').next() == l.split(|&c| c == b'=' || c == b':').next()) {
                    i += 1;
                }
                out.push(String::from_utf8_lossy(l).to_string());
            }
        }
        assert_eq!(i, a.len(), "a line of the original is missing");
        out
    }

    const UE: &str = ";METADATA=(Diff=true, UseCommands=true)\r\n[Internationalization]\r\nCulture=en\r\n\r\n[ScalabilityGroups]\r\nsg.ResolutionQuality=100\r\nsg.ViewDistanceQuality=3\r\nsg.AntiAliasingQuality=4\r\nsg.ShadowQuality=3\r\nsg.PostProcessQuality=2\r\nsg.TextureQuality=3\r\nsg.EffectsQuality=3\r\n\r\n[/Script/EmbarkUserSettings.EmbarkGameUserSettings]\r\nMotionBlurMode=On\r\nbUseVSync=True\r\nResolutionSizeX=1920\r\nFullscreenMode=1\r\nFrameRateLimit=0.000000\r\n\r\n[Other]\r\nbUseVSync=True\r\nsg.ShadowQuality=3\r\n";

    #[test]
    fn unreal_ini_changes_only_profile_keys_in_their_section() {
        let (out, found) = edit(Format::Ini, UE.as_bytes(), &best_edits(ARC)).unwrap();
        assert_eq!(found, 6);
        assert_eq!(
            changed_lines(UE.as_bytes(), &out),
            ["sg.ShadowQuality=0\r\n", "sg.PostProcessQuality=0\r\n", "sg.EffectsQuality=0\r\n", "sg.FoliageQuality=0\r\n", "MotionBlurMode=Off\r\n", "bUseVSync=False\r\n"]
        );
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(text.contains("sg.EffectsQuality=0\r\nsg.FoliageQuality=0\r\n\r\n[/Script/Embark"), "missing key goes to the end of its section");
        assert!(text.contains("FrameRateLimit=0.000000\r\n\r\n[Other]\r\nbUseVSync=True\r\nsg.ShadowQuality=3\r\n"), "[Other] is untouched");
        assert!(text.contains("sg.ViewDistanceQuality=3\r\nsg.AntiAliasingQuality=4") && text.contains("sg.TextureQuality=3") && text.contains("ResolutionSizeX=1920\r\nFullscreenMode=1"));
        assert_eq!(edit(Format::Ini, &out, &best_edits(ARC)).unwrap().0, out, "a second pass changes nothing");
    }

    #[test]
    fn ini_keeps_unix_line_ends_missing_last_line_end_and_other_sections() {
        let src = b"[SystemSettings]\nUseVsync=True\nMotionBlur=TRUE\nResX=1920\n[SystemSettingsMobile]\nMotionBlur=TRUE\nBloom=TRUE";
        let (out, found) = edit(Format::Ini, src, &best_edits(ROCKET_LEAGUE)).unwrap();
        assert_eq!(found, 2);
        assert_eq!(out, b"[SystemSettings]\nUseVsync=False\nMotionBlur=False\nResX=1920\n[SystemSettingsMobile]\nMotionBlur=TRUE\nBloom=TRUE");
        // an Unreal file without the section gets nothing added
        assert_eq!(edit(Format::Ini, b"[Other]\r\nA=1\r\n", &best_edits(ARC)).unwrap(), (b"[Other]\r\nA=1\r\n".to_vec(), 0));
        // added key after a last line without a line end
        assert_eq!(edit(Format::Ini, b"[ScalabilityGroups]\nsg.ShadowQuality=2", &best_edits(&ARC[..4])).unwrap().0, b"[ScalabilityGroups]\nsg.ShadowQuality=0\nsg.PostProcessQuality=0\nsg.EffectsQuality=0\nsg.FoliageQuality=0\n");
        assert!(edit(Format::Ini, &[0xFF, 0xFE, b'[', 0], &best_edits(ARC)).is_err(), "UTF-16 is refused");
    }

    #[test]
    fn minecraft_options_change_only_profile_keys() {
        let src = b"version:3955\nenableVsync:true\nentityShadows:true\nparticles:0\nmaxFps:120\nkey_key.attack:key.mouse.left\nfullscreen:true\n";
        let (out, found) = edit(Format::Colon, src, &best_edits(MC_JAVA)).unwrap();
        assert_eq!(found, 3);
        assert_eq!(changed_lines(src, &out), ["enableVsync:false\n", "entityShadows:false\n", "particles:1\n"]);
        assert_eq!(edit(Format::Colon, b"gfx_vsync:1\r\ngfx_field_of_view:70\r\n", &best_edits(MC_BEDROCK)).unwrap().0, b"gfx_vsync:0\r\ngfx_field_of_view:70\r\n");
    }

    #[test]
    fn gta_xml_changes_attribute_values_only() {
        let src = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n\r\n<Settings>\r\n  <graphics>\r\n    <ShadowQuality value=\"3\" />\r\n    <TextureQuality value=\"2\" />\r\n    <ParticleQuality value=\"2\" />\r\n    <GrassQuality value=\"3\" />\r\n    <PostFX value=\"3\" />\r\n    <DoF value=\"true\" />\r\n    <MotionBlurStrength value=\"0.500000\" />\r\n  </graphics>\r\n  <video>\r\n    <ScreenWidth value=\"1920\" />\r\n    <RefreshRate value=\"60\" />\r\n    <Windowed value=\"0\" />\r\n    <VSync value=\"2\" />\r\n  </video>\r\n</Settings>\r\n";
        let (out, found) = edit(Format::Xml, src.as_bytes(), &best_edits(GTA)).unwrap();
        assert_eq!(found, 7);
        let want = src
            .replace("<ShadowQuality value=\"3\"", "<ShadowQuality value=\"1\"")
            .replace("<ParticleQuality value=\"2\"", "<ParticleQuality value=\"0\"")
            .replace("<GrassQuality value=\"3\"", "<GrassQuality value=\"0\"")
            .replace("<PostFX value=\"3\"", "<PostFX value=\"0\"")
            .replace("<DoF value=\"true\"", "<DoF value=\"false\"")
            .replace("value=\"0.500000\"", "value=\"0.000000\"")
            .replace("<VSync value=\"2\"", "<VSync value=\"0\"");
        assert_eq!(String::from_utf8(out).unwrap(), want);
        // Enhanced keeps its numbers
        assert_eq!(edit(Format::Xml, b"<DoF value=\"1\" />", &best_edits(GTA)).unwrap(), (b"<DoF value=\"0\" />".to_vec(), 1));
    }

    #[test]
    fn profiles_point_at_the_games_own_files() {
        let (l, r, d) = (Path::new("L"), Path::new("R"), Path::new("D"));
        let path = |name, exe| profile_in(name, exe, l, r, d).map(|p| p.targets[0].path.to_string_lossy().to_string());
        assert_eq!(path("ARC Raiders", "x.exe").unwrap(), "L\\PioneerGame\\Saved\\Config\\WindowsClient\\GameUserSettings.ini");
        assert_eq!(path("Rocket League\u{AE}", "x.exe").unwrap(), "D\\My Games\\Rocket League\\TAGame\\Config\\TASystemSettings.ini");
        assert_eq!(path("Grand Theft Auto V Legacy", "c:\\Grand Theft Auto V\\GTA5.exe").unwrap(), "D\\Rockstar Games\\GTA V\\settings.xml");
        assert_eq!(path("Grand Theft Auto V Enhanced", "c:\\Grand Theft Auto V Enhanced\\GTA5_Enhanced.exe").unwrap(), "D\\Rockstar Games\\GTAV Enhanced\\settings.xml");
        assert_eq!(path("Minecraft for Windows", "x.exe").unwrap(), "R\\.minecraft\\options.txt");
        assert_eq!(path("ROB RUN", "x.exe"), None);
        assert_eq!(path("Tom Clancy's Ghost Recon Wildlands", "x.exe"), None);
    }

    fn tweak(dir: &Path, running: &Arc<Fake>) -> (ConfigTweak, PathBuf) {
        let file = dir.join("GameUserSettings.ini");
        let profile = Profile { targets: vec![Target { path: file.clone(), format: Format::Ini, settings: ARC }, Target { path: dir.join("absent.ini"), format: Format::Ini, settings: ARC }], procs: &[] };
        (ConfigTweak { id: "game:g1:config".into(), title: "G: in-game settings".into(), profile: Some(profile), procs: running.clone(), stems: vec!["Game".into()], backup_dir: dir.join("gameconfig"), values: None }, file)
    }

    #[test]
    fn an_apply_cut_off_after_capture_is_put_back_at_start() {
        let d = tempfile::tempdir().unwrap();
        let running = Arc::new(Fake(Mutex::new(vec![])));
        let (t, file) = tweak(d.path(), &running);
        fs::write(&file, UE).unwrap();

        // what Engine::apply has on disk when the process dies inside `t.apply`: backup, journal entry, half-written file
        let mut b = TweakBackup::new(&t.title);
        *running.0.lock().unwrap() = vec!["game"];
        assert!(t.capture(&mut b).is_err() && b.data.is_empty() && !d.path().join("gameconfig").exists(), "refused before anything is copied");
        *running.0.lock().unwrap() = vec![];
        t.capture(&mut b).unwrap();
        assert_eq!(fs::read(&file).unwrap(), UE.as_bytes(), "capture changes no game file");
        let copy = PathBuf::from(&b.data["backup0"]);
        assert!(!b.data.contains_key("after0"));
        let mut store = crate::store::Store::load(d.path());
        store.set_top("Pending", serde_json::json!({ (t.id.clone()): { "Title": t.title, "Op": "apply", "At": "x", "Pid": std::process::id() } }));
        store.set_backup(&t.id, b.clone()).unwrap();
        fs::write(&file, &UE.as_bytes()[..40]).unwrap();

        assert!(!changed_since_apply(&b), "no notice for a cut-off apply");

        let engine = crate::engine::Engine::new(crate::store::Store::load(d.path()));
        let reverts = engine.with_store(|s| pending_reverts(s));
        assert_eq!(reverts.len(), 1);
        let left = engine.recover(|id| reverts.iter().find(|r| r.id == id).map(|r| r as &dyn Tweak));
        assert_eq!((left.len(), left[0].put_back), (1, true));
        assert_eq!(fs::read(&file).unwrap(), UE.as_bytes(), "byte for byte");
        assert!(!copy.exists());
        let disk = crate::store::Store::load(d.path());
        assert!(disk.get_backup(&t.id).is_none() && disk.top("Pending").is_none());
    }

    #[test]
    fn apply_backs_up_the_file_and_revert_restores_it_exactly() {
        let d = tempfile::tempdir().unwrap();
        let running = Arc::new(Fake(Mutex::new(vec!["explorer"])));
        let (t, file) = tweak(d.path(), &running);
        let engine = crate::engine::Engine::new(crate::store::Store::load(d.path()));

        assert!(matches!(engine.apply(&t), Err(Error::Plain(_))), "no settings file yet");
        assert!(engine.with_store(|s| s.get_backup(&t.id).is_none()));

        fs::write(&file, UE).unwrap();
        let mut ro = fs::metadata(&file).unwrap().permissions();
        ro.set_readonly(true);
        fs::set_permissions(&file, ro).unwrap();

        *running.0.lock().unwrap() = vec!["game"];
        assert!(engine.apply(&t).is_err(), "refused while the game runs");
        assert_eq!(fs::read(&file).unwrap(), UE.as_bytes());
        *running.0.lock().unwrap() = vec![];

        assert!(!t.is_applied().unwrap());
        engine.apply(&t).unwrap();
        assert!(t.is_applied().unwrap());
        assert!(fs::metadata(&file).unwrap().permissions().readonly(), "read-only flag kept");
        let b = engine.with_store(|s| s.get_backup(&t.id).cloned()).unwrap();
        let copy = PathBuf::from(&b.data["backup0"]);
        assert_eq!(fs::read(&copy).unwrap(), UE.as_bytes());
        assert_eq!(b.data["file0"], file.to_string_lossy());
        assert!(!changed_since_apply(&b));

        // a second apply keeps the first copy
        engine.apply(&t).unwrap();
        assert_eq!(fs::read(&copy).unwrap(), UE.as_bytes());

        // the game's menu rewrites the file: revert still restores the original, and the change is seen
        let mut p = fs::metadata(&file).unwrap().permissions();
        p.set_readonly(false);
        fs::set_permissions(&file, p.clone()).unwrap();
        fs::write(&file, b"[ScalabilityGroups]\r\nsg.ShadowQuality=1\r\n").unwrap();
        p.set_readonly(true);
        fs::set_permissions(&file, p).unwrap();
        assert!(changed_since_apply(&b));

        *running.0.lock().unwrap() = vec!["Game"];
        assert!(engine.revert(&ConfigTweak::for_revert(running.clone(), &t.id, &t.title)).is_err(), "refused while the game runs");
        *running.0.lock().unwrap() = vec![];
        engine.revert(&ConfigTweak::for_revert(running.clone(), &t.id, &t.title)).unwrap();
        assert_eq!(fs::read(&file).unwrap(), UE.as_bytes(), "byte for byte");
        assert!(fs::metadata(&file).unwrap().permissions().readonly());
        assert!(!copy.exists());
        assert!(engine.with_store(|s| s.get_backup(&t.id).is_none()));

        let mut p = fs::metadata(&file).unwrap().permissions();
        p.set_readonly(false);
        fs::set_permissions(&file, p).unwrap(); // so the temp folder can be removed
    }

    fn e(section: &'static str, key: &'static str, value: &str) -> Edit {
        Edit { section, key, value: value.into(), add: false }
    }
    fn at(settings: &[Setting], key: &str) -> usize {
        settings.iter().position(|s| s.key == key).unwrap()
    }

    #[test]
    fn read_gives_each_formats_values_as_the_file_has_them() {
        let v = read(Format::Ini, UE.as_bytes(), ARC);
        let get = |key| v[at(ARC, key)].as_deref();
        assert_eq!(get("sg.ShadowQuality"), Some("3"), "its own section, not [Other]");
        assert_eq!(get("sg.FoliageQuality"), None);
        assert_eq!((get("MotionBlurMode"), get("bUseVSync"), get("FrameRateLimit")), (Some("On"), Some("True"), Some("0.000000")));
        assert_eq!(read(Format::Ini, b"[SystemSettings]\n UseVsync = TRUE \n", ROCKET_LEAGUE)[at(ROCKET_LEAGUE, "UseVsync")].as_deref(), Some("TRUE"));

        let v = read(Format::Colon, b"version:3955\r\nenableVsync:true\r\nmaxFps:120\r\nkey_key.attack:key.mouse.left\r\n", MC_JAVA);
        assert_eq!((v[at(MC_JAVA, "enableVsync")].as_deref(), v[at(MC_JAVA, "maxFps")].as_deref(), v[at(MC_JAVA, "particles")].as_deref()), (Some("true"), Some("120"), None));

        let v = read(Format::Xml, b"<Settings>\r\n <DoF value=\"true\" />\r\n <VSync value=\"2\" />\r\n <MotionBlurStrength value=\"0.500000\" />\r\n</Settings>", GTA);
        assert_eq!((v[at(GTA, "DoF")].as_deref(), v[at(GTA, "VSync")].as_deref(), v[at(GTA, "MotionBlurStrength")].as_deref()), (Some("1"), Some("2"), Some("0.500000")));
        assert_eq!(v[at(GTA, "GrassQuality")], None);
        assert!(read(Format::Ini, &[0xFF, 0xFE, b'[', 0], ARC).iter().all(Option::is_none), "UTF-16 is not read");
    }

    #[test]
    fn chosen_values_are_written_in_each_format() {
        let (out, found) = edit(Format::Ini, UE.as_bytes(), &[e("ScalabilityGroups", "sg.ShadowQuality", "2"), e(EMBARK, "FrameRateLimit", "144.000000"), e(EMBARK, "bUseVSync", "True")]).unwrap();
        assert_eq!(found, 3);
        assert_eq!(changed_lines(UE.as_bytes(), &out), ["sg.ShadowQuality=2\r\n", "FrameRateLimit=144.000000\r\n"], "a value the file already has changes nothing");
        assert_eq!(read(Format::Ini, &out, ARC)[at(ARC, "FrameRateLimit")].as_deref(), Some("144.000000"));

        let src = b"enableVsync:false\nmaxFps:120\nfov:0.5\n";
        assert_eq!(edit(Format::Colon, src, &[e("", "maxFps", "60"), e("", "enableVsync", "true")]).unwrap().0, b"enableVsync:true\nmaxFps:60\nfov:0.5\n");

        // a switch written as a word stays a word, also when it is turned on
        let (out, _) = edit(Format::Xml, b"<DoF value=\"false\" /><ShadowQuality value=\"1\" /><Windowed value=\"0\" />", &[e("", "DoF", "1"), e("", "ShadowQuality", "3")]).unwrap();
        assert_eq!(out, b"<DoF value=\"true\" /><ShadowQuality value=\"3\" /><Windowed value=\"0\" />");
    }

    #[test]
    fn saving_again_keeps_the_first_backup_and_revert_restores_the_untouched_file() {
        let d = tempfile::tempdir().unwrap();
        let running = Arc::new(Fake(Mutex::new(vec![])));
        let (t, file) = tweak(d.path(), &running);
        fs::write(&file, UE).unwrap();
        let engine = crate::engine::Engine::new(crate::store::Store::load(d.path()));
        let shown: Vec<&str> = t.settings().iter().map(|i| i.setting.key).collect();
        assert_eq!(shown.len(), ARC.len(), "the missing sg.FoliageQuality is listed too: it may be added");

        let t = t.with_values(vec![(0, at(ARC, "sg.ShadowQuality"), "1".into()), (0, at(ARC, "sg.FoliageQuality"), "2".into()), (1, 0, "0".into()), (0, 99, "0".into()), (0, 0, "1\r\nX=1".into())]);
        engine.apply(&t).unwrap();
        let first = fs::read(&file).unwrap();
        assert_eq!(changed_lines(UE.as_bytes(), &first), ["sg.ShadowQuality=1\r\n", "sg.FoliageQuality=2\r\n"], "only the chosen values; bad values are dropped");
        let copy = PathBuf::from(engine.with_store(|s| s.get_backup(&t.id).cloned()).unwrap().data["backup0"].clone());
        assert_eq!(fs::read(&copy).unwrap(), UE.as_bytes());

        // the second save starts from the file MyTune already changed
        let t = t.with_values(vec![(0, at(ARC, "FrameRateLimit"), "144.000000".into()), (0, at(ARC, "bUseVSync"), "False".into())]);
        engine.apply(&t).unwrap();
        let b = engine.with_store(|s| s.get_backup(&t.id).cloned()).unwrap();
        assert_eq!((PathBuf::from(&b.data["backup0"]), b.data.contains_key("backup1")), (copy.clone(), false), "one copy, the first");
        assert_eq!(fs::read(&copy).unwrap(), UE.as_bytes(), "still the file from before MyTune");
        assert_eq!(changed_lines(&first, &fs::read(&file).unwrap()), ["bUseVSync=False\r\n", "FrameRateLimit=144.000000\r\n"]);
        assert!(!changed_since_apply(&b));
        let now = t.settings();
        assert_eq!((now[at(ARC, "sg.ShadowQuality")].value.as_deref(), now[at(ARC, "FrameRateLimit")].value.as_deref()), (Some("1"), Some("144.000000")));

        engine.revert(&ConfigTweak::for_revert(running.clone(), &t.id, &t.title)).unwrap();
        assert_eq!(fs::read(&file).unwrap(), UE.as_bytes(), "byte for byte");
        assert!(!copy.exists() && engine.with_store(|s| s.get_backup(&t.id).is_none()));
    }
}
