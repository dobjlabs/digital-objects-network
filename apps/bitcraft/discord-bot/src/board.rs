use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use twilight_model::{
    gateway::presence::{Activity, Presence},
    id::{
        Id,
        marker::{ApplicationMarker, UserMarker},
    },
};

/// How many players the published leaderboard lists.
pub const TOP_SIZE: usize = 50;

/// Each player's highest known Total Inventory Value.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Leaderboard {
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub updated_at: Option<OffsetDateTime>,
    /// Highest score first.
    pub entries: Vec<Entry>,
    /// Players whose presence is ignored, listed by hand in the saved store.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub banned: BTreeSet<Id<UserMarker>>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub user_id: Id<UserMarker>,
    pub name: String,
    pub bits: f64,
    #[serde(with = "time::serde::rfc3339")]
    pub achieved_at: OffsetDateTime,
}

impl Leaderboard {
    /// Records `bits` for `user` if it beats their known score. Returns whether anything changed.
    pub fn record(&mut self, user: Id<UserMarker>, bits: f64, now: OffsetDateTime) -> bool {
        if self.banned.contains(&user) {
            return false;
        }
        match self.entries.iter_mut().find(|entry| entry.user_id == user) {
            Some(entry) if bits <= entry.bits => return false,
            Some(entry) => {
                entry.bits = bits;
                entry.achieved_at = now;
            }
            None => self.entries.push(Entry {
                user_id: user,
                // Placeholder until the display name is known.
                name: user.to_string(),
                bits,
                achieved_at: now,
            }),
        }
        self.entries.sort_by(|a, b| {
            b.bits
                .total_cmp(&a.bits)
                .then(a.achieved_at.cmp(&b.achieved_at))
        });
        true
    }

    /// The highest `count` scores alone, with no bans.
    pub fn top(&self, count: usize) -> Leaderboard {
        Leaderboard {
            updated_at: self.updated_at,
            entries: self.entries.iter().take(count).cloned().collect(),
            banned: BTreeSet::new(),
        }
    }

    pub fn has_entry(&self, user: Id<UserMarker>) -> bool {
        self.entries.iter().any(|entry| entry.user_id == user)
    }

    /// Updates `user`'s display name if they have an entry. Returns whether anything changed.
    pub fn rename(&mut self, user: Id<UserMarker>, name: &str) -> bool {
        match self.entries.iter_mut().find(|entry| entry.user_id == user) {
            Some(entry) if entry.name != name => {
                entry.name = name.to_owned();
                true
            }
            _ => false,
        }
    }
}

/// The Total Inventory Value a presence shows for Bitcraft, the application `bitcraft`.
pub fn presence_bits(presence: &Presence, bitcraft: Id<ApplicationMarker>) -> Option<f64> {
    presence
        .activities
        .iter()
        .filter(|activity| activity.application_id == Some(bitcraft))
        .filter_map(activity_bits)
        .max_by(f64::total_cmp)
}

fn activity_bits(activity: &Activity) -> Option<f64> {
    let state = activity.state.as_deref()?;
    let bits = state_bits(state);
    // Presence without inventory metrics has no total. One that mentions BIT but does not parse
    // is a forgery or a desktop format this parser does not know.
    if bits.is_none() && state.contains(" BIT") {
        tracing::warn!(state, "Bitcraft presence without a readable BIT total");
    }
    bits
}

