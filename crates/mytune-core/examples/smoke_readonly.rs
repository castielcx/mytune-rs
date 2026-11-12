//! Read-only check of the real tools' output formats. Changes nothing.
//! Run elevated: `cargo run --example smoke_readonly`.

use mytune_core::process::{bcd_entry, run, system32};
use mytune_core::tweaks::{find_guid, system_cmd, PowerPlanTweak, TimerTickTweak, VbsTweak};
use mytune_core::engine::Tweak;

fn main() {
    let p = |f: &str| system32(f).to_string_lossy().into_owned();
    let r = run(&p("powercfg.exe"), &["/getactivescheme"]).unwrap();
    println!("powercfg exit={} guid={:?}", r.exit_code, find_guid(&r.output));
    let l = run(&p("powercfg.exe"), &["/list"]).unwrap();
    println!("powercfg /list exit={} lines={}", l.exit_code, l.output.lines().count());
    println!("bcd entry: {}", bcd_entry());
    let b = run(&p("bcdedit.exe"), &["/enum", bcd_entry()]).unwrap();
    println!("bcdedit /enum exit={} lines={}", b.exit_code, b.output.lines().count());
    let s = run(&p("sc.exe"), &["qc", "Fax"]).unwrap();
    println!("sc qc Fax exit={}", s.exit_code);
    let c = system_cmd();
    println!("power plan is_applied: {:?}", PowerPlanTweak { cmd: c.clone(), laptop: false }.is_applied());
    println!("vbs is_applied: {:?}", VbsTweak::system(c.clone()).is_applied());
    println!("tick is_applied: {:?}", TimerTickTweak { cmd: c, entry: None }.is_applied());
}
