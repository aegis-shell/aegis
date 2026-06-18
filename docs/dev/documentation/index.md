# Documentation Governance

Rules for organizing, writing, reviewing, and updating project
documentation. This directory is intentionally project-neutral: it can be
moved into another repository that uses the same `docs/` layout and still
apply with minimal changes.

This is the v1 governance baseline. It should be strict enough to keep
documentation organized, but generic enough to move between repositories
without carrying project-specific facts.

## Use this guide

Before writing, modifying, or archiving documentation:

1. If adopting this guide in a repository, confirm the repository contracts
   in [Adoption](adoption.md).
2. Route the content with [Routing](routing.md).
3. Write it with [Writing Style](style-guide.md).
4. Check whether the code change requires other documentation updates with
   [Update Checklist](update-checklist.md).
5. For common contributor documents, use the patterns in
   [Common Documents](common-docs.md).
6. For architectural decisions, follow [ADR Workflow](adr-workflow.md).
7. Review the result with [Review Checklist](review-checklist.md).

## Directory map

| Page | Purpose |
|------|---------|
| [Adoption](adoption.md) | Repository contracts and migration checklist |
| [Routing](routing.md) | Where each kind of content belongs |
| [Writing Style](style-guide.md) | Voice, headings, formatting, links, and cross-references |
| [Update Checklist](update-checklist.md) | Which docs must change when code changes |
| [Common Documents](common-docs.md) | Intent and structure for recurring project docs |
| [ADR Workflow](adr-workflow.md) | How to create and supersede ADRs |
| [Review Checklist](review-checklist.md) | How maintainers review documentation changes |

## Maintainer rule

This directory is policy, not ordinary project documentation. AI assistants
may read it and suggest improvements, but must not directly modify it. A
human maintainer applies policy changes.
