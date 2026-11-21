//! A game's in-game settings, read from and written into the game's own settings file. Each supported game has
//! a hand-written list of settings (`Setting`: key, label, kind, choices) with MyTune's own "competitive" value
//! for some of them (V-Sync, motion blur, depth of field and lens flare off; shadows, effects, post-processing
//! and foliage low). Resolution, window mode, FPS caps, sensitivity, keys and sound are never touched, and a key
//! the file does not have is never added.
//!
//! Only the chosen keys change; every other byte of the file stays (line ends, encoding, read-only flag).
//! `ConfigTweak` (id `game:<id>:config`) copies the whole file to `%ProgramData%\MyTune\gameconfig` first and
//! records it in the backup's `Data` (`file<n>`, `backup<n>`, `after<n>` = hash of what was written, `ro<n>`,
//! `mtime<n>`, `procs`, `exe`); revert puts that exact file back. Saving again while applied keeps that first
//! copy, so revert always goes back to the file from before MyTune touched it. Both refuse while the game runs:
//! it would overwrite the file on exit. A revert that cannot reach the file keeps the copy and the record;
//! `forget` drops them when the user says the game is gone.

use crate::backup::TweakBackup;
use crate::engine::Tweak;
use crate::error::{Error, Result};
use crate::procs::Procs;
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Format {
    /// `[Section]` and `Key=Value` lines (Unreal).
    Ini,
    /// `key:value` lines, no sections (Minecraft).
    Colon,
    /// `<Key value="..." />` (GTA V settings.xml); only the attribute's value changes.
    Xml,
    /// One JSON object with top-level `"key":value` pairs (ROB RUN); only the value's text changes.
    Json,
}

