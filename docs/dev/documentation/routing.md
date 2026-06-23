# Routing

Use this page to decide where new documentation belongs. Documentation in
this repository is routed through **three sequential gates**, then a root-file
exception for files that must live at the repository root.

The gates are not three parallel axes. They are a priority-ordered cascade:
each gate closes a question before the next gate is considered, so every
document lands in exactly one location. Apply the gates in order and stop at
the first match.

## Gate 1 — Time: durable decision records

Question: *Is this a durable technical decision and its context, recorded for
the lifetime of the project?*

If yes: `docs/adr/NNNN-<slug>.md`. Stop.

Architectural Decision Records are immutable once accepted. They are not
learning material and not reference material; they are a historical record of
*why* a decision was made. Once a document enters this gate, the
cognitive-mode questions of Gate 3 do not apply to it.

See [ADR Workflow](adr-workflow.md) for the per-record maintenance rules.

## Gate 2 — Audience: contributor-only

Question: *Is the only intended reader a project contributor (setup, build,
test, release, governance, internal maintenance)?*

If yes: `docs/dev/`. Stop.

`docs/dev/` is the firewall between contributor knowledge and user knowledge.
User-facing documentation must never link into it. Within `docs/dev/`,
sub-types include setup, testing, project layout, release process, and this
governance directory itself.

Explanation documents under `docs/explanation/` are dual-audience:
contributors read them for architectural context, but they live on the user
side of the firewall because users need them too. Contributor docs may link
out to explanation docs; the reverse direction is forbidden.

## Gate 3 — Cognitive mode: user-facing Diátaxis

Question: *How is the user engaging with the material?*

[Diátaxis](https://diataxis.fr/) is a documentation framework that splits
content by cognitive mode: learning, doing, looking up, and understanding.
Apply it to everything that passes through Gates 1 and 2 unchanged:

| Directory | Mode | Style |
|-----------|------|-------|
| `docs/tutorials/` | Learning | Second person. Guarantee success. State the expected outcome at every step. |
| `docs/how-to/` | Doing | Imperative mood. Titles start with "How to". Assume the reader knows the basics. |
| `docs/reference/` | Lookup | Prefer tables and lists. Keep prose minimal and factual. Completeness over narrative flow. |
| `docs/explanation/` | Understanding | Discursive; opinionated when useful. Link to ADRs for specific decision history. |

If content seems to belong in two Diátaxis directories, split it into two
documents rather than blending the styles.

## Root files (location exception)

Some files must live at the repository root because tooling, hosting
platforms, or community convention look for them there. This is a **location
constraint**, orthogonal to content routing. Root files are not a fourth gate;
they are a fixed list with a fixed purpose each:

| File | Conceptual home | Why it is at root |
|------|-----------------|-------------------|
| `README.md` | User pitch (Gate 3 adjacent) | Hosting platforms render it by default |
| `CHANGELOG.md` | Time-adjacent: user-facing release history | Community convention |
| `CONTRIBUTING.md` | Gate 2: contributor workflow | Hosting platforms surface it on PR/issue prompts |
| `SECURITY.md` | Mixed audience: users and researchers | Hosting platforms surface it |
| `LICENSE` | Not classified | Legal requirement |
| `CODE_OF_CONDUCT.md` | Not classified | Community requirement |

Route content into a root file only when it matches that file's fixed
purpose. Do not invent new root markdown files to escape the gates; route into
`docs/` instead.

## Decision order (summary)

1. Durable technical decision? → `docs/adr/`
2. Contributor-only? → `docs/dev/`
3. User-facing: learning → `docs/tutorials/`; doing → `docs/how-to/`; lookup
   → `docs/reference/`; understanding → `docs/explanation/`
4. Matches one of the fixed root files above? → repository root
5. Otherwise: it does not belong on this repository's documentation surface.

## Hard boundaries

- Do not create monolithic documentation pages such as `Documentation.md` or
  `Guide.md`.
- Do not duplicate the `README.md` quick start inside `docs/`; link to it.
- Do not put design rationale in reference pages; move it to explanation docs
  or ADRs.
- Do not put implementation coordinates in explanation docs — no source file
  paths or line numbers, no struct field listings, no API signatures, no
  fenced source blocks. Keep explanation conceptual; move the detail to
  `docs/reference/` and link from there.
- Do not put option tables in tutorials; link to reference docs.
- Do not mix user docs and contributor docs; `docs/dev/` is the firewall.
- Do not create an empty documentation directory without an `index.md`.
- Do not hide project-specific assumptions inside generic governance pages;
  record them as repository contracts in [Repository Contracts](contracts.md).
