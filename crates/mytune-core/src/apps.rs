//! Auto Debloat: the preinstalled Store (Appx) apps MyTune offers to uninstall.
//!
//! Not a general uninstaller: only apps in `CATALOG` are listed, and `remove` refuses everything else, so a
//! caller cannot remove the Store, winget, a framework or a codec by passing its id. Removal is for the
//! current user only (no `-AllUsers`, no provisioned package): the app can be installed again from the Store.

use crate::cleaner;
use crate::log;
use crate::special::{powershell, ps_quote};
use crate::tweaks::Cmd;
use crate::Error;
use serde::Deserialize;

/// (name pattern, shown name, reason, warning). An empty warning = suggested (ticked by default); with a
/// warning the app is optional (listed, not ticked). Patterns match the package name without case:
/// `*x` contains, `x*` starts with, otherwise equal.
type Entry = (&'static str, &'static str, &'static str, &'static str);

const AD: &str = "Promoted app that came with Windows.";
const XBOX: &str = "Game Pass and some games need this.";
// not ticked by default: Windows cannot tell a preinstalled stub from an app the user installed
const OWN: &str = "Remove only if you did not install it yourself.";
const TEAMS: &str = "Also the work and school Teams on new Windows versions.";

#[rustfmt::skip]
const CATALOG: &[Entry] = &[
    ("Clipchamp.Clipchamp", "Clipchamp", "Video editor that came with Windows.", ""),
    ("Microsoft.MicrosoftSolitaireCollection", "Solitaire Collection", "Card games with ads.", ""),
    ("Microsoft.BingNews", "News", "News feed app.", ""),
    ("Microsoft.BingWeather", "Weather", "Weather app.", ""),
    ("Microsoft.BingFinance", "Money", "Finance news app.", ""),
    ("Microsoft.BingSports", "Sports", "Sports news app.", ""),
    ("Microsoft.BingSearch", "Bing Search", "Web search in the taskbar search box.", ""),
    ("Microsoft.Bing*", "Bing app", "Bing content app.", ""),
    ("Microsoft.GetHelp", "Get Help", "Microsoft's support chat.", ""),
    ("Microsoft.WindowsFeedbackHub", "Feedback Hub", "Sends feedback to Microsoft.", ""),
    ("Microsoft.Getstarted", "Tips", "Windows tips.", ""),
    ("MSTeams", "Microsoft Teams", "Chat app that came with Windows.", TEAMS),
    ("MicrosoftTeams", "Microsoft Teams", "Chat app that came with Windows.", TEAMS),
    ("Microsoft.MixedReality.Portal", "Mixed Reality Portal", "For VR headsets Windows no longer supports.", ""),
    ("Microsoft.Microsoft3DViewer", "3D Viewer", "Viewer for 3D models.", ""),
    ("Microsoft.MSPaint", "Paint 3D", "Discontinued 3D paint app.", ""),
    ("Microsoft.MicrosoftOfficeHub", "Microsoft 365 (Office)", "Shortcut to Office on the web, not Office itself.", ""),
    ("Microsoft.SkypeApp", "Skype", "Discontinued chat app.", ""),
    ("Microsoft.People", "People", "Contacts app.", ""),
    ("Microsoft.WindowsMaps", "Maps", "Discontinued maps app.", ""),
    ("Microsoft.PowerAutomateDesktop", "Power Automate", "Office automation tool.", ""),
    ("MicrosoftCorporationII.QuickAssist", "Quick Assist", "Remote help tool.", ""),
    ("Microsoft.Windows.DevHome", "Dev Home", "Dashboard for developers.", ""),
    ("*CandyCrush", "Candy Crush", AD, ""),
    ("*Disney", "Disney+", AD, OWN),
    ("*SpotifyMusic", "Spotify", AD, OWN),
    ("*TikTok", "TikTok", AD, OWN),
    ("*Instagram", "Instagram", AD, OWN),
    ("*Facebook", "Facebook", AD, OWN),
    ("*LinkedIn", "LinkedIn", AD, OWN),
    ("*PrimeVideo", "Prime Video", AD, OWN),
    ("*Netflix", "Netflix", AD, OWN),
    ("Microsoft.GamingApp", "Xbox", "Xbox and Game Pass app.", XBOX),
    ("Microsoft.XboxGamingOverlay", "Game Bar", "Win+G overlay.", "Game Bar, its recording and some controller features stop working."),
    ("Microsoft.XboxGameOverlay", "Game Bar plugin", "Part of the Game Bar.", "Game Bar features stop working."),
    ("Microsoft.XboxIdentityProvider", "Xbox Identity Provider", "Signs games in to Xbox.", XBOX),
    ("Microsoft.GamingServices", "Gaming Services", "Installs and runs Xbox app games.", XBOX),
    ("Microsoft.Copilot", "Copilot", "Microsoft's AI assistant.", "Remove only if you do not use Copilot."),
    ("Microsoft.OneDriveSync", "OneDrive", "Cloud file sync.", "Files stop syncing with OneDrive."),
    ("Microsoft.YourPhone", "Phone Link", "Shows your phone on the PC.", "Remove only if you do not link a phone."),
    ("MicrosoftWindows.Client.WebExperience", "Widgets", "The Widgets board (Win+W).", "Widgets and the taskbar weather stop working."),
    ("Microsoft.549981C3F5F10", "Cortana", "Discontinued voice assistant.", "Remove only if you do not use it."),
    ("Microsoft.OutlookForWindows", "Outlook (new)", "Mail and calendar.", "Remove only if you read your mail elsewhere."),
    ("Microsoft.MicrosoftStickyNotes", "Sticky Notes", "Desktop notes.", "Notes that are not synced are lost."),
    ("Microsoft.Todos", "To Do", "Task lists.", "Remove only if you do not use it."),
    ("Microsoft.ZuneMusic", "Media Player", "Plays music and video files.", "Keep it unless you have another player."),
    ("Microsoft.ZuneVideo", "Movies & TV", "Video player and store.", "Keep it unless you have another player."),
    ("Microsoft.WindowsSoundRecorder", "Sound Recorder", "Records from the microphone.", "Remove only if you do not use it."),
    ("Microsoft.WindowsCamera", "Camera", "Takes pictures with a webcam.", "Remove only if you do not use it."),
    ("Microsoft.WindowsCalculator", "Calculator", "The Windows calculator.", "Most people want to keep this."),
    ("Microsoft.Windows.Photos", "Photos", "Opens pictures.", "Keep it unless you have another picture viewer."),
    ("Microsoft.ScreenSketch", "Snipping Tool", "Screenshots (Win+Shift+S).", "Win+Shift+S stops working."),
    ("Microsoft.WindowsNotepad", "Notepad", "Text editor.", "Most people want to keep this."),
    ("Microsoft.Paint", "Paint", "Picture editor.", "Remove only if you do not use it."),
];

