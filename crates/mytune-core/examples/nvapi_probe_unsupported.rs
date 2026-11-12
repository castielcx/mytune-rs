//! Which game settings does the driver reject on the Base Profile? Sets them in a session that is NEVER saved
//! (same probe as C# `Nvidia.IsUnsupported`), so nothing persists.

use mytune_core::nvapi::Session;
use mytune_core::nvidia::game_settings;

fn main() {
    let s = Session::open().unwrap();
    let base = s.base_profile().unwrap();
    for (id, value, name) in game_settings() {
        println!("{name:50} {:?}", s.set(base, id, value).map_err(|e| e.nvapi_status()));
    }
    // no s.save(): dropping the session discards everything
}
