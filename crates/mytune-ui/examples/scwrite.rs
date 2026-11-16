//! Reversible check that SoundCloud writes pass DataDome through the signed-in webview.
//! Run: .\build.ps1 run -p mytune-ui --example scwrite   (needs a saved SoundCloud login)

use mytune_core::soundcloud as sc;
use mytune_ui::soundcloud as ui;

fn main() {
    let token = sc::load_token().expect("sign in to SoundCloud in MyTune first");

    // clean up any stray playlists left by earlier probe runs
    for p in sc::playlists(&token).unwrap_or_default() {
        if p.title == "probe" || p.title == "MyTune test (safe to delete)" {
            println!("cleanup delete {:?} -> {:?}", p.title, ui::delete_playlist(p.id, &token));
        }
    }

    let found = sc::search("lofi", Some(&token)).expect("search");
    let mut playable = found.iter().filter(|t| t.transcoding.is_some());
    let t1 = playable.next().expect("a playable track");
    let t2 = playable.next().expect("a second playable track");

    println!("like   -> {:?}", ui::set_favourite(t1.id, true, &token));
    std::thread::sleep(std::time::Duration::from_secs(1));
    println!("unlike -> {:?}", ui::set_favourite(t1.id, false, &token));

    match ui::make_playlist("MyTune test (safe to delete)", &[t1.id], &token) {
        Ok(pid) => {
            println!("created pid={pid}");
            println!("add    -> {:?}", ui::playlist_add(pid, t2.id, &token));
            println!("remove -> {:?}", ui::playlist_remove(pid, t2.id, &token));
            println!("delete -> {:?}", ui::delete_playlist(pid, &token));
        }
        Err(e) => println!("create failed: {e}"),
    }
}
