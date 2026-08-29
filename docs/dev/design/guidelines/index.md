# Guidelines

Guidelines constrain every foundation, component, pattern, and product
surface. They are review requirements rather than optional polish.

## Inventory

| ID | Guideline | Status | Scope |
|----|-----------|--------|-------|
| 4.1 | [Accessibility](accessibility.md) | Partial | WCAG 2.2 AA target, input parity, semantics, and focus |
| 4.2 | [Voice and Tone](voice-and-tone.md) | Partial | Product language, action labels, and error copy |
| 4.3 | [Internationalization](internationalization.md) | Partial | Locale negotiation, translation, RTL, and CJK behavior |
| 4.4 | [Platform Adaptation](platform-adaptation.md) | Partial | Linux baseline and modality-specific behavior |

## Review order

Review accessibility and internationalization while choosing component
anatomy, not after pixels are fixed. Review voice and tone with interaction
states because copy determines control size and recovery behavior. Apply the
platform baseline last only when it changes a convention without weakening
the other guidelines.

Return to the [Design Language](../index.md).
