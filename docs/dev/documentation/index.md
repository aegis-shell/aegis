# Documentation Governance

Rules for organizing, writing, reviewing, and updating project
documentation. This directory is intentionally project-neutral: it can be
moved into another repository that uses the same `docs/` layout and still
apply with minimal changes.

This is the v1 governance baseline. It should be strict enough to keep
documentation organized, but generic enough to move between repositories
without carrying project-specific facts.

## Documentation model

This repository routes documentation through **three sequential gates**, plus
a root-file exception for files that must live at the repository root:

1. **Time gate** — durable technical decisions and their context go to
   `docs/adr/` as immutable records.
2. **Audience gate** — contributor-only material goes to `docs/dev/`, which is
   the firewall between contributor knowledge and user knowledge.
3. **Cognitive-mode gate** — user-facing material is split by Diátaxis into
   tutorials, how-to, reference, and explanation.

The gates are a priority-ordered cascade, not three parallel axes. Each gate
closes a question before the next gate is considered, so every document lands
in exactly one location. See [Routing](routing.md) for the full decision
flow.

## Use this guide

Before writing, modifying, or archiving documentation:

1. Check which documentation surfaces this repository uses in
   [Repository Contracts](contracts.md).
2. Route the content with [Routing](routing.md).
3. Write it with [Writing Style](style-guide.md).
4. Check whether the code change requires other documentation updates with
   [Update Checklist](update-checklist.md).
5. For architectural decisions, follow [ADR Workflow](adr-workflow.md).
6. Review the result with [Review Checklist](review-checklist.md).

## Directory map

| Page | Purpose |
|------|---------|
| [Repository Contracts](contracts.md) | Layout and optional-file contracts for this repository |
| [Routing](routing.md) | The three gates and where each kind of content belongs |
| [Writing Style](style-guide.md) | Voice, headings, formatting, links, and cross-references |
| [Update Checklist](update-checklist.md) | Which docs must change when code changes |
| [ADR Workflow](adr-workflow.md) | How to create and supersede ADRs |
| [Review Checklist](review-checklist.md) | How maintainers review documentation changes |
| [Adoption](adoption.md) | One-time decisions and checklist for installing this governance |

## Maintainer rule

This directory is policy, not ordinary project documentation. AI assistants
may read it and suggest improvements, but must not directly modify it. A
human maintainer applies policy changes.
