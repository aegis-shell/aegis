# Review Checklist

Use this checklist before approving documentation changes.

## Structure

- File passed through the correct gate in [Routing](routing.md); the gates
  are a priority-ordered cascade, not a free choice.
- ADRs (Time gate) are not mixed with explanation or reference material.
- Explanation docs stay conceptual: no source coordinates, struct layouts, API
  signatures, or fenced source blocks. That detail lives in reference pages,
  linked from the explanation.
- User-facing and contributor content are not mixed; `docs/dev/` is the
  firewall between them.
- Root files are used only for their fixed purpose, not as a routing escape
  hatch for content that belongs under `docs/`.
- New documentation directories include an `index.md`.
- Repository-specific rules are recorded as contracts in
  [Repository Contracts](contracts.md), not hidden in generic policy pages.

## Accuracy

- Statements match the current code.
- Symbol names, config keys, CLI flags, schema fields, and protocol names
  are exact.
- Outdated information has been removed.
- Intra-repository links resolve; there are no broken relative paths.

## Writing conventions

- Document has one `H1`.
- Heading levels are not skipped.
- Code blocks have language tags.
- Inline code uses backticks.
- Link text is descriptive.
- Terminology follows the project glossary when one exists.

## Completeness

- New terms are added to the glossary when the project has one.
- New ADRs are registered in `docs/adr/index.md`.
- Config, API, CLI, and schema changes are reflected in `docs/reference/`.
- User-visible changes have a `CHANGELOG.md` "Unreleased" entry when the
  project uses a changelog.

## Portability

- Governance files avoid project names, local product details, and
  one-repository assumptions.
- Optional contracts are conditional: the doc says what happens if the
  target file or directory does not exist.
- AI-generated policy suggestions are reviewed by a human before being
  applied.
