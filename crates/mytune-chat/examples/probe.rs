//! Developer probe, run by hand: what Discord answers for a channel's history with the saved login.
//! `cargo run -p mytune-chat --example probe -- <part of a channel name>`. It reads only.
use mytune_chat::yapfast_core::yapfast_discord::{
    api::{Api, Span},
    service::{Discord, Status},
    token,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let want = std::env::args().nth(1).unwrap_or_default().to_lowercase();
    let _ = mytune_chat::yapfast_core::init_tls();
    let api = Api::new(token::load()?)?;
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
        for channel in server.text_channels() {
            if !channel.name.to_lowercase().contains(&want) {
                continue;
            }
            println!("{} / {} ({}) access {:?}", server.name, channel.name, channel.id, channel.marks.access);
            const HISTORY: u64 = 1 << 16;
            println!("  @everyone may read history: {:?}", server.roles.get(&server.id).map(|p| p & HISTORY != 0));
            match api.channel(&channel.id).await {
                Ok(c) => {
                    println!("  type {} last_message_id {}", c["type"], c["last_message_id"]);
                    println!("  pins: {:?}", api.pins(&channel.id).await.map(|v| v.len()).map_err(|e| format!("{e:#}")));
                    let found = api.search(Some(&server.id), &channel.id, &[("channel_id", channel.id.clone())], 0).await;
                    if let Ok(v) = &found {
                        for m in v["messages"].as_array().into_iter().flatten().filter_map(|hit| hit.get(0)) {
                            let text: String = m["content"].as_str().unwrap_or("").chars().take(40).collect();
                            println!("    hit {} channel {} type {} embeds {} {:?}", m["id"], m["channel_id"], m["type"], m["embeds"].as_array().map_or(0, Vec::len), text);
                        }
                        let oldest = v["messages"].as_array().and_then(|a| a.last()).and_then(|hit| hit[0]["id"].as_str()).unwrap_or_default().to_owned();
                        let next = api.search(Some(&server.id), &channel.id, &[("channel_id", channel.id.clone()), ("max_id", oldest)], 0).await;
                        println!("  search max_id: {:?}", next.map(|v| (v["total_results"].clone(), v["messages"].as_array().map_or(0, Vec::len), v["messages"][0][0]["id"].clone())).map_err(|e| format!("{e:#}")));
                    }
                    println!("  search: {:?}", found.map(|v| (v["total_results"].clone(), v["messages"].as_array().map_or(0, Vec::len))).map_err(|e| format!("{e:#}")));
                    if let Some(last) = c["last_message_id"].as_str() {
                        println!("  around last: {:?}", api.messages(&channel.id, Span::Around(last)).await.map(|v| v.len()).map_err(|e| format!("{e:#}")));
                    }
                    for o in c["permission_overwrites"].as_array().into_iter().flatten() {
                        let bit = |k: &str| o[k].as_str().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0) & HISTORY != 0;
                        println!("  overwrite {} kind {} role-has-history {:?} allow-history {} deny-history {}", o["id"], o["type"], o["id"].as_str().and_then(|i| server.roles.get(i)).map(|p| p & HISTORY != 0), bit("allow"), bit("deny"));
                    }
                }
                Err(e) => println!("  channel error: {e:#}"),
            }
            match api.messages(&channel.id, Span::Latest).await {
                Ok(v) => {
                    println!("  raw messages: {}", v.len());
                    for m in v.iter().take(3) {
                        let text: String = m["content"].as_str().unwrap_or("").chars().take(60).collect();
                        println!("  type {} flags {} embeds {} snapshots {} content {:?}", m["type"], m["flags"], m["embeds"].as_array().map_or(0, Vec::len), m["message_snapshots"].as_array().map_or(0, Vec::len), text);
                    }
                }
                Err(e) => println!("  error: {e:#}"),
            }
            match discord.history(&server.id, &channel.id, Span::Latest).await {
                Ok(p) => {
                    println!("  engine page: {} messages, full {}, oldest {:?}", p.messages.len(), p.full, p.oldest);
                    if let Some(oldest) = p.oldest.filter(|_| p.full) {
                        match discord.history(&server.id, &channel.id, Span::Before(&oldest.to_string())).await {
                            Ok(p) => println!("  engine older page: {} messages, full {}, oldest {:?}", p.messages.len(), p.full, p.oldest),
                            Err(e) => println!("  engine error: {e:#}"),
                        }
                    }
                }
                Err(e) => println!("  engine error: {e:#}"),
            }
        }
    }
    Ok(())
}