/// Reads the total from the presence state the Bitcraft desktop host writes, such as
/// "31.12 BIT | 0.37 BIT/sec | 1 active job".
fn state_bits(state: &str) -> Option<f64> {
    let (total, _) = state.split_once(" BIT | ")?;
    let (whole, cents) = total.split_once('.')?;
    let digits = |part: &str| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit());
    if !digits(whole) || cents.len() != 2 || !digits(cents) {
        return None;
    }
    total.parse::<f64>().ok().filter(|bits| bits.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const BITCRAFT: u64 = 1_400_000_000_000_000_001;

    fn at(seconds: i64) -> OffsetDateTime {
        OffsetDateTime::from_unix_timestamp(1_761_700_000 + seconds).unwrap()
    }

    fn user(id: u64) -> Id<UserMarker> {
        Id::new(id)
    }

    fn presence(activities: serde_json::Value) -> Presence {
        serde_json::from_value(json!({
            "user": {"id": "7"},
            "guild_id": "9",
            "status": "online",
            "client_status": {"desktop": "online"},
            "activities": activities,
        }))
        .unwrap()
    }

    fn activity(application_id: u64, state: &str) -> serde_json::Value {
        json!({
            "type": 0,
            "name": "Bitcraft",
            "application_id": application_id.to_string(),
            "state": state,
            "created_at": 1_761_700_000_000u64,
        })
    }

    #[test]
    fn reads_the_total_the_desktop_writes() {
        // States the desktop host writes.
        assert_eq!(
            state_bits("31.12 BIT | -0.38 BIT/sec | No active jobs"),
            Some(31.12)
        );
        assert_eq!(
            state_bits("0.00 BIT | 0.00 BIT/sec | No active jobs"),
            Some(0.0)
        );
        assert_eq!(
            state_bits("100.12 BIT | -0.50 BIT/sec | 1 active job"),
            Some(100.12)
        );
        for state in [
            "No active jobs",
            "1 active job",
            "-1.00 BIT | 0.00 BIT/sec | No active jobs",
            "1,234.00 BIT | 0.00 BIT/sec | No active jobs",
            "1.5 BIT | 0.00 BIT/sec | No active jobs",
            "1e3 BIT | 0.00 BIT/sec | No active jobs",
            "NaN BIT | 0.00 BIT/sec | No active jobs",
            ".50 BIT | 0.00 BIT/sec | No active jobs",
            "0.37 BIT/sec | No active jobs",
        ] {
            assert_eq!(state_bits(state), None, "{state}");
        }
        let huge = format!("{}.00 BIT | 0.00 BIT/sec | No active jobs", "9".repeat(400));
        assert_eq!(state_bits(&huge), None);
    }

    #[test]
    fn only_bitcraft_activities_count() {
        let bitcraft = Id::new(BITCRAFT);
        let mixed = presence(json!([
            activity(BITCRAFT + 1, "999.00 BIT | 0.00 BIT/sec | No active jobs"),
            activity(BITCRAFT, "12.50 BIT | 0.25 BIT/sec | 1 active job"),
            {"type": 2, "name": "Spotify", "state": "12345.00 BIT | x"},
        ]));
        assert_eq!(presence_bits(&mixed, bitcraft), Some(12.5));
        let idle = presence(json!([activity(BITCRAFT, "No active jobs")]));
        assert_eq!(presence_bits(&idle, bitcraft), None);
        assert_eq!(presence_bits(&presence(json!([])), bitcraft), None);
    }

    #[test]
    fn keeps_each_players_highest_score_in_order() {
        let mut board = Leaderboard::default();
        assert!(board.record(user(1), 10.0, at(0)));
        assert!(board.record(user(2), 20.0, at(1)));
        assert!(board.record(user(3), 10.0, at(2)));
        assert!(!board.record(user(1), 9.99, at(3)));
        assert!(!board.record(user(1), 10.0, at(3)));
        let order = |board: &Leaderboard| {
            board
                .entries
                .iter()
                .map(|entry| entry.user_id.get())
                .collect::<Vec<_>>()
        };
        // Equal scores rank by who reached them first.
        assert_eq!(order(&board), [2, 1, 3]);
        assert!(board.record(user(3), 25.0, at(4)));
        assert_eq!(order(&board), [3, 2, 1]);
        assert_eq!(board.entries[0].achieved_at, at(4));
        assert_eq!(board.entries[0].name, "3");
        assert!(board.rename(user(3), "Ada"));
        assert!(!board.rename(user(3), "Ada"));
        assert!(!board.rename(user(4), "Nobody"));
        assert_eq!(board.entries[0].name, "Ada");
    }

    #[test]
    fn banned_players_are_ignored() {
        let mut board = Leaderboard::default();
        board.banned.insert(user(1));
        assert!(!board.record(user(1), 2_000_000.0, at(1)));
        assert!(!board.rename(user(1), "Forger"));
        assert!(board.record(user(2), 5.0, at(1)));
        assert_eq!(board.entries.len(), 1);
    }

    #[test]
    fn published_format_round_trips() {
        let mut board = Leaderboard {
            updated_at: Some(at(10)),
            ..Leaderboard::default()
        };
        board.record(user(1_400_000_000_000_000_123), 31.12, at(0));
        board.rename(user(1_400_000_000_000_000_123), "Ada");
        board.banned.insert(user(5));
        let value = serde_json::to_value(&board).unwrap();
        assert_eq!(
            value,
            json!({
                "updatedAt": "2025-10-29T01:06:50Z",
                "entries": [{
                    "userId": "1400000000000000123",
                    "name": "Ada",
                    "bits": 31.12,
                    "achievedAt": "2025-10-29T01:06:40Z",
                }],
                "banned": ["5"],
            })
        );
        assert_eq!(serde_json::from_value::<Leaderboard>(value).unwrap(), board);
        let minimal: Leaderboard = serde_json::from_value(json!({"entries": []})).unwrap();
        assert_eq!(minimal, Leaderboard::default());
    }

    #[test]
    fn the_top_keeps_the_highest_scores_and_no_bans() {
        let mut board = Leaderboard {
            updated_at: Some(at(10)),
            ..Leaderboard::default()
        };
        let players = TOP_SIZE as u64 + 2;
        for player in 1..=players {
            board.record(user(player), player as f64, at(0));
        }
        board.banned.insert(user(999));
        let top = board.top(TOP_SIZE);
        assert_eq!(top.updated_at, board.updated_at);
        assert_eq!(top.entries, board.entries[..TOP_SIZE]);
        assert_eq!(top.entries[0].user_id, user(players));
        let value = serde_json::to_value(&top).unwrap();
        assert!(value.get("banned").is_none(), "{value}");
        assert_eq!(board.top(TOP_SIZE * 2).entries.len(), players as usize);
    }
}
