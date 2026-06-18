# Update Checklist

Use this checklist when code changes may require documentation changes in
the same commit.

| Change | Documentation action |
|--------|----------------------|
| Public API, CLI flag, config key, schema field, or option changed | Update `docs/reference/` |
| New user-discoverable feature added | Add or update a how-to guide in `docs/how-to/` |
| Install, build, or run steps changed | Update `README.md` quick start and the relevant tutorial |
| Development environment or test commands changed | Update `docs/dev/` |
| Architectural decision made | Write a new ADR in `docs/adr/` |
| User-visible behavior changed | Add an "Unreleased" entry to `CHANGELOG.md` |
| Pure internal refactor with no user-visible effect | No documentation change required |

If the repository does not use one of these documentation surfaces, use the
equivalent recorded in [Adoption](adoption.md). If no equivalent exists,
state that in the PR instead of inventing a one-off location.

## PR note

If a PR touches code with a corresponding documentation surface, the PR
description must state whether documentation was updated or why it was not.
