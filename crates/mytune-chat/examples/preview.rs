//! Developer probe, run by hand: what Discord answers for the stream preview of every DM with a call going,
//! with the saved login. It reads only. `cargo run -p mytune-chat --example preview [stream key]`
use mytune_chat::yapfast_core::yapfast_discord::{
    api::Api,
    service::{Discord, Status},
    stream, token,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = mytune_chat::yapfast_core::init_tls();
    let api = Api::new(token::load()?)?;
    let mut keys: Vec<String> = std::env::args().skip(1).collect();
    if keys.is_empty() {
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
        for c in snapshot.conversations.iter().filter(|c| c.call) {
            println!("call in {} ({}) person {:?}", c.name, c.id, c.person);
            keys.extend(c.person.iter().chain(snapshot.user_id.iter()).map(|u| stream::key("@me", &c.id, u)));
        }
    }
    for key in keys {
        match api.stream_preview_url(&key).await {
            Ok(at) => {
                let got = api.download_now(&at, 8 << 20).await;
                let host: String = at.chars().take(60).collect();
                println!("{key}: {host}... -> {:?}", got.map(|d| (d.len(), d.iter().take(4).copied().collect::<Vec<u8>>())).map_err(|e| format!("{e:#}")));
            }
            Err(e) => println!("{key}: {e:#}"),
        }
    }
    Ok(())
}
