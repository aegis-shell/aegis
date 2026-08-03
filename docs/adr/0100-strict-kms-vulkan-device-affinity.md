# ADR-0100: Strict KMS and Vulkan physical-device affinity

- Status: Accepted
- Date: 2026-08-03

## Context

The direct DRM backend selects a KMS primary node through libseat after
checking its connectors, CRTCs, planes, and atomic-modesetting support. Flux
previously made a second, independent choice: it scored every Vulkan physical
device satisfying the renderer requirements and preferred discrete GPUs.

Independent selection is incorrect on hybrid-GPU systems. It can pair KMS on
an integrated GPU with composition on a discrete GPU without an intentional
cross-device copy path. dma-buf import may happen to work on some drivers,
but modifier compatibility, explicit synchronization, direct scanout, power
policy, and buffer ownership no longer share one proven boundary.

Linux-dmabuf feedback identifies the Vulkan renderer and KMS scanout devices,
as established by [ADR-0076](0076-linux-dmabuf-device-feedback-and-reusable-buffer-sync.md),
but feedback describes a choice after creation. It does not constrain Flux's
physical-device selection.

## Decision

Direct DRM sessions bind Flux device creation to the character-device
identity of the live KMS descriptor granted by libseat. Aegis obtains the
major/minor pair with `fstat`, passes it through Flux's typed DRM-node device
extension, and requires `VK_EXT_physical_device_drm` to report the pair as the
candidate's primary or render node. Flux applies its normal feature checks
and scoring only among candidates belonging to that physical GPU.

No match is a startup error containing the KMS path and identity. Aegis does
not fall back to another Vulkan device, use driver-specific environment
selection, or infer that successful dma-buf import makes an unplanned
cross-GPU path supported. `AEGIS_DRM_DEVICE` therefore selects both the KMS
device and the renderer's physical GPU. Nested sessions retain Flux's normal
renderer selection because the outer compositor owns KMS.

The optional explicit-sync extension may retain its existing CPU fence-wait
fallback, but both attempts carry the same mandatory DRM identity. That
fallback changes synchronization implementation, not device affinity.

## Alternatives

- **Keep independent Vulkan scoring and compare afterward.** Rejected because
  post-creation validation cannot select the correct physical device and
  creates resources on a device that must immediately be discarded.
- **Use Vulkan loader or Mesa environment variables.** Rejected because they
  are process-global, implementation-specific controls rather than an Aegis
  and Flux API invariant.
- **Accept any GPU whose dma-buf imports into KMS.** Rejected because one
  successful import does not establish compatible modifiers, synchronization,
  direct scanout, performance, or power behavior for the session.
- **Add an independent renderer-device override now.** Rejected until Aegis
  owns an explicit cross-GPU copy, synchronization, and presentation model.

## Consequences

- Single-GPU and same-GPU hybrid configurations keep the existing zero-copy
  render-to-scanout path with deterministic selection.
- A missing Vulkan ICD or `VK_EXT_physical_device_drm` support for the KMS GPU
  fails closed with the selected device identity.
- Future intentional multi-GPU support requires a new decision and a real
  transfer path; it cannot emerge accidentally from device enumeration order.
- Linux-dmabuf main-device feedback continues to prefer the renderer's render
  node, while the scanout tranche continues to identify the KMS primary node.
