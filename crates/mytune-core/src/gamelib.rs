//! Game library scan for the Game Hub: which games are installed, each real game once.
//!
//! Sources: Steam (libraries + `appmanifest_*.acf`), Epic (`.item` manifests), Ubisoft Connect and Battle.net
//! (uninstall keys), GOG, EA, Rockstar (their registry keys), Riot (`ProgramData\Riot Games\Metadata`) and the
//! Xbox app (`<drive>:\XboxGames\*\Content\MicrosoftGame.config`). Registry and files only, no programs run.
//! `scan` walks install folders: call it on a thread. `merge` adds what is new to state.json's `Games` list
//! (same schema as the C# app: Id, Name, ExePath, PCoresOnly, ...) and never touches an existing Id.

use crate::reg::{self, Hive, RegValue};
use crate::store::Store;
use crate::Result;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub name: String,
    pub launcher: &'static str,
    pub dir: PathBuf,
    /// The game's own program; `None` when none was found in the folder (such a game is not added).
    pub exe: Option<PathBuf>,
    pub uri: Option<String>,
}

// ---------------------------------------------------------------- real-game filter

/// Steam apps that are not games: Steamworks redistributables, Wallpaper Engine, Crosshair X, SteamVR,
/// Steam Linux Runtime (three), Proton Experimental.
const NOT_GAMES: &[u32] = &[228980, 431960, 1366800, 250820, 1070560, 1391110, 1628350, 1493710];
const NOT_GAME_WORDS: &[&str] = &[
    "redistributable", "steamworks", "steamvr", "proton", "steam linux runtime", "server", "sdk", "launcher", "soundtrack",
    "benchmark", "wallpaper", "crosshair", "anti-cheat", "anticheat", "battleye", "social club", "mod tools",
    "ubisoft connect", "battle.net", "online services", "ea app", "gog galaxy",
];

/// shortcut: a deny list by Steam app id and name, so an unknown tool with a plain name is still listed as a
/// game (and a game with "server" or "sdk" in its name is dropped); read the app type from Steam's
/// appinfo.vdf when that happens. Epic and Xbox entries also need their store's own "this is a game" mark.
pub fn is_game(name: &str, steam_id: Option<u32>) -> bool {
    let n = name.to_lowercase();
    !n.trim().is_empty() && !steam_id.is_some_and(|i| NOT_GAMES.contains(&i)) && !NOT_GAME_WORDS.iter().any(|w| n.contains(w))
}

// ---------------------------------------------------------------- one entry per game

/// Words that tell editions, stores and marks apart, not games.
const EDITION_WORDS: &[&str] = &[
    "edition", "legacy", "enhanced", "launcher", "java", "bedrock", "for", "windows", "demo", "goty", "game", "of", "the", "year",
    "complete", "deluxe", "ultimate", "standard", "gold", "premium", "definitive", "tom", "clancy", "s", "tm", "r",
];
const ALIASES: &[(&str, &str)] = &[
    ("gta v", "grand theft auto v"),
    ("gta 5", "grand theft auto v"),
    ("gtav", "grand theft auto v"),
    ("gta5", "grand theft auto v"),
    ("grand theft auto 5", "grand theft auto v"),
    ("rocketleague", "rocket league"),
];

/// The key two copies of one game share: lowercase words without marks, punctuation and edition words.
pub fn title_key(name: &str) -> String {
    let plain: String = name.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { ' ' }).collect();
    let words: Vec<&str> = plain.split_whitespace().collect();
    let kept: Vec<&str> = words.iter().copied().filter(|w| !EDITION_WORDS.contains(w)).collect();
    let key = if kept.is_empty() { words.join(" ") } else { kept.join(" ") };
    ALIASES.iter().find(|a| a.0 == key).map_or(key, |a| a.1.to_string())
}

fn new_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    let h = || std::collections::hash_map::RandomState::new().build_hasher().finish(); // randomly keyed
    format!("{:016x}{:016x}", h(), h())
}

