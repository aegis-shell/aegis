# aegis-atspi

Out-of-process bridge between the Linux AT-SPI accessibility bus and Aegis's
validated semantic tree/action protocol. D-Bus and toolkit objects never enter
the compositor process. The bridge authenticates as a dedicated Actor through the shared
`aegis-ipc-client` library on one persistent, lease-renewed connection,
publishes complete bounded tree revisions, and
verifies revisions again before executing an accessibility action.

Before publication, the bridge requires the AT-SPI application's D-Bus Unix
process id to equal the kernel credential captured from the still-live
Wayland client connection, then requires an exact non-empty toplevel title to
select one window. Process ids travel only over a provider-only IPC endpoint.
Missing, stale, or ambiguous identity correlation publishes nothing.

In a production direct session the compositor supervises this executable and
provisions a compositor-lifetime system principal. Its credential is sent
through an inherited stdin pipe and is never persisted, placed in argv, or
exported in the environment. The executable refuses unsupervised startup;
provider capabilities are not available through ordinary Agent pairing or
administrator pre-provisioning. Nested sessions deliberately do not attach
to the host accessibility bus.

Password contents are never requested or published. Immediately before an
action, the adapter re-reads the live target's role, state, bounds, names,
bounded value fingerprint, and declared actions; a mismatch is reported as a
refusal rather than success.
