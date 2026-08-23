# Knowledge Layers

How operational knowledge is layered by consumer and moment. Use this
page when deciding where a piece of hard-won knowledge belongs, and
when setting up the directories a project needs to hold it.

## The layers

| Layer | Location | Consumer | Moment |
|-------|----------|----------|--------|
| Triage | Runbook symptom table, in `docs/how-to/` | The operator with an outage | During the incident |
| Diagnosis procedures | `docs/how-to/` | The operator, minutes in | During the incident |
| Failure mechanisms | `docs/explanation/failure-modes.md` or equivalent | Anyone asking why signals mislead | After the incident, or before trusting a signal |
| Incident records | `docs/dev/postmortems/` | Contributors maintaining the project | After the incident |
| Decisions | `docs/adr/` | Anyone questioning a design choice | Any time |

The layers are a pipeline, not alternatives. A single incident
typically touches every layer: the outage is triaged from the runbook
table, diagnosed by procedure, explained as a mechanism, recorded as a
postmortem, and — when it changes a design choice — superseded by an
ADR. Removing one layer breaks a different consumer's path.

## Triage versus incident records

Triage is present-tense routing: a symptom table mapping what an
operator observes to the first commands to run. It lives with the
operational how-to documentation and optimizes for seconds.

An incident record (postmortem) is past-tense analysis: symptom,
investigation with dead ends, root cause as a mechanism, a verifiable
fix, and a transferable lesson. It is numbered, immutable, and
append-only like an ADR, and it lives behind the contributor firewall
because its reader is the person changing the project, not the person
keeping it running.

Merging the two destroys both: root-cause narrative buried in a
runbook slows the operator mid-outage, and quick-lookup rows inside a
postmortem break its evidence chain.

## Where each part goes

When an incident produces knowledge, route each part once:

| The knowledge | Goes to |
|---------------|---------|
| "Symptom X: run Y first" | The triage table in the runbook |
| The full diagnostic procedure for a failure class | A how-to guide |
| Why a health signal could not catch it | The failure-modes explanation page |
| The dated record with evidence and root cause | A numbered postmortem |
| A rule that changes what contributors build | `docs/dev/` pages, linked from the postmortem |
| A change to a design choice | A new ADR superseding the old one |

The postmortem is the record; the other layers are the application. A
lesson that exists only in a postmortem is archaeology. A lesson that
exists only in the applied layers has lost its evidence chain. Write
both, and link them.

## Setting up the postmortem directory

Create `docs/dev/postmortems/` with an `index.md` and a `template.md`
before the first incident, not after. The template fixes five
questions in order: what failed, what the evidence showed, what the
root cause was, what changed, and what generalizes. Classify each
record — blind spot, tooling, drift, monitoring, migration, latent —
so patterns become visible across records.

Include near-misses and audit findings, not only outages. A latent
risk found before it fails carries the same knowledge as one found
after, at a fraction of the cost.
