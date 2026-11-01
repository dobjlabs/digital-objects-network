use twilight_model::application::{
    command::{Command, CommandType},
    interaction::application_command::CommandData,
};
use twilight_util::builder::command::CommandBuilder;

const RANK: &str = "rank";

pub fn rank() -> Command {
    CommandBuilder::new(
        RANK,
        "Show your place on the Bitcraft leaderboard",
        CommandType::ChatInput,
    )
    .build()
}

pub fn is_rank(data: &CommandData) -> bool {
    data.name == RANK
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn data(name: &str) -> CommandData {
        serde_json::from_value(json!({"id": "1", "name": name, "type": 1})).unwrap()
    }

    #[test]
    fn rank_is_valid_and_read_by_name() {
        twilight_validate::command::command(&rank()).unwrap();
        assert!(is_rank(&data("rank")));
        assert!(!is_rank(&data("other")));
    }
}
