# Liquid Glass

Liquid Glass is the signature tessera material for the floating control
layer: the Dock, HUD chips, and any chrome body that floats above client
content. It behaves as a thin convex lens — optically flat in its
interior, curved at its rim — rendered analytically from a signed
distance field (SDF) so coverage, refraction, and lighting never
disagree, even at corners.

The compositor evaluates the material at full physical resolution over a
captured backdrop. Two neighboring SDF bodies may be smoothly unioned
into one for spring-driven merges, such as a control lifting out of a
bar.

## Optical model

Each pixel inside the analytic coverage composes these layers in order:

| Layer | Behavior |
|-------|----------|
| Lensing | The sharp backdrop sample displaces inward along the rim normal, strongest at the silhouette, exactly zero in the interior — a magnifier, never a wash |
| Dispersion | Red and blue bend slightly more and less than green, so chromatic fringes live only inside the rim band |
| Scattering | A frost mix across the body that grows through the rim band, simulating soft internal scattering; the body's role scales the interior share |
| Interaction focus | An optional soft field inside the body reduces frost and tint locally, preserves chroma, and applies a restrained directional gain; it reveals one clear target without adding white or drawing an inner rim |
| Adaptive tint | A luminance-opposed body tint: a pearl lift over dark content, a smoke dim over bright content, weakest inside the rim band; decorative bodies pick the polarity per pixel, text-bearing bodies pin it per body |
| Vibrancy | A saturation boost on the transmitted backdrop keeps content lively through the material |
| Key light | A thin (~2 px) highlight hugging the silhouette on the light-facing side, fading around the curve |
| Sheen | A soft, direction-weighted glow across the rim band plus a fresnel term that dies at the bottom — never a full white ring |
| Shadow side | A thin dark line at the silhouette opposite the key light |
| Edge absorption | A faint dark hairline at the silhouette in every direction, stronger over bright content — grazing light dies at the edge, and the body never merges with white |
| Light trough | A faint brightening just inside the bottom rim, where light through the lens pools |
| Drop shadow | The same SDF shifted down, soft falloff, fading with body opacity. It grounds the body and carries separation over uniform bright content |
| Antialiasing | One physical pixel of analytic coverage at the silhouette; sub-LSB dither defeats `rgba8` banding |

The key light sits up-left of the shape (`light_direction` toward the
light). Highlights belong at the top; the bottom edge is the shadow
side. A uniform bright ring around the silhouette is a defect, not the
design.

## Adaptivity

The material reads the blurred backdrop's luminance and opposes it.
Dark content lifts the body toward pearl; bright content dims it
toward smoke, strongly enough that the body stays defined over
uniform white. The tint is weakest inside the rim band, where lensing
and lighting already separate the body from the content. This keeps
glyphs on top of the material legible without a fixed light or dark
style.

Separation over bright content is a system, not a single knob: the
smoke tint, the edge-absorption hairline, and the drop shadow work
together. A body that disappears into a white backdrop is a defect;
weakening any one of the three reintroduces it.

### Role material strengths

The interior is optically flat — refraction lives only in the rim band
— so the reference recipe passes a sharp backdrop through the body
with light frost and a modest tint. Decorative bodies keep that
recipe. Text-bearing bodies cannot: menu and tooltip rows sit directly
on the plate, so their roles multiply interior frost and the adaptive
tint and damp the backdrop's surviving chroma (`frost_strength`,
`tint_strength`, and `saturation` on the role's `GlassStyle`; 1.0 is
the reference recipe). Frost is decoupled from the rim: raising it
thickens interior scattering without touching the lensing, so the
liquid identity stays in the rim while the interior carries the
legibility budget — WCAG AA contrast for the role's text tone over
arbitrary content. The strengths are design tokens, not per-component
choices.

### Pinned plate polarity

