# Contributing to Aegis

Thank you for your interest in contributing to **aegis** (*autonomous surface shell*)!

## Quick Orientation

- **Developer Setup**: Read [Development Setup](docs/dev/setup.md) for build prerequisites, system dependencies, and compiling sibling projects.
- **Architecture**: Review [Architecture Explanation](docs/explanation/architecture.md) and [Architecture Decision Records (ADRs)](docs/adr/index.md) before proposing structural changes.
- **Documentation Rules**: Read [Documentation Governance](docs/dev/documentation/index.md) before writing or modifying documentation.

## Workflow & Guidelines

1. **Branching & Commits**: Write clear, descriptive commit messages following the project's commit history style (e.g. `feat: ...`, `fix: ...`, `refactor: ...`).
2. **Code Style & Formatting**:
   - Run `cargo fmt --all` before committing.
   - Ensure `cargo clippy` passes without warnings on workspace crates.
3. **Tests**: Add unit or integration tests for new functionality where applicable (`cargo test`).
4. **Documentation**:
   - Updates to public interfaces or schemas must update corresponding files in `docs/reference/` and `CHANGELOG.md`.
   - Structural design decisions must be recorded in a new ADR under `docs/adr/` starting from `docs/adr/template.md`.

## Governance Policy Note

The `docs/dev/documentation/` directory contains documentation governance policy. AI assistants and contributors may read it, but updates to governance policy files are reserved for project maintainers.
