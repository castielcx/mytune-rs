//! Developer live test, run by hand, with the saved login. With no argument it only reads: the servers this
//! account owns and its group DMs. `.\build.ps1 run -p mytune-chat --example livetest`
//! Steps that change something on Discord each need their own argument; see `main`.
use mytune_chat::yapfast_core::yapfast_discord::{
    api::{Api, Span},
    service::{Discord, Status},
    token,
};

/// Standard base64 with padding (this crate has no base64 dependency).
fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk.iter().enumerate().fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() { T[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    out
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let arg = |i: usize| args.get(i).cloned().unwrap_or_default();
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
    let me = snapshot.user_id.clone().unwrap_or_default();
    match arg(0).as_str() {
        "" => {
            for s in snapshot.servers.iter().filter(|s| s.owner_id.as_deref() == Some(me.as_str())) {
                println!("OWNED server {} members {:?} text {} voice {}", s.id, s.member_count, s.text_channels.len(), s.voice_channels.len());
                for c in s.text_channels.iter().take(4) {
                    println!("   text {} len(name)={}", c.id, c.name.chars().count());
                }
                for c in s.voice_channels.iter().take(3) {
                    println!("   voice {} len(name)={}", c.id, c.name.chars().count());
                }
            }
            for c in snapshot.conversations.iter().filter(|c| c.group) {
                println!("GROUP {} people {} owned {}", c.id, c.people, c.owned);
            }
        }
        // unread <guild> <channel>: marks the channel unread from its newest message, then prints what Discord keeps
        "unread" => {
            let (guild, channel) = (arg(1), arg(2));
            let newest = api.messages(&channel, Span::Latest).await?;
            let id: u64 = newest.first().and_then(|m| m["id"].as_str()).and_then(|s| s.parse().ok()).ok_or_else(|| anyhow::anyhow!("no message"))?;
            // the request itself, for Discord's answer (the engine only logs a failure)
            println!("ack_manual: {:?}", api.ack_manual(&channel, &(id - 1).to_string(), 1).await.map_err(|e| format!("{e:#}")));
            discord.mark_unread(&guild, &channel, id, true);
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            let s = snapshots.borrow().clone();
            let marks = s.servers.iter().find(|s| s.id == guild).and_then(|s| s.text_channels().into_iter().find(|c| c.id == channel).map(|c| format!("{:?}", c.marks)));
            let dm = s.conversations.iter().find(|c| c.id == channel).map(|c| (c.unread, c.mentions, c.last_read));
            println!("after mark unread from {id}: channel {marks:?} dm (unread, mentions, last_read) {dm:?}");
            discord.mark_read(&guild, &channel, true);
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            let s = snapshots.borrow().clone();
            println!("marked read again: dm {:?}", s.conversations.iter().find(|c| c.id == channel).map(|c| (c.unread, c.mentions, c.last_read)));
        }
        // voicechan <channel>: sets bitrate 32 kbps, limit 5, region rotterdam, prints Discord's copy, puts the old values back
        "voicechan" => {
            let channel = arg(1);
            let before = discord.channel_settings(&channel).await?;
            println!("before: bitrate {} limit {} region {:?}", before.bitrate, before.user_limit, before.region);
            let mut new = before.clone();
            (new.bitrate, new.user_limit, new.region) = (32_000, 5, "rotterdam".into());
            println!("edit: {:?}", discord.edit_channel(&channel, false, true, &new).await.map_err(|e| format!("{e:#}")));
            let now = discord.channel_settings(&channel).await?;
            println!("now: bitrate {} limit {} region {:?}", now.bitrate, now.user_limit, now.region);
            println!("restore: {:?}", discord.edit_channel(&channel, false, true, &before).await.map_err(|e| format!("{e:#}")));
            let end = discord.channel_settings(&channel).await?;
            println!("end: bitrate {} limit {} region {:?} same as before: {}", end.bitrate, end.user_limit, end.region, end == before);
        }
        // voicemsg <guild> <channel>: records 2 s from the default microphone and sends it as a voice message
        "voicemsg" => {
            let (guild, channel) = (arg(1), arg(2));
            let rec = mytune_chat::yapfast_core::voice_note::Recorder::start(None);
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let r = rec.finish().ok_or_else(|| anyhow::anyhow!("no recording"))?;
            let wave = base64(&r.waveform);
            println!("send: {:?}", discord.send_voice_message(&guild, &channel, r.ogg, r.seconds, &wave).await.map_err(|e| format!("{e:#}")));
            let newest = api.messages(&channel, Span::Latest).await?;
            if let Some(m) = newest.first() {
                println!("newest: flags {} attachment {} content_type {} duration {} waveform bytes {}", m["flags"], m["attachments"][0]["filename"], m["attachments"][0]["content_type"], m["attachments"][0]["duration_secs"], m["attachments"][0]["waveform"].as_str().map_or(0, str::len));
            }
        }
        // newgroup: makes a group DM with nobody else in it, a private place to test in
        "newgroup" => println!("group: {:?}", discord.create_group(&[]).await.map_err(|e| format!("{e:#}"))),
        // group <channel> <png file>: renames the group DM and gives it an icon, printing Discord's copy after each
        "group" => {
            let channel = arg(1);
            let show = |c: serde_json::Value| format!("name {} icon {} owner {}", c["name"], c["icon"], c["owner_id"]);
            println!("before: {}", show(api.channel(&channel).await?));
            println!("rename: {:?}", discord.edit_group(&channel, &serde_json::json!({"name": "MyTune test"})).await.map_err(|e| format!("{e:#}")));
            println!("now: {}", show(api.channel(&channel).await?));
            let icon = format!("data:image/png;base64,{}", base64(&std::fs::read(arg(2))?));
            println!("icon: {:?}", discord.edit_group(&channel, &serde_json::json!({"icon": icon})).await.map_err(|e| format!("{e:#}")));
            println!("now: {}", show(api.channel(&channel).await?));
            println!("icon off: {:?}", discord.edit_group(&channel, &serde_json::json!({"icon": null})).await.map_err(|e| format!("{e:#}")));
            println!("end: {}", show(api.channel(&channel).await?));
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let s = snapshots.borrow().clone();
            println!("engine: {:?}", s.conversations.iter().find(|c| c.id == channel).map(|c| (c.name.clone(), c.owned, c.people)));
        }
        other => anyhow::bail!("unknown step {other}"),
    }
    Ok(())
}
