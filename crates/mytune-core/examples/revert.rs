//! Reverts catalog tweaks by id from their backups in state.json.
//! Run elevated with MyTune closed (both write state.json):
//! `cargo run -p mytune-core --example revert -- adv.hags adv.mpo`.

use mytune_core::alltweaks;
use mytune_core::engine::{Engine, Tweak};
use mytune_core::store::Store;

fn main() {
    let ids: Vec<String> = std::env::args().skip(1).collect();
    let items = alltweaks::build(mytune_core::tweaks::system_cmd()).expect("catalog");
    let engine = Engine::new(Store::load_default());
    let mut failed = false;
    for id in &ids {
        let Some(t) = items.iter().find(|t| &t.id == id) else {
            println!("{id}: unknown id");
            failed = true;
            continue;
        };
        match engine.revert(t) {
            Ok(o) => println!("{id}: {o:?}, applied now: {:?}", t.is_applied()),
            Err(e) => {
                println!("{id}: FAILED {e}");
                failed = true;
            }
        }
    }
    std::process::exit(failed as i32);
}
