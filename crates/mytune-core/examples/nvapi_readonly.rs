//! Read-only NVAPI check: opens a DRS session, reads settings of the Base Profile, never saves.

use mytune_core::nvapi::{available, Session};

fn main() {
    println!("nvapi available: {}", available());
    let s = Session::open().expect("open session");
    let base = s.base_profile().expect("base profile");
    println!("base profile name: {:?}", s.profile_name(base));
    let ids: [(u32, &str); 16] = [
        (274197361, "power mgmt"),
        (8102046, "max pre-rendered frames"),
        (11041231, "vsync"),
        (553505273, "triple buffering"),
        (6600001, "preferred refresh"),
        (13510289, "texture filtering quality"),
        (3066610, "trilinear opt"),
        (8703344, "aniso filter opt"),
        (15151633, "aniso sample opt"),
        (11306135, "shader cache size"),
        (276089202, "fxaa"),
        (276158834, "ansel"),
        (549528094, "threaded opt"),
        (0x1194F158, "gsync global"),
        (0x1094F157, "vrr global feature"),
        (0x1094F1F7, "vrr requested"),
    ];
    for (id, name) in ids {
        println!("{name:28} {id:>10}: {:?}", s.get(base, id));
    }
    println!("limiter 0x10835002: {:?}", s.get(base, 0x10835002));
    println!("find app notepad.exe: {:?}", s.find_application("notepad.exe").map(|o| o.is_some()));
    println!("find profile 'MyTuneTest-nope': {:?}", s.find_profile("MyTuneTest-nope").map(|o| o.is_some()));
}
