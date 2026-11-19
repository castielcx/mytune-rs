//! BIOS checklist: reads from Windows what is set in the BIOS and says per setting whether to change it.
//! Read only; `restart_to_firmware` is the one thing here that acts.

use crate::gamewatch::CpuTopology;
use crate::pccheck::{self, reg_str};
use crate::process::{run, run_checked};
use crate::reg::{self, Hive, RegValue};

/// Sort order of the page: what to change first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Change,
    Choice,
    Info,
    Ok,
}

#[derive(Debug, Clone)]
pub struct Row {
    pub title: &'static str,
    pub level: Level,
    pub message: String,
    /// Where the setting usually sits in the BIOS; may be empty.
    pub where_: &'static str,
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub board: String,
    pub bios_version: String,
    pub bios_date: String,
    /// Restarting into the BIOS from Windows works only on UEFI.
    pub uefi: bool,
    pub rows: Vec<Row>,
}

fn row(title: &'static str, level: Level, message: impl Into<String>, where_: &'static str) -> Row {
    Row { title, level, message: message.into(), where_ }
}

const XMP_AT: &str = "Usually on the first BIOS screen or under Tweaker / AI Tweaker / OC: XMP, DOCP or EXPO.";
const BAR_AT: &str = "Advanced > PCI: Above 4G Decoding and Re-Size BAR Support. Needs CSM off.";

/// Slow (starts PowerShell and nvidia-smi): call from a worker thread.
pub fn read() -> Report {
    let key = r"HARDWARE\DESCRIPTION\System\BIOS";
    let s = |n| reg_str(Hive::Hklm, key, n).unwrap_or_default().trim().to_string();
    let board = format!("{} {}", s("BaseBoardManufacturer"), s("BaseBoardProduct")).trim().to_string();
    let bios_date = s("BIOSReleaseDate");
    let uefi = is_uefi();
    let ps = powershell_batch();
    let mut rows = Vec::new();

    if let Some(today) = today() {
        rows.extend(bios_age_row(&bios_date, today));
    }
    rows.extend(memory_rows(&pccheck::parse_mods(&section(&ps, "RAM")), pccheck::on_battery_capable()));
    if let Some(c) = pccheck::resizable_bar() {
        let level = if c.level == pccheck::Level::Ok { Level::Ok } else { Level::Change };
        rows.push(row("Resizable BAR", level, c.message, BAR_AT));
    }
    rows.push(boot_row(uefi));
    if let Some(on) = dword(r"SYSTEM\CurrentControlSet\Control\SecureBoot\State", "UEFISecureBootEnabled") {
        rows.push(secure_boot_row(on == 1));
    }
    rows.extend(tpm_row(&section(&ps, "TPM")));
    let cpu = r"HARDWARE\DESCRIPTION\System\CentralProcessor\0";
    let name = reg_str(Hive::Hklm, cpu, "ProcessorNameString").unwrap_or_default();
    if let Ok(Some(RegValue::Binary(b))) = reg::get(Hive::Hklm, cpu, "Update Revision") {
        rows.extend(microcode_rev(&b).map(|r| microcode_row(name.trim(), r)));
    }
    rows.push(virtualization_row(virtualization_on()));
    let hvci = dword(r"SYSTEM\CurrentControlSet\Control\DeviceGuard\Scenarios\HypervisorEnforcedCodeIntegrity", "Enabled");
    rows.push(memory_integrity_row(hvci == Some(1)));
    rows.extend(cpu_rows(&CpuTopology::detect()));
    rows.extend(igpu_row(&section(&ps, "GPU").lines().collect::<Vec<_>>()));

    rows.sort_by_key(|r| r.level);
    Report { board, bios_version: s("BIOSVersion"), bios_date, uefi, rows }
}

/// Restarts the PC straight into the BIOS setup. UEFI only. Never call it in tests.
pub fn restart_to_firmware() -> crate::Result<()> {
    run_checked("shutdown.exe", &["/r", "/fw", "/t", "0"]).map(|_| ())
}

// ---------------------------------------------------------------- Windows reads

