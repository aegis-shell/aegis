# Documentation Governance

Rules for organizing, writing, reviewing, and updating project
documentation. This directory is intentionally project-neutral: it can
be moved into another repository that uses the same `docs/` layout and
still apply with minimal changes.

This is the v2 governance baseline: three routing gates (time,
audience, cognitive mode), repository contracts split out of adoption,
a pattern library for recurring documents, and a knowledge-layering
map for operational knowledge.

## Use this guide

Before writing, modifying, or archiving documentation:

1. If adopting this guide in a repository, work through
   [Adoption](adoption.md).
2. Route the content through the three gates in [Routing](routing.md).
3. Write it with [Writing Style](style-guide.md).
4. Check whether the code change requires other documentation updates
   with [Update Checklist](update-checklist.md).
5. For common contributor documents, use the patterns in
   [Common Documents](common-docs.md).
6. For architectural decisions, follow [ADR Workflow](adr-workflow.md).
7. For incident records and operational know-how, read
   [Knowledge Layers](knowledge-layers.md) first.
8. Review the result with [Review Checklist](review-checklist.md).

## Directory map

| Page | Purpose |
|------|---------|
| [Adoption](adoption.md) | One-time decisions and checklist for installing this governance |
| [Repository Contracts](contracts.md) | Layout and optional-file contracts for this repository |
| [Routing](routing.md) | The three gates and where each kind of content belongs |
| [Writing Style](style-guide.md) | Voice, headings, formatting, links, and cross-references |
| [Update Checklist](update-checklist.md) | Which docs must change when code changes |
| [Common Documents](common-docs.md) | Intent and structure for recurring project docs |
| [Knowledge Layers](knowledge-layers.md) | How operational knowledge is layered: triage, procedures, mechanisms, incident records, decisions |
| [ADR Workflow](adr-workflow.md) | How to create and supersede ADRs |
| [Review Checklist](review-checklist.md) | How maintainers review documentation changes |

## Maintainer rule

This directory is policy, not ordinary project documentation. AI assistants
may read it and suggest improvements, but must not directly modify it. A
human maintainer applies policy changes.
