//! Windows toasts for what happens while MyTune is behind other windows: a new message or mention (the engine's
//! alerts, with who wrote where and the text, already filtered by mute and notification levels) and a DM call
//! ringing (seen as a change in the room list). They show through the Action Center with a hidden PowerShell, so
//! no extra crate or app id is needed. Settings: Notifications > "Show desktop notifications". Nothing shows in
//! Do Not Disturb or while MyTune is in front.

use super::*;

#[derive(Clone, Default)]
struct Seen {
    /// Per room: unread, mentions, incoming call.
    rooms: HashMap<String, (u64, u64, bool)>,
    ready: bool,
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// Shows a toast with a title and a line, from a thread so the UI never waits on PowerShell.
fn show(title: String, line: String) {
    std::thread::spawn(move || {
        use std::os::windows::process::CommandExt;
        let (t, l) = (xml_escape(&title).replace('\'', "''"), xml_escape(&line).replace('\'', "''"));
        let script = format!(
            "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; \
             $x = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
             $n = $x.GetElementsByTagName('text'); $n.Item(0).AppendChild($x.CreateTextNode('{t}')) | Out-Null; $n.Item(1).AppendChild($x.CreateTextNode('{l}')) | Out-Null; \
             $toast = [Windows.UI.Notifications.ToastNotification]::new($x); \
             [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}}\\WindowsPowerShell\\v1.0\\powershell.exe').Show($toast)"
        );
        let _ = std::process::Command::new("powershell").args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", &script]).creation_flags(0x0800_0000).output();
    });
}

impl ChatPage {
    /// Called every frame from the app (any page): toasts for what is new since the last frame.
    pub fn notify(&mut self, ctx: &egui::Context) {
        let Some(chat) = self.chat.clone() else { return };
        let id = egui::Id::new("toast_seen");
        let mut seen: Seen = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
        let enabled = chat.with_settings(|st| st.notifications);
        let (rooms, dnd) = chat.with_state(|s| (s.rooms.iter().map(|r| (r.id.clone(), r.name.clone(), r.direct, r.muted, r.unread, r.mentions, r.incoming)).collect::<Vec<_>>(), s.do_not_disturb()));
        let focused = ctx.input(|i| i.focused);
        // messages and mentions come from the engine's alerts, which already follow mute and notification
        // levels, and carry who wrote where and the text
        for a in chat.take_alerts() {
            // the DM list shows it under the name
            if !a.body.trim().is_empty() {
                let lid = egui::Id::new("last_msgs");
                let mut last: HashMap<String, (String, u64)> = ctx.data(|d| d.get_temp(lid)).unwrap_or_default();
                last.insert(a.room.clone(), (a.body.replace('\n', " "), mytune_chat::yapfast_core::clock::now_ms() / 1000));
                ctx.data_mut(|d| d.insert_temp(lid, last));
            }
            if seen.ready && enabled && !focused && !dnd {
                let body: String = super::markdown::plain(&a.body).chars().take(240).collect();
                show(tidy(&a.title), if body.trim().is_empty() { "sent a message".to_owned() } else { body });
            }
        }
        let mut now = HashMap::new();
        for (rid, name, direct, muted, unread, mentions, incoming) in rooms {
            if seen.ready && enabled && !focused && !dnd {
                if let Some(&(u0, m0, c0)) = seen.rooms.get(&rid) {
                    let tidy_name = tidy(&name);
                    if incoming && !c0 {
                        show(tidy_name.clone(), "is calling you on Discord".to_owned());
                    }
                    let _ = (direct, muted, unread, u0, mentions, m0);
                }
            }
            now.insert(rid, (unread, mentions, incoming));
        }
        seen.rooms = now;
        seen.ready = true;
        ctx.data_mut(|d| d.insert_temp(id, seen));
    }
}