Decorative roles keep the shader's per-pixel polarity: each pixel
opposes its own backdrop sample. Text-bearing roles instead pin the
plate polarity for the whole body against their text tone — a smoke
plate under light text, a pearl plate under dark text. Per-pixel
polarity zebra-stripes over mixed content: bright glyphs behind one
menu row flip that row's plate while the next row stays unchanged, and
the row over bright content ends with light text on a lightened plate.
Measuring the backdrop and deriving the polarity from it is equally
wrong for text bodies, because a pearl lift over dark content pulls
the plate toward the light text it must contrast with. Pinning fixes
the direction against the text; measured backdrop statistics modulate
strength only, never direction.

### Region-level backdrop adaptation

The material library measures each identified body's backdrop on the
GPU — mean luminance and high-frequency energy — and returns the
statistics with a frames-in-flight lag. The compositor owns the policy
on top of the raw numbers: exponential temporal smoothing keyed by the
region's stable identity (the first sample after a region appears
snaps, so a freshly opened surface adapts within the stats lag, not
the smoothing constant), quantization to steps far below a visible
difference with hysteresis at each step boundary (an emitted change
re-runs the glass composite, so the shipped values must not dither),
and a polarity-aware recovery that hands tint strength back, down to a
floor, as the measured backdrop approaches the calm state friendly to
the pinned plate. That recovery is where the liquid look lives: the
boosted role strengths are the worst-case budget, not the resting
state.

Adaptation is opt-in. A region declares a stable identity derived from
its layer id; anonymous regions render their declared material
verbatim. The app context menus — the Dock's and the launcher's — and
the Dock's hover surface (tooltip and live preview) adapt today. The
decision and its contrast budget are recorded in
[ADR-0120](../../adr/0120-glass-material-roles-and-region-level-backdrop-adaptation.md);
the per-group material overrides and the GPU statistics reduction are
the Optics-side mechanism recorded in Optics ADR-0063 and ADR-0065.

## Parameters

Distances are physical pixels of the capture image. The compositor
scales the logical values by the output scale before dispatch.
Descriptor parameters set the dispatch-wide look:

| Parameter | Logical default | Meaning |
|-----------|-----------------|---------|
| `refraction` | 8.0 | Maximum rim displacement of the sharp backdrop |
| `chromatic_aberration` | 1.25 | Extra red/blue bend separation in the rim band |
| `edge_width` | 18.0 | Rim band thickness carrying the lens curve |
| `saturation` | 1.08 | Vibrancy multiplier on the transmitted backdrop |
| `brightness` | 1.02 | Exposure multiplier on the body |
| `glare` | 0.55 | Key-light, sheen, and shadow-side strength |
| `light_direction` | (-0.45, -0.89) | Direction toward the key light (up-left) |
| `opacity` | 1.0 | Multiplied into every body's coverage |
| `size_reference` | 72.0 | Body small-side size at which rim and lensing render at full strength; 0 disables size scaling |
| `size_scale_min` | 0.15 | Floor of the size-scaling factor |
| `tint_strength` | 1.0 | Reference adaptive body tint; bodies scale it through their role |
| `frost_strength` | 1.0 | Reference scattering strength; bodies scale it through their role |

Each body additionally carries its own optical character:

| Group field | Meaning |
|-------------|---------|
| `opacity` | Per-body visibility, multiplied into coverage |
| `shadow_alpha` | Drop-shadow strength cap; 0 disables the shadow |
| `shadow_blur` | Drop-shadow falloff softness, used verbatim |
| `shadow_offset_y` | Drop-shadow downward offset, used verbatim |
| `tint_color` | RGB multiplier on the adaptive tint, for accent-tinted glass (white = neutral) |
| `frost_strength` | Per-body multiplier on interior scattering, from the body's role (1.0 = reference recipe) |
| `tint_strength` | Per-body multiplier on the adaptive tint, from the body's role, modulated by backdrop adaptation |
| `saturation` | Per-body multiplier on the backdrop's surviving chroma (below 1.0 damps busy content under text) |
| `plate_polarity` | Plate direction: 0 pins the smoke plate, 1 pins pearl, negative keeps per-pixel adaptive polarity |
| `id` | Stable cross-frame identity opting the body into backdrop adaptation; 0 = anonymous, declared material only |
| `adaptation` | Compositor-fed smoothed backdrop statistics for an identified body; components always declare none |
| `focus` | Rounded-rectangle bounds of one soft interaction field inside the primary body |
| `focus_strength` | Field visibility from 0 to 1; positive focus requires one body and is mutually exclusive with smooth union |

