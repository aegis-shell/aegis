# Quality and Metrics

Status: **Partial**.

Current tests in `tessera-design` enforce exact token values and relationships,
and `tessera-ui` tests shared geometry, state calculations, and motion math.
There is no complete design lint, automated WCAG suite, component visual
catalog, or token-coverage report.

## Quality gates

| Gate | Purpose | Current status |
|-----|---------|----------------|
| Token invariant tests | Preserve role values and cross-appearance relationships | Adopted in `tessera-design` |
| Component behavior tests | Preserve geometry, motion, and interaction calculations | Partial across `tessera-ui` and consumers |
| Design lint | Reject literals or variants that bypass an adopted role | Not implemented |
| Accessibility tests | Check semantics, keyboard paths, focus, targets, and contrast | Partial and mostly manual |
| Visual regression | Compare deterministic component and material scenarios | Material-specific harnesses only |
| Localization layout tests | Exercise expansion, CJK, RTL, and text scale | Partial; RTL unavailable |

## Metrics

Metrics describe adoption and risk; they do not replace review.

| Metric | Definition |
|-------|------------|
| Token coverage | Adopted visual decisions represented by a semantic token divided by all inventoried shared visual decisions |
| Consumer coverage | In-scope surfaces using the adopted token or component API divided by all inventoried consumers of that role |
| State coverage | Cataloged and tested supported states divided by the component's declared state matrix |
| Accessibility coverage | Required checks executed across the component matrix divided by all required checks |
| Literal debt | Unwaived visual literals inside reusable product UI outside their semantic owner |
| Visual drift | Unreviewed reference-image differences for deterministic scenarios |

Every metric report defines its inventory and exclusions. A percentage
without a stable denominator is not a quality signal.

## Waivers

A temporary waiver records the owner, exact scope, reason, expiry condition,
and replacement role or upstream capability. Source media, protocol-defined
colors, and one-off debug presentation may be excluded when their ownership
is explicit. “Looks close enough” is not a waiver reason.

## Initial automation work

- Generate a literal and token-consumer inventory for Tessera-owned UI.
- Add semantic contrast tests for opaque role pairs and backdrop scenario
  tests for translucent text surfaces.
- Add keyboard and focus tests to shared component scenarios.
- Build deterministic native catalog captures before enforcing visual diffs.
- Report missing localization keys and unsupported catalog states in CI.

See [Accessibility](../guidelines/accessibility.md),
[Token Pipeline](token-pipeline.md), and
[Component Documentation](component-documentation.md).
