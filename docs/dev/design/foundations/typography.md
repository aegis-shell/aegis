# Typography

Status: **Partial**.

The shared type scale is adopted for compositor chrome. Typeface selection,
line-height roles, and end-to-end application of the user's text-scale
preference remain incomplete.

## Type scale

`aegis-design::TypeScale` defines scheme-invariant logical sizes.

| Role | Size | Use |
|-----|------|-----|
| `caption` | 10 px | Dense secondary annotations |
| `footnote` | 11 px | Supporting status and metadata |
| `label` | 12 px | Standard control and row labels |
| `body` | 13 px | Input and reading text |
| `headline` | 15 px | Section and dialog headings |
| `title` | 20 px | Prominent surface titles |
| `hero` | 24 px | Singular high-emphasis values |

Use the semantic role, not a raw size, in reusable components. Hierarchy
comes from role, spacing, and tone together; adding weight or size locally to
repair a weak layout creates a second scale.

## Typeface and scaling

Desktop preferences carry interface and monospace font names plus a text
scale in the validated range `0.5..=3.0`. The current chrome type scale is
widely consumed, but those preferences are not yet propagated through every
rendering path. Until that integration is complete, the settings surface
must not imply that all Aegis chrome already follows the selected typeface or
scale.

## Composition rules

- Use the interface face for chrome and a monospace face only for data whose
  alignment or syntax requires it.
- Let text wrap before reducing its semantic size. Truncate only bounded,
  low-priority labels and preserve an accessible full value.
- Do not encode state with weight or capitalization alone.
- Measure translated strings with the resolved font; byte or character count
  is not a layout metric.
- Keep numerals stable where live metrics would otherwise cause layout
  jitter.
- Define line height as a role before multiple components copy the same
  literal.

## Adoption work

- Propagate interface face, monospace face, and text scale to all chrome.
- Add semantic line-height and weight roles supported by the text engine.
- Test fallback coverage and vertical metrics for Simplified Chinese.
- Add maximum-scale examples for dialogs, settings rows, menus, and toasts.

See [Internationalization](../guidelines/internationalization.md) for CJK and
text-expansion requirements.
