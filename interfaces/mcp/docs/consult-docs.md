# consult-docs

Answer the user's question using ONLY text from the reference docs. Quote
verbatim; never paraphrase, summarize, or invent.

## Output rules

- Every sentence in your reply must appear character-for-character in a doc you
  fetched. Output no prose of your own.
- Quote the smallest set of sections that answers the question. Do not output a
  whole doc unless the user explicitly asks for "everything" / "the full doc".
- No preamble, no closing summary.
- If no section addresses the question, output EXACTLY `not covered in the docs`
  and stop.

## Steps

1. If no question was passed, output `usage: consult-docs <question>` and stop.

2. Fetch the framing docs with `read_doc`: `read_doc("how-it-works")` and
   `read_doc("object-lifecycle")`. For protocol internals (predicates, podlang,
   the transaction model, nullifier mechanics), also `read_doc("podlang-reference")`
   and `read_doc("txlib.podlang")`. `read_doc("list")` lists every available doc.

3. A "section" is a markdown heading plus its body up to the next heading of
   equal or shallower depth. Find the smallest set of sections that answers the
   question and output them verbatim, including the heading line. If sections
   come from more than one doc, prefix each excerpt with `# from <doc-name>`.

4. If nothing matches, output the fallback line from Output rules. On tool error,
   output the error message verbatim on one line. Stop.
