//! Account preferences that follow the person to every device: pinned chats, archived chats, "mute for a while" and
//! quiet hours. They are small named settings in the sync log (last writer wins per setting, like the muted list), so
//! a pin made on the laptop shows on the phone. Both apps show what the engine holds; neither keeps its own copy.
//!
//! The engine has no clock zone, so the apps pass the local minute of the day when they ask [`Core::should_notify`].

use super::{Core, StorageOp};
use crate::api::EngineEvent;
use crate::error::ErrorCode;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub(super) const KV_PINS: &str = "pref.pins";
pub(super) const KV_ARCHIVED: &str = "pref.archived";
pub(super) const KV_MUTE_UNTIL: &str = "pref.mute";
pub(super) const KV_QUIET: &str = "pref.quiet";
pub(super) const KV_SECURE: &str = "pref.secure";
pub(super) const KV_SILENT: &str = "pref.silent";
/// Every key above; anything starting with this is a synced preference.
pub(super) const PREFIX: &str = "pref.";

/// Pinned chats at the top of the list, at most this many.
pub const MAX_PINS: usize = 3;
/// The most chats kept archived and muted (a guard against a runaway list in the sync log).
const MAX_LISTED: usize = 500;
/// "Muted until": a chat muted with no end. Small enough for JavaScript to hold exactly (below 2^53).
pub const MUTE_FOREVER: i64 = 9_000_000_000_000_000;

/// No notifications between two times of day, in minutes after midnight (the window may cross midnight).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuietHours {
    pub enabled: bool,
    pub from: u16,
    pub to: u16,
}

impl Default for QuietHours {
    fn default() -> Self {
        Self {
            enabled: false,
            from: 22 * 60,
            to: 7 * 60,
        }
    }
}

