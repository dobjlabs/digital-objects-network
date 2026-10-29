use twilight_model::{
    application::{
        command::{Command, CommandType},
        interaction::{
            InteractionContextType,
            application_command::{CommandData, CommandDataOption, CommandOptionValue},
        },
    },
    id::{Id, marker::UserMarker},
};
use twilight_util::builder::command::{CommandBuilder, UserBuilder};

const RANK: &str = "rank";

/// A `/rank` request: the player to rank, or whoever asked.
#[derive(Debug, PartialEq)]
pub struct Rank(pub Option<Id<UserMarker>>);

pub fn definitions() -> [Command; 1] {
    [CommandBuilder::new(
        RANK,
        "Show a player's place on the Bitcraft leaderboard",
        CommandType::ChatInput,
    )
    .contexts([InteractionContextType::Guild])
    .option(UserBuilder::new(
        "player",
        "Player to look up; you, if left out",
    ))
    .build()]
}

pub fn rank(data: &CommandData) -> Option<Rank> {
    match (data.name.as_str(), data.options.as_slice()) {
        (RANK, []) => Some(Rank(None)),
        (
            RANK,
            [
                CommandDataOption {
                    value: CommandOptionValue::User(player),
                    ..
                },
            ],
        ) => Some(Rank(Some(*player))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn data(name: &str, options: serde_json::Value) -> CommandData {
        serde_json::from_value(json!({
            "id": "1",
            "name": name,
            "type": 1,
            "options": options,
        }))
        .unwrap()
    }

    #[test]
    fn definitions_are_valid_and_open_to_everyone() {
        for command in definitions() {
            twilight_validate::command::command(&command).unwrap();
            assert_eq!(command.default_member_permissions, None, "{}", command.name);
        }
    }

    #[test]
    fn reads_rank() {
        let player = json!([{"name": "player", "type": 6, "value": "42"}]);
        assert_eq!(rank(&data("rank", json!([]))), Some(Rank(None)));
        assert_eq!(
            rank(&data("rank", player.clone())),
            Some(Rank(Some(Id::new(42))))
        );
        for name in ["leaderboard", "leaderboard-mod", "other"] {
            assert_eq!(rank(&data(name, json!([]))), None, "{name}");
        }
    }
}
