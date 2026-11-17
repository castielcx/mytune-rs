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

/// Shows a toast with a title and a line, from a thread so the UI never waits on PowerShell.
fn show(title: String, line: String) {
    // Each toast is a PowerShell start (~0.5 s of CPU): a busy server sent dozens at once while MyTune was
    // in the tray. Two at a time; the rest of a burst is dropped (the unread marks still show them).
    static RUNNING: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    use std::sync::atomic::Ordering::Relaxed;
    if RUNNING.fetch_add(1, Relaxed) >= 2 {
        RUNNING.fetch_sub(1, Relaxed);
        return;
    }
    std::thread::spawn(move || {
        use std::os::windows::process::CommandExt;
        // The texts are other people's: they go in as environment variables, never as part of the script, so
        // nothing in a message can run as PowerShell. CreateTextNode does the XML escaping.
        let script = "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; \
             $x = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
             $n = $x.GetElementsByTagName('text'); $n.Item(0).AppendChild($x.CreateTextNode($env:MYTUNE_TOAST_TITLE)) | Out-Null; $n.Item(1).AppendChild($x.CreateTextNode($env:MYTUNE_TOAST_LINE)) | Out-Null; \
             $toast = [Windows.UI.Notifications.ToastNotification]::new($x); \
             [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\WindowsPowerShell\\v1.0\\powershell.exe').Show($toast)";
        let _ = std::process::Command::new("powershell").args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", script]).env("MYTUNE_TOAST_TITLE", title.replace('\0', "")).env("MYTUNE_TOAST_LINE", line.replace('\0', "")).creation_flags(0x0800_0000).output();
        RUNNING.fetch_sub(1, Relaxed);
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