/// Never listed and never removed, whatever the catalog says: the Store and winget, frameworks, codecs,
/// parts of the Windows shell, Edge, driver control panels, language packs and MyTune.
#[rustfmt::skip]
const PROTECTED: &[&str] = &[
    "Microsoft.WindowsStore", "Microsoft.StorePurchaseApp", "Microsoft.DesktopAppInstaller", "Microsoft.Winget*",
    "Microsoft.Services.Store*",
    "Microsoft.VCLibs*", "Microsoft.NET.*", "Microsoft.UI.Xaml*", "Microsoft.WindowsAppRuntime*", "*WinAppRuntime",
    "Microsoft.DirectX*",
    "*VideoExtension", "*ImageExtension", "Microsoft.WebMediaExtensions",
    "Microsoft.SecHealthUI", "Microsoft.Windows.ShellExperienceHost", "Microsoft.Windows.StartMenuExperienceHost",
    "MicrosoftWindows.Client.CBS", "MicrosoftWindows.Client.Core",
    "Microsoft.MicrosoftEdge*", "Microsoft.Win32WebViewHost", "*WebView2",
    "NVIDIACorp.*", "AdvancedMicroDevicesInc*", "AppUp.Intel*", "RealtekSemiconductorCorp.*",
    "Microsoft.LanguageExperiencePack*",
    "*MyTune", "Microsoft.WindowsTerminal*",
];

