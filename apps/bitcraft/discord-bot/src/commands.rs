use twilight_model::{
    application::{
        command::{Command, CommandType},
        interaction::{
            InteractionContextType,
            application_command::{CommandData, CommandDataOption, CommandOptionValue},
        },
    },
    guild::Permissions,
    id::{Id, marker::UserMarker},
};
use twilight_util::builder::command::{CommandBuilder, SubCommandBuilder, UserBuilder};

const LEADERBOARD: &str = "leaderboard";
const RANK: &str = "rank";
const MODERATION: &str = "leaderboard-mod";

#[derive(Debug, PartialEq)]
pub enum Request {
    Leaderboard,
    /// The player to rank, or whoever asked.
    Rank(Option<Id<UserMarker>>),
    Moderate(Moderation),
}

#[derive(Debug, PartialEq)]
pub enum Moderation {
    Ban(Id<UserMarker>),
    Unban(Id<UserMarker>),
}

pub fn definitions() -> [Command; 3] {
    let player = |description| UserBuilder::new("player", description).required(true);
    [
        CommandBuilder::new(
            LEADERBOARD,
            "Show the top of the Bitcraft leaderboard",
            CommandType::ChatInput,
        )
        .contexts([InteractionContextType::Guild])
        .build(),
        CommandBuilder::new(
            RANK,
            "Show a player's place on the Bitcraft leaderboard",
            CommandType::ChatInput,
        )
        .contexts([InteractionContextType::Guild])
        .option(UserBuilder::new(
            "player",
            "Player to look up; you, if left out",
        ))
        .build(),
        CommandBuilder::new(
            MODERATION,
            "Moderate the Bitcraft leaderboard",
            CommandType::ChatInput,
        )
        .default_member_permissions(Permissions::MODERATE_MEMBERS)
        .contexts([InteractionContextType::Guild])
        .option(
            SubCommandBuilder::new("ban", "Remove a player's score and ignore their presence")
                .option(player("Player to remove")),
        )
        .option(
            SubCommandBuilder::new("unban", "Count a banned player's presence again")
                .option(player("Player to restore")),
        )
        .build(),
    ]
}

pub fn request(data: &CommandData) -> Option<Request> {
    match (data.name.as_str(), data.options.as_slice()) {
        (LEADERBOARD, []) => Some(Request::Leaderboard),
        (RANK, []) => Some(Request::Rank(None)),
        (
            RANK,
            [
                CommandDataOption {
                    value: CommandOptionValue::User(player),
                    ..
                },
            ],
        ) => Some(Request::Rank(Some(*player))),
        (
            MODERATION,
            [
                CommandDataOption {
                    name: subcommand,
                    value: CommandOptionValue::SubCommand(options),
                },
            ],
        ) => {
            let [
                CommandDataOption {
                    value: CommandOptionValue::User(player),
                    ..
                },
            ] = options.as_slice()
            else {
                return None;
            };
            match subcommand.as_str() {
                "ban" => Some(Request::Moderate(Moderation::Ban(*player))),
                "unban" => Some(Request::Moderate(Moderation::Unban(*player))),
                _ => None,
            }
        }
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
    fn definitions_are_valid_and_only_moderation_is_restricted() {
        for command in definitions() {
            twilight_validate::command::command(&command).unwrap();
            assert_eq!(
                command.default_member_permissions.is_some(),
                command.name == MODERATION,
                "{}",
                command.name
            );
        }
    }

    #[test]
    fn reads_every_command() {
        let player = json!([{"name": "player", "type": 6, "value": "42"}]);
        assert_eq!(
            request(&data("leaderboard", json!([]))),
            Some(Request::Leaderboard)
        );
        assert_eq!(request(&data("rank", json!([]))), Some(Request::Rank(None)));
        assert_eq!(
            request(&data("rank", player.clone())),
            Some(Request::Rank(Some(Id::new(42))))
        );
        assert_eq!(
            request(&data(
                "leaderboard-mod",
                json!([{"name": "ban", "type": 1, "options": player}])
            )),
            Some(Request::Moderate(Moderation::Ban(Id::new(42))))
        );
        assert_eq!(
            request(&data(
                "leaderboard-mod",
                json!([{"name": "unban", "type": 1, "options": player}])
            )),
            Some(Request::Moderate(Moderation::Unban(Id::new(42))))
        );
        for (name, options) in [
            (
                "leaderboard",
                json!([{"name": "ban", "type": 1, "options": player}]),
            ),
            (
                "leaderboard-mod",
                json!([{"name": "ban", "type": 1, "options": []}]),
            ),
            ("other", json!([])),
        ] {
            assert_eq!(
                request(&data(name, options.clone())),
                None,
                "{name} {options}"
            );
        }
    }
}