/// Adds the found games that `games` does not have yet; returns how many. A found game is the same as a
/// listed one when the exe path or the title key matches, so editions, a second store and a rescan add nothing.
/// A listed game whose exe is gone gets the found path (moved or reinstalled); nothing else of it changes.
pub fn merge(games: &mut Vec<Value>, found: &[Found]) -> usize {
    let mut added = 0;
    for f in found {
        let Some(exe) = &f.exe else { continue };
        let (exe_s, dir_s, key) = (exe.to_string_lossy().to_string(), f.dir.to_string_lossy().to_string(), title_key(&f.name));
        let hit = games.iter_mut().find(|g| {
            g["ExePath"].as_str().is_some_and(|p| p.eq_ignore_ascii_case(&exe_s)) || g["Name"].as_str().is_some_and(|n| title_key(n) == key)
        });
        match hit {
            Some(g) => {
                if !g["ExePath"].as_str().is_some_and(|p| Path::new(p).is_file()) && exe.is_file() {
                    g["ExePath"] = json!(exe_s);
                    g["InstallDir"] = json!(dir_s);
                }
            }
            None => {
                games.push(json!({
                    "Id": new_id(), "Name": f.name, "ExePath": exe_s, "PCoresOnly": false, "InstallDir": dir_s,
                    "Launcher": f.launcher, "LaunchUri": f.uri, "Favorite": false, "PlaySeconds": 0,
                }));
                added += 1;
            }
        }
    }
    added
}

/// Changes the `Games` list and saves. The list is read from disk first: the game watcher writes play time
/// there from its own copy of the state, and this copy's older list would undo that.
pub fn update_games<R>(store: &mut Store, f: impl FnOnce(&mut Vec<Value>) -> R) -> Result<R> {
    let mut games = Store::load(store.dir()).top("Games").and_then(Value::as_array).cloned().unwrap_or_default();
    let r = f(&mut games);
    store.set_top("Games", Value::Array(games));
    store.save()?;
    Ok(r)
}

// ---------------------------------------------------------------- the game's own exe

const HELPERS: &[&str] = &[
    "crash", "launcher", "unins", "easyanticheat", "eac_", "beservice", "battleye", "redist", "dxsetup", "setup", "install", "helper",
    "report", "updat", "start_protected_game", "dotnet", "cef", "webview", "overlay", "handler", "sentry", "benchmark", "playgtav",
    "server",
];
const SKIP_DIRS: &[&str] = &["_commonredist", "redist", "easyanticheat", "battleye", "directx", "support", "__installer", "extras", "thirdparty", "plugins"];

fn is_helper(stem: &str) -> bool {
    HELPERS.iter().any(|h| stem.contains(h))
}

/// shortcut: the store's own exe when it names one that is not a launcher stub, else a pick by name and size
/// within 4 folder levels (Unity player, then Unreal `-Shipping`, then the largest); a game whose real
/// program is small or deeper gets the wrong exe and is never seen running. Add a per-game table then.
pub fn main_exe(dir: &Path, hint: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = hint.map(|h| dir.join(h.replace('/', "\\"))) {
        if p.is_file() && !p.file_stem().is_some_and(|s| is_helper(&s.to_string_lossy().to_lowercase())) {
            return Some(p);
        }
    }
    let (mut best, mut budget) = (None, 20_000u32);
    walk(dir, 0, &mut budget, &mut best);
    best.map(|b: (u8, u64, PathBuf)| b.2)
}

fn walk(dir: &Path, depth: u8, budget: &mut u32, best: &mut Option<(u8, u64, PathBuf)>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if *budget == 0 {
            return;
        }
        *budget -= 1;
        let (p, name) = (e.path(), e.file_name().to_string_lossy().to_lowercase());
        let Ok(kind) = e.file_type() else { continue };
        if kind.is_dir() {
            if depth < 3 && !SKIP_DIRS.contains(&name.as_str()) {
                walk(&p, depth + 1, budget, best);
            }
        } else if let Some(stem) = name.strip_suffix(".exe").filter(|s| !is_helper(s)) {
            // Unity's player has a `<name>_Data` folder beside it; Unreal's game binary ends in -Shipping
            let unity = p.file_stem().is_some_and(|s| p.with_file_name(format!("{}_Data", s.to_string_lossy())).is_dir());
            let rank = if unity { 2 } else if stem.ends_with("-shipping") { 1 } else { 0 };
            let size = e.metadata().map(|m| m.len()).unwrap_or(0);
            if best.as_ref().map_or(true, |b| (rank, size) > (b.0, b.1)) {
                *best = Some((rank, size, p));
            }
        }
    }
}