fn like(pattern: &str, name: &str) -> bool {
    let (p, n) = (pattern.to_ascii_lowercase(), name.to_ascii_lowercase());
    if let Some(part) = p.strip_prefix('*') {
        n.contains(part)
    } else if let Some(start) = p.strip_suffix('*') {
        n.starts_with(start)
    } else {
        p == n
    }
}

/// The catalog entry of a package, `None` when it is protected or not in the catalog.
fn offer(name: &str, framework: bool, locked: bool) -> Option<&'static Entry> {
    if framework || locked || PROTECTED.iter().any(|p| like(p, name)) {
        return None;
    }
    CATALOG.iter().find(|e| like(e.0, name))
}

/// One installed app MyTune offers to remove.
#[derive(Debug, Clone, PartialEq)]
pub struct App {
    /// Package name, e.g. `Microsoft.BingNews`.
    pub name: String,
    /// What `remove` takes.
    pub full_name: String,
    pub family: String,
    pub publisher: String,
    pub title: &'static str,
    pub reason: &'static str,
    /// Empty for a suggested app; an optional app says here what is lost.
    pub warning: &'static str,
    /// Size of the install folder, `None` when it cannot be read.
    pub size: Option<u64>,
}

impl App {
    pub fn suggested(&self) -> bool {
        self.warning.is_empty()
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Raw {
    name: String,
    package_full_name: String,
    #[serde(default)]
    package_family_name: Option<String>,
    #[serde(default)]
    publisher: Option<String>,
    #[serde(default)]
    is_framework: Option<bool>,
    #[serde(default)]
    non_removable: Option<bool>,
    #[serde(default)]
    sig: Option<String>,
    #[serde(default)]
    install_location: Option<String>,
}

// PowerShell 5.1 prints one result as a bare object, several as an array.
#[derive(Deserialize)]
#[serde(untagged)]
enum Rows {
    Many(Vec<Raw>),
    One(Raw),
}

/// The offered apps in `Get-AppxPackage ... | ConvertTo-Json` output (with their install folder), suggested
/// ones first, then by name.
fn parse(json: &str) -> Result<Vec<(App, Option<String>)>, String> {
    let t = json.trim().trim_start_matches('\u{FEFF}');
    if t.is_empty() {
        return Ok(Vec::new());
    }
    let rows = match serde_json::from_str::<Rows>(t).map_err(|e| format!("Unexpected app list: {e}"))? {
        Rows::Many(v) => v,
        Rows::One(r) => vec![r],
    };
    let mut out: Vec<(App, Option<String>)> = rows
        .into_iter()
        .filter_map(|r| {
            let locked = r.non_removable.unwrap_or(false) || r.sig.as_deref() == Some("System");
            let e = offer(&r.name, r.is_framework.unwrap_or(false), locked)?;
            let app = App {
                name: r.name,
                full_name: r.package_full_name,
                family: r.package_family_name.unwrap_or_default(),
                publisher: r.publisher.unwrap_or_default(),
                title: e.1,
                reason: e.2,
                warning: e.3,
                size: None,
            };
            Some((app, r.install_location))
        })
        .collect();
    out.sort_by_key(|(a, _)| (!a.suggested(), a.title));
    Ok(out)
}

const LIST: &str = "[Console]::OutputEncoding=[Text.Encoding]::UTF8; Get-AppxPackage | Select-Object Name,PackageFullName,PackageFamilyName,Publisher,IsFramework,NonRemovable,@{n='Sig';e={\"$($_.SignatureKind)\"}},InstallLocation | ConvertTo-Json -Compress";

/// The current user's installed apps that are in the catalog. Read only. Takes a few seconds: call it on a thread.
pub fn list(cmd: &dyn Cmd) -> Result<Vec<App>, String> {
    let out = powershell(cmd, LIST).map_err(|e| e.to_string())?;
    Ok(parse(&out)?
        .into_iter()
        .map(|(mut app, folder)| {
            app.size = folder.map(|f| cleaner::size_specs(&[f])).filter(|&b| b > 0);
            app
        })
        .collect())
}

/// Uninstalls one app for the current user. `Err` is the reason for the user. Anything that is not an offered
/// catalog app is refused before a command runs; the script checks Windows' own flags again.
pub fn remove(cmd: &dyn Cmd, full_name: &str) -> Result<(), String> {
    // full name = Name_Version_Architecture_ResourceId_PublisherId
    let name = full_name.split('_').next().unwrap_or("");
    let plain = full_name.chars().all(|c| c.is_ascii_alphanumeric() || "._-~".contains(c));
    if !plain || !full_name.contains('_') || offer(name, false, false).is_none() {
        log::info(&format!("Auto Debloat: refused {full_name}"));
        return Err("MyTune does not remove this app.".into());
    }
    let script = format!(
        "try {{ $p = Get-AppxPackage | Where-Object {{ $_.PackageFullName -eq {} }}; if ($p) {{ if ($p.IsFramework -or $p.NonRemovable -or \"$($p.SignatureKind)\" -eq 'System') {{ throw 'Windows does not allow removing this app.' }}; Remove-AppxPackage -Package $p.PackageFullName }} }} catch {{ Write-Output $_.Exception.Message; exit 1 }}",
        ps_quote(full_name)
    );
    match powershell(cmd, &script) {
        Ok(_) => {
            log::info(&format!("Auto Debloat: removed {full_name}"));
            Ok(())
        }
        Err(e) => {
            let text = match &e {
                Error::ProcessFailed { output, .. } if !output.trim().is_empty() => output.trim().to_string(),
                _ => e.to_string(),
            };
            log::error(&format!("Auto Debloat: {full_name} not removed"), Some(&text));
            // Windows adds lines about activity ids and event logs: the first line is the reason
            Err(text.lines().next().unwrap_or("Failed").chars().take(200).collect())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::RunResult;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn kind(name: &str) -> Option<bool> {
        offer(name, false, false).map(|e| e.3.is_empty())
    }

    #[test]
    fn catalog_sorts_apps_into_suggested_optional_protected() {
        for n in ["Clipchamp.Clipchamp", "Microsoft.BingNews", "Microsoft.BingWallpaper", "microsoft.gethelp", "Microsoft.MSPaint", "king.com.CandyCrushSaga"] {
            assert_eq!(kind(n), Some(true), "{n} should be suggested");
        }
        for n in ["Microsoft.GamingApp", "Microsoft.GamingServices", "Microsoft.XboxGamingOverlay", "Microsoft.Paint", "Microsoft.WindowsNotepad", "MicrosoftWindows.Client.WebExperience", "Microsoft.Copilot", "MSTeams", "MicrosoftTeams", "SpotifyAB.SpotifyMusic", "4DF9E0F8.Netflix", "Disney.37853FC22B2CE", "AmazonVideo.PrimeVideo", "BytedancePte.Ltd.TikTok", "Facebook.InstagramBeta", "FACEBOOK.FACEBOOK", "7EE7776C.LinkedInforWindows"] {
            assert_eq!(kind(n), Some(false), "{n} should be optional");
        }
        for n in [
            "Microsoft.WindowsStore", "Microsoft.StorePurchaseApp", "Microsoft.DesktopAppInstaller", "Microsoft.VCLibs.140.00", "Microsoft.NET.Native.Runtime.2.2",
            "Microsoft.UI.Xaml.2.8", "Microsoft.WindowsAppRuntime.1.5", "MicrosoftCorporationII.WinAppRuntime.Main.1.8", "Microsoft.HEVCVideoExtension", "Microsoft.HEIFImageExtension",
            "Microsoft.VP9VideoExtensions", "Microsoft.AV1VideoExtension", "Microsoft.WebpImageExtension", "Microsoft.WebMediaExtensions", "Microsoft.RawImageExtension",
            "Microsoft.SecHealthUI", "Microsoft.Windows.ShellExperienceHost", "Microsoft.Windows.StartMenuExperienceHost", "Microsoft.MicrosoftEdge.Stable", "Microsoft.Win32WebViewHost",
            "NVIDIACorp.NVIDIAControlPanel", "AdvancedMicroDevicesInc-2.AMDRadeonSoftware", "AppUp.IntelGraphicsExperience", "RealtekSemiconductorCorp.RealtekAudioControl",
            "Microsoft.LanguageExperiencePacken-US", "Someone.MyTune",
        ] {
            assert!(PROTECTED.iter().any(|p| like(p, n)), "{n} should be protected");
            assert_eq!(kind(n), None);
        }
        // not in the catalog: never offered
        for n in ["OpenAI.Codex", "Microsoft.MinecraftUWP", "Microsoft.Xbox.TCUI", ""] {
            assert_eq!(kind(n), None, "{n}");
        }
    }

    #[test]
    fn frameworks_and_locked_packages_are_dropped() {
        assert!(offer("Clipchamp.Clipchamp", true, false).is_none());
        assert!(offer("Clipchamp.Clipchamp", false, true).is_none());
        assert!(offer("Clipchamp.Clipchamp", false, false).is_some());
    }

    #[test]
    fn no_catalog_pattern_reaches_a_protected_name() {
        for e in CATALOG {
            let bare = e.0.trim_matches('*');
            assert!(!PROTECTED.iter().any(|p| like(p, bare)), "{} is protected", e.0);
            assert!(!e.1.is_empty() && !e.2.is_empty());
        }
    }

    struct Fake(AtomicUsize, i32);
    impl Cmd for Fake {
        fn run(&self, _: &str, _: &[&str]) -> crate::Result<RunResult> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(RunResult { exit_code: self.1, output: "Removal failed.\nNOTE: see the event log".into() })
        }
    }

    #[test]
    fn remove_refuses_protected_and_unlisted_ids_before_any_command() {
        let cmd = Fake(AtomicUsize::new(0), 0);
        for id in [
            "Microsoft.WindowsStore_22410.1401.3.0_x64__8wekyb3d8bbwe",
            "Microsoft.DesktopAppInstaller_1.24.25200.0_x64__8wekyb3d8bbwe",
            "Microsoft.VCLibs.140.00_14.0.33519.0_x64__8wekyb3d8bbwe",
            "Microsoft.HEVCVideoExtension_2.2.9.0_x64__8wekyb3d8bbwe",
            "Microsoft.SecHealthUI_1000.27840.1000.0_x64__8wekyb3d8bbwe",
            "NVIDIACorp.NVIDIAControlPanel_8.1.968.0_x64__56jybvy8sckqj",
            "OpenAI.Codex_1.0.0.0_x64__2p2nqsd0c76g0",
            "Microsoft.BingNews",
            "Microsoft.BingNews_1.0.0.0_x64__8wekyb3d8bbwe'; Remove-Item C:\\ -Recurse; '",
            "",
        ] {
            assert!(remove(&cmd, id).is_err(), "{id}");
        }
        assert_eq!(cmd.0.load(Ordering::SeqCst), 0, "a refused id must not run a command");
    }

    #[test]
    fn remove_runs_one_command_for_a_catalog_app_and_reports_the_first_line() {
        let ok = Fake(AtomicUsize::new(0), 0);
        assert_eq!(remove(&ok, "Microsoft.BingNews_4.55.62231.0_x64__8wekyb3d8bbwe"), Ok(()));
        assert_eq!(ok.0.load(Ordering::SeqCst), 1);
        let bad = Fake(AtomicUsize::new(0), 1);
        assert_eq!(remove(&bad, "Microsoft.BingNews_4.55.62231.0_x64__8wekyb3d8bbwe"), Err("Removal failed.".into()));
    }

    const NEWS: &str = r#"{"Name":"Microsoft.BingNews","PackageFullName":"Microsoft.BingNews_4.55.62231.0_x64__8wekyb3d8bbwe","PackageFamilyName":"Microsoft.BingNews_8wekyb3d8bbwe","Publisher":"CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond, S=Washington, C=US","IsFramework":false,"NonRemovable":false,"Sig":"Store","InstallLocation":"C:\\Program Files\\WindowsApps\\Microsoft.BingNews_4.55.62231.0_x64__8wekyb3d8bbwe"}"#;

    #[test]
    fn parses_one_object_and_an_array() {
        let one = parse(NEWS).unwrap();
        assert_eq!(one.len(), 1);
        let (app, folder) = &one[0];
        assert_eq!((app.title, app.suggested()), ("News", true));
        assert_eq!(app.family, "Microsoft.BingNews_8wekyb3d8bbwe");
        assert!(app.publisher.starts_with("CN=Microsoft") && folder.as_deref().unwrap().ends_with("8wekyb3d8bbwe"));

        let many = format!(
            "\u{FEFF}[{NEWS},{},{},{},{},{}]",
            // optional
            r#"{"Name":"Microsoft.GamingApp","PackageFullName":"Microsoft.GamingApp_1_x64__8wekyb3d8bbwe","PackageFamilyName":"Microsoft.GamingApp_8wekyb3d8bbwe","Publisher":"CN=Microsoft","IsFramework":false,"NonRemovable":false,"Sig":"Store","InstallLocation":null}"#,
            // protected by name, by the framework flag, by NonRemovable, and not in the catalog
            r#"{"Name":"Microsoft.WindowsStore","PackageFullName":"Microsoft.WindowsStore_1_x64__8wekyb3d8bbwe","PackageFamilyName":"f","Publisher":"p","IsFramework":false,"NonRemovable":false,"Sig":"Store","InstallLocation":"x"}"#,
            r#"{"Name":"Microsoft.VCLibs.140.00","PackageFullName":"Microsoft.VCLibs.140.00_1_x64__8wekyb3d8bbwe","PackageFamilyName":"f","Publisher":"p","IsFramework":true,"NonRemovable":false,"Sig":"Store","InstallLocation":"x"}"#,
            r#"{"Name":"Clipchamp.Clipchamp","PackageFullName":"Clipchamp.Clipchamp_1_x64__yxz26nhyzhsrt","PackageFamilyName":"f","Publisher":"p","IsFramework":false,"NonRemovable":true,"Sig":"Store","InstallLocation":"x"}"#,
            r#"{"Name":"OpenAI.Codex","PackageFullName":"OpenAI.Codex_1_x64__2p2nqsd0c76g0","PackageFamilyName":"f","Publisher":"p","IsFramework":false,"NonRemovable":false,"Sig":"Store","InstallLocation":"x"}"#,
        );
        let names: Vec<_> = parse(&many).unwrap().into_iter().map(|(a, _)| (a.name.clone(), a.suggested())).collect();
        assert_eq!(names, [("Microsoft.BingNews".to_string(), true), ("Microsoft.GamingApp".to_string(), false)]);

        assert!(parse("  ").unwrap().is_empty());
        assert!(parse("not json").is_err());
    }

    /// Read only: lists this PC's apps. `cargo test -p mytune-core --lib apps::tests::real -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_list() {
        for a in list(&*crate::tweaks::system_cmd()).unwrap() {
            println!("{} | {} | suggested {} | {:?} | {}", a.title, a.full_name, a.suggested(), a.size, a.warning);
        }
    }
}
