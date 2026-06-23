# Adoption

Use this page when copying this documentation governance directory into a
new repository. It covers the one-time decisions and steps to install the
governance.

For the documentation surfaces an already-adopting repository uses (layout
and optional-file contracts), see [Repository Contracts](contracts.md).

## Project decisions

Decide these before adopting the guide:

| Decision | Default |
|----------|---------|
| Documentation language | American English |
| Documentation layout | Diátaxis under `docs/` |
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
3. Create `docs/index.md`, `docs/dev/index.md`, and an `index.md` for each
   `docs/` subdirectory the repository adopts, if they do not exist.
4. Decide which optional contracts the repository uses; record them per
   [Repository Contracts](contracts.md).
5. Update links in `README.md`, `CONTRIBUTING.md`, and `docs/index.md`.
6. Run a link check or manually verify changed links.
7. Keep the governance directory stable after adoption; propose policy
   changes for human review instead of letting routine edits rewrite it.
