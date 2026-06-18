# Adoption

Use this page when copying this documentation governance directory into a
new repository or checking whether an existing repository can adopt it.

## Required layout

This guide assumes the repository uses this documentation layout:

| Path | Required | Purpose |
|------|----------|---------|
| `README.md` | Yes | Project pitch and shortest successful start path |
| `docs/index.md` | Yes | Documentation entry point |
| `docs/dev/` | Yes | Contributor-only documentation |
| `docs/dev/documentation/` | Yes | Documentation governance |
| `docs/how-to/` | Recommended | Task-oriented user guides |
| `docs/reference/` | Recommended | API, CLI, configuration, and schema lookup |
| `docs/explanation/` | Recommended | Conceptual background and design explanation |
| `docs/tutorials/` | Optional | Learning-oriented walkthroughs |
| `docs/adr/` | Optional | Architecture Decision Records |

If the target repository does not use this layout, either adapt
[Routing](routing.md) first or keep this directory out of the repository.

## Repository contracts

Some rules refer to common files that not every repository has. Treat them
as contracts:

| Contract | If present | If absent |
|----------|------------|-----------|
| `CHANGELOG.md` | User-visible changes update it in the same commit | Omit changelog checks from review |
| `CONTRIBUTING.md` | Contributor workflow links to `docs/dev/` | Add one before expecting outside contributions |
| `docs/adr/index.md` | ADRs are registered there | Create the index before writing ADRs, or disable ADR workflow |
| `docs/adr/template.md` | New ADRs start from the template | Create a template before requiring ADRs |
| `docs/reference/glossary.md` | New canonical terms update it | Keep terminology local to the relevant doc |
| `docs/reference/api.md` | Public API changes update it | Use the project's equivalent reference surface |
| `docs/tutorials/01-getting-started.md` | Setup changes update it with the README | Update the closest getting-started tutorial instead |

Do not silently assume an optional contract exists. Either add it, link to
the repository's equivalent, or mark that rule as not used by the project.

## Project decisions

Decide these before adopting the guide:

| Decision | Default |
|----------|---------|
| Documentation language | American English |
| Documentation layout | Diataxis under `docs/` |
| Contributor documentation path | `docs/dev/` |
| Architecture decision format | ADRs under `docs/adr/` |
| User-visible change log | `CHANGELOG.md` with an `Unreleased` section |
| AI policy edits | AI may suggest governance changes but must not apply them |

Record deviations in the target repository's root `AGENTS.md` or
equivalent contributor instruction file.

## Adoption checklist

1. Copy `docs/dev/documentation/` into the target repository.
2. Add or update the target repository's root `AGENTS.md` to require this
   guide before documentation changes.
3. Create `docs/index.md` and `docs/dev/index.md` if they do not exist.
4. Decide which optional contracts the repository uses.
5. Update links in `README.md`, `CONTRIBUTING.md`, and `docs/index.md`.
6. Run a link check or manually verify changed links.
7. Keep the governance directory stable after adoption; propose policy
   changes for human review instead of letting routine edits rewrite it.