// ---------------------------------------------------------------- store files

/// Every value of `key` in Valve's text format (`"key"  "value"` on one line, at any depth).
/// shortcut: a value with an escaped quote is cut short; no Steam name or path seen has one.
pub fn vdf_values(text: &str, key: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| {
            let mut q = l.split('"'); // "", key, blanks, value, ""
            q.nth(1)?.eq_ignore_ascii_case(key).then_some(())?;
            Some(q.nth(1)?.replace("\\\\", "\\"))
        })
        .collect()
}

/// (app id, name, install folder name) of a fully installed app.
pub fn steam_app(acf: &str) -> Option<(u32, String, String)> {
    let v = |k| vdf_values(acf, k).into_iter().next();
    let flags: u32 = v("StateFlags")?.parse().ok()?;
    (flags & 4 != 0).then_some(())?; // 4 = fully installed
    Some((v("appid")?.parse().ok()?, v("name")?, v("installdir")?))
}

/// (name, install folder, launch exe, app name) of a finished install that Epic files under "games".
pub fn epic_item(item: &str) -> Option<(String, String, String, String)> {
    let v: Value = serde_json::from_str(item).ok()?;
    let game = v["AppCategories"].as_array()?.iter().any(|c| c == "games");
    (game && v["bIsIncompleteInstall"].as_bool() != Some(true)).then_some(())?;
    let s = |k: &str| v[k].as_str().map(String::from);
    Some((s("DisplayName")?, s("InstallLocation")?, s("LaunchExecutable").unwrap_or_default(), s("AppName").unwrap_or_default()))
}

/// The value of `attr="..."` in an XML text (MicrosoftGame.config).
fn attr(xml: &str, attr: &str) -> Option<String> {
    let at = xml.find(&format!("{attr}=\""))? + attr.len() + 2;
    Some(xml[at..at + xml[at..].find('"')?].to_string())
}

// ---------------------------------------------------------------- scan

fn reg_str(path: &str, name: &str) -> Option<String> {
    match reg::get(Hive::Hklm, path, name).ok()?? {
        RegValue::String(s) | RegValue::ExpandString(s) => Some(s).filter(|s| !s.trim().is_empty()),
        _ => None,
    }
}

fn push(out: &mut Vec<Found>, launcher: &'static str, name: &str, dir: &str, hint: Option<&str>, uri: Option<String>, steam_id: Option<u32>) {
    let dir = PathBuf::from(dir.trim().replace('/', "\\").trim_end_matches('\\'));
    if is_game(name, steam_id) && dir.is_dir() {
        let exe = main_exe(&dir, hint);
        out.push(Found { name: name.replace(['\u{AE}', '\u{2122}', '\u{A9}'], "").trim().to_string(), launcher, dir, exe, uri });
    }
}

fn steam(out: &mut Vec<Found>) {
    let Ok(Some(RegValue::String(root))) = reg::get(Hive::Hkcu, "Software\\Valve\\Steam", "SteamPath") else { return };
    let mut libs = vec![root.clone()];
    if let Ok(t) = fs::read_to_string(Path::new(&root).join("steamapps\\libraryfolders.vdf")) {
        libs.extend(vdf_values(&t, "path"));
    }
    let mut seen = Vec::new();
    for lib in libs {
        let apps = PathBuf::from(lib.replace('/', "\\")).join("steamapps");
        let low = apps.to_string_lossy().to_lowercase();
        if seen.contains(&low) {
            continue;
        }
        seen.push(low);
        for e in fs::read_dir(&apps).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().to_lowercase();
            if !(n.starts_with("appmanifest_") && n.ends_with(".acf")) {
                continue;
            }
            if let Some((id, name, folder)) = fs::read_to_string(e.path()).ok().as_deref().and_then(steam_app) {
                push(out, "Steam", &name, &apps.join("common").join(folder).to_string_lossy(), None, Some(format!("steam://rungameid/{id}")), Some(id));
            }
        }
    }
}

