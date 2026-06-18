# ADR Workflow

Use an Architecture Decision Record when the project records a durable
technical decision and its context.

If the repository does not use ADRs, do not create ad hoc decision notes in
other directories. Either adopt `docs/adr/` first or record the rationale in
`docs/explanation/`.

## Create an ADR

1. Copy `docs/adr/template.md` to `docs/adr/NNNN-<slug>.md`, using the
   next number from `docs/adr/index.md`. If the repository has no template
   or index, create those before requiring ADRs.
2. Fill in context, decision, alternatives, and consequences.
3. Add an entry to `docs/adr/index.md`.
4. Set status to `Accepted` when the decision is final.
5. If the ADR introduces or renames a term, update
   `docs/reference/glossary.md` in the same commit when the project has a
   glossary.

## Supersede an ADR

Do not edit an accepted ADR to change its decision. Write a new ADR and set
the old ADR's status to `Superseded by ADR-NNNN`.
