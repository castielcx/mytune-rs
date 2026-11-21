//! Real NVAPI test on THROWAWAY driver profiles only. Needs an NVIDIA driver, so it is #[ignore]d:
//! `cargo test --test real_nvidia -- --ignored --nocapture`
//!
//! - The global / G-SYNC tweaks are aimed (via `Target::Named`) at a profile `MyTuneTest-<pid>` created here.
//! - The per-game tweak creates its own profile `MyTune - MyTuneTestGame`, which its revert deletes.
//! - NVIDIA's Base Profile is only READ: a snapshot before and after must be identical.
//! A Drop guard deletes every profile even if an assertion fails.

use mytune_core::engine::{Engine, RevertOutcome, Tweak};
use mytune_core::nvapi::{Session, SettingValue};
use mytune_core::nvidia::*;
use mytune_core::store::Store;

fn init() {
    // keep the log and state out of the real %ProgramData%\MyTune
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| std::env::set_var("MYTUNE_DIR", std::env::temp_dir().join("mytune-real-nvidia-test")));
}

fn engine() -> (tempfile::TempDir, Engine) {
    let d = tempfile::tempdir().unwrap();
    let e = Engine::new(Store::load(d.path()));
    (d, e)
}

type Snapshot = Vec<(u32, Option<SettingValue>)>;

fn base_snapshot() -> Snapshot {
    let s = Session::open().unwrap();
    let base = s.base_profile().unwrap();
    let mut ids: Vec<u32> = game_settings().iter().map(|x| x.0).collect();
    ids.extend(GSYNC_IDS);
    ids.push(FRAME_RATE_LIMITER);
    ids.into_iter().map(|id| (id, s.get(base, id).unwrap())).collect()
}

/// Deletes the named profiles on drop.
struct Cleanup(Vec<String>);
impl Drop for Cleanup {
    fn drop(&mut self) {
        if let Ok(s) = Session::open() {
            for n in &self.0 {
                if let Ok(Some(p)) = s.find_profile(n) {
                    let _ = s.delete_profile(p);
                }
            }
            let _ = s.save();
        }
    }
}

fn get(profile: &str, id: u32) -> Option<SettingValue> {
    let s = Session::open().unwrap();
    let p = s.find_profile(profile).unwrap().expect("profile exists");
    s.get(p, id).unwrap()
}

fn set(profile: &str, id: u32, v: u32) {
    let s = Session::open().unwrap();
    let p = s.find_profile(profile).unwrap().expect("profile exists");
    s.set(p, id, v).unwrap();
    s.save().unwrap();
}

fn user_value(profile: &str, id: u32) -> Option<u32> {
    // a value the user (or a tweak) put into the profile, as opposed to a driver default
    get(profile, id).filter(|v| v.location == SettingValue::LOCATION_CURRENT_PROFILE && !v.is_current_predefined).map(|v| v.value)
}