fn epic(out: &mut Vec<Found>) {
    let dir = program_data().join("Epic\\EpicGamesLauncher\\Data\\Manifests");
    for e in fs::read_dir(dir).into_iter().flatten().flatten() {
        if e.path().extension().is_some_and(|x| x == "item") {
            if let Some((name, at, exe, app)) = fs::read_to_string(e.path()).ok().as_deref().and_then(epic_item) {
                push(out, "Epic", &name, &at, Some(&exe), Some(format!("com.epicgames.launcher://apps/{app}?action=launch&silent=true")), None);
            }
        }
    }
}

/// Ubisoft Connect and Battle.net games, from the uninstall list.
fn uninstall_list(out: &mut Vec<Found>) {
    for root in ["SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall", "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall"] {
        for k in reg::subkeys(Hive::Hklm, root).unwrap_or_default() {
            let path = format!("{root}\\{k}");
            let un = reg_str(&path, "UninstallString").unwrap_or_default().to_lowercase();
            let launcher = if k.starts_with("Uplay Install ") {
                "Ubisoft"
            } else if un.contains("battle.net") && un.contains("--uid=") {
                "Battle.net"
            } else {
                continue;
            };
            if let (Some(name), Some(dir)) = (reg_str(&path, "DisplayName"), reg_str(&path, "InstallLocation")) {
                push(out, launcher, &name, &dir, None, None, None);
            }
        }
    }
}

/// GOG, EA and Rockstar keep one registry key per game.
fn registry_stores(out: &mut Vec<Found>) {
    let gog = "SOFTWARE\\WOW6432Node\\GOG.com\\Games";
    for k in reg::subkeys(Hive::Hklm, gog).unwrap_or_default() {
        let path = format!("{gog}\\{k}");
        if reg_str(&path, "dependsOn").is_some() {
            continue; // an add-on of another game
        }
        if let (Some(name), Some(dir)) = (reg_str(&path, "gameName"), reg_str(&path, "path")) {
            let exe = reg_str(&path, "exe").and_then(|e| Path::new(&e).file_name().map(|f| f.to_string_lossy().to_string()));
            push(out, "GOG", &name, &dir, exe.as_deref(), None, None);
        }
    }
    for root in ["SOFTWARE\\WOW6432Node\\EA Games", "SOFTWARE\\EA Games"] {
        for k in reg::subkeys(Hive::Hklm, root).unwrap_or_default() {
            let path = format!("{root}\\{k}");
            if let Some(dir) = reg_str(&path, "Install Dir") {
                push(out, "EA", &reg_str(&path, "DisplayName").unwrap_or(k), &dir, None, None, None);
            }
        }
    }
    let rockstar = "SOFTWARE\\WOW6432Node\\Rockstar Games";
    for k in reg::subkeys(Hive::Hklm, rockstar).unwrap_or_default() {
        if let Some(dir) = reg_str(&format!("{rockstar}\\{k}"), "InstallFolder") {
            push(out, "Rockstar", &k, &dir, None, None, None);
        }
    }
}

/// shortcut: only Riot's two big games are known by name and exe; add a row for another one.
const RIOT: &[(&str, &str, &str)] = &[
    ("valorant", "VALORANT", "ShooterGame\\Binaries\\Win64\\VALORANT-Win64-Shipping.exe"),
    ("league_of_legends", "League of Legends", "Game\\League of Legends.exe"),
];

fn riot(out: &mut Vec<Found>) {
    for e in fs::read_dir(program_data().join("Riot Games\\Metadata")).into_iter().flatten().flatten() {
        let product = e.file_name().to_string_lossy().to_lowercase();
        let Some(known) = RIOT.iter().find(|r| product.starts_with(r.0)) else { continue };
        let Ok(yaml) = fs::read_to_string(e.path().join(format!("{product}.product_settings.yaml"))) else { continue };
        if let Some(dir) = yaml.lines().find_map(|l| l.trim().strip_prefix("product_install_full_path:")) {
            push(out, "Riot", known.1, dir.trim().trim_matches('"'), Some(known.2), None, None);
        }
    }
}

