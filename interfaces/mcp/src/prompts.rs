//! Opt-in MCP prompts that layer a text-game UX over the generic tools, without
//! baking game logic into the tools or the always-on server instructions. A
//! client invokes `play` to enter the dispatcher; framework commands (help,
//! create-command, consult-docs, start) are built in here, and user-authored
//! commands come from [`crate::commands`]. Both are surfaced as MCP prompts, so
//! this works in any MCP client.

use rmcp::model::{GetPromptResult, Prompt, PromptArgument, PromptMessage, PromptMessageRole};
use serde_json::{Map, Value};

use crate::commands::UserCommand;

/// The dispatcher entry prompt. Composed server-side (it needs the command
/// store) via [`play_result`], so it is not a plain builtin.
pub const PLAY: &str = "play";

/// Dispatcher rules injected when the player enters `play`.
const PLAY_PERSONA: &str = include_str!("../docs/play.md");

/// A framework command baked into the server: a Case 1 target the dispatcher
/// can route to without any install.
struct Builtin {
    name: &'static str,
    description: &'static str,
    body: &'static str,
}

const BUILTINS: &[Builtin] = &[
    Builtin {
        name: "help",
        description: "Show the command menu: built-ins plus the player's saved commands.",
        body: include_str!("../docs/help.md"),
    },
    Builtin {
        name: "create-command",
        description: "Define a new reusable command (a macro of steps) and save it.",
        body: include_str!("../docs/create-command.md"),
    },
    Builtin {
        name: "consult-docs",
        description: "Answer a question about Digital Objects from the reference docs.",
        body: include_str!("../docs/consult-docs.md"),
    },
    Builtin {
        name: "preview",
        description: "Open the live dashboard (a pane in Claude Code, otherwise a URL to visit).",
        body: include_str!("../docs/preview.md"),
    },
    Builtin {
        name: "preview-stop",
        description: "Close the dashboard preview pane.",
        body: include_str!("../docs/preview-stop.md"),
    },
];

/// The prompts advertised to clients: the `play` entry plus every builtin.
/// User-authored commands are appended by the server (it owns the store).
pub fn list() -> Vec<Prompt> {
    let mut prompts = vec![Prompt::new(
        PLAY,
        Some(
            "Enter interactive play: a terse command game over the loaded plugin's Digital \
             Objects. Opt-in; type 'exit' to leave.",
        ),
        Some(vec![
            PromptArgument::new("command")
                .with_description("Optional first command to run on entering, e.g. \"help\".")
                .with_required(false),
        ]),
    )];
    for builtin in BUILTINS {
        prompts.push(Prompt::new(
            builtin.name,
            Some(builtin.description),
            Some(vec![
                PromptArgument::new("args")
                    .with_description("Optional arguments for the command.")
                    .with_required(false),
            ]),
        ));
    }
    prompts
}

/// Resolve a builtin command by name. `play` is intentionally excluded -- it is
/// composed server-side by [`play_result`] -- as are user commands.
pub fn get(name: &str, arguments: Option<&Map<String, Value>>) -> Option<GetPromptResult> {
    let builtin = BUILTINS.iter().find(|builtin| builtin.name == name)?;
    let mut messages = vec![PromptMessage::new_text(
        PromptMessageRole::User,
        builtin.body,
    )];
    append_args(&mut messages, arguments);
    Some(GetPromptResult::new(messages).with_description(builtin.description))
}

/// Compose the `play` dispatcher: its rules, the live command catalog, and an
/// optional first command. Needs the store's commands, so the server calls it.
pub fn play_result(
    stored: &[UserCommand],
    arguments: Option<&Map<String, Value>>,
) -> GetPromptResult {
    let mut messages = vec![
        PromptMessage::new_text(PromptMessageRole::User, PLAY_PERSONA),
        PromptMessage::new_text(PromptMessageRole::User, catalog_message(stored)),
    ];
    if let Some(command) = arguments
        .and_then(|args| args.get("command"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|command| !command.is_empty())
    {
        messages.push(PromptMessage::new_text(
            PromptMessageRole::User,
            format!("First command: {command}"),
        ));
    } else {
        messages.push(PromptMessage::new_text(
            PromptMessageRole::User,
            "The player just entered. Run the `help` command to show the menu.",
        ));
    }
    GetPromptResult::new(messages).with_description("Interactive play")
}

/// The "Installed commands:" block the dispatcher routes against: builtins
/// first, then saved commands.
fn catalog_message(stored: &[UserCommand]) -> String {
    let mut out = String::from("Installed commands:\n");
    for builtin in BUILTINS {
        out.push_str(&format!("- {} -- {}\n", builtin.name, builtin.description));
    }
    for command in stored {
        out.push_str(&format!("- {} -- {}\n", command.name, command.description));
    }
    out
}

fn append_args(messages: &mut Vec<PromptMessage>, arguments: Option<&Map<String, Value>>) {
    if let Some(args) = arguments
        .and_then(|args| args.get("args"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|args| !args.is_empty())
    {
        messages.push(PromptMessage::new_text(
            PromptMessageRole::User,
            format!("Arguments: {args}"),
        ));
    }
}

/// Build the dynamic prompt entry for a user-authored command.
pub fn user_command_prompt(command: &UserCommand) -> Prompt {
    Prompt::new(
        command.name.clone(),
        Some(command.description.clone()),
        Some(vec![
            PromptArgument::new("args")
                .with_description("Optional arguments passed to the command.")
                .with_required(false),
        ]),
    )
}

/// Inject a user-authored command's body, with any caller arguments appended.
pub fn user_command_result(
    command: &UserCommand,
    arguments: Option<&Map<String, Value>>,
) -> GetPromptResult {
    let mut messages = vec![PromptMessage::new_text(
        PromptMessageRole::User,
        command.body.clone(),
    )];
    append_args(&mut messages, arguments);
    GetPromptResult::new(messages).with_description(command.description.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_exposes_play_and_builtins() {
        let names: Vec<String> = list().into_iter().map(|prompt| prompt.name).collect();
        for expected in [
            PLAY,
            "help",
            "create-command",
            "consult-docs",
            "preview",
            "preview-stop",
        ] {
            assert!(
                names.iter().any(|name| name == expected),
                "missing {expected}"
            );
        }
    }

    #[test]
    fn get_returns_builtin_body() {
        assert!(get("help", None).is_some());
        assert!(get("consult-docs", None).is_some());
    }

    #[test]
    fn get_play_is_not_a_plain_builtin() {
        // `play` is composed server-side (it needs the command store).
        assert!(get(PLAY, None).is_none());
    }

    #[test]
    fn get_unknown_prompt_is_none() {
        assert!(get("nope", None).is_none());
    }

    #[test]
    fn play_result_shows_menu_on_bare_entry() {
        // persona + catalog + the "run help" entry directive
        let result = play_result(&[], None);
        assert_eq!(result.messages.len(), 3);
    }

    #[test]
    fn play_result_runs_first_command_when_given() {
        let mut args = Map::new();
        args.insert("command".to_string(), Value::String("help".to_string()));
        let result = play_result(&[], Some(&args));
        assert_eq!(result.messages.len(), 3);
    }

    #[test]
    fn catalog_lists_builtins_and_stored() {
        let stored = vec![UserCommand {
            name: "stock-up".to_string(),
            description: "gather raw materials".to_string(),
            body: "step".to_string(),
        }];
        let catalog = catalog_message(&stored);
        assert!(catalog.contains("help"));
        assert!(catalog.contains("create-command"));
        assert!(catalog.contains("stock-up"));
    }
}
