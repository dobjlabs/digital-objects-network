use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::warn;
use twilight_http::Client;
use twilight_model::{
    channel::message::AllowedMentions,
    id::{Id, marker::ChannelMarker},
};

use crate::board::Leaderboard;

const PODIUM_SIZE: usize = 3;

/// Players appear as mentions, which Discord renders as their current server names.
pub fn new_leader(leaderboard: &Leaderboard) -> Option<String> {
    let leader = leaderboard.entries.first()?;
    let headline = format!(
        "<@{}> is the new #1 on the Bitcraft leaderboard!",
        leader.user_id
    );
    let podium = leaderboard
        .entries
        .iter()
        .take(PODIUM_SIZE)
        .enumerate()
        .map(|(index, entry)| {
            format!(
                "{}. <@{}> - {:.2} BIT",
                index + 1,
                entry.user_id,
                entry.bits
            )
        });
    Some(
        std::iter::once(headline)
            .chain(podium)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

pub async fn post(
    http: Arc<Client>,
    channel_id: Id<ChannelMarker>,
    mut messages: mpsc::UnboundedReceiver<String>,
) {
    let no_pings = AllowedMentions::default();
    while let Some(message) = messages.recv().await {
        if let Err(error) = http
            .create_message(channel_id)
            .content(&message)
            .allowed_mentions(Some(&no_pings))
            .await
        {
            warn!(?error, "Could not announce the new leader");
        }
    }
}

#[cfg(test)]
mod tests {
    use time::OffsetDateTime;

    use super::*;

    #[test]
    fn lists_up_to_three_players_with_their_scores() {
        let mut leaderboard = Leaderboard::default();
        assert_eq!(new_leader(&leaderboard), None);
        leaderboard.record(Id::new(11), 5.0, OffsetDateTime::UNIX_EPOCH);
        assert_eq!(
            new_leader(&leaderboard).unwrap(),
            "<@11> is the new #1 on the Bitcraft leaderboard!\n1. <@11> - 5.00 BIT"
        );
        leaderboard.record(Id::new(12), 31.125, OffsetDateTime::UNIX_EPOCH);
        leaderboard.record(Id::new(13), 1.5, OffsetDateTime::UNIX_EPOCH);
        leaderboard.record(Id::new(14), 0.25, OffsetDateTime::UNIX_EPOCH);
        assert_eq!(
            new_leader(&leaderboard).unwrap(),
            "<@12> is the new #1 on the Bitcraft leaderboard!\n\
             1. <@12> - 31.12 BIT\n\
             2. <@11> - 5.00 BIT\n\
             3. <@13> - 1.50 BIT"
        );
    }
}
