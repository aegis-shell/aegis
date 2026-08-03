# aegis-semantic

Transport-neutral validation and routing for application accessibility trees.
It accepts bounded window-local nodes from an authenticated adapter, verifies
their graph and geometry, namespaces ids by the owning window, and produces
the semantic objects consumed by `aegis-authority` observations.

The crate performs no D-Bus, Wayland, IPC, rendering, or Agent reasoning.
It accepts only complete bounded revisions, rejects cycles, missing parents,
escaping geometry, stale revisions, provider takeover, oversized text/tree
payloads, and ids outside the owning window namespace.
