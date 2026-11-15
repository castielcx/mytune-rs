//! Developer probe, run by hand: every server's stickers (id, format, name) with the saved login. It reads only.
//! `cargo run -p mytune-chat --example stickers`
use mytune_chat::yapfast_core::yapfast_discord::service::{Discord, Status};
use mytune_chat::yapfast_core::yapfast_discord::token;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = mytune_chat::yapfast_core::init_tls();
    let discord = Discord::start(token::load()?)?;
    let mut snapshots = discord.snapshots();
    let snapshot = loop {
        snapshots.changed().await?;
        let s = snapshots.borrow_and_update().clone();
        match &s.status {
            Status::Online => break s,
            Status::Offline(why) => anyhow::bail!("{why}"),
            Status::Connecting => {}
        }
    };
    for server in &snapshot.servers {
        for sticker in server.stickers.iter() {
            println!("{}\t{}\t{}", sticker.id, sticker.format, sticker.name);
        }
    }
    Ok(())
}
