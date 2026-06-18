# Review Checklist

Use this checklist before approving documentation changes.

## Structure

- File is in the correct category; see [Routing](routing.md).
- User-facing and contributor content are not mixed.
- New documentation directories include an `index.md`.
- Repository-specific rules are recorded as contracts in
  [Adoption](adoption.md), not hidden in generic policy pages.

## Accuracy

- Statements match the current code.
- Symbol names, config keys, CLI flags, schema fields, and protocol names
  are exact.
- Outdated information has been removed.

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