/// One PowerShell start for every CIM read (each start costs about 0.5 s). Lines are tagged "RAM ", "TPM ", "GPU ".
fn powershell_batch() -> String {
    let script = "$ErrorActionPreference='SilentlyContinue'; \
        Get-CimInstance Win32_PhysicalMemory | ForEach-Object { \"RAM $($_.ConfiguredClockSpeed) $($_.SMBIOSMemoryType)\" }; \
        $t = Get-CimInstance -Namespace root/cimv2/Security/MicrosoftTpm -ClassName Win32_Tpm; \
        if ($t) { \"TPM $($t.IsEnabled_InitialValue) $($t.IsActivated_InitialValue)\" }; \
        Get-CimInstance Win32_VideoController | Where-Object { $_.ConfigManagerErrorCode -eq 0 -and $_.Name -notmatch 'Basic|Remote|Virtual|Parsec|Mirage' } | ForEach-Object { \"GPU $($_.Name)\" }";
    run("powershell.exe", &["-NoProfile", "-Command", script]).map(|r| r.output).unwrap_or_default()
}

/// The lines tagged `tag`, without the tag.
fn section(out: &str, tag: &str) -> String {
    out.lines().filter_map(|l| l.strip_prefix(tag)?.strip_prefix(' ')).collect::<Vec<_>>().join("\n")
}

fn dword(path: &str, name: &str) -> Option<u32> {
    match reg::get(Hive::Hklm, path, name).ok()? {
        Some(RegValue::Dword(n)) => Some(n),
        _ => None,
    }
}

fn is_uefi() -> bool {
    use windows::Win32::System::SystemInformation::{FirmwareTypeUefi, GetFirmwareType, FIRMWARE_TYPE};
    let mut t = FIRMWARE_TYPE::default();
    unsafe { GetFirmwareType(&mut t) }.is_ok() && t == FirmwareTypeUefi
}

fn virtualization_on() -> bool {
    use windows::Win32::System::Threading::{IsProcessorFeaturePresent, PF_VIRT_FIRMWARE_ENABLED};
    // with a hypervisor running (VBS, Hyper-V) the firmware flag reads off, CPUID bit 31 says one runs
    let hypervisor = std::arch::x86_64::__cpuid(1).ecx & (1 << 31) != 0;
    hypervisor || unsafe { IsProcessorFeaturePresent(PF_VIRT_FIRMWARE_ENABLED) }.as_bool()
}

fn today() -> Option<(u32, u32, u32)> {
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    (t.wYear > 0).then(|| (t.wYear as u32, t.wMonth as u32, t.wDay as u32))
}

// ---------------------------------------------------------------- verdicts (pure)

/// `date` as Windows stores it ("MM/DD/YYYY"), `today` as (year, month, day).
fn bios_age_row(date: &str, today: (u32, u32, u32)) -> Option<Row> {
    let mut p = date.trim().split('/').map(|x| x.parse::<u32>().ok());
    let (m, d, y) = (p.next()??, p.next()??, p.next()??);
    ((y + 2, m, d) < today).then(|| {
        row(
            "BIOS update",
            Level::Info,
            format!("Your BIOS is from {y}. A newer version may be out: they bring processor fixes and better memory support. Get it from your board maker's website."),
            "",
        )
    })
}

fn memory_rows(mods: &[(u32, u32)], laptop: bool) -> Vec<Row> {
    let Some(first) = mods.first() else { return vec![] };
    let mut rows = vec![];
    let mut speed = pccheck::ram_verdict(mods, laptop);
    if speed.as_ref().is_some_and(|c| c.level == pccheck::Level::Warn) {
        // the single-stick warning: the speed verdict is then asked as if a second stick were there
        rows.push(row("Memory channels", Level::Change, speed.take().unwrap().message, ""));
        speed = pccheck::ram_verdict(&[*first, *first], laptop);
    } else if mods.len() >= 2 {
        rows.push(row("Memory channels", Level::Ok, format!("{} RAM sticks installed, so memory can run dual-channel.", mods.len()), ""));
    }
    if let Some(c) = speed {
        let level = if c.level == pccheck::Level::Ok { Level::Ok } else { Level::Change };
        rows.push(row("Memory profile (XMP / EXPO)", level, c.message, XMP_AT));
    }
    rows
}

fn boot_row(uefi: bool) -> Row {
    if uefi {
        row("Boot mode", Level::Ok, "UEFI, as it should be.", "")
    } else {
        row(
            "Boot mode",
            Level::Change,
            "Legacy (CSM). Resizable BAR, Secure Boot and Windows 11 need UEFI. Switching needs a GPT system drive, so read up before changing.",
            "Boot: CSM / Legacy support off, boot mode UEFI.",
        )
    }
}

fn secure_boot_row(on: bool) -> Row {
    if on {
        row("Secure Boot", Level::Ok, "On. No effect on fps.", "")
    } else {
        row("Secure Boot", Level::Info, "Off. No effect on fps, but Valorant, FACEIT and Battlefield 6 refuse to start without it.", "Boot or Security: Secure Boot. Needs UEFI boot mode.")
    }
}

