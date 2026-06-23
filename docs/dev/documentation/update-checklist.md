# Update Checklist

Use this checklist when code changes may require documentation changes in
the same commit. Each row maps a change type to the documentation surface
that owns it; those surfaces correspond to the gates in
[Routing](routing.md).

| Change | Documentation action | Gate |
|--------|----------------------|------|
| Architectural decision made | Write a new ADR in `docs/adr/` | Time |
| Development environment or test commands changed | Update `docs/dev/` | Audience |
| Public API, CLI flag, config key, schema field, or option changed | Update `docs/reference/` | Cognitive mode |
| New user-discoverable feature added | Add or update a how-to guide in `docs/how-to/` | Cognitive mode |
| Feature deprecated or removed | Mark deprecated in `docs/reference/`, update or remove `docs/how-to/` examples, add a migration note to `CHANGELOG.md` | Cognitive mode / Root |
| Install, build, or run steps changed | Update `README.md` quick start and the relevant tutorial | Root / Cognitive mode |
| User-visible behavior changed | Add an "Unreleased" entry to `CHANGELOG.md` | Root |
| Pure internal refactor with no user-visible effect | No documentation change required | — |

If the repository does not use one of these documentation surfaces, use the
equivalent recorded in [Repository Contracts](contracts.md). If no equivalent exists,
state that in the PR instead of inventing a one-off location.

## PR note

If a PR touches code with a corresponding documentation surface, the PR
description must state whether documentation was updated or why it was not.
