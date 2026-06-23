# Repository Contracts

Reference data about the documentation surfaces this repository uses. Read
this when a rule depends on an optional file or directory and you need to
know whether this repository has it.

For the one-time process of installing this governance in a new repository,
see [Adoption](adoption.md).

## Required layout

This guide routes documentation through three gates plus a root-file
exception (see [Routing](routing.md)). Each gate maps to a location:

| Gate | Path | Required | Purpose |
|------|------|----------|---------|
| Time | `docs/adr/` | Optional | Architecture Decision Records (immutable) |
| Audience | `docs/dev/` | Yes | Contributor-only documentation (the firewall) |
| Audience | `docs/dev/documentation/` | Yes | Documentation governance (this directory) |
| Cognitive mode | `docs/tutorials/` | Optional | Learning-oriented walkthroughs |
| Cognitive mode | `docs/how-to/` | Recommended | Task-oriented user guides |
| Cognitive mode | `docs/reference/` | Recommended | API, CLI, configuration, and schema lookup |
| Cognitive mode | `docs/explanation/` | Recommended | Conceptual background and design explanation |
| Root | `README.md` | Yes | Project pitch and shortest successful start path |
| Root | `docs/index.md` | Yes | Documentation entry point |

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
