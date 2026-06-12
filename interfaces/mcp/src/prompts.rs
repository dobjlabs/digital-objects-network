//! Opt-in MCP prompts that layer a command UX over the generic tools, without
//! baking command logic into the tools or the always-on server instructions. A
//! client invokes `start` to enter the dispatcher; framework commands (help,
//! create-command, consult-docs, dashboard) are built in here, and user-authored
//! commands come from [`crate::commands`]. A built-in and a saved command are
//! the same shape (name/description/body), so they share one prompt builder and
//! one body-injection path.

use rmcp::model::{GetPromptResult, Prompt, PromptArgument, PromptMessage, PromptMessageRole};
use serde_json::{Map, Value};

use crate::commands::UserCommand;

/// The dispatcher entry prompt. Composed server-side (it needs the command
/// store) via [`start_result`], so it is not a plain builtin.
pub const START: &str = "start";

/// The built-in whose body is generated, not static: its menu is rendered live
/// from [`BUILTINS`] + the saved commands via [`help_command`].
pub const HELP: &str = "help";

/// Dispatcher rules injected when a session starts.
const START_PERSONA: &str = include_str!("../docs/start.md");

/// A framework command baked into the server: a Case 1 target the dispatcher
/// can route to without any install.
struct Builtin {
    name: &'static str,
    description: &'static str,
    body: &'static str,
}

const BUILTINS: &[Builtin] = &[
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
        name: "dashboard",
        description: "Open or close the live dashboard. Pass `stop` to close.",
        body: include_str!("../docs/dashboard.md"),
    },
    // `help`'s body here is just the output framing; the menu is appended live
    // by `help_command`, so the built-in descriptions are not duplicated.
    Builtin {
        name: HELP,
        description: "Show the command menu: built-ins plus saved commands.",
        body: include_str!("../docs/help.md"),
    },
];

fn builtin_to_command(builtin: &Builtin) -> UserCommand {
    UserCommand {
        name: builtin.name.to_string(),
        description: builtin.description.to_string(),
        body: builtin.body.to_string(),
    }
}

/// Whether `name` is the `start` entry or a built-in command -- a name a saved
/// command may not take. The single source of truth for reserved names.
pub fn is_reserved(name: &str) -> bool {
    name == START || BUILTINS.iter().any(|builtin| builtin.name == name)
}

/// The built-in command of this name, as a [`UserCommand`] (the dispatcher and
/// `get_command` load it to follow when the user types the name). `start` is the
/// entry, not a routable command, so it is excluded.
pub fn builtin_command(name: &str) -> Option<UserCommand> {
    BUILTINS
        .iter()
        .find(|builtin| builtin.name == name)
        .map(builtin_to_command)
}

/// The prompts advertised to clients: the `start` entry plus every builtin.
/// User-authored commands are appended by the server (it owns the store).
pub fn list() -> Vec<Prompt> {
    let mut prompts = vec![Prompt::new(
        START,
        Some(
            "Start a command session over the loaded plugin's Digital Objects. Opt-in; type \
             'exit' to leave.",
        ),
        Some(vec![
            PromptArgument::new("command")
                .with_description("Optional first command to run on entering, e.g. \"help\".")
                .with_required(false),
        ]),
    )];
    prompts.extend(
        BUILTINS
            .iter()
            .map(|builtin| command_prompt(&builtin_to_command(builtin))),
    );
    prompts
}

/// Compose the `start` dispatcher: its rules, the live command catalog, and an
/// optional first command. Needs the store's commands, so the server calls it.
pub fn start_result(
    stored: &[UserCommand],
    arguments: Option<&Map<String, Value>>,
) -> GetPromptResult {
    let mut messages = vec![
        PromptMessage::new_text(PromptMessageRole::User, START_PERSONA),
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
            "The session just started. Run the `help` command to show the menu.",
        ));
    }
    GetPromptResult::new(messages).with_description("Command session")
}

/// Every routable command as `(name, description)`: builtins first, then saved.
/// The single source for both the `start` catalog and the `help` menu, so a
/// built-in's description is written once (in [`BUILTINS`]).
fn command_menu(stored: &[UserCommand]) -> Vec<(String, String)> {
    BUILTINS
        .iter()
        .map(|builtin| (builtin.name.to_string(), builtin.description.to_string()))
        .chain(
            stored
                .iter()
                .map(|command| (command.name.clone(), command.description.clone())),
        )
        .collect()
}

