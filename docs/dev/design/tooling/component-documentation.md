# Component Documentation

Status: **Draft**.

Tessera does not yet ship a Storybook, component catalog, or interactive design
playground. The target is a native `lens` catalog that renders the same code
paths as compositor chrome. A web Storybook is useful only for portable web
components; it is not a pixel-accurate substitute for the native renderer.

## Catalog contract

Each adopted component receives one catalog entry containing:

| Area | Required examples |
|-----|-------------------|
| Anatomy | Named slots, content limits, and token dependencies |
| Variants | Every supported semantic variant; no arbitrary style knobs |
| State | Rest, hover, pressed, focused, disabled, loading, invalid, and selected as applicable |
| Appearance | Dark, light, and future high-contrast variants |
| Content | Short, long, empty, Simplified Chinese, and pseudo-localized copy |
| Layout | Minimum bounds, typical bounds, maximum text scale, and fractional output scale |
| Input | Pointer, keyboard, reduced motion, and semantic inspection |
| Failure | Error, unavailable dependency, and stale or partial data where applicable |

The catalog entry links to the component contract and identifies the owning
crate. It exercises public component APIs rather than copying internal paint
code into a demo.

## Playground boundary

A playground may expose content, bounds, appearance, state, and accessibility
preferences. It must not expose unrestricted color, radius, or animation
sliders that imply unsupported variants. Debug controls are visibly separate
from the rendered component and are excluded from reference captures.

## Reference capture

Reference images record named scenarios with deterministic fonts, locale,
scale, time, and data. Dynamic materials also include hostile backdrop cases.
Captures supplement behavioral tests; they do not make an inaccessible
component acceptable because its pixels stayed unchanged.

## Adoption gate

The catalog becomes partial when one executable can enumerate and render
native component scenarios deterministically. It becomes adopted when CI can
capture those scenarios, reviewers can inspect diffs, and new adopted
components are required to add their state matrix.

See [Quality and Metrics](quality-and-metrics.md) and
[Internationalization](../guidelines/internationalization.md).
