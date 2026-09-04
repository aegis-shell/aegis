# Complex Data Components

Status: **Draft**.

Tessera has no adopted product-wide data grid, tree, chart, or rich editor.
This page defines the boundary and intake criteria so feature work does not
grow incompatible component families.

## Reserved families

| Family | Intended use | Required capability before adoption |
|-------|--------------|--------------------------------------|
| Data table or grid | Comparable records with stable columns | Keyboard cell navigation, sorting semantics, resizing, virtualization, and accessible headers |
| Tree | Hierarchical resources or settings | Expand/collapse semantics, depth narration, keyboard traversal, and large-tree performance |
| Chart | Trends, distributions, and live metrics | Non-color encoding, data summary, scale labeling, and reduced-motion behavior |
| Rich editor | Structured user-authored content | Selection, input methods, undo/redo, clipboard, accessibility, and durable document semantics |

## Boundary

Generic selection, text editing, accessibility roles, and virtualized layout
mechanics belong in Optics `lens`. Tessera owns product-specific columns,
commands, semantic colors, empty states, and domain actions. A feature must
not implement a private editor engine or data-grid interaction model merely
to ship a single screen.

## Baseline requirements

- Provide a linear keyboard path and a programmatic semantic representation.
- Preserve focus and selection across sorting, filtering, and incremental
  updates by stable identity rather than row index.
- Show loading, empty, partial, stale, and error states without replacing the
  complete component tree unnecessarily.
- Offer a textual equivalent for chart meaning and never distinguish series
  by color alone.
- Bound rendering work for large data and document the virtualization
  threshold.
- Keep destructive bulk actions explicit, scoped, and reversible where the
  domain allows it.

## Adoption gate

A family becomes partial after its generic mechanics and semantic model have
an owner. It becomes adopted after keyboard and accessibility tests, data
volume tests, state examples, and at least two real consumers validate the
same API.

Until then, prefer simple lists, settings rows, or read-only summaries built
from adopted components.
