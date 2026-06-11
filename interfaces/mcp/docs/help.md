# help

## Output rules

- Plain text only. No markdown headings, bold, bullets, or code fences.
- No preamble, no closing commentary beyond the lines below.

## Steps

1. Print the built-in commands, one per line, exactly:

   create-command -- define a new command
   consult-docs -- answer a question from the reference docs
   dashboard -- open or close the live dashboard (pass `stop` to close)
   help -- show this menu

2. Call `list_commands`. Print each saved command on its own line as
   `<name> -- <description>`. If there are none, skip this step.

3. End with exactly one line:

   type a command, or 'create-command' to add one