pub struct Edit {
    pub section: &'static str,
    pub key: &'static str,
    pub value: String,
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
#[derive(Debug, Clone, Copy)]
pub struct Setting {
    pub section: &'static str,
    pub key: &'static str,
    pub label: &'static str,
    pub group: Group,
    pub kind: Kind,
    /// MyTune's competitive value (as written in the file); `None`: no opinion.
    pub best: Option<&'static str>,
    /// Listed as unavailable when the file lacks the key. Other settings show only when the file has it.
    pub show_absent: bool,
}

const G: Group = Group::Graphics;
const P: Group = Group::Performance;

/// A switch whose competitive value is off.
const fn sw(section: &'static str, key: &'static str, label: &'static str, group: Group, on: &'static str, off: &'static str) -> Setting {
    Setting { section, key, label, group, kind: Kind::Switch { on, off }, best: Some(off), show_absent: false }
}
const fn pick(key: &'static str, label: &'static str, group: Group, choices: &'static [(&'static str, &'static str)], best: Option<&'static str>) -> Setting {
    Setting { section: "", key, label, group, kind: Kind::Choice(choices), best, show_absent: false }
}
const fn num(section: &'static str, key: &'static str, label: &'static str, group: Group, max: f64, decimals: usize) -> Setting {
    Setting { section, key, label, group, kind: Kind::Number { min: 0.0, max, decimals }, best: None, show_absent: false }
}
/// A graphics choice in a section.
const fn opt(section: &'static str, key: &'static str, label: &'static str, choices: &'static [(&'static str, &'static str)], best: Option<&'static str>) -> Setting {
    Setting { section, key, label, group: G, kind: Kind::Choice(choices), best, show_absent: false }
}
/// A graphics number with its lowest value as the competitive one.
const fn low(key: &'static str, label: &'static str, min: f64, max: f64, decimals: usize, best: &'static str) -> Setting {
    Setting { section: "", key, label, group: G, kind: Kind::Number { min, max, decimals }, best: Some(best), show_absent: false }
}
/// Unreal's quality levels. shortcut: the names are Unreal's defaults, a game's own menu may call them differently.
const UE_LEVELS: &[(&str, &str)] = &[("Low", "0"), ("Medium", "1"), ("High", "2"), ("Epic", "3"), ("Ultra", "4")];
/// An Unreal `sg.*` quality level.
const fn sg(key: &'static str, label: &'static str, best: Option<&'static str>) -> Setting {
    Setting { section: "ScalabilityGroups", key, label, group: G, kind: Kind::Choice(UE_LEVELS), best, show_absent: true }
}

/// The edits that write every setting's competitive value.
fn best_edits(settings: &[Setting]) -> Vec<Edit> {
    settings.iter().filter_map(|s| Some(Edit { section: s.section, key: s.key, value: s.best?.to_string() })).collect()
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

/// UTF-8 byte order mark at the start of a text file.
const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

fn edit_lines(src: &[u8], edits: &[Edit], sep: u8, sections: bool) -> (Vec<u8>, usize) {
    let mut out = Vec::with_capacity(src.len() + 64);
    let (bom, src) = src.split_at(if src.starts_with(BOM) { BOM.len() } else { 0 });
    out.extend_from_slice(bom); // kept as it was; the lines below must not see it, or the first header is missed
    let mut seen = vec![false; edits.len()];
    let mut section = String::new();
    for line in src.split_inclusive(|&b| b == b'\n') {
        let body = line.trim_ascii();
        if sections && body.starts_with(b"[") && body.ends_with(b"]") {
            section = String::from_utf8_lossy(&body[1..body.len() - 1]).to_string();
            out.extend_from_slice(line);
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
    }
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

/// Where `"key":value` has its value: (start, end, was a string). For a string, start..end is the text between
/// the quotes. shortcut: first match of the key anywhere in the file; the supported file is flat.
fn json_span(src: &[u8], key: &str) -> Option<(usize, usize, bool)> {
    let pat = format!("\"{key}\"");
    let mut from = 0;
    while let Some(at) = src[from..].windows(pat.len()).position(|w| w == pat.as_bytes()) {
        let mut i = from + at + pat.len();
        from = i;
        while src.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if src.get(i) != Some(&b':') {
            continue; // the text of a value, not a key
        }
        i += 1;
        while src.get(i).is_some_and(u8::is_ascii_whitespace) {
            i += 1;
        }
        if src.get(i) == Some(&b'"') {
            let mut j = i + 1;
            while j < src.len() && src[j] != b'"' {
                j += if src[j] == b'\\' { 2 } else { 1 };
            }
            return (j < src.len()).then_some((i + 1, j, true));
        }
        let len = src[i..].iter().position(|b| matches!(b, b',' | b'}' | b']') || b.is_ascii_whitespace())?;
        return (len > 0).then_some((i, i + len, false));
    }
    None
}

fn edit_json(src: &[u8], edits: &[Edit]) -> (Vec<u8>, usize) {
    let (mut out, mut found) = (src.to_vec(), 0);
    for e in edits {
        let Some((start, end, _)) = json_span(&out, e.key) else { continue };
        found += 1;
        out.splice(start..end, e.value.bytes());
    }
    (out, found)
}

fn read_json(src: &[u8], settings: &[Setting]) -> Vec<Option<String>> {
    settings.iter().map(|s| json_span(src, s.key).map(|(a, b, _)| String::from_utf8_lossy(&src[a..b]).to_string())).collect()
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
        Format::Json => edit_json(src, edits),
    })
}

fn read_lines(src: &[u8], settings: &[Setting], sep: u8, sections: bool) -> Vec<Option<String>> {
    let mut out = vec![None; settings.len()];
    let mut section = String::new();
    let src = src.strip_prefix(BOM).unwrap_or(src);
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
        Format::Json => read_json(src, settings),
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
    // NVIDIA's lowest preset: High Quality Shaders off, Particle Detail and World Detail "Performance"
    // (World Detail is these three keys; Render Quality is a resolution scale and stays)
    sw(RL, "bAllowHighQualityMaterials", "High quality shaders", G, "True", "False"),
    opt(RL, "ParticleLODBias", "Particle detail", &[("High quality", "0"), ("Performance", "1")], Some("1")),
    opt(RL, "DetailMode", "World detail", &[("Performance", "0"), ("Quality", "1"), ("High quality", "2")], Some("0")),
    sw(RL, "StaticDecals", "Static decals", G, "True", "False"),
    opt(RL, "DecalCullDistanceScale", "Decal distance", &[("Short", "0.500000"), ("Full", "1.000000")], Some("0.500000")),
];
/// Ghost Recon's `GRB.ini` / `GRW.ini`: sections, keys and every level's number are the ones NVIDIA App's
/// wrapper for the game writes; the competitive value is NVIDIA's lowest preset. Left out on purpose:
/// window mode, resolution scale (`Supersampling`) and Temporal Injection (an upscaler).
const GR_QP: &str = "Quality And Performance";
const GR_6: &[(&str, &str)] = &[("Low", "0"), ("Medium", "1"), ("High", "2"), ("Very High", "3"), ("Ultra", "4"), ("Ultimate", "5")];
const GR_4: &[(&str, &str)] = &[("Low", "0"), ("High", "1"), ("Very High", "2"), ("Ultra", "3")];
const GR_ON: &[(&str, &str)] = &[("Off", "0"), ("On", "1")];
const BREAKPOINT: &[Setting] = &[
    opt(GR_QP, "TextureQuality", "Textures", GR_6, Some("0")),
    opt(GR_QP, "LevelOfDetail", "Level of detail", GR_6, Some("0")),
    opt(GR_QP, "TerrainQuality", "Terrain", GR_6, Some("0")),
    opt(GR_QP, "GrassQuality", "Grass", GR_6, Some("0")),
    opt(GR_QP, "ShadowQuality", "Sun shadows", GR_6, Some("0")),
    opt(GR_QP, "LongRangeShadows", "Long range shadows", &[("Low", "0"), ("High", "1"), ("Ultimate", "2")], Some("0")),
    sw(GR_QP, "ScreenSpaceShadows", "Screen space shadows", G, "1", "0"),
    opt(GR_QP, "AmbientOcclusion", "Ambient occlusion", &[("Disabled", "0"), ("Medium", "1"), ("High", "2"), ("Very High", "3"), ("Ultra", "4"), ("Ultimate", "5")], Some("0")),
    opt(GR_QP, "ScreenSpaceReflections", "Screen space reflections", &[("Low", "0"), ("Medium", "1"), ("High", "2"), ("Very High", "3")], Some("0")),
    opt(GR_QP, "VolumetricFog", "Volumetric fog", GR_6, Some("0")),
    opt(GR_QP, "SubSurfaceScattering", "Subsurface scattering", &[("Off", "0"), ("High", "1"), ("Very High", "2")], Some("0")),
    opt(GR_QP, "ClothSimulation", "Cloth simulation", &[("Low", "0"), ("Medium", "1"), ("High", "2")], Some("0")),
    opt(GR_QP, "Anisotropy", "Anisotropic filtering", &[("Off", "0"), ("2x", "1"), ("4x", "2"), ("8x", "3"), ("16x", "4")], Some("0")),
    sw(GR_QP, "Bloom", "Bloom", G, "1", "0"),
    // NVIDIA keeps it on in every preset: listed, no opinion
    opt(GR_QP, "EnableAntiAliasing", "Anti-aliasing", GR_ON, None),
];
const WILDLANDS: &[Setting] = &[
    opt(GR_QP, "TextureQuality", "Textures", &[("Low", "0"), ("Medium", "1"), ("High", "2"), ("Ultra", "3")], Some("0")),
    opt(GR_QP, "GeometryQuality", "Level of detail", GR_4, Some("0")),
    opt(GR_QP, "TerrainQuality", "Terrain", GR_4, Some("0")),
    opt(GR_QP, "GrassQuality", "Vegetation", GR_4, Some("0")),
    sw(GR_QP, "EnhancedGrass", "Turf effects", G, "1", "0"),
    opt(GR_QP, "DrawDistance", "Draw distance", &[("Low", "0"), ("Medium", "1"), ("High", "2"), ("Very High", "3")], Some("0")),
    opt(GR_QP, "ShadowQuality", "Shadows", &[("Off", "0"), ("Medium", "1"), ("High", "2"), ("Very High", "3"), ("Ultra", "4")], Some("0")),
    opt(GR_QP, "LongRangeShadows", "Long range shadows", &[("Off", "0"), ("On", "1"), ("Ultra", "2")], Some("0")),
    opt(GR_QP, "AmbientOcclusion", "Ambient occlusion", &[("Off", "0"), ("SSBC", "1"), ("HBAO+", "2")], Some("0")),
    opt(GR_QP, "GodRays", "God rays", &[("Off", "0"), ("On", "1"), ("Enhanced", "2")], Some("0")),
    sw(GR_QP, "SubSurfaceScattering", "Subsurface scattering", G, "1", "0"),
    sw(GR_QP, "HighQualityDOF", "High quality depth of field", G, "1", "0"),
    opt(GR_QP, "AntiAliasingMode", "Anti-aliasing", &[("Off", "0"), ("FXAA", "1"), ("Temporal AA", "2"), ("SMAA", "3"), ("SMAA + FXAA", "4")], Some("0")),
    opt(GR_QP, "Anisotropy", "Anisotropic filtering", &[("Off", "0"), ("2x", "2"), ("4x", "4"), ("8x", "8"), ("16x", "16")], Some("0")),
    // NVIDIA keeps it on in every preset: listed, no opinion
    opt(GR_QP, "LensFlare", "Lens flare", GR_ON, None),
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
const GTA_PCT: &[(&str, &str)] = &[
    ("0%", "0.000000"), ("10%", "0.100000"), ("20%", "0.200000"), ("30%", "0.300000"), ("40%", "0.400000"), ("50%", "0.500000"),
    ("60%", "0.600000"), ("70%", "0.700000"), ("80%", "0.800000"), ("90%", "0.900000"), ("100%", "1.000000"),
];
/// GTA V Legacy only: `GTA` plus the rest of NVIDIA's lowest preset for it, with the values its wrapper writes.
/// Enhanced has other scales and its own preset, so it keeps `GTA`. Left out: SSAO, FXAA_Enabled, MSAA,
/// TXAA_Enabled and ReflectionMSAA (NVIDIA names them, but no Legacy file was at hand to confirm them),
/// TextureQuality (NVIDIA picks it from the video memory), DirectX version, window mode.
const GTA_LEGACY: &[Setting] = &[
    GTA[0], GTA[1], GTA[2], GTA[3], GTA[4], GTA[5], GTA[6],
    pick("ShaderQuality", "Shaders", G, GTA_3, Some("0")),
    pick("ReflectionQuality", "Reflections", G, GTA_4, Some("0")),
    pick("WaterQuality", "Water", G, GTA_3, Some("0")),
    pick("Tessellation", "Tessellation", G, &[("Off", "0"), ("Normal", "1"), ("High", "2"), ("Very High", "3")], Some("0")),
    pick("AnisotropicFiltering", "Anisotropic filtering", G, &[("Off", "0"), ("x2", "2"), ("x4", "4"), ("x8", "8"), ("x16", "16")], Some("0")),
    pick("Shadow_SoftShadows", "Soft shadows", G, &[("Sharp", "0"), ("Soft", "1"), ("Softer", "2"), ("Softest", "3"), ("AMD CHS", "4"), ("NVIDIA PCSS", "5")], Some("0")),
    sw("", "Shadow_LongShadows", "Long shadows", G, "1", "0"),
    sw("", "UltraShadows_Enabled", "High resolution shadows", G, "1", "0"),
    pick("Shadow_Distance", "Extended shadows distance", G, &[
        ("0%", "1.000000"), ("10%", "1.100000"), ("20%", "1.200000"), ("30%", "1.300000"), ("40%", "1.400000"), ("50%", "1.500000"),
        ("60%", "1.600000"), ("70%", "1.700000"), ("80%", "1.800000"), ("90%", "1.900000"), ("100%", "2.000000"),
    ], Some("1.000000")),
    pick("LodScale", "Distance scaling", G, GTA_PCT, Some("0.000000")),
    pick("MaxLodScale", "Extended distance scaling", G, GTA_PCT, Some("0.000000")),
    sw("", "HdStreamingInFlight", "High detail streaming while flying", G, "1", "0"),
    pick("CityDensity", "Population density", G, GTA_PCT, Some("0.000000")),
    pick("PedVarietyMultiplier", "Pedestrian variety", G, GTA_PCT, Some("0.000000")),
    pick("VehicleVarietyMultiplier", "Vehicle variety", G, GTA_PCT, Some("0.000000")),
];
const MC_JAVA: &[Setting] = &[
    sw("", "entityShadows", "Entity shadows", G, "true", "false"),
    pick("particles", "Particles", G, &[("All", "0"), ("Decreased", "1"), ("Minimal", "2")], Some("1")),
    num("", "renderDistance", "Render distance (chunks)", G, 32.0, 0),
    sw("", "enableVsync", "V-Sync", P, "true", "false"),
    // NVIDIA's lowest preset, values as its wrapper writes them. Left out: renderClouds (quoted in new
    // versions, bare in old ones), graphicsPreset, textureFiltering and maxAnisotropyBit (written together)
    sw("", "ao", "Smooth lighting", G, "true", "false"),
    pick("biomeBlendRadius", "Biome blend", G, &[("Fastest", "0"), ("Fast", "1"), ("Normal", "2"), ("High", "3"), ("Very High", "4"), ("Extreme", "5"), ("Showoff", "6"), ("Maximum", "7")], Some("0")),
    pick("mipmapLevels", "Mipmap levels", G, &[("Off", "0"), ("1", "1"), ("2", "2"), ("3", "3"), ("4", "4")], Some("0")),
    pick("prioritizeChunkUpdates", "Chunk builder", P, &[("Threaded", "0"), ("Semi blocking", "1"), ("Fully blocking", "2")], Some("0")),
    low("simulationDistance", "Simulation distance (chunks)", 5.0, 32.0, 0, "5"),
    low("entityDistanceScaling", "Entity distance (1 = 100%)", 0.5, 5.0, 2, "0.5"),
    low("cloudRange", "Cloud distance (chunks)", 2.0, 128.0, 0, "32"),
    low("weatherRadius", "Weather effect radius", 3.0, 10.0, 0, "3"),
];
const MC_BEDROCK: &[Setting] = &[sw("", "gfx_vsync", "V-Sync (Bedrock)", P, "1", "0")];
/// ROB RUN's `settings_steam_*.dat`. Only the values seen in the file on disk are offered for the level
/// settings: the game's other level names are not known, so they are not guessed.
const ROB_RUN: &[Setting] = &[
    sw("", "vsync_setting", "V-Sync", P, "true", "false"),
    pick("cast_shadow_setting", "Shadows", G, &[("Off", "disabled")], Some("disabled")),
    pick("anti_aliasing_mode_id_setting", "Anti-aliasing", G, &[("Off", "disabled")], Some("disabled")),
    pick("settings_grass", "Grass", G, &[("Low", "low")], Some("low")),
    pick("settings_water", "Water", G, &[("Minimum", "minimum")], Some("minimum")),
    pick("renderers_culling_preset_id_setting", "Object culling", P, &[("Low", "low")], Some("low")),
    Setting { section: "", key: "performance_info_fps", label: "Show FPS", group: P, kind: Kind::Switch { on: "true", off: "false" }, best: None, show_absent: false },
];

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
    let (local, roaming) = (env("LOCALAPPDATA"), env("APPDATA"));
    let hand = profile_in(name, exe_path, &local, &roaming, &documents());
    with_lowest(hand, nvidia(name, &local), &local, &roaming)
}

/// True when the game has no hand-written profile: its settings come from NVIDIA App's cache alone.
pub fn nvidia_only(name: &str, exe_path: &str) -> bool {
    profile_in(name, exe_path, &env("LOCALAPPDATA"), &env("APPDATA"), &documents()).is_none()
}

fn profile_in(name: &str, exe_path: &str, local: &Path, roaming: &Path, docs: &Path) -> Option<Profile> {
    let one = |path: PathBuf, format, settings| Profile { targets: vec![Target { path, format, settings }], procs: &[] };
    Some(match crate::gamelib::title_key(name).as_str() {
        "arc raiders" => one(local.join("PioneerGame\\Saved\\Config\\WindowsClient\\GameUserSettings.ini"), Format::Ini, ARC),
        "wardogs" => one(local.join("Wardogs\\Saved\\Config\\WindowsClient\\GameUserSettings.ini"), Format::Ini, WARDOGS),
        "rocket league" => one(docs.join("My Games\\Rocket League\\TAGame\\Config\\TASystemSettings.ini"), Format::Ini, ROCKET_LEAGUE),
        "grand theft auto v" => {
            let (folder, settings) = if exe_path.to_lowercase().contains("enhanced") { ("GTAV Enhanced", GTA) } else { ("GTA V", GTA_LEGACY) };
            one(docs.join("Rockstar Games").join(folder).join("settings.xml"), Format::Xml, settings)
        }
        // GRB_vulkan / the _UPP copies: the game's other exes (names not checked on disk; a wrong one never matches)
        "ghost recon breakpoint" => Profile { procs: &["GRB", "GRB_vulkan", "GRB_UPP"], ..one(docs.join("My Games\\Ghost Recon Breakpoint\\GRB.ini"), Format::Ini, BREAKPOINT) },
        "ghost recon wildlands" => Profile { procs: &["GRW", "GRW_Upp"], ..one(docs.join("My Games\\Ghost Recon Wildlands\\GRW.ini"), Format::Ini, WILDLANDS) },
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
        "rob run" => {
            let dir = local.parent().unwrap_or(local).join("LocalLow\\Kefir\\ROB RUN\\settings");
            let targets = fs::read_dir(dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|f| f.file_name().to_string_lossy().starts_with("settings_steam_") && f.path().extension().is_some_and(|e| e == "dat"))
                .map(|f| Target { path: f.path(), format: Format::Json, settings: ROB_RUN })
                .collect();
            Profile { targets, procs: &[] }
        }
        _ => return None,
    })
}

// ---------------------------------------------------------------- NVIDIA App's lowest preset

/// One setting of NVIDIA's lowest preset (`nvcache`), ready for a `Target`.
struct Lowest {
    /// The file as NVIDIA's wrapper names it (see `nvcache::Found::file`).
    file: &'static str,
    format: Format,
    setting: Setting,
    /// JSON: the value sits in quotes.
    text: bool,
}

fn leak(s: &str) -> &'static str {
    Box::leak(s.to_string().into_boxed_str())
}

/// `Setting` holds `&'static str`, so the strings are leaked; `nvidia` does this once per game and run.
fn lowest_of(found: Vec<crate::nvcache::Found>) -> Vec<Lowest> {
    let one = |f: crate::nvcache::Found| {
        let kind = match f.range {
            Some((min, max, decimals)) => Kind::Number { min, max, decimals },
            None => Kind::Choice(Box::leak(f.choices.iter().map(|(l, v)| (leak(l), leak(v))).collect::<Vec<_>>().into_boxed_slice())),
        };
        let setting = Setting { section: leak(&f.section), key: leak(&f.key), label: leak(&f.label), group: G, kind, best: Some(leak(&f.best)), show_absent: false };
        Lowest { file: leak(&f.file), format: if f.ini { Format::Ini } else { Format::Json }, setting, text: f.text }
    };
    found.into_iter().map(one).collect()
}

/// NVIDIA's lowest preset for the game from NVIDIA App's local cache; empty when the cache has nothing usable.
/// Read once per game and run; what was left out goes to the log.
fn nvidia(name: &str, local: &Path) -> &'static [Lowest] {
    static SEEN: Mutex<Option<HashMap<String, &'static [Lowest]>>> = Mutex::new(None);
    let key = crate::gamelib::title_key(name);
    let mut seen = SEEN.lock().unwrap_or_else(|e| e.into_inner());
    let seen = seen.get_or_insert_with(HashMap::new);
    if let Some(known) = seen.get(&key) {
        return known;
    }
    let (found, skipped) = crate::nvcache::for_game(&local.join("NVIDIA Corporation\\NVIDIA App\\NvBackend\\Recommendations"), &key);
    if !found.is_empty() || !skipped.is_empty() {
        let kept: Vec<&str> = found.iter().map(|f| f.label.as_str()).collect();
        crate::log::info(&format!("NVIDIA App lowest preset for {name}: {} settings ({}); left out: {}", kept.len(), kept.join(", "), skipped.join(", ")));
    }
    let lowest: &'static [Lowest] = Box::leak(lowest_of(found).into_boxed_slice());
    seen.insert(key, lowest);
    lowest
}

