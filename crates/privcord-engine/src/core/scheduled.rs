//! Scheduled send (build step 10): a message written now that goes out at a time the person picks. It is kept on this
//! device and sent by it when the time comes (or the next time the app runs after that, when the device was off), as
//! an ordinary message from then on. It is not part of the sync log: only one device may send it, and the one it was
//! written on is the one the person chose.

use super::{Core, StorageOp};
use crate::error::ErrorCode;
use privcord_domain::{MessageId, UserId};
use serde::{Deserialize, Serialize};

const KV_SCHEDULED: &str = "scheduled.v1";
/// The most messages waiting at once, and the furthest ahead one may be set.
pub const MAX_SCHEDULED: usize = 50;
pub const MAX_AHEAD_MS: i64 = 366 * 24 * 60 * 60 * 1000;

/// A message waiting for its time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Scheduled {
    pub id: String,
    /// A friend's uid, or a group's id.
    pub target: String,
    pub group: bool,
    pub text: String,
    /// When it goes out (milliseconds).
    pub at: i64,
}

/// A ready-made time for "send later".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuickTime {
    pub label: String,
    /// When it goes out (milliseconds since 1970 UTC).
    pub at: i64,
}

const HOUR_MS: i64 = 60 * 60 * 1000;
const DAY_MS: i64 = 24 * HOUR_MS;

/// Up to three sensible times to send a message later, in the person's own time: in an hour, this evening (6 pm, if it
/// is still at least half an hour away) and tomorrow morning (8 am). `utc_offset_min` is minutes east of UTC.
pub fn quick_times(now_ms: i64, utc_offset_min: i32) -> Vec<QuickTime> {
    let offset = i64::from(utc_offset_min) * 60_000;
    let local = now_ms + offset;
    let midnight_local = local.div_euclid(DAY_MS) * DAY_MS;
    let at_local = |day_start: i64, hour: i64| day_start + hour * HOUR_MS - offset;
    let mut times = vec![QuickTime {
        label: "In an hour".into(),
        // A round five minutes, so it reads as a time someone picked.
        at: (now_ms + HOUR_MS).div_euclid(300_000) * 300_000 + 300_000,
    }];
    let evening = at_local(midnight_local, 18);
    if evening - now_ms >= HOUR_MS / 2 {
        times.push(QuickTime {
            label: "This evening".into(),
            at: evening,
        });
    }
    times.push(QuickTime {
        label: "Tomorrow morning".into(),
        at: at_local(midnight_local + DAY_MS, 8),
    });
    times
}

impl Core {
    /// Ready-made times for "send later", from the person's own clock.
    pub fn quick_times(&self) -> Vec<QuickTime> {
        quick_times(self.now(), self.utc_offset_min)
    }

    /// Messages waiting to be sent, soonest first.
    pub fn scheduled(&self) -> Vec<Scheduled> {
        let mut all: Vec<Scheduled> = self.kv_json(KV_SCHEDULED).unwrap_or_default();
        all.sort_by(|a, b| a.at.cmp(&b.at).then_with(|| a.id.cmp(&b.id)));
        all
    }

    pub(super) fn save_scheduled_list(&mut self, all: &[Scheduled]) -> Result<(), ErrorCode> {
        self.save_scheduled(all)
    }

    fn save_scheduled(&mut self, all: &[Scheduled]) -> Result<(), ErrorCode> {
        let value = serde_json::to_vec(all).map_err(|_| ErrorCode::Storage)?;
        self.ports.storage.apply(&[StorageOp::SetKv {
            key: KV_SCHEDULED.into(),
            value,
        }])?;
        Ok(())
    }

