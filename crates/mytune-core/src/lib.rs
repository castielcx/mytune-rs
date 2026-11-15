//! MyTune core: registry helpers, backup/revert engine, state.json store, log, process helper.
//! Compatible with the C# MyTune 2 `state.json` format.

pub mod alltweaks;
pub mod audio;
pub mod backup;
pub mod booster;
pub mod catalog;
pub mod engine;
pub mod etw;
pub mod gamewatch;
pub mod heatwatch;
pub mod error;
pub mod log;
pub mod mouse;
pub mod nvapi;
pub mod pccheck;
pub mod netrepair;
pub mod nvidia;
pub mod pergame;
pub mod process;
pub mod procs;
pub mod reg;
pub mod session;
pub mod special;
pub mod soundcloud;
pub mod speedtest;
pub mod store;
pub mod throttle;
pub mod tweaks;
pub mod autostart;
pub mod settingsbackup;
pub mod sysreport;
pub mod updater;
pub mod cleaner;
pub mod layout;
pub mod stability;
pub mod startup;
#[cfg(feature = "taskbar-look")]
pub mod taskbar;
pub mod tools;
pub mod vibrance;
pub mod diagnostics;
pub mod display;
pub mod vpn;
mod util;

pub use error::{Error, Result};
