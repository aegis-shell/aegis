# ADR-0129: Color management — wp_color_management_v1 server, uniform framebuffer encoding, KMS HDR signaling

- Status: Accepted
- Date: 2026-08-16

## Context

ADR-0001 assigns rendering capabilities to flux and protocol/policy to the
compositor; ADR-0028 defers color management and HDR to "the rendering work
that needs them"; the roadmap's M7 remainder is "basic color management".
Flux gained its color pipeline in optics ADR-0069/0070 (scRGB linear
working space, explicit output transform, parametric + ICC image tags,
deep-color/HDR offscreen containers), which unblocks the compositor side.

The DRM backend's presentation model is one shared horizontal-desktop
framebuffer with per-output source rectangles, not the per-output
presentation surfaces ADR-0028's decided text implies. That reality
constrains what "per-output color" can mean: a single framebuffer carries
exactly one pixel encoding per frame.

## Decision

**Client content is tagged through `wp_color_management_v1` (staging,
advertised at version 1).** The server implements the manager, per-output
and per-surface extension objects, surface feedback, and both image
description creators (parametric and ICC, the latter parsed by flux's
in-tree ICC parser, optics ADR-0070). Tags are double-buffered surface
state applied at commit; the renderer maps them onto flux image color tags
at texture (re)creation — shm upload and zero-copy dma-buf import alike.
The compositor never converts pixels itself (ADR-0001).

**The framebuffer's encoding is a session-wide decision.** SDR is 8-bit
sRGB by default; a 10-bit RGB10A2-class container engages when every
active output opts in (`deep_color`) and every primary plane supports it;
HDR (BT.2020 PQ) engages when every active output opts in (`hdr`) and
advertises ST 2084 through EDID (CTA-861.3 static metadata block). Mixed
HDR/SDR desktops stay SDR — per-output divergence requires per-output
presentation surfaces, a separate architectural change. HDR commits set
the connector `Colorspace` (BT2020_RGB), an `HDR_OUTPUT_METADATA` blob
(CTA-861.3 type 1), and `max bpc`; SDR commits reset them explicitly
because KMS properties are sticky.

**A display profile drives the framebuffer space in SDR sessions.** A
`[[output]]` entry's `icc_profile` path (matrix+TRC profiles, extracted to
flux's parametric model) makes the framebuffer render in that display's
actual space — exact for the single-output session, an approximation over
mixed displays. LUT-only profiles and HDR mode are out of scope.

**Night light is KMS-level, not shader-level.** `[night_light]` programs a
per-channel gain ramp into each CRTC's `GAMMA_LUT` on a 1 Hz-evaluated
schedule with a temperature fade — zero render-pipeline cost, and it
composes with every color mode above.

## Alternatives

- **Per-output presentation surfaces now.** The ADR-0028 text anticipates
  them and mixed HDR/SDR requires them, but the shared-framebuffer DRM
  backend shipped differently and reworking presentation is a milestone of
  its own. The uniform-encoding rule ships correct behavior today and
  keeps that migration open.
- **Convert client content in the compositor (CPU or compositor-side
  shaders).** Rejected by ADR-0001: conversion is flux's job; the
  compositor supplies tags only.
- **wlr_gamma_control_unstable_v1 for night light.** An external-tool
  protocol would serve the same user need, but a built-in scheduled
  feature is the baseline users expect from GNOME/KDE; the protocol can
  still be added later without changing the KMS mechanism.
- **Stay on the 4× 32-bit client fourccs.** Rejected: HDR video clients
  allocate 10-bit buffers; the advertisement now includes XBGR/ABGR2101010
  and the renderer imports them through flux's RGB10A2 support.

## Consequences

- Config grows `[[output]] hdr`, `deep_color`, `icc_profile`, and a
  `[night_light]` section (`enable`, `temperature`, `start`, `end`,
  `fade_seconds`); see `docs/reference/config.md`.
- `OutputInfo` exposes per-output EDID color capabilities
  (`color_caps`), so the settings UI and IPC can offer HDR only where the
  display supports it.
- The pinned optics tag (v0.0.15) predates the flux color pipeline; a
  canonical build needs an optics release containing ADR-0069/0070 and
  the offscreen format/dmabuf-tag work. Local Optics mode carries it
  meanwhile.
- Mixed HDR/SDR multi-output and per-output final transforms are the
  documented follow-ups gated on per-output presentation surfaces.
- `wp_color_manager_v1` is advertised at v1: `windows_scrgb`,
  `windows_bt2100`, and the v2 `get_image_description` answer
  `unsupported_feature` until a client need appears.
