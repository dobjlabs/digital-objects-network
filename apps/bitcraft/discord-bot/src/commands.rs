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

const NAME: &str = "leaderboard";

#[derive(Debug, PartialEq)]
pub enum Moderation {
    Ban(Id<UserMarker>),
    Unban(Id<UserMarker>),
}

pub fn leaderboard() -> Command {
    let player = |description| UserBuilder::new("player", description).required(true);
    CommandBuilder::new(
        NAME,
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
    .build()
}

pub fn moderation(data: &CommandData) -> Option<Moderation> {
    if data.name != NAME {
        return None;
    }
    let [
        CommandDataOption {
            name: subcommand,
            value: CommandOptionValue::SubCommand(options),
        },
    ] = data.options.as_slice()
    else {
        return None;
    };
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
        "ban" => Some(Moderation::Ban(*player)),
        "unban" => Some(Moderation::Unban(*player)),
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
    fn definition_is_valid() {
        twilight_validate::command::command(&leaderboard()).unwrap();
    }

    #[test]
    fn reads_ban_and_unban() {
        let player = json!([{"name": "player", "type": 6, "value": "42"}]);
        assert_eq!(
            moderation(&data(
                "leaderboard",
                json!([{"name": "ban", "type": 1, "options": player}])
            )),
            Some(Moderation::Ban(Id::new(42)))
        );
        assert_eq!(
            moderation(&data(
                "leaderboard",
                json!([{"name": "unban", "type": 1, "options": player}])
            )),
            Some(Moderation::Unban(Id::new(42)))
        );
        assert_eq!(
            moderation(&data(
                "other",
                json!([{"name": "ban", "type": 1, "options": player}])
            )),
            None
        );
        assert_eq!(
            moderation(&data(
                "leaderboard",
                json!([{"name": "ban", "type": 1, "options": []}])
            )),
            None
        );
    }
}
