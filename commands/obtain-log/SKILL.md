---
name: bitcraft-obtain-log
description: Bitcraft command — obtain a new Log object by running FindLog. Use when the user types "obtain-log", "find a log", "get a log", or asks bitcraft to produce a Log.
---

# obtain-log

Produce one new `Log` object. `FindLog` takes no inputs.

## Steps

1. Call `run_action` with `action_id="FindLog"` and empty `input_object_paths`.
2. When it returns, report exactly one line:

   ```
   Log → <output_path>
   ```

No commentary. If the call errors, print the error message verbatim and stop.

## Related

- `craft-wood` — refine the new Log into Wood.
