use twilight_model::id::{Id, marker::UserMarker};

use crate::board::{Leaderboard, TOP_SIZE};

const JOIN_HINT: &str =
    "It counts players who turn on Share activity on Discord and leaderboard in the Bitcraft menu.";
const MESSAGE_LIMIT: usize = 2000;

/// Players appear as mentions, which Discord renders as their current server names.
fn ranking(leaderboard: &Leaderboard) -> impl Iterator<Item = String> + '_ {
    leaderboard
        .entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            format!(
                "{}. <@{}> - {:.2} BIT",
                index + 1,
                entry.user_id,
                entry.bits
            )
        })
}

/// The leaderboard as Discord messages, split between lines to fit Discord's
/// length limit. There are none while nobody has a score.
pub fn standings(top: &Leaderboard) -> Vec<String> {
    if top.entries.is_empty() {
        return Vec::new();
    }
    let mut messages = vec!["**Bitcraft leaderboard**".to_owned()];
    for line in ranking(top) {
        let message = messages.last_mut().unwrap();
        if message.len() + 1 + line.len() > MESSAGE_LIMIT {
            messages.push(line);
        } else {
            message.push('\n');
            message.push_str(&line);
        }
    }
    messages
}

/// A player's place on the last published leaderboard, with its age in a
/// Discord timestamp that each reader sees in their own time zone.
pub fn rank(published: &Leaderboard, player: Id<UserMarker>) -> String {
    let Some(updated_at) = published.updated_at else {
        return "The Bitcraft leaderboard has not been published yet.".to_owned();
    };
    let as_of = format!("<t:{}:R>", updated_at.unix_timestamp());
    match published
        .entries
        .iter()
        .position(|entry| entry.user_id == player)
    {
        Some(index) => format!(
            "<@{player}> is #{} on the Bitcraft leaderboard with {:.2} BIT, as of {as_of}.",
            index + 1,
            published.entries[index].bits
        ),
        None => format!(
            "<@{player}> is not in the top {TOP_SIZE} of the Bitcraft leaderboard, as of {as_of}. {JOIN_HINT}"
        ),
    }
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;

    use super::*;

    fn with_players(scores: &[(u64, f64)]) -> Leaderboard {
        let mut leaderboard = Leaderboard::default();
        for &(player, bits) in scores {
            leaderboard.record(Id::new(player), bits, OffsetDateTime::UNIX_EPOCH);
        }
        leaderboard
    }

    #[test]
    fn standings_list_every_player_in_messages_discord_accepts() {
        assert!(standings(&Leaderboard::default()).is_empty());
        assert_eq!(
            standings(&with_players(&[(1, 2.0), (2, 3.125)])),
            ["**Bitcraft leaderboard**\n1. <@2> - 3.12 BIT\n2. <@1> - 2.00 BIT"]
        );

        let scores: Vec<_> = (0..TOP_SIZE as u64)
            .map(|index| (1_400_000_000_000_000_000 + index, 100_000.0 + index as f64))
            .collect();
        let messages = standings(&with_players(&scores).top(TOP_SIZE));
        assert_eq!(messages.len(), 2);
        assert!(
            messages
                .iter()
                .all(|message| message.len() <= MESSAGE_LIMIT)
        );
        assert!(messages[0].starts_with("**Bitcraft leaderboard**\n1. "));
        let lines: Vec<_> = messages
            .iter()
            .flat_map(|message| message.lines())
            .collect();
        assert_eq!(lines.len(), TOP_SIZE + 1);
        assert_eq!(
            lines[TOP_SIZE],
            "50. <@1400000000000000000> - 100000.00 BIT"
        );
    }

    #[test]
    fn rank_reads_the_published_leaderboard() {
        let mut published = with_players(&[(1, 2.0), (2, 3.0), (3, 1.0)]);
        assert_eq!(
            rank(&published, Id::new(1)),
            "The Bitcraft leaderboard has not been published yet."
        );
        published.updated_at = Some(OffsetDateTime::from_unix_timestamp(1_793_188_800).unwrap());
        assert_eq!(
            rank(&published, Id::new(1)),
            "<@1> is #2 on the Bitcraft leaderboard with 2.00 BIT, as of <t:1793188800:R>."
        );
        assert_eq!(
            rank(&published, Id::new(9)),
            format!(
                "<@9> is not in the top 50 of the Bitcraft leaderboard, as of <t:1793188800:R>. {JOIN_HINT}"
            )
        );
    }
}