/// The "Installed commands:" menu the dispatcher matches against at session
/// start: builtins first, then saved commands.
fn catalog_message(stored: &[UserCommand]) -> String {
    let mut out = String::from("Installed commands:\n");
    for (name, description) in command_menu(stored) {
        out.push_str(&format!("- {name} -- {description}\n"));
    }
    out
}

/// The `help` command, with its menu rendered live from [`BUILTINS`] + the
/// saved commands. The built-in body supplies only the output framing; the
/// descriptions live in [`BUILTINS`] alone, so there is nothing for the doc to
/// keep in sync. The server resolves `help` through this instead of the static
/// builtin so the menu reflects commands saved earlier this session.
pub fn help_command(stored: &[UserCommand]) -> UserCommand {
    let builtin = BUILTINS.iter().find(|builtin| builtin.name == HELP);
    let framing = builtin.map(|builtin| builtin.body).unwrap_or_default();
    let description = builtin
        .map(|builtin| builtin.description)
        .unwrap_or_default();
    let mut body = framing.trim_end().to_string();
    body.push_str("\n\n| Command | Description |\n| --- | --- |\n");
    for (name, desc) in command_menu(stored) {
        body.push_str(&format!(
            "| {} | {} |\n",
            escape_cell(&name),
            escape_cell(&desc)
        ));
    }
    body.push_str("\ntype a command, or 'create-command' to add one");
    UserCommand {
        name: HELP.to_string(),
        description: description.to_string(),
        body,
    }
}

/// Escape a value for a one-line Markdown table cell.
fn escape_cell(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}

/// The prompt entry for a command (built-in or saved): name, description, and an
/// optional `args` string.
pub fn command_prompt(command: &UserCommand) -> Prompt {
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

/// Inject a command's body (built-in or saved) to follow, with any caller
/// arguments appended.
pub fn command_result(
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
    fn list_exposes_start_and_builtins() {
        let names: Vec<String> = list().into_iter().map(|prompt| prompt.name).collect();
        for expected in [START, "help", "create-command", "consult-docs", "dashboard"] {
            assert!(
                names.iter().any(|name| name == expected),
                "missing {expected}"
            );
        }
    }

    #[test]
    fn builtin_command_loads_body_excluding_start() {
        assert!(builtin_command("help").is_some());
        assert!(builtin_command("create-command").is_some());
        assert!(builtin_command("dashboard").is_some());
        assert!(builtin_command("start").is_none());
        assert!(builtin_command("nope").is_none());
    }

    #[test]
    fn is_reserved_covers_start_and_builtins() {
        assert!(is_reserved("start"));
        assert!(is_reserved("dashboard"));
        assert!(!is_reserved("my-command"));
    }

    #[test]
    fn start_result_shows_menu_on_bare_entry() {
        // persona + catalog + the "run help" entry directive
        let result = start_result(&[], None);
        assert_eq!(result.messages.len(), 3);
    }

    #[test]
    fn start_result_runs_first_command_when_given() {
        let mut args = Map::new();
        args.insert("command".to_string(), Value::String("help".to_string()));
        let result = start_result(&[], Some(&args));
        assert_eq!(result.messages.len(), 3);
    }

    #[test]
    fn command_result_appends_args() {
        let command = builtin_command("help").unwrap();
        let mut args = Map::new();
        args.insert("args".to_string(), Value::String("now".to_string()));
        assert_eq!(command_result(&command, Some(&args)).messages.len(), 2);
        assert_eq!(command_result(&command, None).messages.len(), 1);
    }

    #[test]
    fn catalog_lists_builtins_and_stored() {
        let stored = vec![UserCommand {
            name: "stock-up".to_string(),
            description: "gather inputs".to_string(),
            body: "step".to_string(),
        }];
        let catalog = catalog_message(&stored);
        assert!(catalog.contains("help"));
        assert!(catalog.contains("create-command"));
        assert!(catalog.contains("stock-up"));
    }

    #[test]
    fn help_command_renders_live_menu_from_builtins() {
        let stored = vec![UserCommand {
            name: "stock-up".to_string(),
            description: "gather inputs".to_string(),
            body: "step".to_string(),
        }];
        let help = help_command(&stored);
        assert_eq!(help.name, HELP);
        assert!(help.body.contains("| Command | Description |"));
        // Built-in descriptions are sourced from BUILTINS, not re-typed in the
        // doc, so the rendered row matches the catalog verbatim.
        assert!(
            help.body
                .contains("| create-command | Define a new reusable command")
        );
        // Saved commands appear too, so the menu matches what routing resolves.
        assert!(help.body.contains("| stock-up | gather inputs |"));
    }
}