/// The files a wrapper's path means. It must start with `%profile%`, `%localappdata%` or `%appdata%` and stay
/// below it; one `*` in the file name stands for any text. A path without `*` is returned even when absent.
fn nvidia_files(raw: &str, local: &Path, roaming: &Path) -> Vec<PathBuf> {
    let profile = local.parent().and_then(Path::parent);
    for (token, dir) in [("%profile%\\", profile), ("%localappdata%\\", Some(local)), ("%appdata%\\", Some(roaming))] {
        let Some(rest) = raw.get(token.len()..).filter(|_| raw.get(..token.len()).is_some_and(|p| p.eq_ignore_ascii_case(token))) else { continue };
        let odd = rest.contains(['%', ':', '/', '?', '"', '<', '>', '|']) || rest.split('\\').any(|part| part.is_empty() || part == "." || part == "..");
        let (Some(dir), false) = (dir, odd) else { return vec![] };
        let path = dir.join(rest);
        let (Some(folder), Some(file)) = (path.parent(), path.file_name().map(|f| f.to_string_lossy().to_lowercase())) else { return vec![] };
        return match file.split_once('*') {
            None if !rest.contains('*') => vec![path],
            Some((start, end)) if rest.matches('*').count() == 1 => {
                let fits = |name: String| name.len() >= start.len() + end.len() && name.starts_with(start) && name.ends_with(end);
                fs::read_dir(folder).into_iter().flatten().flatten().filter(|f| fits(f.file_name().to_string_lossy().to_lowercase())).map(|f| f.path()).filter(|p| p.is_file()).collect()
            }
            _ => vec![],
        };
    }
    vec![]
}

/// True when the JSON file's value and the one to write are the same kind of bare value.
fn same_kind(now: &[u8], best: Option<&str>) -> bool {
    let word = |v: &str| matches!(v, "true" | "false");
    let now = String::from_utf8_lossy(now);
    word(&now) == best.is_some_and(word) && (word(&now) || now.parse::<f64>().is_ok())
}

/// `hand` plus the lowest settings this file can take: the file has the key (once, in JSON) with the same kind
/// of value, and `hand` does not set it already. `None`: nothing to add. The list is leaked (`Target` wants
/// `&'static`), so it is kept per file until the file changes.
fn merged(path: &Path, format: Format, hand: &'static [Setting], lowest: &[&'static Lowest]) -> Option<&'static [Setting]> {
    type Kept = HashMap<(PathBuf, usize), (u64, Option<std::time::SystemTime>, &'static [Setting])>;
    static KEPT: Mutex<Option<Kept>> = Mutex::new(None);
    let meta = fs::metadata(path).ok().filter(|m| m.is_file() && m.len() <= 2 * 1024 * 1024)?;
    let (stamp, key) = ((meta.len(), meta.modified().ok()), (path.to_path_buf(), hand.as_ptr() as usize));
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    let kept = kept.get_or_insert_with(HashMap::new);
    let list = match kept.get(&key) {
        Some((len, time, list)) if (*len, *time) == stamp => *list,
        _ => {
            let src = fs::read(path).ok()?;
            let mut all = hand.to_vec();
            for l in lowest {
                let s = &l.setting;
                let fits = match format {
                    Format::Json => json_span(&src, s.key).is_some_and(|(a, b, quoted)| quoted == l.text && (quoted || same_kind(&src[a..b], s.best)) && json_span(&src[b..], s.key).is_none()),
                    _ => read(format, &src, std::slice::from_ref(s))[0].is_some(),
                };
                if fits && !hand.iter().any(|h| h.key.eq_ignore_ascii_case(s.key) && h.section.eq_ignore_ascii_case(s.section)) {
                    all.push(*s);
                }
            }
            let list: &'static [Setting] = if all.len() > hand.len() { Box::leak(all.into_boxed_slice()) } else { hand };
            kept.insert(key, (stamp.0, stamp.1, list));
            list
        }
    };
    (list.len() > hand.len()).then_some(list)
}

/// The game's profile with NVIDIA's lowest preset added: a hand-written profile gains the settings it lacks,
/// a game without one gets a profile from the preset alone (no targets until the game has written its file).
fn with_lowest(hand: Option<Profile>, lowest: &'static [Lowest], local: &Path, roaming: &Path) -> Option<Profile> {
    if lowest.is_empty() {
        return hand;
    }
    let mut profile = hand.unwrap_or(Profile { targets: vec![], procs: &[] });
    let mut files: Vec<(&str, Format)> = Vec::new();
    for l in lowest {
        if !files.iter().any(|f| f.0 == l.file) {
            files.push((l.file, l.format));
        }
    }
    for (raw, format) in files {
        let here: Vec<&'static Lowest> = lowest.iter().filter(|l| l.file == raw && l.format == format).collect();
        for path in nvidia_files(raw, local, roaming) {
            let at = profile.targets.iter().position(|t| t.path.as_os_str().eq_ignore_ascii_case(path.as_os_str()));
            if at.is_some_and(|i| profile.targets[i].format != format) {
                continue; // the hand-written profile reads this file another way
            }
            let Some(settings) = merged(&path, format, at.map_or(&[], |i| profile.targets[i].settings), &here) else { continue };
            match at {
                Some(i) => profile.targets[i].settings = settings,
                None => profile.targets.push(Target { path, format, settings }),
            }
        }
    }
    Some(profile)
}

