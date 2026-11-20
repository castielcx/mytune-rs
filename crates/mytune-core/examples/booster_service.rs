//! The game booster as a console program: `cargo run --release --example booster_service`.
//!
//! Watches the Game Hub games in `%ProgramData%\MyTune\state.json` and, while one runs, applies the features that
//! are switched on in `Settings` (GameTimer, QuietDuringGames, AutoPowerPlan, ThrottleBackground). Ctrl+C (or
//! closing the window) puts everything back.
//!
//! It refuses to start while the C# MyTune is running: both would write `state.json` and fight over the same
//! settings. NOT started by any test.

use mytune_core::booster::{Booster, NtTimer};
use mytune_core::gamewatch::{CpuTopology, GameService, GameWatcher};
use mytune_core::procs::{Procs, WinProcs};
use mytune_core::store::Store;
use mytune_core::tweaks::system_cmd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use windows::core::BOOL;
use windows::Win32::System::Console::SetConsoleCtrlHandler;

static STOP: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn on_ctrl(_: u32) -> BOOL {
    STOP.store(true, Ordering::SeqCst);
    BOOL(1)
}

fn main() {
    let procs: Arc<dyn Procs> = Arc::new(WinProcs);
    if procs.list().map(|l| l.iter().any(|p| p.name.eq_ignore_ascii_case("MyTune"))).unwrap_or(false) {
        eprintln!("The C# MyTune is running. Close it first: both would write state.json.");
        std::process::exit(1);
    }
    unsafe {
        let _ = SetConsoleCtrlHandler(Some(on_ctrl), true);
    }
    let store = Store::load_default();
    let topo = CpuTopology::detect();
    println!("CPU: {} cores, performance mask {:#x}, hybrid {}, V-Cache mask {:#x}", topo.cores, topo.performance_mask, topo.is_hybrid, topo.vcache_mask);
    println!("Watching games. Ctrl+C to stop.");
    let booster = Booster::new(system_cmd(), Arc::new(NtTimer), procs.clone());
    let mut service = GameService::new(store, GameWatcher::new(procs), booster);
    match service.run(&STOP) {
        Ok(()) => println!("Stopped, everything put back."),
        Err(e) => {
            eprintln!("Stopped, but some features could not be undone: {e}");
            std::process::exit(2);
        }
    }
}