/// `line`: "<IsEnabled> <IsActivated>" from Win32_Tpm, empty when there is no TPM or it could not be read.
fn tpm_row(line: &str) -> Option<Row> {
    let mut p = line.split_whitespace();
    let enabled = p.next()?.eq_ignore_ascii_case("true");
    let active = p.next().is_some_and(|v| v.eq_ignore_ascii_case("true"));
    Some(if enabled && active {
        row("TPM", Level::Ok, "On. No effect on fps.", "")
    } else {
        row("TPM", Level::Info, "Off. No effect on fps, but Windows 11, Valorant, FACEIT and Battlefield 6 need it.", "Security or Advanced: Intel PTT or AMD fTPM.")
    })
}

/// "Update Revision" data: 8 bytes with the revision in the upper four, or 4 bytes with only the revision.
fn microcode_rev(b: &[u8]) -> Option<u32> {
    let at = match b.len() {
        8.. => 4,
        4..=7 => 0,
        _ => return None,
    };
    Some(u32::from_le_bytes(b[at..at + 4].try_into().ok()?))
}

/// Intel Core 13th or 14th generation for desktops (laptop models end in U, H, HX or P).
fn raptor_desktop(name: &str) -> bool {
    let model = ["i3-", "i5-", "i7-", "i9-"].iter().find_map(|k| name.split(k).nth(1)).map(|m| m.split_whitespace().next().unwrap_or(""));
    match model {
        Some(m) => {
            let digits: String = m.chars().take_while(char::is_ascii_digit).collect();
            let suffix = m[digits.len()..].to_ascii_uppercase();
            let gen = (digits.len() == 5 && (digits.starts_with("13") || digits.starts_with("14"))) || name.contains("13th Gen") || name.contains("14th Gen");
            gen && !suffix.starts_with(['U', 'H', 'P'])
        }
        None => name.contains("13th Gen") || name.contains("14th Gen"),
    }
}

fn microcode_row(name: &str, rev: u32) -> Row {
    let t = "CPU microcode";
    if !raptor_desktop(name) {
        return row(t, Level::Info, format!("Revision 0x{rev:X}. BIOS updates bring newer microcode."), "");
    }
    if rev < 0x12B {
        row(t, Level::Change, format!("Revision 0x{rev:X}. This processor generation can be damaged by too high voltage with old microcode. Update the BIOS: a version with microcode 0x12B or newer."), "")
    } else {
        row(t, Level::Ok, format!("Revision 0x{rev:X}: has the fix for this processor generation's voltage problem."), "")
    }
}

fn virtualization_row(on: bool) -> Row {
    let at = "Advanced > CPU: Intel Virtualization (VT-x / VMX) or SVM Mode on AMD.";
    if on {
        row("Virtualization", Level::Info, "On. Leave it on; turning it off gains nothing by itself.", at)
    } else {
        row("Virtualization", Level::Info, "Off. Turning it on costs nothing by itself; memory integrity, WSL and Android apps need it.", at)
    }
}

fn memory_integrity_row(on: bool) -> Row {
    if on {
        row(
            "Memory integrity",
            Level::Choice,
            "On. Costs a few percent fps in some games, but keeps malware out of the core of Windows, and Valorant can require it. To turn it off: Optimize > Advanced > Disable Virtualization-Based Security.",
            "",
        )
    } else {
        row("Memory integrity", Level::Ok, "Off, so it costs no fps.", "")
    }
}

fn cpu_rows(t: &CpuTopology) -> Vec<Row> {
    if t.cores == 0 {
        return vec![];
    }
    let mut rows = vec![if t.all_mask.count_ones() > t.cores {
        row("Hyper-Threading / SMT", Level::Ok, "On.", "")
    } else {
        row("Hyper-Threading / SMT", Level::Info, "Off, or this processor does not have it. Most games run best with it on.", "Advanced > CPU: Hyper-Threading (Intel) or SMT Mode (AMD).")
    }];
    // a dual-CCD X3D part also reports two core kinds: those are not efficiency cores
    if t.is_hybrid && t.vcache_mask == 0 && t.cores > t.p_cores {
        rows.push(row("Efficiency cores", Level::Ok, format!("{} efficiency cores active. Leave them on.", t.cores - t.p_cores), ""));
    }
    rows
}

