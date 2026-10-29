use twilight_model::id::{Id, marker::UserMarker};

use crate::board::Leaderboard;

const TOP_SIZE: usize = 10;
const JOIN_HINT: &str = "Players appear once they turn on Share activity on Discord and leaderboard in the Bitcraft menu.";

/// Players appear as mentions, which Discord renders as their current server names.
fn ranking(leaderboard: &Leaderboard, count: usize) -> impl Iterator<Item = String> + '_ {
    leaderboard
        .entries
        .iter()
        .take(count)
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

pub fn leaderboard(leaderboard: &Leaderboard) -> String {
    let players = leaderboard.entries.len();
    if players == 0 {
        return format!("No scores yet. {JOIN_HINT}");
    }
    let mut lines = vec!["**Bitcraft leaderboard**".to_owned()];
    lines.extend(ranking(leaderboard, TOP_SIZE));
    if players > TOP_SIZE {
        lines.push(format!("Top {TOP_SIZE} of {players} players."));
    }
    lines.join("\n")
}

pub fn rank(leaderboard: &Leaderboard, player: Id<UserMarker>) -> String {
    let players = leaderboard.entries.len();
    match leaderboard
        .entries
        .iter()
        .position(|entry| entry.user_id == player)
    {
        Some(index) => format!(
            "<@{player}> is #{} of {players} with {:.2} BIT.",
            index + 1,
            leaderboard.entries[index].bits
        ),
        None if leaderboard.banned.contains(&player) => {
            format!("<@{player}> was removed from the leaderboard by a moderator.")
        }
        None => format!("<@{player}> is not on the leaderboard yet. {JOIN_HINT}"),
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
    fn the_leaderboard_shows_the_top_ten() {
        assert_eq!(
            leaderboard(&Leaderboard::default()),
            format!("No scores yet. {JOIN_HINT}")
        );
        assert_eq!(
            leaderboard(&with_players(&[(1, 2.0), (2, 3.0)])),
            "**Bitcraft leaderboard**\n1. <@2> - 3.00 BIT\n2. <@1> - 2.00 BIT"
        );
        let scores: Vec<_> = (1..=12).map(|player| (player, player as f64)).collect();
        let text = leaderboard(&with_players(&scores));
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 12);
        assert_eq!(lines[1], "1. <@12> - 12.00 BIT");
        assert_eq!(lines[10], "10. <@3> - 3.00 BIT");
        assert_eq!(lines[11], "Top 10 of 12 players.");
    }

    #[test]
    fn rank_places_a_player_or_says_why_they_are_absent() {
        let mut leaderboard = with_players(&[(1, 2.0), (2, 3.0), (3, 1.0)]);
        assert_eq!(
            rank(&leaderboard, Id::new(1)),
            "<@1> is #2 of 3 with 2.00 BIT."
        );
        assert_eq!(
            rank(&leaderboard, Id::new(9)),
            format!("<@9> is not on the leaderboard yet. {JOIN_HINT}")
        );
        leaderboard.ban(Id::new(2));
        assert_eq!(
            rank(&leaderboard, Id::new(2)),
            "<@2> was removed from the leaderboard by a moderator."
        );
        assert_eq!(
            rank(&leaderboard, Id::new(1)),
            "<@1> is #1 of 2 with 2.00 BIT."
        );
    }
}
