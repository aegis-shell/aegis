# Common Documents

Use this page as a portable pattern library for recurring project
documents. Each section defines what the document is for, what it should
include, and what it should avoid.

## `docs/dev/testing.md`

Intent: help contributors run, interpret, and extend the test suite.

Should include:

- Scope: what this testing document covers and what it does not cover.
- Test model: unit tests, examples or manual checks, sanitizers, and other
  verification channels.
- Run commands: full suite, single test, verbose logs, and common variants.
- Coverage matrix: test name, what it covers, when to run it, and relevant
  design reference.
- Failure triage: symptoms and likely subsystem to inspect first.
- Sanitizer usage: when to use it and how to configure it.
- Add-test rules: naming, registration, helper APIs, determinism, and
  dependency boundaries.
- Links to setup, examples or manual verification, project layout, and ADRs.

Should not include:

- Full development environment setup.
- Full user-facing build or run instructions.
- Long architectural rationale.
- Detailed visual, platform, or manual verification procedures.

Recommended structure:

```text
# Testing
## Scope
## Test Model
## Run Tests
## Coverage
## Interpret Failures
## Sanitizers
## Add a Test
## Manual or Example Verification
## See Also
```

## `docs/dev/setup.md`

Intent: help contributors create and maintain a working development
environment.

Should include:

- Required toolchain and system dependencies.
- Configure and build commands for contributor builds.
- Optional development features, such as examples, sanitizers, debug flags,
  editor integration, or local dependency overrides.
- How to rebuild after dependency or option changes.
- Common setup failures and first checks.
- Links to testing, project layout, and user-facing quick start.

Should not include:

- User-facing product introduction.
- Feature tutorials.
- Complete API or option reference.
- Release process unless setup directly depends on it.

Recommended structure:

```text
# Setup
## Prerequisites
## Configure
## Build
## Development Options
## Local Dependencies
## Troubleshooting
## See Also
```

## `docs/dev/project-layout.md`

Intent: help contributors find code, understand ownership boundaries, and
place new files.

Should include:

- Source tree map.
- Module-to-purpose table.
- Module-to-design-reference table when ADRs or explanation docs exist.
- Rules for where new source, tests, examples, and docs go.

Should not include:

- Full architecture explanation.
- Step-by-step build instructions.
- Release or roadmap planning.

## `docs/dev/release.md`

Intent: help maintainers perform a repeatable release.

Should include:

- Preconditions.
- Versioning and changelog steps.
- Build and test gates.
- Artifact creation and publication steps.
- Post-release checks.

Should not include:

- General project setup.
- Product roadmap.
- Unstable internal plans.

## `README.md`

Intent: give a new reader the project identity, value, and shortest
successful start path.

Should include:

- What the project is.
- Who it is for.
- The smallest useful install, build, run, or usage command.
- Links to full docs, contribution guide, license, and security policy when
  those files exist.

Should not include:

- Complete API reference.
- Full development setup.
- Long design rationale.
- Every supported option or configuration key.

Recommended structure:

```text
# Project Name
## What It Is
## Quick Start
## Documentation
## Contributing
## License
```

## `CONTRIBUTING.md`

Intent: help contributors understand the expected workflow before opening
issues, patches, or pull requests.

Should include:

- Where contributor docs live.
- Build and test expectations, linked to `docs/dev/`.
- Commit and pull request expectations.
- Documentation update expectations.
- Code of conduct and security links when those files exist.

Should not include:

- Full setup instructions.
- Full test matrices.
- Project roadmap or release planning.

## `CHANGELOG.md`

Intent: record user-visible changes across releases.

Should include:

- An `Unreleased` section when the project tracks pending changes.
- Released versions with dates.
- User-visible additions, fixes, removals, and behavior changes.
- Migration notes when users must change code or configuration.

Should not include:

- Internal refactors with no user-visible effect.
- Full commit logs.
- Design rationale better suited to explanation docs or ADRs.

## `SECURITY.md`

Intent: tell users and researchers how to report security issues safely.

Should include:

- Supported versions or support policy.
- Private reporting channel.
- Expected response process or timeline when the project has one.
- Disclosure expectations.

Should not include:

- Public exploit details.
- General bug reporting instructions.
- Legal language that maintainers cannot honor.

## `docs/reference/api.md`

Intent: provide exact lookup for the public API.

Should include:

- Public symbols, signatures, parameters, return values, and defaults.
- Ownership, lifetime, threading, and error contracts.
- Version or stability notes when the project tracks them.
- Links to how-to guides for task examples.

Should not include:

- Design rationale.
- Step-by-step tutorials.
- Contributor-only implementation details.

## `docs/reference/glossary.md`

Intent: define canonical terms so docs and code use the same vocabulary.

Should include:

- Term.
- Short definition.
- Primary related doc or ADR.
- Avoided synonyms when confusion is likely.

Should not include:

- Long essays.
- Duplicate API reference entries.
- Terms used only once in one local document.
