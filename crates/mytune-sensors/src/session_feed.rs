//! Glue: sensor samples into the session history, game state into the sensor cadence.
//! Lives here (not in core) because core cannot depend on this crate.

use crate::{Sensors, Snapshot};
use mytune_core::gamewatch::GameService;
use mytune_core::session::Sample;
use std::sync::Arc;

pub fn sample_of(s: &Snapshot) -> Sample {
    Sample { cpu_temp: s.cpu_temp, gpu_temp: s.gpu_temp, cpu_load: s.cpu_load, gpu_load: s.gpu_load }
}

/// After this, every sensor sample goes to `svc.sessions` and the sensors run at the fast cadence while a game runs.
/// Also runs the heat warning: it is logged and counted in the open session (no tray balloon in Rust).
pub fn attach(sensors: &Arc<Sensors>, svc: &mut GameService) {
    let tracker = svc.sessions.clone();
    let heat = svc.heat.clone();
    sensors.on_update(move |s| {
        let warn = heat.lock().unwrap().on_sample(s.cpu_temp, s.gpu_temp);
        let mut t = tracker.lock().unwrap();
        t.on_sample(sample_of(s));
        if let Some(what) = warn {
            mytune_core::log::info(&format!("heat warning: {what}"));
            t.note_heat_warning();
        }
    });
    let sn = sensors.clone();
    svc.on_gaming = Some(Box::new(move |g| sn.set_game_running(g)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_the_four_fields() {
        let s = Snapshot { cpu_temp: Some(1.0), gpu_temp: Some(2.0), cpu_load: Some(3.0), gpu_load: None, ..Default::default() };
        let m = sample_of(&s);
        assert_eq!((m.cpu_temp, m.gpu_temp, m.cpu_load, m.gpu_load), (Some(1.0), Some(2.0), Some(3.0), None));
    }
}
