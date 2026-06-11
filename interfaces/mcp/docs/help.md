Print the command menu as terse plain text. List the built-in commands first --
`help`, `create-command`, `consult-docs` -- then the player's saved commands:
call `list_commands` and render each as `name -- description`. No markdown, no
headings. End with one line:

type a command, or 'create-command' to add one
