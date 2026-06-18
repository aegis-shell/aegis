# Writing Style

Use American English: `behavior`, `color`, `organize`, and
`categorized`.

## Voice and tone

- Use second person for tutorials and how-to guides.
- Use imperative mood for how-to steps: "Run `ninja`", not "You should
  run `ninja`".
- Use third person for explanation and reference: "the daemon", not "we"
  or "I".
- Be direct. Avoid hedging. If something depends on conditions, name the
  conditions.
- Do not anthropomorphize: "the daemon forwards the key", not "the daemon
  decides to forward the key".
- Prefer present tense for current behavior. Use future tense only for
  planned work in roadmap or release planning documents.

## Headings

- Use exactly one `H1` per document.
- Use `H2` for sections and `H3` for subsections. Do not skip levels.
- Prefer descriptive noun phrases: "Position Anchors", not "What About
  Position?" or "Understanding Anchors".
- How-to titles are the exception; they start with "How to".

## Inline formatting

| Element | Format |
|---------|--------|
| File paths, CLI flags, config keys, symbol names | `` `backtick` `` |
| Commands to run | Fenced code block with language tag |
| New term on first use | **Bold** plus a definition in the same sentence |
| Emphasis | Italics, used rarely |

## Code blocks

- Always specify the language tag, such as `bash`, `c`, `toml`, or `text`.
- Use `text` for pseudo-output and protocol examples.
- Split long code blocks with prose when a block exceeds about 15 lines.
- Show commands from the repository root unless the surrounding text states
  another working directory.

## Terminology

- Use canonical terms from `docs/reference/glossary.md` when the project has
  one.
- First use: full term. Later uses may use a short form if the first use
  defines it.
- Code symbols must be backticked and never abbreviated; they must stay
  greppable.
- Protocol and standard names must use official spelling.

## Reference pages

- Optimize for lookup, not persuasion.
- Prefer tables, lists, signatures, schemas, and exact defaults.
- Use short prose only to define scope, constraints, or relationships that a
  table cannot express clearly.
- Move rationale, tradeoffs, and history to `docs/explanation/` or
  `docs/adr/`.

## Lists and tables

- Use numbered lists for sequential steps.
- Use bullet lists for unordered collections.
- Use tables for comparison across consistent attributes.
- Every table must have a header row.

## Links

- Link text describes the target: "[Configuration Reference](...)", not
  "[here](...)".
- Use relative paths for intra-repository links.
- Prefer `[topic](...)` over `[see topic](...)`.

## Cross-references

- Link the first mention of a concept in a document to its explanation or
  glossary entry. Later mentions do not need repeated links.
- Explanation docs link to ADRs for decision history.
- ADRs link to explanation docs for background.
- How-to guides link to `docs/reference/` for option tables instead of
  inlining them.
- Glossary entries link back to the primary explanation doc or ADR.
- Do not link from user-facing docs into `docs/dev/`.

## Portability

- Keep governance pages project-neutral. Use placeholders such as "the
  project" instead of repository names.
- Put repository-specific facts in ordinary project docs, not in this
  directory.
- When a rule depends on an optional file, such as `CHANGELOG.md` or
  `docs/reference/glossary.md`, state the condition.