#[test]
#[ignore]
fn nvidia_tweaks_on_throwaway_profiles() {
    init();
    assert!(available(), "needs an NVIDIA driver");
    let before = base_snapshot();
    let t_name = format!("MyTuneTest-{}", std::process::id());
    let game_profile = "MyTune - MyTuneTestGame";
    let _cleanup = Cleanup(vec![t_name.clone(), game_profile.into()]);
    {
        // stale leftovers from an earlier crashed run
        let s = Session::open().unwrap();
        for n in [t_name.as_str(), game_profile] {
            if let Some(p) = s.find_profile(n).unwrap() {
                s.delete_profile(p).unwrap();
            }
        }
        let p = s.create_profile(&t_name).unwrap();
        s.save().unwrap();
        assert!(s.find_profile(&t_name).unwrap().is_some());
        let _ = p;
    }
    let (_d, e) = engine();

    // ---- 1. global tweak: apply, then revert removes what it set
    let global = NvidiaGlobalProfileTweak { target: Target::Named(t_name.clone()) };
    assert!(!global.is_applied().unwrap());
    e.apply(&global).unwrap();
    assert!(global.is_applied().unwrap(), "applied on the throwaway profile");
    assert_eq!(user_value(&t_name, POWER_MANAGEMENT_MODE), Some(1));
    assert_eq!(user_value(&t_name, VERTICAL_SYNC), Some(0x08416747));
    assert_eq!(user_value(&t_name, SHADER_CACHE_SIZE), Some(u32::MAX));
    assert_eq!(e.revert(&global).unwrap(), RevertOutcome::Reverted);
    for (id, _, name) in GLOBAL_SETTINGS {
        assert_eq!(user_value(&t_name, id), None, "{name} still set after revert");
    }
    assert!(!global.is_applied().unwrap());

    // ---- 2. a value the user had before is put back
    set(&t_name, TRIPLE_BUFFERING, 1);
    assert_eq!(user_value(&t_name, TRIPLE_BUFFERING), Some(1));
    e.apply(&global).unwrap();
    assert_eq!(user_value(&t_name, TRIPLE_BUFFERING), Some(0));
    e.revert(&global).unwrap();
    assert_eq!(user_value(&t_name, TRIPLE_BUFFERING), Some(1), "the user's own value is back");
    assert_eq!(user_value(&t_name, POWER_MANAGEMENT_MODE), None);

    // ---- 3. G-SYNC switch
    let gs = NvidiaGsyncTweak { target: Target::Named(t_name.clone()) };
    assert!(gs.is_applied().unwrap(), "nothing switched off yet");
    gs.revert(&Default::default()).unwrap(); // a backup without state changes nothing
    assert!(gs.is_applied().unwrap(), "revert never switches G-SYNC off by itself");
    e.apply(&gs).unwrap();
    e.revert(&gs).unwrap();
    assert!(gs.is_applied().unwrap(), "it was on before, so it is on after");
    for id in GSYNC_IDS {
        assert_eq!(user_value(&t_name, id), None);
    }

    // ---- 4. per game, profile created by the tweak, then deleted by revert
    let game = NvidiaGameTweak { id: "game:test:nvidia".into(), exe: "MyTuneTestGame.exe".into(), game_name: "MyTuneTestGame".into(), frame_cap: 60 };
    assert!(!game.is_applied().unwrap());
    e.apply(&game).unwrap();
    {
        let s = Session::open().unwrap();
        assert!(s.find_profile(game_profile).unwrap().is_some(), "profile created");
        assert!(s.find_application("MyTuneTestGame.exe").unwrap().is_some(), "exe attached");
    }
    assert!(game.is_applied().unwrap());
    assert_eq!(user_value(game_profile, POWER_MANAGEMENT_MODE), Some(1));
    assert_eq!(user_value(game_profile, THREADED_OPTIMIZATION), Some(1));
    println!("frame limiter in game profile: {:?}", user_value(game_profile, FRAME_RATE_LIMITER));
    assert_eq!(e.revert(&game).unwrap(), RevertOutcome::Reverted);
    {
        let s = Session::open().unwrap();
        assert!(s.find_profile(game_profile).unwrap().is_none(), "created profile deleted");
        assert!(s.find_application("MyTuneTestGame.exe").unwrap().is_none());
    }

    // ---- 5. per game, exe already in an existing profile that has a user frame cap: cap survives
    {
        let s = Session::open().unwrap();
        let p = s.find_profile(&t_name).unwrap().unwrap();
        s.create_application(p, "MyTuneTestGame2.exe").unwrap();
        s.save().unwrap();
    }
    set(&t_name, FRAME_RATE_LIMITER, 30);
    let game2 = NvidiaGameTweak { id: "game:test2:nvidia".into(), exe: "MyTuneTestGame2.exe".into(), game_name: "MyTuneTestGame2".into(), frame_cap: 0 };
    e.apply(&game2).unwrap();
    assert!(game2.is_applied().unwrap());
    assert_eq!(user_value(&t_name, POWER_MANAGEMENT_MODE), Some(1));
    assert_eq!(e.revert(&game2).unwrap(), RevertOutcome::Reverted);
    assert_eq!(user_value(&t_name, POWER_MANAGEMENT_MODE), None);
    assert_eq!(user_value(&t_name, TRIPLE_BUFFERING), Some(1), "pre-existing user value kept");
    assert_eq!(user_value(&t_name, FRAME_RATE_LIMITER), Some(30), "the user's own frame cap survives");

    // ---- 6. revert without a backup changes nothing
    assert_eq!(e.revert(&global).unwrap(), RevertOutcome::NoBackup);

    // ---- 7. the real Base Profile: read only
    let base_global = NvidiaGlobalProfileTweak::base();
    println!("Base Profile: global tweak applied = {:?}, gsync on = {:?}", base_global.is_applied().unwrap(), NvidiaGsyncTweak::base().is_applied().unwrap());

    drop(_cleanup);
    {
        let s = Session::open().unwrap();
        assert!(s.find_profile(&t_name).unwrap().is_none(), "throwaway profile deleted");
        assert!(s.find_profile(game_profile).unwrap().is_none());
    }
    assert_eq!(base_snapshot(), before, "Base Profile unchanged");
}