// ---------------------------------------------------------------- the tweak

fn hash(bytes: &[u8]) -> String {
    format!("{:016x}", bytes.iter().fold(0xcbf29ce484222325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100000001b3))) // FNV-1a
}

fn set_readonly(path: &Path, on: bool) -> std::io::Result<()> {
    let mut p = fs::metadata(path)?.permissions();
    #[allow(clippy::permissions_set_readonly_false)] // Windows: this is only the read-only attribute
    p.set_readonly(on);
    fs::set_permissions(path, p)
}

/// `ReplaceFileW` keeps the replaced file's ACL, streams and creation time.
fn swap_in(path: &Path, tmp: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_IGNORE_MERGE_ERRORS};
    let wide = |p: &Path| p.as_os_str().encode_wide().chain([0]).collect::<Vec<u16>>();
    let (to, from) = (wide(path), wide(tmp));
    unsafe { ReplaceFileW(PCWSTR(to.as_ptr()), PCWSTR(from.as_ptr()), PCWSTR::null(), REPLACEFILE_IGNORE_MERGE_ERRORS, None, None) }.map_err(std::io::Error::other)
}

/// Writes through a temp file beside `path`, so a crash leaves the old file or the new one, never half of one.
/// `readonly`: the flag afterwards (`None`: as the file has it now). `modified`: the time to put back.
fn replace_file(path: &Path, bytes: &[u8], readonly: Option<bool>, modified: Option<SystemTime>) -> std::io::Result<()> {
    let tmp = path.with_file_name(format!("{}.mytune-tmp", path.file_name().unwrap_or_default().to_string_lossy()));
    let written = fs::File::create(&tmp).and_then(|mut f| f.write_all(bytes).and_then(|()| f.sync_all()));
    if let Err(e) = written {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    let old = fs::metadata(path).ok().map(|m| m.permissions().readonly());
    if old == Some(true) {
        if let Err(e) = set_readonly(path, false) {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
    }
    // not every drive can do ReplaceFile; a rename is still all or nothing
    let swapped = if old.is_some() { swap_in(path, &tmp).or_else(|_| fs::rename(&tmp, path)) } else { fs::rename(&tmp, path) };
    if swapped.is_err() {
        let _ = fs::remove_file(&tmp);
    } else if let Some(t) = modified {
        let _ = fs::OpenOptions::new().write(true).open(path).and_then(|f| f.set_modified(t));
    }
    if (if swapped.is_ok() { readonly.or(old) } else { old }) == Some(true) {
        let _ = set_readonly(path, true);
    }
    swapped
}

fn slot(b: &TweakBackup, kind: &str, n: usize) -> Option<String> {
    b.data.get(&format!("{kind}{n}")).cloned()
}

/// A file time as stored in the backup: nanoseconds since 1970.
fn time_text(t: SystemTime) -> Option<String> {
    Some(t.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_nanos().to_string())
}

fn time_of(text: &str) -> Option<SystemTime> {
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_nanos(text.parse().ok()?))
}

/// The process's full image path; `None` when Windows does not say (another user's or a protected process).
fn image_path(pid: u32) -> Option<PathBuf> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut n = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n).is_ok();
        let _ = CloseHandle(h);
        ok.then(|| PathBuf::from(String::from_utf16_lossy(&buf[..n as usize])))
    }
}

/// True when this process is the game. Anything running from the game's folder is (an Unreal stub starts another
/// exe there); the game's exe name running from elsewhere is another program. With no path to compare, the name
/// decides. shortcut: helper names like `javaw` have no known path, so any `javaw` still counts for Minecraft.
fn is_game_process(name: &str, image: Option<&Path>, stems: &[String], exe_stem: Option<&str>, dir: Option<&Path>) -> bool {
    if let (Some(i), Some(d)) = (image, dir) {
        if i.starts_with(d) {
            return true;
        }
        if exe_stem.is_some_and(|s| name.eq_ignore_ascii_case(s)) {
            return false;
        }
    }
    stems.iter().any(|s| name.eq_ignore_ascii_case(s))
}

/// True when the game is open. `exe`: its full path when known (empty: names only).
fn game_running(procs: &dyn Procs, stems: &[String], exe: &str) -> Result<bool> {
    let game = Path::new(exe);
    // a drive root is no game folder: everything on the drive would count
    let dir = game.parent().filter(|d| d.parent().is_some()).and_then(|d| fs::canonicalize(d).ok());
    let exe_stem = game.file_stem().and_then(|s| s.to_str());
    Ok(procs.list()?.iter().any(|p| {
        let image = dir.as_ref().and_then(|_| image_path(p.pid)).and_then(|i| fs::canonicalize(i).ok());
        is_game_process(&p.name, image.as_deref(), stems, exe_stem, dir.as_deref())
    }))
}

/// True when the game of a stored backup is open, judged from what the backup holds: `procs` and `exe` for a
/// settings file, the exe path in the value name for a GPU preference.
pub fn backup_game_running(b: &TweakBackup) -> Result<bool> {
    let exe = b.data.get("exe").cloned().or_else(|| b.registry.first().map(|r| r.name.clone())).unwrap_or_default();
    let mut stems: Vec<String> = b.data.get("procs").map(|s| s.split('|').map(String::from).collect()).unwrap_or_default();
    stems.extend(Path::new(&exe).file_stem().map(|s| s.to_string_lossy().to_string()));
    game_running(&crate::procs::WinProcs, &stems, &exe)
}

/// Drops a backup on the user's word (the game is gone): the record first, then the copies it names.
pub fn forget(engine: &crate::engine::Engine, id: &str) -> Result<()> {
    let Some(b) = engine.with_store(|s| s.get_backup(id).cloned()) else { return Ok(()) };
    engine.with_store(|s| s.remove_backup(id))?;
    for copy in (0..).map_while(|n| slot(&b, "backup", n)) {
        let _ = fs::remove_file(copy);
    }
    Ok(())
}

/// After a failed apply the engine may have rolled back and dropped the record: deletes this tweak's copies that no
/// backup names, in memory or on disk (another MyTune may have saved one). Does nothing if state.json is unreadable.
pub fn drop_stray_copies(engine: &crate::engine::Engine, id: &str) {
    let dir = engine.with_store(|s| s.dir().to_path_buf());
    let mut disk = crate::store::Store::load(&dir);
    if disk.ensure_read().is_err() {
        return;
    }
    let prefix = format!("{}-", id.replace(':', "-"));
    engine.with_store(|s| {
        let named = |st: &crate::store::Store, p: &Path| st.applied_ids().iter().filter_map(|i| st.get_backup(i)).any(|b| b.data.values().any(|v| Path::new(v).as_os_str().eq_ignore_ascii_case(p.as_os_str())));
        for f in fs::read_dir(dir.join("gameconfig")).into_iter().flatten().flatten() {
            let path = f.path();
            if f.file_name().to_string_lossy().starts_with(&prefix) && !named(s, &path) && !named(&disk, &path) {
                let _ = fs::remove_file(path);
            }
        }
    });
}

/// True when a file was changed after MyTune wrote it (by the game's own menu, usually). Revert still puts
/// the copy from before the profile back; the caller says so.
pub fn changed_since_apply(b: &TweakBackup) -> bool {
    (0..).map_while(|n| Some((slot(b, "file", n)?, slot(b, "after", n)?))).any(|(file, after)| fs::read(file).is_ok_and(|now| hash(&now) != after))
}

/// A number control's value as written in the file. Whole-number settings round halves up.
pub fn number_text(x: f64, decimals: usize) -> String {
    format!("{:.decimals$}", if decimals == 0 { x.round() } else { x })
}

/// Where a setting lives: (file, section, key).
pub type KeyAt = (PathBuf, &'static str, &'static str);

pub struct ConfigTweak {
    pub id: String,
    pub title: String,
    /// `None`: a game without a profile, or a tweak built only to revert (revert needs the backup alone).
    pub profile: Option<Profile>,
    pub procs: Arc<dyn Procs>,
    /// Process names (no `.exe`) that mean the game is open.
    pub stems: Vec<String>,
    /// The game's exe, for the running check by path. Empty when unknown.
    pub exe: String,
    /// Where the copies of the original files go.
    pub backup_dir: PathBuf,
    /// What to write: (target, setting in the target's list, value as written in the file). `None`: every
    /// setting's competitive value.
    pub values: Option<Vec<(usize, usize, String)>>,
    /// The copies a finished revert no longer needs; deleted once the backup record is saved away.
    spent: Mutex<Vec<String>>,
}

