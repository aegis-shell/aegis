# Multimodal Feedback

Status: **Draft**.

Aegis has no shared product sound-effect or haptic implementation. This page
reserves the semantic contract so later feedback does not become a set of
component-local noises or device assumptions.

## Feedback channels

| Channel | Appropriate use | Constraint |
|--------|-----------------|------------|
| Visual | Primary state, progress, result, and recovery | Must remain sufficient when audio and haptics are unavailable. |
| Sound | Time-sensitive confirmation, warning, or background completion | Must respect user volume, mute, and do-not-disturb policy. |
| Haptic | Direct manipulation confirmation on hardware that supports it | Must be optional and never the only carrier of meaning. |

## Semantic events

A future feedback API uses named events such as action confirmed, action
refused, warning, critical alert, drag attachment, and task completed. A
component requests the event; it does not select a file, waveform, playback
device, or vibration pattern.

## Rules

- Provide a silent experience with no loss of function or state awareness.
- Do not play a sound for routine hover, focus movement, or repeated live
  updates.
- Rate-limit recurring warnings and coalesce bursts from the same source.
- Do not use startling feedback for expected validation errors.
- Respect session lock, privacy, do-not-disturb, and output-device changes.
- Stop or transfer continuous feedback when ownership of the interaction
  changes.
- Test without an audio device and on hardware with no haptic capability.

## Adoption gate

This foundation becomes partial only after a shared preference model,
semantic event type, and no-device behavior exist. It becomes adopted after
at least two independent surfaces consume the shared API and automated tests
cover suppression, rate limiting, and fallback behavior.

See [State and Feedback](../patterns/state-and-feedback.md) for the visible
part of the same event contract.
