//! Internet page, "Network repair" card (C# `NetRepairVM`) and the net.* tweak list.
//!
//! Buttons: flush DNS, renew IP, register DNS, reset Winsock, reset TCP/IP, all of them. Winsock and
//! TCP/IP resets need a restart. `netsh int ip reset` often fails on Windows 11 (protected parts of the
//! stack), so on failure the separate IPv4 / IPv6 resets run instead, like C#.
//! Differences from C#: programs go through `Cmd` (fake in tests, default 120 s timeout instead of 90 s).

use crate::catalog::{self, TweakFlags};
use crate::engine::Tweak;
use crate::error::Result;
use crate::process::system32;
use crate::special::{adapter_tweaks, ProtocolBindingsTweak};
use crate::tweaks::Cmd;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub label: &'static str,
    pub file: &'static str,
    pub args: &'static str,
    pub restart: bool,
}

const FLUSH: Step = Step { label: "Flush DNS cache", file: "ipconfig.exe", args: "/flushdns", restart: false };
const RELEASE: Step = Step { label: "Release IP address", file: "ipconfig.exe", args: "/release", restart: false };
const RENEW: Step = Step { label: "Renew IP address", file: "ipconfig.exe", args: "/renew", restart: false };
const REGISTER: Step = Step { label: "Register DNS", file: "ipconfig.exe", args: "/registerdns", restart: false };
const WINSOCK: Step = Step { label: "Reset Winsock", file: "netsh.exe", args: "winsock reset", restart: true };
const TCPIP: Step = Step { label: "Reset TCP/IP", file: "netsh.exe", args: "int ip reset", restart: true };

/// Keys: flushdns, renew, registerdns, winsock, tcpip, all.
pub fn steps(key: &str) -> Vec<Step> {
    match key {
        "flushdns" => vec![FLUSH],
        "renew" => vec![RELEASE, RENEW],
        "registerdns" => vec![REGISTER],
        "winsock" => vec![WINSOCK],
        "tcpip" => vec![TCPIP],
        "all" => vec![FLUSH, RELEASE, RENEW, REGISTER, WINSOCK, TCPIP],
        _ => vec![],
    }
}

/// The first lines of what the command printed, indented, so a failure says why.
fn detail(output: &str) -> String {
    let t: Vec<String> = output.lines().map(str::trim).filter(|l| !l.is_empty()).take(6).map(|l| format!("   {l}")).collect();
    if t.is_empty() {
        String::new()
    } else {
        t.join("\n") + "\n"
    }
}

/// Runs the steps for `key`, calling `log` with each line chunk. Returns true when a restart is needed.
pub fn repair(cmd: &dyn Cmd, key: &str, log: &mut dyn FnMut(&str)) -> bool {
    let mut restart = false;
    let exec = |label: &str, file: &str, args: &str, log: &mut dyn FnMut(&str)| -> bool {
        log(&format!("▸ {label}…\n"));
        let exe = system32(file).to_string_lossy().into_owned();
        let parts: Vec<&str> = args.split(' ').collect();
        match cmd.run(&exe, &parts) {
            Ok(r) if r.exit_code == 0 => {
                log("   Done.\n");
                true
            }
            Ok(r) => {
                log(&format!("   Failed ({}).\n{}", r.exit_code, detail(&r.output)));
                false
            }
            Err(e) => {
                log(&format!("   Failed ({e}).\n"));
                false
            }
        }
    };
    for s in steps(key) {
        let ok = exec(s.label, s.file, s.args, log);
        restart |= s.restart && ok;
        if !ok && s.args == "int ip reset" {
            for (name, fallback) in [("IPv4", "int ipv4 reset"), ("IPv6", "int ipv6 reset")] {
                restart |= exec(&format!("Reset {name} instead"), "netsh.exe", fallback, log);
            }
        }
    }
    if restart {
        log("Restart your PC to finish the reset.\n");
    }
    restart
}

/// A net.* tweak with the text the catalog gives it.
pub struct NetTweak {
    pub tweak: Box<dyn Tweak + Send + Sync>,
    pub description: String,
    pub details: String,
    pub flags: TweakFlags,
}

/// net.adapterpower, net.adapteradvanced, net.wifi, net.bindings (the active network tweaks).
pub fn net_tweaks(cmd: Arc<dyn Cmd>) -> Result<Vec<NetTweak>> {
    let mut all: Vec<(String, Box<dyn Tweak + Send + Sync>)> = adapter_tweaks(cmd.clone()).into_iter().map(|t| (t.id.clone(), Box::new(t) as Box<dyn Tweak + Send + Sync>)).collect();
    all.push(("net.bindings".into(), Box::new(ProtocolBindingsTweak { cmd })));
    let entries = catalog::entries()?;
    let mut out = Vec::new();
    for (id, tweak) in all {
        if let Some(e) = entries.iter().find(|e| e.id == id && !e.removed) {
            out.push(NetTweak {
                tweak,
                description: e.description.clone().unwrap_or_default(),
                details: e.details.clone().unwrap_or_default(),
                flags: TweakFlags::parse(&e.flags)?,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::RunResult;
    use std::sync::Mutex;

    struct Fake(Mutex<Vec<String>>, i32);
    impl Cmd for Fake {
        fn run(&self, file: &str, args: &[&str]) -> Result<RunResult> {
            let line = format!("{} {}", std::path::Path::new(file).file_name().unwrap().to_string_lossy().to_lowercase(), args.join(" "));
            self.0.lock().unwrap().push(line.clone());
            // only "int ip reset" fails when the fake is told to
            let code = if self.1 != 0 && line.ends_with("int ip reset") { self.1 } else { 0 };
            Ok(RunResult { exit_code: code, output: if code != 0 { "Access is denied.".into() } else { String::new() } })
        }
    }

    #[test]
    fn all_runs_in_order_and_asks_for_restart() {
        let f = Fake(Mutex::new(vec![]), 0);
        let mut log = String::new();
        assert!(repair(&f, "all", &mut |s| log.push_str(s)));
        assert_eq!(
            *f.0.lock().unwrap(),
            ["ipconfig.exe /flushdns", "ipconfig.exe /release", "ipconfig.exe /renew", "ipconfig.exe /registerdns", "netsh.exe winsock reset", "netsh.exe int ip reset"]
        );
        assert!(log.ends_with("Restart your PC to finish the reset.\n"));
    }

    #[test]
    fn tcpip_failure_falls_back_to_v4_and_v6() {
        let f = Fake(Mutex::new(vec![]), 1);
        let mut log = String::new();
        assert!(repair(&f, "tcpip", &mut |s| log.push_str(s)));
        assert_eq!(*f.0.lock().unwrap(), ["netsh.exe int ip reset", "netsh.exe int ipv4 reset", "netsh.exe int ipv6 reset"]);
        assert!(log.contains("Failed (1).\n   Access is denied."));
    }

    #[test]
    fn flush_needs_no_restart_and_unknown_key_does_nothing() {
        let f = Fake(Mutex::new(vec![]), 0);
        assert!(!repair(&f, "flushdns", &mut |_| {}));
        assert!(steps("nope").is_empty());
    }

    #[test]
    fn net_tweaks_come_from_catalog() {
        let t = net_tweaks(crate::tweaks::system_cmd()).unwrap();
        let ids: Vec<&str> = t.iter().map(|n| n.tweak.id()).collect();
        assert_eq!(ids, ["net.adapterpower", "net.adapteradvanced", "net.wifi", "net.bindings"]);
        assert!(t[1].flags.has(TweakFlags::EXPERIMENTAL) && !t[3].description.is_empty());
    }
}