/// A setting of a game as its file has it now.
pub struct Item {
    pub target: usize,
    pub index: usize,
    pub setting: &'static Setting,
    /// `None`: the file lacks the key. Such a row is shown as unavailable; MyTune never adds a key.
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
            exe: exe_path.into(),
            backup_dir: state_dir.join("gameconfig"),
            values: None,
            spent: Mutex::new(vec![]),
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
            out.extend(values.into_iter().enumerate().filter(|(i, v)| v.is_some() || t.settings[*i].show_absent).map(|(index, value)| Item { target, index, setting: &t.settings[index], value }));
        }
        out
    }

    pub fn key_at(&self, target: usize, index: usize) -> Option<KeyAt> {
        let t = self.profile.as_ref()?.targets.get(target)?;
        let s = t.settings.get(index)?;
        Some((t.path.clone(), s.section, s.key))
    }

    /// True when every value still points at the key it had when the dialog read the file (`opened`). The list
    /// is rebuilt when a file changes, so the same (target, index) can name another key by then.
    pub fn keys_match(&self, opened: &[(usize, usize, KeyAt)], values: &[(usize, usize, String)]) -> bool {
        values.iter().all(|v| opened.iter().find(|k| (k.0, k.1) == (v.0, v.1)).is_some_and(|k| self.key_at(v.0, v.1).as_ref() == Some(&k.2)))
    }

    fn edits(&self, target: usize, t: &Target) -> Vec<Edit> {
        let Some(values) = &self.values else { return best_edits(t.settings) };
        // a value that would break the line or the attribute is dropped, whatever sent it
        let ok = |v: &str| !v.is_empty() && !v.contains(['\r', '\n', '"', '<', '>']);
        values.iter().filter(|v| v.0 == target && ok(&v.2)).filter_map(|v| t.settings.get(v.1).map(|s| Edit { section: s.section, key: s.key, value: v.2.clone() })).collect()
    }

    /// For reverting a stored backup by its id alone (Restore page).
    pub fn for_revert(procs: Arc<dyn Procs>, id: &str, title: &str) -> ConfigTweak {
        ConfigTweak { id: id.into(), title: title.into(), profile: None, procs, stems: vec![], exe: String::new(), backup_dir: PathBuf::new(), values: None, spent: Mutex::new(vec![]) }
    }

    fn not_running(&self, stems: &[String], exe: &str) -> Result<()> {
        match game_running(&*self.procs, stems, exe)? {
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
        self.not_running(&self.stems, &self.exe)?;
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
                let base = self.id.replace(':', "-");
                // never over a copy with other bytes: it can be a live backup's only original, and the engine
                // captures again after a failed apply, when this file may already hold MyTune's values
                let free = (0..).map(|k| self.backup_dir.join(if k == 0 { format!("{base}-{n}-{name}") } else { format!("{base}-{n}.{k}-{name}") })).find(|p| fs::read(p).map_or(true, |have| have == src));
                let copy = free.unwrap_or_default();
                if !copy.exists() {
                    fs::write(&copy, &src)?;
                }
                if fs::read(&copy)? != src {
                    return Err(Error::Plain("The copy of the game's settings file does not match the file. Nothing changed.".into()));
                }
                let meta = fs::metadata(&t.path).ok();
                if let Some(m) = &meta {
                    b.data.insert(format!("ro{n}"), if m.permissions().readonly() { "1" } else { "0" }.into());
                }
                if let Some(t) = meta.and_then(|m| m.modified().ok()).and_then(time_text) {
                    b.data.insert(format!("mtime{n}"), t);
                }
                b.data.insert(format!("file{n}"), path);
                b.data.insert(format!("backup{n}"), copy.to_string_lossy().to_string());
            }
            b.data.insert("procs".into(), self.stems.join("|"));
            if !self.exe.is_empty() {
                b.data.insert("exe".into(), self.exe.clone());
            }
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
            replace_file(&path, &new, None, None)?;
            b.data.insert(format!("after{n}"), hash(&new));
        }
        Ok(())
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        let stems: Vec<String> = b.data.get("procs").map(|s| s.split('|').map(String::from).collect()).unwrap_or_default();
        self.not_running(&stems, b.data.get("exe").map_or("", String::as_str))?;
        let (mut fails, mut copies) = (Vec::new(), Vec::new());
        for n in 0.. {
            let (Some(file), Some(copy)) = (slot(b, "file", n), slot(b, "backup", n)) else { break };
            let path = Path::new(&file);
            if !path.parent().is_some_and(Path::is_dir) {
                fails.push(format!("{file}: its folder is missing or offline. The backup copy is kept. If the game was uninstalled, use Forget."));
            } else if !Path::new(&copy).exists() {
                // `on_reverted` deletes a copy only after its record left state.json, so a record without its copy
                // means something else deleted it. A changed file proves nothing: games rewrite theirs on exit.
                fails.push(format!("{file}: the backup copy is gone, nothing restored. If you do not need the original, use Forget."));
            } else {
                let readonly = slot(b, "ro", n).map(|v| v == "1");
                let modified = slot(b, "mtime", n).and_then(|t| time_of(&t));
                match fs::read(&copy).and_then(|orig| replace_file(path, &orig, readonly, modified)) {
                    Ok(()) => copies.push(copy),
                    Err(e) => fails.push(format!("{file}: {e}")),
                }
            }
        }
        if !fails.is_empty() {
            return Err(Error::Revert(fails)); // the copies stay for another try
        }
        *self.spent.lock().unwrap_or_else(|e| e.into_inner()) = copies;
        Ok(())
    }
    /// Runs after the backup left state.json, so a crash before this leaves a stray file, never an Undo without its copy.
    fn on_reverted(&self, _: &mut crate::store::Store) -> Result<()> {
        for c in std::mem::take(&mut *self.spent.lock().unwrap_or_else(|e| e.into_inner())) {
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
        assert_eq!(found, 5);
        assert_eq!(
            changed_lines(UE.as_bytes(), &out),
            ["sg.ShadowQuality=0\r\n", "sg.PostProcessQuality=0\r\n", "sg.EffectsQuality=0\r\n", "MotionBlurMode=Off\r\n", "bUseVSync=False\r\n"]
        );
        let text = String::from_utf8(out.clone()).unwrap();
        assert!(!text.contains("sg.FoliageQuality"), "a key the file lacks is never added");
        assert!(text.contains("FrameRateLimit=0.000000\r\n\r\n[Other]\r\nbUseVSync=True\r\nsg.ShadowQuality=3\r\n"), "[Other] is untouched");
        assert!(text.contains("sg.ViewDistanceQuality=3\r\nsg.AntiAliasingQuality=4") && text.contains("sg.TextureQuality=3") && text.contains("ResolutionSizeX=1920\r\nFullscreenMode=1"));
        assert_eq!(edit(Format::Ini, &out, &best_edits(ARC)).unwrap().0, out, "a second pass changes nothing");
    }

    #[test]
    fn json_changes_only_profile_values() {
        let src = br#"{"setting_quality":"custom","cast_shadow_setting": "low","vsync_setting":true,"x":"vsync_setting","settings_grass":"high","resolution_setting":"1920x1080"}"#;
        let (out, found) = edit(Format::Json, src, &best_edits(ROB_RUN)).unwrap();
        assert_eq!(found, 3);
        assert_eq!(out, br#"{"setting_quality":"custom","cast_shadow_setting": "disabled","vsync_setting":false,"x":"vsync_setting","settings_grass":"low","resolution_setting":"1920x1080"}"#);
        assert_eq!(read(Format::Json, &out, ROB_RUN)[..3], [Some("false".into()), Some("disabled".into()), None]);
        assert_eq!(edit(Format::Json, &out, &best_edits(ROB_RUN)).unwrap().0, out);
    }

    #[test]
    fn ini_keeps_unix_line_ends_missing_last_line_end_and_other_sections() {
        let src = b"[SystemSettings]\nUseVsync=True\nMotionBlur=TRUE\nResX=1920\n[SystemSettingsMobile]\nMotionBlur=TRUE\nBloom=TRUE";
        let (out, found) = edit(Format::Ini, src, &best_edits(ROCKET_LEAGUE)).unwrap();
        assert_eq!(found, 2);
        assert_eq!(out, b"[SystemSettings]\nUseVsync=False\nMotionBlur=False\nResX=1920\n[SystemSettingsMobile]\nMotionBlur=TRUE\nBloom=TRUE");
        // an Unreal file without the section gets nothing added
        assert_eq!(edit(Format::Ini, b"[Other]\r\nA=1\r\n", &best_edits(ARC)).unwrap(), (b"[Other]\r\nA=1\r\n".to_vec(), 0));
        // a last line without a line end keeps that, and the absent keys stay absent
        assert_eq!(edit(Format::Ini, b"[ScalabilityGroups]\nsg.ShadowQuality=2", &best_edits(&ARC[..4])).unwrap().0, b"[ScalabilityGroups]\nsg.ShadowQuality=0");
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
        assert!(profile_in("ROB RUN", "x.exe", l, r, d).is_some(), "ROB RUN has a profile (no files here, so no targets)");
        assert_eq!(path("Tom Clancy's Ghost Recon Wildlands", "x.exe").unwrap(), "D\\My Games\\Ghost Recon Wildlands\\GRW.ini");
        assert_eq!(path("Tom Clancy's Ghost Recon\u{AE} Breakpoint", "x.exe").unwrap(), "D\\My Games\\Ghost Recon Breakpoint\\GRB.ini");
        assert_eq!(path("Elden Ring", "x.exe"), None);
        let gta = |exe| profile_in("Grand Theft Auto V", exe, l, r, d).unwrap().targets[0].settings.len();
        assert_eq!((gta("c:\\GTA5.exe"), gta("c:\\GTA5_Enhanced.exe")), (GTA_LEGACY.len(), GTA.len()), "Enhanced keeps the short list");
    }

    /// GRB.ini as Breakpoint wrote it on this PC (adapter ids taken out).
    const GRB: &str = "[Startup]\r\n[File Version]\r\nIniFileVersion=2\r\n[Vendor]\r\nAdapterIndex=0\r\n[Display Settings]\r\nWindowMode=1\r\nDisplayWidth=1920\r\nDisplayHeight=1080\r\nRefreshRateNumerator=240\r\nRefreshRateDenominator=1\r\nOutputMonitor=0\r\nVSync=0\r\nTemporalInjection=0\r\nFogBlur=1\r\nFpsLimit=0\r\nBackgroundFpsLimit=30\r\nFOV=1.00000\r\nMain Monitor=0\r\nUseHDR=0\r\nUseFreeSync2=0\r\n[Quality And Performance]\r\nEnableAntiAliasing=1\r\nUseSharpening=0\r\nSharpenLevel=0.50000\r\nSupersampling=1.00000\r\nTextureQuality=2\r\nAmbientOcclusion=1\r\nLevelOfDetail=1\r\nAnisotropy=0\r\nTerrainQuality=1\r\nGrassQuality=1\r\nScreenSpaceReflections=1\r\nShadowQuality=1\r\nScreenSpaceShadows=0\r\nMotionBlur=0\r\nBloom=0\r\nVolumetricFog=1\r\nSubSurfaceScattering=0\r\nLongRangeShadows=0\r\nClothSimulation=1\r\n";

    #[test]
    fn breakpoint_ini_gets_nvidias_lowest_and_keeps_display_settings() {
        let (out, found) = edit(Format::Ini, GRB.as_bytes(), &best_edits(BREAKPOINT)).unwrap();
        assert_eq!(found, BREAKPOINT.len() - 1, "every key but anti-aliasing, which has no competitive value");
        assert_eq!(
            changed_lines(GRB.as_bytes(), &out),
            ["TextureQuality=0\r\n", "AmbientOcclusion=0\r\n", "LevelOfDetail=0\r\n", "TerrainQuality=0\r\n", "GrassQuality=0\r\n", "ScreenSpaceReflections=0\r\n", "ShadowQuality=0\r\n", "VolumetricFog=0\r\n", "ClothSimulation=0\r\n"]
        );
        assert!(read(Format::Ini, GRB.as_bytes(), BREAKPOINT).iter().all(Option::is_some), "the real file has every key of the profile");
        assert_eq!(edit(Format::Ini, &out, &best_edits(BREAKPOINT)).unwrap().0, out);
        // never in a profile: window mode, resolution, V-Sync and FPS cap of this file, upscaler, resolution scale
        for s in [BREAKPOINT, WILDLANDS].concat() {
            assert!(s.section == GR_QP && !["Supersampling", "TemporalInjection", "UseSharpening", "SharpenLevel"].contains(&s.key), "{}", s.key);
        }
    }

    #[test]
    fn wildlands_ini_gets_nvidias_lowest() {
        // no GRW.ini on this PC: GRB.ini's shape with the keys NVIDIA's Wildlands wrapper writes
        let src = "[Display Settings]\r\nWindowMode=2\r\nShadowQuality=9\r\n[Quality And Performance]\r\nSupersampling=1.00000\r\nTextureQuality=3\r\nGeometryQuality=3\r\nTerrainQuality=2\r\nGrassQuality=3\r\nEnhancedGrass=1\r\nDrawDistance=3\r\nShadowQuality=4\r\nLongRangeShadows=2\r\nAmbientOcclusion=2\r\nGodRays=2\r\nSubSurfaceScattering=1\r\nHighQualityDOF=1\r\nAntiAliasingMode=2\r\nAnisotropy=16\r\nLensFlare=1\r\n";
        let (out, found) = edit(Format::Ini, src.as_bytes(), &best_edits(WILDLANDS)).unwrap();
        assert_eq!(found, WILDLANDS.len() - 1);
        let want = "[Display Settings]\r\nWindowMode=2\r\nShadowQuality=9\r\n[Quality And Performance]\r\nSupersampling=1.00000\r\nTextureQuality=0\r\nGeometryQuality=0\r\nTerrainQuality=0\r\nGrassQuality=0\r\nEnhancedGrass=0\r\nDrawDistance=0\r\nShadowQuality=0\r\nLongRangeShadows=0\r\nAmbientOcclusion=0\r\nGodRays=0\r\nSubSurfaceScattering=0\r\nHighQualityDOF=0\r\nAntiAliasingMode=0\r\nAnisotropy=0\r\nLensFlare=1\r\n";
        assert_eq!(String::from_utf8(out).unwrap(), want);
    }

    #[test]
    fn rocket_league_world_detail_changes_in_its_own_section_only() {
        // lines of the real TASystemSettings.ini
        let src = "[SystemSettings]\r\nStaticDecals=True\r\nDecalCullDistanceScale=1.000000\r\nDynamicShadows=False\r\nAmbientOcclusion=False\r\nbAllowHighQualityMaterials=True\r\nParticleLODBias=0\r\nDetailMode=2\r\nScreenPercentage=100.000000\r\n\r\n[SystemSettingsSplitScreen2]\r\nDetailMode=1\r\n\r\n[SystemSettingsMobile]\r\nStaticDecals=True\r\n";
        let (out, _) = edit(Format::Ini, src.as_bytes(), &best_edits(ROCKET_LEAGUE)).unwrap();
        assert_eq!(changed_lines(src.as_bytes(), &out), ["StaticDecals=False\r\n", "DecalCullDistanceScale=0.500000\r\n", "bAllowHighQualityMaterials=False\r\n", "ParticleLODBias=1\r\n", "DetailMode=0\r\n"]);
        assert!(String::from_utf8(out).unwrap().ends_with("ScreenPercentage=100.000000\r\n\r\n[SystemSettingsSplitScreen2]\r\nDetailMode=1\r\n\r\n[SystemSettingsMobile]\r\nStaticDecals=True\r\n"));
    }

    #[test]
    fn gta_legacy_gets_the_rest_of_nvidias_lowest() {
        // element names as in the settings.xml on this PC (Enhanced), switches as Legacy's words
        let src = "<Settings>\r\n  <graphics>\r\n    <Tessellation value=\"2\" />\r\n    <LodScale value=\"1.000000\" />\r\n    <ShadowQuality value=\"2\" />\r\n    <ReflectionQuality value=\"2\" />\r\n    <AnisotropicFiltering value=\"16\" />\r\n    <WaterQuality value=\"2\" />\r\n    <ShaderQuality value=\"2\" />\r\n    <Shadow_SoftShadows value=\"5\" />\r\n    <UltraShadows_Enabled value=\"true\" />\r\n    <Shadow_Distance value=\"2.000000\" />\r\n    <Shadow_LongShadows value=\"true\" />\r\n    <CityDensity value=\"0.900000\" />\r\n    <PedVarietyMultiplier value=\"0.900000\" />\r\n    <VehicleVarietyMultiplier value=\"0.900000\" />\r\n    <HdStreamingInFlight value=\"true\" />\r\n    <MaxLodScale value=\"0.300000\" />\r\n    <TextureQuality value=\"2\" />\r\n    <MSAA value=\"4\" />\r\n  </graphics>\r\n  <video>\r\n    <Windowed value=\"0\" />\r\n  </video>\r\n  <Presets>\r\n    <ReflectionQuality value=\"2\" />\r\n  </Presets>\r\n</Settings>\r\n";
        let (out, found) = edit(Format::Xml, src.as_bytes(), &best_edits(GTA_LEGACY)).unwrap();
        assert_eq!(found, 16);
        let want = "<Settings>\r\n  <graphics>\r\n    <Tessellation value=\"0\" />\r\n    <LodScale value=\"0.000000\" />\r\n    <ShadowQuality value=\"1\" />\r\n    <ReflectionQuality value=\"0\" />\r\n    <AnisotropicFiltering value=\"0\" />\r\n    <WaterQuality value=\"0\" />\r\n    <ShaderQuality value=\"0\" />\r\n    <Shadow_SoftShadows value=\"0\" />\r\n    <UltraShadows_Enabled value=\"false\" />\r\n    <Shadow_Distance value=\"1.000000\" />\r\n    <Shadow_LongShadows value=\"false\" />\r\n    <CityDensity value=\"0.000000\" />\r\n    <PedVarietyMultiplier value=\"0.000000\" />\r\n    <VehicleVarietyMultiplier value=\"0.000000\" />\r\n    <HdStreamingInFlight value=\"false\" />\r\n    <MaxLodScale value=\"0.000000\" />\r\n    <TextureQuality value=\"2\" />\r\n    <MSAA value=\"4\" />\r\n  </graphics>\r\n  <video>\r\n    <Windowed value=\"0\" />\r\n  </video>\r\n  <Presets>\r\n    <ReflectionQuality value=\"2\" />\r\n  </Presets>\r\n</Settings>\r\n";
        assert_eq!(String::from_utf8(out).unwrap(), want);
        assert_eq!(GTA_LEGACY[..GTA.len()].iter().map(|s| (s.key, s.best)).collect::<Vec<_>>(), GTA.iter().map(|s| (s.key, s.best)).collect::<Vec<_>>());
    }

    #[test]
    fn minecraft_gets_nvidias_lowest_and_keeps_render_distance() {
        let src = b"ao:true\nbiomeBlendRadius:2\nentityDistanceScaling:1.0\nmipmapLevels:4\nprioritizeChunkUpdates:2\nrenderDistance:12\nsimulationDistance:12\ncloudRange:128\nweatherRadius:10\nrenderClouds:\"true\"\ngraphicsPreset:\"fancy\"\nfullscreen:true\n";
        let (out, _) = edit(Format::Colon, src, &best_edits(MC_JAVA)).unwrap();
        assert_eq!(out, b"ao:false\nbiomeBlendRadius:0\nentityDistanceScaling:0.5\nmipmapLevels:0\nprioritizeChunkUpdates:0\nrenderDistance:12\nsimulationDistance:5\ncloudRange:32\nweatherRadius:3\nrenderClouds:\"true\"\ngraphicsPreset:\"fancy\"\nfullscreen:true\n");
    }

    #[test]
    fn nvidia_lowest_joins_the_profile_for_keys_the_file_has() {
        let d = tempfile::tempdir().unwrap();
        let (local, roaming) = (d.path().join("AppData\\Local"), d.path().join("AppData\\Roaming"));
        let dir = d.path().join("AppData\\LocalLow\\Kefir\\ROB RUN\\settings");
        fs::create_dir_all(&dir).unwrap();
        // hbao: a string where NVIDIA writes true/false; the preset's other keys are not in the file
        let src = br#"{"setting_quality":"custom","anti_aliasing_mode_id_setting":"taa","cast_shadow_setting":"high","global_texture_mipmap_limit":0,"lod_bias_setting":0.600000024,"settings_grass":"high","hbao_setting":"yes","resolution_setting":"1920x1080","vsync_setting":true}"#;
        fs::write(dir.join("settings_steam_4262460_1.dat"), src).unwrap();
        fs::write(dir.join("steam_autocloud.vdf"), b"x").unwrap();
        let (found, _) = crate::nvcache::lowest(&crate::nvcache::tests::rob_run(), crate::nvcache::tests::ROB_POPS.as_bytes());
        let lowest: &'static [Lowest] = Box::leak(lowest_of(found).into_boxed_slice());
        let keys = |p: &Profile| p.targets[0].settings.iter().map(|s| s.key).collect::<Vec<_>>();

        // a game without a hand-written profile
        let p = with_lowest(None, lowest, &local, &roaming).unwrap();
        assert_eq!(p.targets.len(), 1);
        assert_eq!(keys(&p), ["anti_aliasing_mode_id_setting", "cast_shadow_setting", "global_texture_mipmap_limit", "lod_bias_setting", "settings_grass"]);
        let (out, n) = edit(Format::Json, src, &best_edits(p.targets[0].settings)).unwrap();
        assert_eq!(n, 5);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            r#"{"setting_quality":"custom","anti_aliasing_mode_id_setting":"disabled","cast_shadow_setting":"disabled","global_texture_mipmap_limit":2,"lod_bias_setting":0.4,"settings_grass":"low","hbao_setting":"yes","resolution_setting":"1920x1080","vsync_setting":true}"#
        );
        assert!(matches!(p.targets[0].settings[0].kind, Kind::Choice(c) if c == [("Disabled", "disabled"), ("SMAA", "smaa"), ("TAA", "taa")]));
        assert!(matches!(p.targets[0].settings[3].kind, Kind::Number { decimals: 1, .. }));

        // the hand-written profile keeps its settings and gains the ones it lacks
        let p = with_lowest(profile_in("ROB RUN", "x.exe", &local, &roaming, Path::new("D")), lowest, &local, &roaming).unwrap();
        assert_eq!(p.targets.len(), 1);
        let hand: Vec<&str> = ROB_RUN.iter().map(|s| s.key).collect();
        assert_eq!(keys(&p), [hand.as_slice(), &["global_texture_mipmap_limit", "lod_bias_setting"]].concat());
        assert_eq!(p.targets[0].settings[1].best, Some("disabled"));

        // a key that is there twice is not taken; nor is anything while the game has not written its file
        fs::write(dir.join("settings_steam_4262460_1.dat"), br#"{"a":{"lod_bias_setting":1},"lod_bias_setting":0.6,"pad":0}"#).unwrap();
        assert!(with_lowest(None, lowest, &local, &roaming).unwrap().targets.is_empty());
        assert!(with_lowest(None, lowest, &d.path().join("none\\AppData\\Local"), &roaming).unwrap().targets.is_empty());
        assert!(with_lowest(None, &[], &local, &roaming).is_none());
        assert!(nvidia_files("%profile%\\..\\x.dat", &local, &roaming).is_empty() && nvidia_files("%game%\\cfg\\client.cfg", &local, &roaming).is_empty());
        assert_eq!(nvidia_files("%LocalAppData%\\G\\a.ini", &local, &roaming), [local.join("G\\a.ini")]);
    }

    fn tweak(dir: &Path, running: &Arc<Fake>) -> (ConfigTweak, PathBuf) {
        let file = dir.join("GameUserSettings.ini");
        let profile = Profile { targets: vec![Target { path: file.clone(), format: Format::Ini, settings: ARC }, Target { path: dir.join("absent.ini"), format: Format::Ini, settings: ARC }], procs: &[] };
        (ConfigTweak { id: "game:g1:config".into(), title: "G: in-game settings".into(), profile: Some(profile), procs: running.clone(), stems: vec!["Game".into()], exe: String::new(), backup_dir: dir.join("gameconfig"), values: None, spent: Mutex::new(vec![]) }, file)
    }

    #[test]
    fn no_hand_written_profile_holds_an_fps_cap() {
        for s in [ARC, WARDOGS, ROCKET_LEAGUE, BREAKPOINT, WILDLANDS, GTA, GTA_LEGACY, MC_JAVA, MC_BEDROCK, ROB_RUN].concat() {
            assert!(!["FrameRateLimit", "maxFps", "FpsLimit"].contains(&s.key), "{}", s.key);
        }
    }

    #[test]
    fn a_revert_that_cannot_reach_the_file_keeps_copy_and_record_until_forgotten() {
        let d = tempfile::tempdir().unwrap();
        let running = Arc::new(Fake(Mutex::new(vec![])));
        let game = d.path().join("game");
        fs::create_dir_all(&game).unwrap();
        let (t, file) = tweak(&game, &running);
        fs::write(&file, UE).unwrap();
        let engine = crate::engine::Engine::new(crate::store::Store::load(d.path()));
        engine.apply(&t).unwrap();
        let copy = PathBuf::from(engine.with_store(|s| s.get_backup(&t.id).cloned()).unwrap().data["backup0"].clone());
        let gone = ConfigTweak::for_revert(running.clone(), &t.id, &t.title);
        let mut b = engine.with_store(|s| s.get_backup(&t.id).cloned()).unwrap();
        b.data.insert("file0".into(), d.path().join("offline\\GameUserSettings.ini").to_string_lossy().to_string());
        engine.with_store(|s| s.set_backup(&t.id, b)).unwrap();

        assert!(matches!(engine.revert(&gone), Err(Error::Revert(_))));
        assert!(copy.exists() && engine.with_store(|s| s.get_backup(&t.id).is_some()), "nothing is lost");

        forget(&engine, &t.id).unwrap();
        assert!(!copy.exists() && engine.with_store(|s| s.get_backup(&t.id).is_none()));
    }

    #[test]
    fn capture_never_overwrites_another_copy_and_only_strays_are_dropped() {
        let d = tempfile::tempdir().unwrap();
        let running = Arc::new(Fake(Mutex::new(vec![])));
        let (t, file) = tweak(d.path(), &running);
        fs::write(&file, UE).unwrap();
        let engine = crate::engine::Engine::new(crate::store::Store::load(d.path()));
        engine.apply(&t).unwrap();
        let live = PathBuf::from(engine.with_store(|s| s.get_backup(&t.id).cloned()).unwrap().data["backup0"].clone());

        // what the engine's check after a failed apply does: capture again while the file holds other bytes
        fs::write(&file, b"[ScalabilityGroups]\r\nsg.ShadowQuality=3\r\n").unwrap();
        let mut again = TweakBackup::new(&t.title);
        t.capture(&mut again).unwrap();
        let second = PathBuf::from(&again.data["backup0"]);
        assert_ne!(second, live);
        assert_eq!(fs::read(&live).unwrap(), UE.as_bytes(), "the live original is untouched");

        let other = d.path().join("gameconfig\\game-g2-config-0-x.ini");
        fs::write(&other, b"another game's copy").unwrap();
        drop_stray_copies(&engine, &t.id);
        assert!(live.exists() && !second.exists() && other.exists(), "only this tweak's unnamed copy goes");

        engine.with_store(|s| s.remove_backup(&t.id)).unwrap();
        drop_stray_copies(&engine, &t.id);
        assert!(!live.exists() && other.exists(), "no record names it any more");
    }

    #[test]
    fn a_missing_copy_never_counts_as_restored() {
        let d = tempfile::tempdir().unwrap();
        let running = Arc::new(Fake(Mutex::new(vec![])));
        let (t, file) = tweak(d.path(), &running);
        fs::write(&file, UE).unwrap();
        let engine = crate::engine::Engine::new(crate::store::Store::load(d.path()));
        engine.apply(&t).unwrap();
        let copy = PathBuf::from(engine.with_store(|s| s.get_backup(&t.id).cloned()).unwrap().data["backup0"].clone());
        fs::remove_file(&copy).unwrap();

        let undo = ConfigTweak::for_revert(running.clone(), &t.id, &t.title);
        assert!(engine.revert(&undo).is_err(), "the file still has MyTune's values and nothing can restore them");
        assert!(engine.with_store(|s| s.get_backup(&t.id).is_some()));

        fs::write(&file, b"the game rewrote its file on exit").unwrap();
        assert!(matches!(engine.revert(&undo), Err(Error::Revert(_))), "a changed file is no proof the original is back");
        assert!(engine.with_store(|s| s.get_backup(&t.id).is_some()), "Forget stays possible");
    }

    #[test]
    fn replace_keeps_flag_and_time_and_leaves_no_temp_file() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("a.ini");
        fs::write(&file, b"old").unwrap();
        set_readonly(&file, true).unwrap();
        replace_file(&file, b"new", None, None).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"new");
        assert!(fs::metadata(&file).unwrap().permissions().readonly(), "the flag stays");
        let then = SystemTime::UNIX_EPOCH + Duration::from_secs(1_600_000_000);
        replace_file(&file, b"back", Some(false), Some(then)).unwrap();
        let m = fs::metadata(&file).unwrap();
        assert_eq!((m.permissions().readonly(), m.modified().unwrap()), (false, then));
        assert_eq!(time_of(&time_text(then).unwrap()), Some(then));
        assert_eq!(fs::read_dir(d.path()).unwrap().count(), 1, "no temp file left");
    }

    #[test]
    fn the_running_check_goes_by_path_where_it_knows_one() {
        let (stems, dir) = (vec!["Game".to_string(), "javaw".to_string()], Path::new("C:\\Games\\G"));
        let at = |p: &'static str| Some(Path::new(p));
        assert!(is_game_process("Game", at("C:\\Games\\G\\Game.exe"), &stems, Some("Game"), Some(dir)));
        assert!(is_game_process("Game-Win64-Shipping", at("C:\\Games\\G\\Bin\\Game-Win64-Shipping.exe"), &stems, Some("Game"), Some(dir)), "a stub's real exe");
        assert!(!is_game_process("game", at("C:\\Tools\\Game.exe"), &stems, Some("Game"), Some(dir)), "same name, other program");
        assert!(is_game_process("Game", None, &stems, Some("Game"), Some(dir)), "no path readable: the name decides");
        assert!(is_game_process("javaw", at("C:\\Java\\javaw.exe"), &stems, Some("Game"), Some(dir)), "a helper has no known path");
        assert!(!is_game_process("explorer", at("C:\\Windows\\explorer.exe"), &stems, Some("Game"), Some(dir)));
    }

    #[test]
    fn utf8_bom_file_round_trip_keeps_bom_and_untouched_bytes() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("bom.ini");
        fs::write(&file, b"\xEF\xBB\xBF[SystemSettings]\r\nUseVsync=True\r\nResX=1920\r\n; note\r\n[SystemSettingsMobile]\r\nMotionBlur=TRUE").unwrap();
        let src = fs::read(&file).unwrap();
        let at = ROCKET_LEAGUE.iter().position(|s| s.key == "UseVsync").unwrap();
        assert_eq!(read(Format::Ini, &src, ROCKET_LEAGUE)[at].as_deref(), Some("True"));
        let (out, found) = edit(Format::Ini, &src, &best_edits(ROCKET_LEAGUE)).unwrap();
        assert_eq!(found, 1);
        fs::write(&file, &out).unwrap();
        assert_eq!(fs::read(&file).unwrap(), b"\xEF\xBB\xBF[SystemSettings]\r\nUseVsync=False\r\nResX=1920\r\n; note\r\n[SystemSettingsMobile]\r\nMotionBlur=TRUE");
        // a first key without a section (Minecraft's format)
        let colon = b"\xEF\xBB\xBFenableVsync:true\nmaxFps:120\n";
        assert!(read(Format::Colon, colon, MC_JAVA).iter().any(|v| v.as_deref() == Some("true")));
        assert_eq!(edit(Format::Colon, colon, &best_edits(MC_JAVA)).unwrap().0, b"\xEF\xBB\xBFenableVsync:false\nmaxFps:120\n");
    }

    #[test]
    fn number_text_keeps_fractions() {
        assert_eq!(number_text(72.5, 6), "72.500000");
        assert_eq!(number_text(72.5, 0), "73");
        assert_eq!(number_text(120.0, 0), "120");
    }

    #[test]
    fn a_shifted_setting_list_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let running = Arc::new(Fake(Mutex::new(vec![])));
        let (mut t, _) = tweak(d.path(), &running);
        let opened = vec![(0, 0, t.key_at(0, 0).unwrap())];
        let value = |target, index| vec![(target, index, "1".to_string())];
        assert!(t.keys_match(&opened, &value(0, 0)));
        assert!(!t.keys_match(&opened, &value(0, 1)), "not a setting the dialog showed");
        t.profile.as_mut().unwrap().targets[0].settings = &ARC[1..];
        assert!(!t.keys_match(&opened, &value(0, 0)), "same index, other key");
        t.profile.as_mut().unwrap().targets.clear();
        assert!(!t.keys_match(&opened, &value(0, 0)), "target gone");
    }

    #[test]
    fn only_pending_gpu_tweaks_get_a_recovery_revert() {
        let d = tempfile::tempdir().unwrap();
        let mut store = crate::store::Store::load(d.path());
        store.set_backup("game:applied:gpu", TweakBackup::new("applied, not interrupted")).unwrap();
        store.set_top("Pending", serde_json::json!({ "game:a:gpu": { "Title": "A" }, "game:a:config": { "Title": "B" }, "net.x": { "Title": "C" } }));
        let r = crate::pergame::pending_reverts(&store);
        assert_eq!(r.iter().map(|t| (t.id(), t.title())).collect::<Vec<_>>(), [("game:a:config", "B"), ("game:a:gpu", "A")], "journal entries only, catalog ids come from the tweak list");
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
        let reverts = engine.with_store(|s| crate::pergame::pending_reverts(s));
        assert_eq!(reverts.len(), 1);
        let left = engine.recover(|id| reverts.iter().find(|r| r.id() == id).map(|r| &**r));
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
        Edit { section, key, value: value.into() }
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
        assert_eq!((get("MotionBlurMode"), get("bUseVSync")), (Some("On"), Some("True")));
        assert_eq!(read(Format::Ini, b"[SystemSettings]\n UseVsync = TRUE \n", ROCKET_LEAGUE)[at(ROCKET_LEAGUE, "UseVsync")].as_deref(), Some("TRUE"));

        let v = read(Format::Colon, b"version:3955\r\nenableVsync:true\r\nrenderDistance:12\r\nkey_key.attack:key.mouse.left\r\n", MC_JAVA);
        assert_eq!((v[at(MC_JAVA, "enableVsync")].as_deref(), v[at(MC_JAVA, "renderDistance")].as_deref(), v[at(MC_JAVA, "particles")].as_deref()), (Some("true"), Some("12"), None));

        let v = read(Format::Xml, b"<Settings>\r\n <DoF value=\"true\" />\r\n <VSync value=\"2\" />\r\n <MotionBlurStrength value=\"0.500000\" />\r\n</Settings>", GTA);
        assert_eq!((v[at(GTA, "DoF")].as_deref(), v[at(GTA, "VSync")].as_deref(), v[at(GTA, "MotionBlurStrength")].as_deref()), (Some("1"), Some("2"), Some("0.500000")));
        assert_eq!(v[at(GTA, "GrassQuality")], None);
        assert!(read(Format::Ini, &[0xFF, 0xFE, b'[', 0], ARC).iter().all(Option::is_none), "UTF-16 is not read");
    }

    #[test]
    fn chosen_values_are_written_in_each_format() {
        let (out, found) = edit(Format::Ini, UE.as_bytes(), &[e("ScalabilityGroups", "sg.ShadowQuality", "2"), e(EMBARK, "MotionBlurMode", "Off"), e(EMBARK, "bUseVSync", "True")]).unwrap();
        assert_eq!(found, 3);
        assert_eq!(changed_lines(UE.as_bytes(), &out), ["sg.ShadowQuality=2\r\n", "MotionBlurMode=Off\r\n"], "a value the file already has changes nothing");
        assert_eq!(read(Format::Ini, &out, ARC)[at(ARC, "MotionBlurMode")].as_deref(), Some("Off"));

        let src = b"enableVsync:false\nparticles:0\nfov:0.5\n";
        assert_eq!(edit(Format::Colon, src, &[e("", "particles", "1"), e("", "enableVsync", "true")]).unwrap().0, b"enableVsync:true\nparticles:1\nfov:0.5\n");

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
        assert_eq!(shown.len(), ARC.len(), "the missing sg.FoliageQuality is listed too, as unavailable");
        assert!(t.settings()[at(ARC, "sg.FoliageQuality")].value.is_none());

        let t = t.with_values(vec![(0, at(ARC, "sg.ShadowQuality"), "1".into()), (0, at(ARC, "sg.FoliageQuality"), "2".into()), (1, 0, "0".into()), (0, 99, "0".into()), (0, 0, "1\r\nX=1".into())]);
        engine.apply(&t).unwrap();
        let first = fs::read(&file).unwrap();
        assert_eq!(changed_lines(UE.as_bytes(), &first), ["sg.ShadowQuality=1\r\n"], "only the chosen values; an absent key is not added, bad values are dropped");
        let copy = PathBuf::from(engine.with_store(|s| s.get_backup(&t.id).cloned()).unwrap().data["backup0"].clone());
        assert_eq!(fs::read(&copy).unwrap(), UE.as_bytes());

        // the second save starts from the file MyTune already changed
        let t = t.with_values(vec![(0, at(ARC, "MotionBlurMode"), "Off".into()), (0, at(ARC, "bUseVSync"), "False".into())]);
        engine.apply(&t).unwrap();
        let b = engine.with_store(|s| s.get_backup(&t.id).cloned()).unwrap();
        assert_eq!((PathBuf::from(&b.data["backup0"]), b.data.contains_key("backup1")), (copy.clone(), false), "one copy, the first");
        assert_eq!(fs::read(&copy).unwrap(), UE.as_bytes(), "still the file from before MyTune");
        assert_eq!(changed_lines(&first, &fs::read(&file).unwrap()), ["MotionBlurMode=Off\r\n", "bUseVSync=False\r\n"]);
        assert!(!changed_since_apply(&b));
        let now = t.settings();
        assert_eq!((now[at(ARC, "sg.ShadowQuality")].value.as_deref(), now[at(ARC, "MotionBlurMode")].value.as_deref()), (Some("1"), Some("Off")));

        engine.revert(&ConfigTweak::for_revert(running.clone(), &t.id, &t.title)).unwrap();
        assert_eq!(fs::read(&file).unwrap(), UE.as_bytes(), "byte for byte");
        assert!(!copy.exists() && engine.with_store(|s| s.get_backup(&t.id).is_none()));
    }
}
