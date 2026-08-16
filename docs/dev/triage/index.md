# Issue Triage

Case-based debugging knowledge for attributing bug reports between
aegis and everything around it — clients, toolkits, drivers, the
session stack. This directory collects triage know-how so a familiar
symptom resolves in minutes instead of reopening a finished
investigation.

Each page covers one symptom class and captures:

- the invariants that define correct compositor behavior for that
  class,
- a fast diagnostic recipe (protocol traces, logs, reproduction),
- the attribution conclusion and where a fix belongs,
- confirmed third-party behaviors that aegis must not work around.

The shared policy: land a compositor change only when evidence shows
aegis violating the protocol or its own invariants; report confirmed
client bugs upstream and record them here. See
[ADR-0001](../../adr/0001-scope-and-responsibility-boundary.md) for the
responsibility boundary this follows.

## Cases

| Page | Symptom class |
|------|---------------|
| [Cursor Issue Triage](cursor.md) | Cursor appearance and behavior: client vs compositor attribution |

## Adding a case

Write one page per symptom class. State the invariant first, then the
recipe, then the outcome; keep traces short and greppable. Register the
page in the table above.