/// shortcut: only the Xbox app's default folder name on each drive; a renamed games folder (`.GamingRoot`)
/// and Store apps under WindowsApps are not seen.
fn xbox(out: &mut Vec<Found>) {
    for drive in 'C'..='Z' {
        for e in fs::read_dir(format!("{drive}:\\XboxGames")).into_iter().flatten().flatten() {
            let content = e.path().join("Content");
            let Ok(cfg) = fs::read_to_string(content.join("MicrosoftGame.config")) else { continue };
            let name = attr(&cfg, "DefaultDisplayName").filter(|n| !n.starts_with("ms-resource")).unwrap_or_else(|| e.file_name().to_string_lossy().to_string());
            let exe = cfg.find("<Executable ").and_then(|at| attr(&cfg[at..], "Name"));
            push(out, "Xbox", &name, &content.to_string_lossy(), exe.as_deref(), None, None);
        }
    }
}

fn program_data() -> PathBuf {
    PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into()))
}

/// Every installed game the stores know, tools and launchers left out. Copies of one game are still in
/// here; `merge` folds them. Reads folders: not for the UI thread.
pub fn scan() -> Vec<Found> {
    let mut out = Vec::new();
    steam(&mut out);
    epic(&mut out);
    uninstall_list(&mut out);
    registry_stores(&mut out);
    riot(&mut out);
    xbox(&mut out);
    out.sort_by_key(|f| f.name.to_lowercase().contains("demo")); // the full game wins over its demo
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(name: &str, launcher: &'static str, exe: &str) -> Found {
        Found { name: name.into(), launcher, dir: PathBuf::from("C:\\G"), exe: Some(PathBuf::from(exe)), uri: None }
    }

    #[test]
    fn title_keys_fold_editions_marks_and_aliases() {
        for n in ["Minecraft Launcher", "Minecraft: Java Edition", "Minecraft for Windows", "Minecraft- Java Edition", "Minecraft Bedrock"] {
            assert_eq!(title_key(n), "minecraft", "{n}");
        }
        for n in ["Grand Theft Auto V Legacy", "Grand Theft Auto V Enhanced", "GTA V", "GTA5", "Grand Theft Auto V"] {
            assert_eq!(title_key(n), "grand theft auto v", "{n}");
        }
        assert_eq!(title_key("Rocket League\u{AE}"), "rocket league");
        assert_eq!(title_key("Tom Clancy's Ghost Recon\u{AE} Breakpoint"), title_key("Ghost Recon Breakpoint"));
        assert_ne!(title_key("Tom Clancy's Ghost Recon Wildlands"), title_key("Ghost Recon Breakpoint"));
        assert_eq!(title_key("The Game"), "the game", "a title of edition words only keeps them");
    }

    #[test]
    fn copies_of_one_game_become_one_entry() {
        let mut games = Vec::new();
        let scan = [
            found("Grand Theft Auto V Legacy", "Steam", "C:\\S\\GTA5.exe"),
            found("Grand Theft Auto V", "Rockstar", "D:\\R\\GTA5.exe"),
            found("Minecraft Launcher", "Xbox", "C:\\X\\Minecraft.exe"),
            found("Minecraft: Java Edition", "Xbox", "C:\\X\\MinecraftJava.exe"),
            found("Minecraft for Windows", "Xbox", "C:\\X\\Minecraft.Windows.exe"),
            Found { exe: None, ..found("No Exe Game", "Steam", "") },
        ];
        assert_eq!(merge(&mut games, &scan), 2);
        let names: Vec<&str> = games.iter().map(|g| g["Name"].as_str().unwrap()).collect();
        assert_eq!(names, ["Grand Theft Auto V Legacy", "Minecraft Launcher"]);
        assert_eq!(games[0]["Id"].as_str().unwrap().len(), 32);
        assert_ne!(games[0]["Id"], games[1]["Id"]);
        assert_eq!(games[0]["PCoresOnly"], false);
    }

    #[test]
    fn rescan_keeps_existing_entries_and_ids() {
        let mut games: Vec<Value> = serde_json::from_str(
            r#"[{"Id":"95c9ad235291483fb4fa42734f1ba15c","Name":"ARC Raiders","ExePath":"c:\\steam\\Arc Raiders\\PioneerGame.exe","PCoresOnly":true,"PlaySeconds":66},
                {"Id":"324f2f16174046248f85f38d75e1799a","Name":"Minecraft for Windows","ExePath":"C:\\XboxGames\\Minecraft for Windows\\Content\\Minecraft.Windows.exe","PCoresOnly":false}]"#,
        )
        .unwrap();
        let before = games.clone();
        let scan = [
            found("ARC Raiders", "Steam", "C:\\steam\\Arc Raiders\\PioneerGame\\Binaries\\Win64\\PioneerGame-Win64-Shipping.exe"), // by name
            found("Renamed By Store", "Xbox", "c:\\xboxgames\\minecraft for windows\\content\\minecraft.windows.exe"),              // by exe
            found("Minecraft: Java Edition", "Xbox", "C:\\X\\MinecraftJava.exe"),
            found("ROB RUN", "Steam", "C:\\steam\\Rob Run\\ROB RUN.exe"),
        ];
        assert_eq!(merge(&mut games, &scan), 1);
        assert_eq!(&games[..2], &before[..], "existing entries are untouched");
        assert_eq!(games[2]["Name"], "ROB RUN");
        assert_eq!(merge(&mut games, &scan), 0, "a second scan adds nothing");
    }

    #[test]
    fn filter_drops_tools_and_keeps_games() {
        assert!(!is_game("Steamworks Common Redistributables", Some(228980)));
        assert!(!is_game("Wallpaper Engine", Some(431960)));
        assert!(!is_game("Crosshair X", Some(1366800)));
        assert!(!is_game("Renamed", Some(1366800)), "the app id alone is enough");
        for n in ["SteamVR", "Proton 9.0", "Steam Linux Runtime 3.0 (sniper)", "DayZ Server", "Source SDK Base 2013", "Rockstar Games Launcher", "Rockstar Games Social Club", "Some Game Soundtrack", "3DMark Benchmark", "Ubisoft Connect", "Battle.net", ""] {
            assert!(!is_game(n, None), "{n}");
        }
        for n in ["ARC Raiders", "WARDOGS", "Grand Theft Auto V Legacy", "ROB RUN", "Rocket League", "Minecraft for Windows"] {
            assert!(is_game(n, Some(1)), "{n}");
        }
    }

    const ACF: &str = "\"AppState\"\n{\n\t\"appid\"\t\t\"4262460\"\n\t\"name\"\t\t\"ROB RUN\"\n\t\"StateFlags\"\t\t\"6\"\n\t\"installdir\"\t\t\"Rob Run\"\n\t\"UserConfig\"\n\t{\n\t\t\"language\"\t\t\"english\"\n\t}\n}\n";

    #[test]
    fn steam_files_parse() {
        assert_eq!(steam_app(ACF), Some((4262460, "ROB RUN".into(), "Rob Run".into())));
        assert_eq!(steam_app(&ACF.replace("\"6\"", "\"1026\"")), None, "still downloading");
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Program Files (x86)\\\\Steam\"\n\t\t\"apps\"\n\t\t{\n\t\t\t\"228980\"\t\t\"255260445\"\n\t\t}\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"D:\\\\SteamLibrary\"\n\t}\n}\n";
        assert_eq!(vdf_values(vdf, "path"), ["C:\\Program Files (x86)\\Steam", "D:\\SteamLibrary"]);
    }

    #[test]
    fn epic_item_needs_the_games_category() {
        let item = r#"{"AppName":"Sugar","DisplayName":"Rocket League®","InstallLocation":"C:\\Program Files\\Epic Games\\rocketleague","LaunchExecutable":"Binaries/Win64/Launcher.exe","AppCategories":["public","games","applications"],"bIsIncompleteInstall":false}"#;
        let (name, dir, exe, app) = epic_item(item).unwrap();
        assert_eq!((name.as_str(), dir.as_str(), exe.as_str(), app.as_str()), ("Rocket League®", "C:\\Program Files\\Epic Games\\rocketleague", "Binaries/Win64/Launcher.exe", "Sugar"));
        assert!(epic_item(&item.replace("\"games\",", "\"engines\",")).is_none());
        assert!(epic_item(&item.replace("\"bIsIncompleteInstall\":false", "\"bIsIncompleteInstall\":true")).is_none());
    }

    #[test]
    fn xbox_config_attributes() {
        let cfg = "<Game configVersion=\"1\">\n  <Identity Name=\"Microsoft.MinecraftUWP\" />\n  <ShellVisuals DefaultDisplayName=\"Minecraft for Windows\"\n      Description=\"Minecraft\"/>\n  <ExecutableList>\n    <Executable Name=\"Minecraft.Windows.exe\" Id=\"Game\"/>\n  </ExecutableList>\n</Game>";
        assert_eq!(attr(cfg, "DefaultDisplayName").as_deref(), Some("Minecraft for Windows"));
        assert_eq!(cfg.find("<Executable ").and_then(|at| attr(&cfg[at..], "Name")).as_deref(), Some("Minecraft.Windows.exe"));
    }

    /// Reads this PC, writes nothing: `cargo test -p mytune-core --lib gamelib::tests::real_scan -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn real_scan() {
        use crate::engine::Tweak;
        let found = scan();
        for f in &found {
            println!("FOUND {} | {} | {:?}", f.name, f.launcher, f.exe);
        }
        let store = Store::load_default();
        let mut games = store.top("Games").and_then(Value::as_array).cloned().unwrap_or_default();
        println!("ADDED {}", merge(&mut games, &found));
        for g in &games {
            let (name, exe) = (g["Name"].as_str().unwrap_or(""), g["ExePath"].as_str().unwrap_or(""));
            let t = crate::gameprofile::ConfigTweak::new(std::sync::Arc::new(crate::procs::WinProcs), g["Id"].as_str().unwrap_or(""), name, exe, store.dir());
            println!("GAME {name} | {exe} | profile={} files={:?} applied={:?}", t.profile.is_some(), t.files(), t.is_applied().ok());
        }
    }

    #[test]
    fn main_exe_skips_stubs_and_helpers() {
        let d = tempfile::tempdir().unwrap();
        let file = |rel: &str, len: usize| {
            let p = d.path().join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, vec![0u8; len]).unwrap();
        };
        // Rocket League: the store names the launcher stub
        file("Binaries/Win64/Launcher.exe", 900);
        file("Binaries/Win64/RocketLeague.exe", 500);
        file("Binaries/Win64/EasyAntiCheat/EasyAntiCheat_Setup.exe", 5000);
        file("UnityCrashHandler64.exe", 4000);
        assert_eq!(main_exe(d.path(), Some("Binaries/Win64/Launcher.exe")).unwrap().file_name().unwrap(), "RocketLeague.exe");
        assert_eq!(main_exe(d.path(), Some("Binaries/Win64/RocketLeague.exe")).unwrap().file_name().unwrap(), "RocketLeague.exe");
        // Unreal: the small -Shipping binary wins over a bigger plain exe; Unity's player wins over both
        file("Game/Binaries/Win64/Game-Win64-Shipping.exe", 100);
        assert_eq!(main_exe(d.path(), None).unwrap().file_name().unwrap(), "Game-Win64-Shipping.exe");
        file("ROB RUN.exe", 10);
        file("start_protected_game.exe", 9000);
        fs::create_dir_all(d.path().join("ROB RUN_Data")).unwrap();
        assert_eq!(main_exe(d.path(), None).unwrap().file_name().unwrap(), "ROB RUN.exe");
        assert_eq!(main_exe(&d.path().join("ROB RUN_Data"), None), None);
    }
}
