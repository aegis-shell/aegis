# Accessibility

Status: **Partial**.

Tessera-owned UI targets Web Content Accessibility Guidelines (WCAG) 2.2 level
AA where its criteria apply to desktop software. WCAG 2.1 AA remains the
compatibility floor. Platform semantics, keyboard access, screen-reader
behavior, and reduced motion are part of the same contract.

The target is normative for new design work. The status remains partial
because automated contrast coverage, complete compositor-chrome semantics,
and shared focus treatment are not yet complete.

## Visual requirements

| Area | Target |
|-----|--------|
| Normal text | Contrast ratio of at least 4.5:1 against its resolved background |
| Large text | Contrast ratio of at least 3:1 when it meets the standard's large-text definition |
| Controls and meaningful graphics | At least 3:1 against adjacent colors where the criterion applies |
| Pointer targets | At least 24 by 24 logical px or sufficient spacing, subject to the standard's exceptions |
| Focus | Visible in every appearance and distinguishable from selection, hover, and validation |
| Color | Never the only means of conveying state or required action |

Adaptive and translucent materials are tested over hostile backdrop classes,
not only against a nominal token pair. Text-bearing Liquid Glass roles carry
a legibility budget; a component must not weaken it locally.

## Keyboard and focus

- Every action available by pointer has a keyboard path unless the action is
  inherently path-based and has an equivalent command.
- Focus order follows reading and task order, not paint order accidents.
- Opening a modal moves focus inside it; closing returns focus to the opener
  or the nearest stable successor.
- Menus, dialogs, lists, and search results define arrow, activation,
  cancellation, and boundary behavior.
- Focus remains visible during keyboard use and is never removed to make a
  screenshot look cleaner.
- No interaction creates a keyboard trap. Modal containment always has a
  documented completion or cancellation path.

## Semantics and screen readers

Interactive elements expose role, accessible name, state, value, and action.
Dynamic errors and status changes are announced without stealing focus.
Password and secret text is never published through semantic observation.

Tessera has an out-of-process AT-SPI adapter for validated application
accessibility trees and actions. Complete screen-reader exposure for every
compositor-owned chrome component remains incomplete and is part of this
guideline's adoption work.

## Motion and preferences

The global `reduced_motion` preference is adopted: all chrome and window
motion resolves to the end state in at most one frame. The desktop preference
model also carries high contrast, typeface, text scale, cursor theme, and
cursor size. Their presence in settings is not proof that every chrome path
applies them; verification follows the matrix below.

## Verification matrix

Every adopted component is checked in both light and dark appearances with:

- keyboard-only navigation and activation;
- pointer at standard and fractional output scale;
- reduced motion enabled;
- maximum supported text scale and long translated content;
- normal and high-contrast preferences once high contrast is implemented;
- semantic inspection and screen-reader traversal where the platform bridge
  exposes the component;
- contrast evaluation for each state and supported backdrop class.

See [Motion](../foundations/motion.md),
[Internationalization](internationalization.md), and
[Quality and Metrics](../tooling/quality-and-metrics.md).
