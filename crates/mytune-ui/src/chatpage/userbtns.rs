//! What can be done with another person, shared by the profile card, the member list menu and the message menu.
//! Everything goes through engine actions; the Friend/Block/Note entries follow `Application::relation`.
use mytune_chat::yapfast_core::application::{Action, AppState, PersonAsk, Relation};
use mytune_chat::yapfast_core::chat::RelationshipChange;

pub struct UserAct {
    pub glyph: &'static str,
    pub label: &'static str,
    pub acts: Vec<Action>,
    pub danger: bool,
}

/// Entries for Discord user `user`; empty for yourself or a Matrix id. `mention` is whether a channel is open to put
/// a mention in.
/// The Discord user id the engine's actions take: a message's sender (and a profile opened from one) is
/// `discord:<id>`, which Discord refused as "not snowflake" and no friend or DM lookup matched.
pub fn raw(user: &str) -> &str {
    mytune_chat::yapfast_core::discord::sender_user(user).unwrap_or(user)
}

pub fn list(s: &AppState, user: &str) -> Vec<UserAct> {
    let user = raw(user);
    if user.is_empty() || user.starts_with('@') || s.discord_user.is_none() || s.relation(user) == Relation::Me {
        return vec![];
    }
    let u = || user.to_owned();
    let mut v = vec![UserAct { glyph: "\u{E8BD}", label: "Message", acts: vec![Action::MessagePerson(u())], danger: false }, UserAct { glyph: "\u{E717}", label: "Call", acts: vec![Action::CallPerson(u())], danger: false }];
    if s.selected_room().is_some() {
        v.push(UserAct { glyph: "\u{E910}", label: "Mention", acts: vec![Action::MentionPerson(u())], danger: false });
    }
    let blocked = s.is_blocked(user);
    match s.relation(user) {
        Relation::Friend => v.push(UserAct { glyph: "\u{E8FA}", label: "Remove Friend", acts: vec![Action::AskPerson { user: u(), ask: PersonAsk::Unfriend }], danger: true }),
        Relation::Incoming => v.push(UserAct { glyph: "\u{E73E}", label: "Accept Friend", acts: vec![Action::Befriend(u())], danger: false }),
        Relation::Outgoing => v.push(UserAct { glyph: "\u{E711}", label: "Cancel Request", acts: vec![Action::RemoveFriendRequest(u())], danger: false }),
        Relation::Stranger if !blocked => v.push(UserAct { glyph: "\u{E8FA}", label: "Add Friend", acts: vec![Action::Befriend(u())], danger: false }),
        _ => {}
    }
    v.push(UserAct { glyph: "\u{E70B}", label: "Add Note", acts: vec![Action::AskPerson { user: u(), ask: PersonAsk::Note }], danger: false });
    if blocked {
        v.push(UserAct { glyph: "\u{E8FB}", label: "Unblock", acts: vec![Action::ChangeRelationship { user: u(), change: RelationshipChange::Unblock }], danger: false });
    } else {
        v.push(UserAct { glyph: "\u{E8F8}", label: "Block", acts: vec![Action::AskPerson { user: u(), ask: PersonAsk::Block }], danger: true });
    }
    v
}
