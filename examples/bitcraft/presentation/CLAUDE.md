# Bitcraft presentation pack

`copy/classes.md` and `copy/actions.md` hold every class and action
summary and description apps show. They are written and maintained by hand
by Brian. Never edit, rewrite, reformat, rename or regenerate them, even
when recipes, classes or actions change. (Brian's local Claude Code settings
also deny edits to them.)

When a change leaves the copy wrong or incomplete, tell the user instead:

- which entries now quote wrong amounts or describe behavior that changed;
- which classes or actions need a new entry, and which entries match
  nothing (`generate.py` lists both, with file and line);
- a suggested wording, in your reply, if it helps.

`generate.py`, its icon rules, `CLASS_ART_FILES` and `test_generate.py` are
code and can be changed as usual. Regenerating with
`just bitcraft-presentation` only rewrites `pack.json` and
`assets/actions/` from the copy, which is fine. See `README.md` for the copy
format.
