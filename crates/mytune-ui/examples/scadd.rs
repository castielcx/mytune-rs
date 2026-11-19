//! Reversible test of adding a track to an EXISTING playlist (the real failing case).
//! Run: .\build.ps1 run -p mytune-ui --example scadd

use mytune_core::soundcloud as sc;
use mytune_ui::soundcloud as ui;

fn main() {
    let token = sc::load_token().expect("sign in first");
    let lists = sc::playlists(&token).expect("playlists");
    println!("playlists: {}", lists.len());
    let Some(p) = lists.into_iter().find(|p| p.count > 0) else {
        println!("no non-empty playlist to test");
        return;
    };
    let before = sc::playlist_tracks(p.id, Some(&token)).map(|t| t.len()).unwrap_or(0);
    println!("playlist {:?} id={} tracks_before={} (count field={})", p.title, p.id, before, p.count);
    println!("raw ids -> {:?}", sc::playlist_track_ids(p.id, Some(&token)).map(|v| v.len()));

    let found = sc::search("lofi", Some(&token)).expect("search");
    let t = found.iter().find(|t| t.transcoding.is_some()).expect("track");

    println!("add {} -> {:?}", t.id, ui::playlist_add(p.id, t.id, &token));
    std::thread::sleep(std::time::Duration::from_secs(1));
    let after = sc::playlist_tracks(p.id, Some(&token)).map(|x| x.len()).unwrap_or(0);
    println!("tracks_after_add={after}");

    // restore
    println!("remove -> {:?}", ui::playlist_remove(p.id, t.id, &token));
    std::thread::sleep(std::time::Duration::from_secs(1));
    let restored = sc::playlist_tracks(p.id, Some(&token)).map(|x| x.len()).unwrap_or(0);
    println!("tracks_after_remove={restored}");
}
