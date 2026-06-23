# ADR Workflow

ADRs are the maintenance rules for the Time gate in [Routing](routing.md).
Use an Architecture Decision Record when the project records a durable
technical decision and its context.

If the repository does not use ADRs, do not create ad hoc decision notes in
other directories. Either adopt `docs/adr/` first or record the rationale in
`docs/explanation/`.

## When to write an ADR

Write an ADR when a decision is **durable**: reversing it would require
coordinating multiple contributors, breaking compatibility, or significant
rework. Examples include choosing a persistence model, changing the public
module topology, or adopting a protocol.

Do not write an ADR for routine implementation choices that a single
contributor can reverse in a normal PR. Record those in code comments or
`docs/explanation/` instead.

## Statuses

Each ADR carries a status. The canonical set:

| Status | Meaning |
|--------|---------|
| `Proposed` | Drafted and open for discussion; not yet binding. |
| `Accepted` | Decision is final and in effect. |
| `Superseded by ADR-NNNN` | Replaced by a later ADR. The text stays intact. |
| `Rejected` | Considered and not adopted; kept as a record. |
| `Withdrawn` | Proposed but retracted before a decision. |

Only `Accepted` ADRs are binding. `Proposed` ADRs may be referenced for
context but are not yet authoritative.

## Create an ADR

1. Copy `docs/adr/template.md` to `docs/adr/NNNN-<slug>.md`, using the
   next number from `docs/adr/index.md`. If the repository has no template
   or index, create those before requiring ADRs.
2. Fill in context, decision, alternatives, and consequences.
3. Set status to `Proposed` while the decision is under discussion.
4. Add an entry to `docs/adr/index.md`.
5. Set status to `Accepted` when the decision is final.
6. If the ADR introduces or renames a term, update
   `docs/reference/glossary.md` in the same commit when the project has a
   glossary.

## Supersede an ADR

Do not edit an accepted ADR to change its decision. Instead:

1. Write a new ADR whose context references the ADR it replaces.
2. Set the new ADR's status to `Accepted`.
3. Set the old ADR's status to `Superseded by ADR-NNNN` (the new number).
4. Leave the old ADR's decision text intact.
5. Update both rows in `docs/adr/index.md`.
