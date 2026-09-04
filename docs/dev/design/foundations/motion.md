# Motion

Status: **Partial**.

Motion explains spatial or state change without delaying input feedback.
Tessera owns the policy: what moves, when it starts, and how strong it is.
Optics owns any mechanism that touches pixels or GPU work.

## Shared vocabulary

`tessera-ui::motion` provides frame-rate-independent interpolation, cubic
easing, smoothstep, stagger, exponential approach and decay, and an analytic
damped spring. Components retain their own clocks and animation state.

| Motion form | Use |
|------------|-----|
| Immediate | Hover, press, drag attachment, and direct manipulation |
| Ease out | Elements entering or settling toward a destination |
| Ease in | Elements leaving or collapsing toward a destination |
| Smoothstep | Geometry morphs that need quiet velocity at both ends |
| Spring | Material or control motion where slight physical response adds meaning |
| Stagger | Revealing a small ordered group whose sequence communicates hierarchy |

Window transitions have adopted durations: 180 ms for ordinary geometry,
200 ms for open, 180 ms for close, and 200/240/360 ms for scale, suck, and
genie minimize styles. These are window-policy values, not a universal
duration scale for every component.

## Choreography rules

- A response to input begins in the same frame. Animation may explain the
  result but never gates activation.
- Animate semantic properties such as bounds, opacity, or focus strength;
  avoid unrelated simultaneous changes that obscure causality.
- Keep one dominant movement per transition and stagger only when order adds
  information.
- Interruptions retarget from the current presented state instead of jumping
  back to the previous endpoint.
- Pixel deformations and image effects belong in Optics; their strength and
  trigger policy remain in Tessera.

## Reduced motion

`reduced_motion` is one desktop-wide accessibility switch. Every chrome and
window transition resolves to its end state in at most one frame, without a
fade, scale, parallax, or elastic after-motion. Functional state feedback,
color, and content changes remain visible.

## Adoption work

The vocabulary is shared, but duration roles and component choreography are
still distributed. Promote repeated timing only when it represents the same
semantic transition across components. Add interruption and reduced-motion
cases whenever a motion pattern becomes shared.

See [ADR-0029](../../../adr/0029-animation-and-effect-policy.md) and
[ADR-0139](../../../adr/0139-animation-effect-placement.md).