fn integrated(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    if n.contains("NVIDIA") {
        return false;
    }
    if n.contains("INTEL") {
        // discrete Arc cards carry a model letter and number (A770, B580)
        let arc = n.split("ARC").nth(1).map(|r| r.trim_start_matches("(TM)").trim_start());
        return !arc.is_some_and(|r| r.len() > 1 && r.starts_with(['A', 'B']) && r.as_bytes()[1].is_ascii_digit());
    }
    n.contains("RADEON") && !n.contains(" RX ") && !n.contains("PRO W")
}

fn igpu_row(names: &[&str]) -> Option<Row> {
    let names: Vec<&str> = names.iter().map(|n| n.trim()).filter(|n| !n.is_empty()).collect();
    (names.iter().any(|n| integrated(n)) && names.iter().any(|n| !integrated(n))).then(|| {
        row("Integrated graphics", Level::Info, "Both the processor's graphics and the graphics card are active. That is fine. Make sure the monitor cable is in the graphics card, not the mainboard.", "")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn microcode() {
        assert_eq!(microcode_rev(&[0, 0, 0, 0, 0x2B, 0x01, 0, 0]), Some(0x12B));
        assert_eq!(microcode_rev(&[0x37, 0x01, 0, 0]), Some(0x137));
        assert_eq!(microcode_rev(&[1, 2]), None);
        let k = "13th Gen Intel(R) Core(TM) i5-13600K";
        assert!(raptor_desktop(k));
        assert!(raptor_desktop("Intel(R) Core(TM) i7-14700KF"));
        assert!(raptor_desktop("13th Gen Intel(R) Core(TM) i5-13400"));
        assert!(!raptor_desktop("13th Gen Intel(R) Core(TM) i7-13700H"));
        assert!(!raptor_desktop("13th Gen Intel(R) Core(TM) i9-13980HX"));
        assert!(!raptor_desktop("13th Gen Intel(R) Core(TM) i5-1335U"));
        assert!(!raptor_desktop("12th Gen Intel(R) Core(TM) i5-12600K"));
        assert!(!raptor_desktop("AMD Ryzen 7 7800X3D 8-Core Processor"));
        assert_eq!(microcode_row(k, 0x129).level, Level::Change);
        assert_eq!(microcode_row(k, 0x12B).level, Level::Ok);
        assert_eq!(microcode_row("AMD Ryzen 7 7800X3D", 0xA601206).level, Level::Info);
    }

    #[test]
    fn memory_integrity_and_age() {
        assert_eq!(memory_integrity_row(true).level, Level::Choice);
        assert_eq!(memory_integrity_row(false).level, Level::Ok);
        assert!(bios_age_row("08/19/2026", (2026, 11, 19)).is_none());
        assert!(bios_age_row("11/20/2024", (2026, 11, 19)).is_none());
        assert!(bios_age_row("11/18/2024", (2026, 11, 19)).is_some());
        assert!(bios_age_row("", (2026, 11, 19)).is_none());
    }

    #[test]
    fn memory_tpm_gpu() {
        let r = memory_rows(&[(3200, 26), (3200, 26)], false);
        assert_eq!(r.iter().map(|r| r.level).collect::<Vec<_>>(), [Level::Ok, Level::Ok]);
        let r = memory_rows(&[(4800, 34)], false);
        assert_eq!(r.iter().map(|r| (r.title, r.level)).collect::<Vec<_>>(), [("Memory channels", Level::Change), ("Memory profile (XMP / EXPO)", Level::Change)]);
        assert!(memory_rows(&[], false).is_empty());
        assert_eq!(tpm_row("True True").unwrap().level, Level::Ok);
        assert_eq!(tpm_row("False False").unwrap().level, Level::Info);
        assert!(tpm_row("").is_none());
        assert_eq!(section("RAM 1 2\nTPM True True\nGPU A\nGPU B", "GPU"), "A\nB");
        assert!(igpu_row(&["Intel(R) UHD Graphics 770", "NVIDIA GeForce RTX 4070"]).is_some());
        assert!(igpu_row(&["AMD Radeon(TM) Graphics", "AMD Radeon RX 7800 XT"]).is_some());
        assert!(igpu_row(&["Intel(R) Arc(TM) A770 Graphics"]).is_none());
        assert!(igpu_row(&["NVIDIA GeForce RTX 4070"]).is_none());
    }

    #[test]
    #[ignore]
    fn real() {
        let r = read();
        println!("Board: {}\nBIOS version: {}\nBIOS date: {}\nUEFI: {}", r.board, r.bios_version, r.bios_date, r.uefi);
        for x in &r.rows {
            println!("[{:?}] {}: {} | {}", x.level, x.title, x.message, x.where_);
        }
    }
}