impl QuietHours {
    /// Is `minute_of_day` (0..1440, in the person's own time) inside the window? An empty window is never quiet.
    pub fn contains(&self, minute_of_day: u16) -> bool {
        if !self.enabled || self.from == self.to {
            return false;
        }
        let t = minute_of_day % 1440;
        if self.from < self.to {
            t >= self.from && t < self.to
        } else {
            t >= self.from || t < self.to
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Prefs {
    /// Conversation ids (a friend's uid, a group id, `chan:name`), in the order they were pinned.
    pub pins: Vec<String>,
    pub archived: Vec<String>,
    /// Conversation id to the time (milliseconds) its notifications are off until; [`MUTE_FOREVER`] for no end.
    pub mute_until: BTreeMap<String, i64>,
    pub quiet: QuietHours,
    /// Hide this account from screenshots, screen recording and the app switcher (each device applies it).
    pub secure_screen: bool,
    /// Chats whose notifications arrive without sound or vibration (the app still gets them, flagged silent).
    pub silent_chats: Vec<String>,
}

impl Prefs {
    /// Pin or unpin: at most [`MAX_PINS`], and the oldest pin drops off when another is added.
    pub fn toggled_pins(pins: &[String], id: &str) -> Vec<String> {
        if pins.iter().any(|p| p == id) {
            return pins.iter().filter(|p| *p != id).cloned().collect();
        }
        let mut next: Vec<String> = pins.to_vec();
        next.push(id.to_owned());
        let drop = next.len().saturating_sub(MAX_PINS);
        next.drain(..drop);
        next
    }

    /// May a notification from `conversation` be shown now? Not while muted, and not in quiet hours.
    pub fn should_notify(&self, conversation: &str, now_ms: i64, minute_of_day: u16) -> bool {
        if self
            .mute_until
            .get(conversation)
            .is_some_and(|until| *until > now_ms)
        {
            return false;
        }
        !self.quiet.contains(minute_of_day)
    }

    /// Drops what has run out (a mute whose time is past).
    fn tidy(&mut self, now_ms: i64) {
        self.mute_until.retain(|_, until| *until > now_ms);
    }
}

fn kv_for(key: &str) -> String {
    format!("{key}.v1")
}

impl Core {
    pub fn prefs(&self) -> Prefs {
        let mut p = self.prefs.clone();
        p.tidy(self.now());
        p
    }

    pub(super) fn load_prefs(&mut self) {
        let get = |this: &Self, key: &str| -> Option<Value> {
            this.ports
                .storage
                .kv(&kv_for(key))
                .ok()
                .flatten()
                .and_then(|b| serde_json::from_slice(&b).ok())
        };
        let mut next = Prefs::default();
        if let Some(v) = get(self, KV_PINS) {
            next.pins = serde_json::from_value(v).unwrap_or_default();
            next.pins.truncate(MAX_PINS);
        }
        if let Some(v) = get(self, KV_ARCHIVED) {
            next.archived = serde_json::from_value(v).unwrap_or_default();
            next.archived.truncate(MAX_LISTED);
        }
        if let Some(v) = get(self, KV_MUTE_UNTIL) {
            next.mute_until = serde_json::from_value(v).unwrap_or_default();
        }
        if let Some(v) = get(self, KV_QUIET) {
            next.quiet = serde_json::from_value(v).unwrap_or_default();
            next.quiet.from %= 1440;
            next.quiet.to %= 1440;
        }
        if let Some(v) = get(self, KV_SECURE) {
            next.secure_screen = v.as_bool().unwrap_or(false);
        }
        if let Some(v) = get(self, KV_SILENT) {
            next.silent_chats = serde_json::from_value(v).unwrap_or_default();
            next.silent_chats.truncate(MAX_LISTED);
        }
        self.prefs = next;
    }

    fn save_pref(&mut self, key: &str, value: Value) -> Result<(), ErrorCode> {
        self.ports.storage.apply(&[StorageOp::SetKv {
            key: kv_for(key),
            value: serde_json::to_vec(&value).map_err(|_| ErrorCode::Storage)?,
        }])?;
        self.hub.publish(EngineEvent::SettingsChanged);
        Ok(())
    }

    /// A conversation is gone: no pin, archive, silence or mute stays for it.
    pub(super) fn forget_chat_prefs(&mut self, conversation: &str) {
        let (mut pins, mut archived, mut silent) = (
            self.prefs.pins.clone(),
            self.prefs.archived.clone(),
            self.prefs.silent_chats.clone(),
        );
        let mut mute = self.prefs.mute_until.clone();
        pins.retain(|c| c != conversation);
        archived.retain(|c| c != conversation);
        silent.retain(|c| c != conversation);
        mute.remove(conversation);
        if pins != self.prefs.pins {
            let _ = self.save_pref(KV_PINS, serde_json::json!(pins));
            self.prefs.pins = pins;
        }
        if archived != self.prefs.archived {
            let _ = self.save_pref(KV_ARCHIVED, serde_json::json!(archived));
            self.prefs.archived = archived;
        }
        if silent != self.prefs.silent_chats {
            let _ = self.save_pref(KV_SILENT, serde_json::json!(silent));
            self.prefs.silent_chats = silent;
        }
        if mute != self.prefs.mute_until {
            let _ = self.save_pref(KV_MUTE_UNTIL, serde_json::json!(mute));
            self.prefs.mute_until = mute;
        }
    }

    /// Pin a chat to the top (or take it off). At most three; the oldest pin drops off.
    pub fn toggle_pin(&mut self, conversation: &str) -> Result<(), ErrorCode> {
        self.require_live()?;
        let next = Prefs::toggled_pins(&self.prefs.pins, conversation);
        self.save_pref(KV_PINS, serde_json::json!(next))?;
        self.prefs.pins = next;
        Ok(())
    }

    /// Archive a chat (hide it from the list until it is brought back), or bring it back.
    pub fn set_archived(&mut self, conversation: &str, archived: bool) -> Result<(), ErrorCode> {
        self.require_live()?;
        let mut next = self.prefs.archived.clone();
        next.retain(|c| c != conversation);
        if archived {
            if next.len() >= MAX_LISTED {
                next.remove(0);
            }
            next.push(conversation.to_owned());
        }
        self.save_pref(KV_ARCHIVED, serde_json::json!(next))?;
        self.prefs.archived = next;
        Ok(())
    }

    /// Deliver a chat's notifications without sound or vibration (or back to normal).
    pub fn set_silent(&mut self, conversation: &str, silent: bool) -> Result<(), ErrorCode> {
        self.require_live()?;
        let mut next = self.prefs.silent_chats.clone();
        next.retain(|c| c != conversation);
        if silent {
            if next.len() >= MAX_LISTED {
                next.remove(0);
            }
            next.push(conversation.to_owned());
        }
        self.save_pref(KV_SILENT, serde_json::json!(next))?;
        self.prefs.silent_chats = next;
        Ok(())
    }

    /// Turn a chat's notifications off until `until_ms` ([`MUTE_FOREVER`] for no end), or on again with `0`.
    pub fn set_mute_until(&mut self, conversation: &str, until_ms: i64) -> Result<(), ErrorCode> {
        self.require_live()?;
        let now = self.now();
        let mut next = self.prefs.mute_until.clone();
        next.retain(|_, u| *u > now);
        if until_ms > now {
            next.insert(conversation.to_owned(), until_ms);
        } else {
            next.remove(conversation);
        }
        self.save_pref(KV_MUTE_UNTIL, serde_json::json!(next))?;
        self.prefs.mute_until = next;
        Ok(())
    }

    /// Hide the app from screenshots, screen recording and the app switcher, on every device of the account.
    pub fn set_secure_screen(&mut self, on: bool) -> Result<(), ErrorCode> {
        self.require_live()?;
        self.save_pref(KV_SECURE, serde_json::json!(on))?;
        self.prefs.secure_screen = on;
        Ok(())
    }

    /// Quiet hours: no notifications between two times of day (minutes after midnight).
    pub fn set_quiet_hours(&mut self, enabled: bool, from: u16, to: u16) -> Result<(), ErrorCode> {
        self.require_live()?;
        let next = QuietHours {
            enabled,
            from: from % 1440,
            to: to % 1440,
        };
        self.save_pref(KV_QUIET, serde_json::json!(next))?;
        self.prefs.quiet = next;
        Ok(())
    }

    /// The person's time zone as minutes east of UTC. The apps tell the engine at start and when it changes (travel,
    /// daylight saving), so quiet hours mean the same on every platform and in the tray.
    pub fn set_utc_offset(&mut self, minutes: i32) {
        self.utc_offset_min = minutes.clamp(-14 * 60, 14 * 60);
    }

    /// Minutes after local midnight.
    fn local_minute_of_day(&self) -> u16 {
        let utc_min = self.now().div_euclid(60_000);
        u16::try_from((utc_min + i64::from(self.utc_offset_min)).rem_euclid(1440)).unwrap_or(0)
    }

    /// May a notification from this conversation be shown now? Not while muted, and not in quiet hours.
    pub fn should_notify(&self, conversation: &str) -> bool {
        self.prefs
            .should_notify(conversation, self.now(), self.local_minute_of_day())
    }

    /// Every notification goes through here: the engine itself keeps quiet for muted chats and in quiet hours, so no
    /// app has to.
    pub(super) fn notify_message(&self, mut n: privcord_ports::Notification) {
        let privcord_ports::Notification::MessageReceived {
            conversation,
            silent,
            ..
        } = &mut n;
        let key = conversation.storage_key();
        *silent = self.prefs.silent_chats.contains(&key);
        if self.should_notify(&key) {
            self.ports.notifier.notify(n);
        }
    }
}

/// What one chat takes on this device.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ChatStorage {
    pub id: String,
    pub messages: u64,
    pub files: u64,
    /// Text plus file bytes.
    pub bytes: u64,
    pub file_bytes: u64,
    pub file_ids: Vec<String>,
}

impl Core {
    /// What each chat takes on this device, biggest first. Messages deleted for everyone are free.
    pub fn storage_by_chat(&self) -> Vec<ChatStorage> {
        let mut by: BTreeMap<String, ChatStorage> = BTreeMap::new();
        // Read from storage a row at a time: only the totals are kept.
        for m in self.ports.storage.messages().unwrap_or_default() {
            if self.deleted.contains(&m.message.message_id) {
                continue;
            }
            let id = m.conversation.storage_key();
            let row = by.entry(id.clone()).or_insert_with(|| ChatStorage {
                id,
                messages: 0,
                files: 0,
                bytes: 0,
                file_bytes: 0,
                file_ids: Vec::new(),
            });
            row.messages += 1;
            row.bytes += m.message.text.len() as u64;
            if let Some(info) = self
                .ports
                .storage
                .file_info(&m.message.message_id)
                .ok()
                .flatten()
            {
                row.files += 1;
                row.file_bytes += info.size;
                row.bytes += info.size;
                row.file_ids.push(m.message.message_id.as_str().to_owned());
            }
        }
        let mut rows: Vec<ChatStorage> = by.into_values().collect();
        rows.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.id.cmp(&b.id)));
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pins(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn at_most_three_pins_and_the_oldest_drops_off() {
        let mut p = Vec::new();
        for id in ["a", "b", "c", "d"] {
            p = Prefs::toggled_pins(&p, id);
        }
        assert_eq!(p, pins(&["b", "c", "d"]));
        assert_eq!(Prefs::toggled_pins(&p, "c"), pins(&["b", "d"]));
    }

    #[test]
    fn quiet_hours_cross_midnight_and_an_empty_window_is_never_quiet() {
        let night = QuietHours {
            enabled: true,
            from: 22 * 60,
            to: 7 * 60,
        };
        assert!(night.contains(23 * 60));
        assert!(night.contains(3 * 60));
        assert!(!night.contains(7 * 60));
        assert!(!night.contains(12 * 60));
        let day = QuietHours {
            enabled: true,
            from: 13 * 60,
            to: 14 * 60,
        };
        assert!(day.contains(13 * 60 + 30));
        assert!(!day.contains(14 * 60));
        assert!(!QuietHours {
            enabled: false,
            ..night
        }
        .contains(23 * 60));
        assert!(!QuietHours {
            enabled: true,
            from: 480,
            to: 480
        }
        .contains(480));
    }

    #[test]
    fn a_muted_chat_and_quiet_hours_both_stop_a_notification() {
        let mut p = Prefs::default();
        p.mute_until.insert("x".into(), 2_000);
        assert!(!p.should_notify("x", 1_000, 600));
        assert!(p.should_notify("x", 3_000, 600), "the mute has run out");
        assert!(p.should_notify("y", 1_000, 600));
        p.quiet = QuietHours {
            enabled: true,
            from: 22 * 60,
            to: 7 * 60,
        };
        assert!(!p.should_notify("y", 1_000, 23 * 60));
        p.mute_until.insert("z".into(), MUTE_FOREVER);
        assert!(!p.should_notify("z", 8_000_000_000_000_000, 600));
    }
}
