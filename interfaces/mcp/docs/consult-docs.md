Answer the user's question about Digital Objects or how they work, using only
the reference docs. The question is passed as the argument.

Call `read_doc` to get the material -- `read_doc("how-it-works")` for the usage
framing, `read_doc("object-lifecycle")` for object states, or `read_doc("list")`
to see every available doc -- then answer briefly, in plain text. Do not invent
facts the docs do not state; if they do not cover it, say so in one line.