## Scaling with size

Rim band and lensing scale down for bodies smaller than
`size_reference`; a full-size bar uses the parameters as given. Shadow
geometry is *not* rescaled: component-sized shadows are the caller's
policy — the Dock scales its declared shadow by its own morph progress,
and a HUD chip declares a tight shadow outright.

Components select the shared `Chip`, `Tooltip`, `Menu`,
`FloatingPanel`, `ProminentPanel`, or `Dock` role instead of assembling
a shadow tuple or a material recipe. The role table lives in
[Surfaces](surfaces.md). Explicit shadow adjustment is reserved for
continuous geometry such as the Dock's collapse morph.

Only the curve shapes (the lens profile and the falloff curves) are the
material's identity and stay in the shader. Every policy knob —
geometry, lighting, tone, per-body shadow and tint, and size scaling —
is a caller parameter. The boundary rule is recorded in Optics
ADR-0047.

## Usage rules

- Reserve Liquid Glass for the floating control layer: Dock, HUD chips,
  and comparable chrome. Content surfaces use the quiet fills in
  [Surfaces](surfaces.md).
- One region, one body. Declare each floating body once; the SDF pass
  owns its shape, so no rectangular clip or corner patch-up may follow.
- Do not stack glass on glass by default. A body over another glass body
  usually reads as clutter; ordinary upper elements belong to the lower
  material as content. When cumulative optics are intentional, declare an
  explicit `BackdropLayerSource::Layer` edge, keep the upper footprint
  bounded, and review both readability and GPU cost. Paint order alone is
  never a sampling relation.
- Interactive selection inside a glass panel uses that panel's single focus
  field. Its bounds remain inside the primary body. It changes local clarity
  and color-preserving contrast, never coverage, silhouette, shadow, rim, or
  additive white.
- Hover is immediate foreground feedback: a neutral low-alpha wash using the
  shared focus tokens. Selection combines the optical field with the selected
  wash and restrained sibling de-emphasis. Neither state uses an accent or
  outline merely to communicate structure.
- Live preview content uses an analytic `radii.control` clip matching its
  interaction geometry. Every surface in one composed client tree reuses that
  clip; a rectangular scissor may bound work but may not define the visible
  corners. Nonfocused previews stay opaque and recede through brightness,
  because lowering their alpha washes client pixels into the glass below.
- Painted foreground layers on top of glass stay minimal: no painted
  borders, no opaque fills, tint alpha at or below the Dock's resting
  value. The glass rim supplies the edge definition.
- Text-bearing floating bodies use the `Menu` or `Tooltip` role, whose
  recipe carries the legibility budget. Never raise frost, tint, or
  chroma damping ad hoc in a component, and never pick a polarity
  outside the role.
- A body that adapts to its backdrop declares a stable region identity
  derived from its layer id and keeps it for the body's lifetime.
  Anonymous bodies render their declared recipe verbatim.
- Frosted rectangular blur remains the fallback for regions that are not
  analytic bodies and for surfaces whose format the glass pass rejects.

## Motion

Springs and merges animate the SDF parameters — body and focus bounds,
corner radius, union blend radius, focus strength, and per-body opacity —
rather than cross-fading rendered images. A control merging into a bar
shares one SDF body with it, so the neck forms and releases optically instead
of through a two-layer blend. A body in smooth-union motion cannot carry a
focus field in the same frame. With `reduced_motion`, elastic behavior
resolves to its end state immediately.

## Verification

The Optics build tree carries a headless A/B harness,
`liquid_glass_study`, that composites glass bodies over a hostile
backdrop (fine stripes, text rows, saturated blobs, dark and bright
zones) and writes a PPM for pixel-level review. Its panel case includes an
interaction focus field. Run it after any change to the glass shader and
compare the rim, lensing, body tint, and focus field against the
references in this page before judging the change on a live desktop.
