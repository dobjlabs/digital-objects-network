//! Opt-in MCP prompts that layer a text-game ("MUD") UX over the generic
//! tools, without baking any game logic into the tools or the always-on
//! server instructions. A client invokes `play` to enter the mode; the
//! injected persona discovers the loaded plugin's actions at runtime, so the
//! server stays plugin-agnostic.

use rmcp::model::{GetPromptResult, Prompt, PromptArgument, PromptMessage, PromptMessageRole};
use serde_json::{Map, Value};

use crate::commands::UserCommand;

const PLAY: &str = "play";
const HELP: &str = "help";
const CREATE: &str = "create-command";

/// The persona injected when a client invokes the `play` prompt. Generic over
/// whatever plugin is loaded -- it tells the model to discover commands via the
/// generic tools rather than naming any specific action or class.
const PLAY_PERSONA: &str = include_str!("../docs/play.md");

const HELP_TEXT: &str = "Show the command menu, in game style. Call `list_actions` (and \
`list_classes` for context) for the plugin's built-in commands, and `list_commands` for the \
player's saved commands. Print each as a command line: a short verb form of the name plus what \
it consumes and produces. Put actions that need no inputs first, then saved commands. Plain \
text, aligned, no markdown. Finish with one line: type a command, or 'exit' to leave.";

/// Guided authoring flow injected by the `create-command` prompt.
const CREATE_FLOW: &str = include_str!("../docs/create-command.md");

pub fn list() -> Vec<Prompt> {
    vec![
        Prompt::new(
            PLAY,
            Some(
                "Enter interactive play: turn this chat into a terse text-game (MUD) over the \
                 loaded plugin's actions. Opt-in; leave with 'exit'.",
            ),
            Some(vec![
                PromptArgument::new("command")
                    .with_description(
                        "Optional first command to run on entering, e.g. \"look\" or \
                         \"craft wood\".",
                    )
                    .with_required(false),
            ]),
        ),
        Prompt::new(
            HELP,
            Some(
                "Show the command menu: the loaded plugin's actions plus your saved commands, in \
                 game style.",
            ),
            None,
        ),
        Prompt::new(
            CREATE,
            Some("Define a new reusable command (a macro of steps) and save it."),
            None,
        ),
    ]
}

pub fn get(name: &str, arguments: Option<&Map<String, Value>>) -> Option<GetPromptResult> {
    match name {
        PLAY => {
            let mut messages = vec![PromptMessage::new_text(
                PromptMessageRole::User,
                PLAY_PERSONA,
            )];
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
            }
            Some(GetPromptResult::new(messages).with_description("Interactive play mode"))
        }
        HELP => Some(
            GetPromptResult::new(vec![PromptMessage::new_text(
                PromptMessageRole::User,
                HELP_TEXT,
            )])
            .with_description("Command menu"),
        ),
        CREATE => Some(
            GetPromptResult::new(vec![PromptMessage::new_text(
                PromptMessageRole::User,
                CREATE_FLOW,
            )])
            .with_description("Define a command"),
        ),
        _ => None,
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
    GetPromptResult::new(messages).with_description(command.description.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_exposes_builtin_prompts() {
        let names: Vec<String> = list().into_iter().map(|prompt| prompt.name).collect();
        assert!(names.iter().any(|name| name == PLAY));
        assert!(names.iter().any(|name| name == HELP));
        assert!(names.iter().any(|name| name == CREATE));
    }

    #[test]
    fn get_play_returns_persona_only_without_args() {
        let result = get(PLAY, None).expect("play prompt exists");
        assert_eq!(result.messages.len(), 1);
    }

    #[test]
    fn get_play_appends_first_command_when_provided() {
        let mut args = Map::new();
        args.insert("command".to_string(), Value::String("look".to_string()));
        let result = get(PLAY, Some(&args)).expect("play prompt exists");
        assert_eq!(result.messages.len(), 2);
    }

    #[test]
    fn get_play_ignores_blank_command() {
        let mut args = Map::new();
        args.insert("command".to_string(), Value::String("   ".to_string()));
        let result = get(PLAY, Some(&args)).expect("play prompt exists");
        assert_eq!(result.messages.len(), 1);
    }

    #[test]
    fn get_unknown_prompt_is_none() {
        assert!(get("nope", None).is_none());
    }
}
