# Routing

Use this page to decide where new documentation belongs. If content seems
to belong in two places, split it into two documents.

## Diátaxis layout

| Directory | Purpose | Style |
|-----------|---------|-------|
| `docs/tutorials/` | Learning-oriented | Second person. Guarantee success. State the expected outcome at every step. |
| `docs/how-to/` | Task-oriented | Imperative mood. Titles start with "How to". Assume the reader knows the basics. |
| `docs/reference/` | Lookup-oriented | Prefer tables and lists. Keep prose minimal and factual. Completeness over narrative flow. |
| `docs/explanation/` | Understanding-oriented | Discursive; opinionated when useful. Link to ADRs for specific decision history. |
| `docs/adr/` | Architecture Decision Records | Numbered, immutable, append-only. Supersede accepted ADRs with new ADRs. |
| `docs/dev/` | Contributor docs only | Setup, testing, release, and project maintenance. Never mix with user docs. |

Allowed root-level markdown: `README.md`, `CHANGELOG.md`,
`CONTRIBUTING.md`, `LICENSE`, `SECURITY.md`, and `CODE_OF_CONDUCT.md`.
If a repository does not use one of those files, omit the rule that refers
to it or define the repository's equivalent in [Adoption](adoption.md).

## Decision order

Apply in order; stop at the first match:

1. Records a past architectural decision: `docs/adr/NNNN-<slug>.md`.
2. Needed to set up the development environment or contribute:
   `docs/dev/`.
3. Reader follows step-by-step to learn the system: `docs/tutorials/`.
4. Reader is trying to accomplish a specific named task: `docs/how-to/`.
5. Reader scans for a config key, API field, CLI flag, schema field, or
   exact option: `docs/reference/`.
6. Explains why something works the way it does: `docs/explanation/`.
7. Gives a one-minute pitch plus the minimal run command: `README.md`.

## Hard boundaries

- Do not create monolithic documentation pages such as `Documentation.md`
  or `Guide.md`.
- Do not duplicate the `README.md` quick start inside `docs/`; link to it.
- Do not put design rationale in reference pages; move it to explanation
  docs or ADRs.
- Do not put option tables in tutorials; link to reference docs.
- Do not mix user docs and contributor docs; `docs/dev/` is the firewall.
- Do not create an empty documentation directory without an `index.md`.
- Do not hide project-specific assumptions inside generic governance pages;
  record them as repository contracts in [Adoption](adoption.md).
