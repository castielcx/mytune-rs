//! Cross-check with the C# app (headless harness in the scratchpad), HKCU\Software\MyTuneTest only.
//! 1. Writes known ORIGINAL values of every kind, captures them with the Rust backup code.
//! 2. Overwrites them with junk (and creates a value that did not exist), leaving that state behind.
//! 3. Saves state.json (on top of a C#-style file with keys Rust does not own) into argv[1].
//! The C# harness then restores from Rust's state.json and checks the originals came back.

use mytune_core::backup::TweakBackup;
use mytune_core::reg::{self, Hive, RegValue};
use mytune_core::store::Store;

const KEY: &str = "Software\\MyTuneTest";

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("dir argument"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("state.json"),
        r#"{"Settings":{"Theme":"dark"},"Games":[{"Name":"g"}],"FirstRunDone":true,"Applied":{}}"#,
    )
    .unwrap();

    let _ = reg::delete_tree(Hive::Hkcu, KEY);
    let originals = [
        ("DWord", RegValue::Dword(u32::MAX)),
        ("QWord", RegValue::Qword(u64::MAX)),
        ("String", RegValue::String("a\\b \"q\" é".into())),
        ("ExpandString", RegValue::ExpandString("%PATH%".into())),
        ("MultiString", RegValue::MultiString(vec!["a".into(), "b c".into()])),
        ("Binary", RegValue::Binary(vec![0, 1, 2, 255])),
    ];
    for (n, v) in &originals {
        reg::set(Hive::Hkcu, KEY, n, v).unwrap();
    }
    let mut b = TweakBackup::new("Sample tweak é");
    for (n, _) in &originals {
        b.capture(Hive::Hkcu, KEY, n).unwrap();
    }
    b.capture(Hive::Hkcu, &format!("{KEY}\\NewSub"), "Gone").unwrap(); // did not exist
    b.data.insert("enabled".into(), "1".into());
    // now "apply" the tweak: junk everywhere
    for (n, _) in &originals {
        reg::set(Hive::Hkcu, KEY, n, &RegValue::Dword(1)).unwrap();
    }
    reg::set(Hive::Hkcu, &format!("{KEY}\\NewSub"), "Gone", &RegValue::Dword(1)).unwrap();

    let mut s = Store::load(&dir);
    s.set_backup("sample.tweak", b).unwrap();
    s.set_backup("sample.second", TweakBackup::new("second")).unwrap(); // second save -> .bak exists
}
