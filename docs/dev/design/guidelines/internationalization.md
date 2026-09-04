# Internationalization

Status: **Partial**.

Tessera shell chrome currently negotiates English (`en-US`) and Simplified
Chinese (`zh-CN`) from POSIX locale variables or BCP 47 tags. Unknown locales
and Traditional Chinese fall back to English. The shell catalog is strongly
typed and embedded so missing keys fail during startup or tests rather than
leaking identifiers into the UI.

Localization is not yet unified across every executable. Some lock-screen
copy uses local English and Chinese selection, and right-to-left layout is not
implemented.

## Message rules

- Store user-visible copy under a typed message identifier; do not scatter
  locale branches through presentation code.
- Translate complete messages, not concatenated fragments.
- Keep application names, protocol names, keyboard glyphs, and user content
  separate from surrounding translatable grammar.
- Format dates, times, numbers, percentages, and units from the active locale
  rather than embedding English punctuation or order.
- Give translators context for control type, variables, and security impact.
- Treat missing or incomplete catalogs as a test failure for a declared
  supported language.

## Layout and text expansion

Components solve from measured text and available bounds. They allow labels
and descriptions to grow vertically, preserve primary actions, and avoid
fixed widths derived from English copy. Test at least long English, complete
Simplified Chinese, and the maximum supported text scale.

Truncation is limited to bounded secondary identifiers such as long window or
application titles. The full value remains available to semantic tools or an
accessible secondary presentation.

## Right-to-left layout

RTL is a reserved design requirement, not a current capability. When added,
layout direction mirrors reading-order relationships:

- leading and trailing replace hard-coded left and right for content layout;
- navigation progression, disclosure indicators, and directional icons
  mirror when their meaning is spatial;
- media controls, clocks, numeric data, and brand marks retain their intrinsic
  direction where appropriate;
- pointer coordinates and physical output geometry remain physical and are
  not silently mirrored in domain models.

## CJK typography

- Resolve a CJK-capable fallback face with compatible weight and vertical
  metrics.
- Avoid synthetic letter spacing intended for Latin uppercase.
- Do not rely on italic styling for semantic emphasis.
- Review line breaking around punctuation and mixed Latin/CJK text.
- Measure icon and text alignment against actual CJK glyph metrics rather
  than a Latin baseline sample alone.

## Adoption work

- Move all Tessera-owned UI copy behind one catalog contract.
- Add parameterized messages, plural support, and locale-aware formatting.
- Introduce layout direction into shared component and geometry APIs.
- Add pseudo-localized expansion and RTL examples to the component catalog.

See [Typography](../foundations/typography.md) and
[Component Documentation](../tooling/component-documentation.md).
