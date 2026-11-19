//! The call this account is in, kept in `%ProgramData%\MyTune\call.txt` while it lasts, so that a start soon after
//! a crash joins it again. A call that ends removes the file; only a crash or a kill leaves it behind.

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

/// A file older than this is not joined again: the crash was too long ago. The file is touched every `TOUCH`
/// while events come, so a silent call may age out (then nothing is joined, the safe side).
const FRESH: Duration = Duration::from_secs(5 * 60);
const TOUCH: Duration = Duration::from_secs(30);
/// How long after the start the call left behind may still be joined: Discord has to be back by then.
const WAIT: Duration = Duration::from_secs(60);

struct Kept {
    /// The call left by the run before, until it is joined again.
    pending: Option<String>,
    loaded: Instant,
    room: Option<String>,
    touched: Instant,
}

static KEPT: Mutex<Option<Kept>> = Mutex::new(None);

fn path() -> PathBuf {
    crate::settings::Settings::path().with_file_name("call.txt")
}

fn fresh(written: SystemTime, now: SystemTime) -> bool {
    now.duration_since(written).is_ok_and(|age| age < FRESH)
}

/// Notes the call the account is in now. Gives the room to join again, once, when the run before left one and
/// it can be joined (`ready`).
pub fn track(room: Option<&str>, ready: impl Fn(&str) -> bool) -> Option<String> {
    // the comparison copy runs beside the installed MyTune: two copies must not both rejoin one call
    if cfg!(feature = "preview") {
        return None;
    }
    let mut kept = KEPT.lock().unwrap_or_else(|e| e.into_inner());
    let kept = kept.get_or_insert_with(|| {
        let file = path();
        let recent = std::fs::metadata(&file).and_then(|m| m.modified()).is_ok_and(|at| fresh(at, SystemTime::now()));
        let pending = recent.then(|| std::fs::read_to_string(&file).ok()).flatten().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty());
        Kept { pending, loaded: Instant::now(), room: None, touched: Instant::now() }
    });
    if room != kept.room.as_deref() || (room.is_some() && kept.touched.elapsed() > TOUCH) {
        let _ = match room {
            Some(room) => std::fs::write(path(), room),
            None => std::fs::remove_file(path()),
        };
        kept.room = room.map(str::to_owned);
        kept.touched = Instant::now();
    }
    if room.is_some() || kept.loaded.elapsed() > WAIT {
        kept.pending = None;
    }
    kept.pending.take_if(|room| ready(room))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_recent_file_counts() {
        let now = SystemTime::now();
        assert!(fresh(now - Duration::from_secs(60), now));
        assert!(!fresh(now - Duration::from_secs(600), now));
        assert!(!fresh(now + Duration::from_secs(60), now));
    }
}