    /// Writes a message to go out at `at`. A time already past sends it at once (the next tick).
    pub fn schedule_message(
        &mut self,
        target: &str,
        group: bool,
        text: &str,
        at: i64,
    ) -> Result<String, ErrorCode> {
        self.require_live()?;
        let text = text.trim();
        if text.is_empty() {
            return Err(ErrorCode::EmptyMessage);
        }
        if text.chars().count() > privcord_domain::limits::MAX_TEXT_LENGTH {
            return Err(ErrorCode::MessageTooLong);
        }
        if group {
            if !self.groups.as_ref().is_some_and(|g| {
                self.uid
                    .as_ref()
                    .is_some_and(|u| g.is_member(target, u.as_str()))
            }) {
                return Err(ErrorCode::NoSuchGroup);
            }
        } else if !UserId::new(target).is_ok_and(|u| self.contacts.contains_key(&u)) {
            return Err(ErrorCode::UnknownContact);
        }
        let now = self.now();
        let mut all = self.scheduled();
        if all.len() >= MAX_SCHEDULED || at > now + MAX_AHEAD_MS {
            return Err(ErrorCode::Busy);
        }
        let mut b = [0u8; 8];
        self.ports.entropy.fill(&mut b).map_err(ErrorCode::from)?;
        let id: String = b.iter().map(|x| format!("{x:02x}")).collect();
        all.push(Scheduled {
            id: id.clone(),
            target: target.to_owned(),
            group,
            text: text.to_owned(),
            at,
        });
        self.save_scheduled(&all)?;
        Ok(id)
    }

    /// Takes a scheduled message back (nothing is sent).
    pub fn cancel_scheduled(&mut self, id: &str) -> Result<(), ErrorCode> {
        let mut all = self.scheduled();
        let before = all.len();
        all.retain(|s| s.id != id);
        if all.len() != before {
            self.save_scheduled(&all)?;
        }
        Ok(())
    }

    /// Sends what is due. One that cannot be sent for good (the friend or group is gone) is dropped; one that fails for
    /// a passing reason stays for the next tick.
    pub(super) fn tick_scheduled(&mut self) {
        let now = self.now();
        let due: Vec<Scheduled> = self
            .scheduled()
            .into_iter()
            .filter(|s| s.at <= now)
            .collect();
        if due.is_empty() {
            return;
        }
        let mut done = Vec::new();
        for s in due {
            let sent: Result<MessageId, ErrorCode> = if s.group {
                self.send_group_message(&s.target, &s.text, None)
            } else {
                match UserId::new(s.target.as_str()) {
                    Ok(u) => self.send_message(&u, &s.text, None),
                    Err(_) => Err(ErrorCode::UnknownContact),
                }
            };
            match sent {
                Ok(_)
                | Err(
                    ErrorCode::UnknownContact
                    | ErrorCode::NoSuchGroup
                    | ErrorCode::DirectNotAllowed
                    | ErrorCode::EmptyMessage
                    | ErrorCode::MessageTooLong,
                ) => {
                    done.push(s.id);
                }
                Err(_) => {}
            }
        }
        let mut all = self.scheduled();
        all.retain(|s| !done.contains(&s.id));
        let _ = self.save_scheduled(&all);
    }
}

#[cfg(test)]
#[allow(clippy::indexing_slicing)]
mod quick_tests {
    use super::*;

    #[test]
    fn offers_an_hour_this_evening_and_tomorrow_morning_in_local_time() {
        // 2023-11-14 10:00 UTC, in a zone 2 hours east: 12:00 local.
        let now = 1_699_956_000_000;
        let t = quick_times(now, 120);
        assert_eq!(t.len(), 3);
        assert_eq!(t[0].label, "In an hour");
        assert!(t[0].at >= now + HOUR_MS && t[0].at < now + HOUR_MS + 300_000 + 1);
        assert_eq!(t[0].at % 300_000, 0);
        // 18:00 local is 16:00 UTC the same day.
        assert_eq!(t[1].at, now + 6 * HOUR_MS);
        // 08:00 local tomorrow is 06:00 UTC the next day.
        assert_eq!(t[2].at, now + 20 * HOUR_MS);
    }

    #[test]
    fn leaves_out_this_evening_once_it_is_too_close_or_past() {
        // 17:45 local.
        let t = quick_times(1_699_956_000_000 + 5 * HOUR_MS + 45 * 60_000, 120);
        assert_eq!(
            t.iter().map(|q| q.label.as_str()).collect::<Vec<_>>(),
            ["In an hour", "Tomorrow morning"]
        );
        // 23:00 local: tomorrow morning is nine hours away.
        let t = quick_times(1_699_956_000_000 + 11 * HOUR_MS, 120);
        assert_eq!(
            t.last().map(|q| q.at - (1_699_956_000_000 + 11 * HOUR_MS)),
            Some(9 * HOUR_MS)
        );
    }
}
