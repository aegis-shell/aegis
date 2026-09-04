# How to Manage Audit History

The durable security-audit history at
`$XDG_DATA_HOME/tessera/audit/events-v2.jsonl` records every authority
decision the compositor makes. It is append-only and hash-chained; Tessera
never deletes it silently. This guide shows how to inspect it, verify it,
archive it, and configure a retention policy that keeps disk use bounded.

All `tessera audit` commands are local: they operate directly on the store and
never contact a running compositor. A live session holds the store's
advisory lock, so run them from another TTY or after logging out.

## Inspect the Current State

Run:

```bash
tessera audit status
```

The output reports the next sequence number, the chain tail, how many sealed
segments exist with their original and compressed sizes, the active stream
size, the total on-disk footprint, and any recorded export destination. Add
`--json` for machine-readable output.

## Verify the History

Fast verification checks every sealed segment's presence, size, and
compressed digest against the authenticated manifest:

```bash
tessera audit verify
```

Full verification additionally decompresses every segment and replays the
complete hash chain. Run it before relying on an archive, or after any
filesystem incident:

```bash
tessera audit verify --full
```

Both commands fail with a diagnostic naming the offending segment if
anything does not match.

## Archive, Then Free Space

Pruning requires proof that the history exists somewhere else first.

1. Copy the sealed segments out of `$XDG_DATA_HOME/tessera/segments/` to your
   archive, together with `events-v2.jsonl.manifest` and
   `events-v2.jsonl.key`.
2. Record the export in the manifest so pruning can proceed:

   ```bash
   tessera audit export /mnt/audit-archive/2026-08
   ```

3. Delete all but the newest segments, keeping, for example, eight:

   ```bash
   tessera audit prune 8
   ```

The removed segments' cryptographic identities stay recorded in the
manifest's pruned history, so the removal itself remains auditable.
`--force` skips the export requirement; use it only when you accept losing
the only copy.

## Configure Automatic Retention

Add a retention policy to `config.toml` instead of pruning by hand:

```toml
[audit]
segment_max_mib = 64
retain_segments = 8
```

When the active stream reaches `segment_max_mib` it is sealed into a
compressed immutable segment, and the chain continues in a fresh active
file — sequence numbers never reset. After each seal, Tessera prunes down to
`retain_segments`, but only once those segments carry export
acknowledgements. Acknowledge each archive run with `tessera audit export` and
the steady state holds: one active segment plus the configured number of
sealed ones.

With the default `retain_segments = 0` nothing is ever pruned; the store
grows until the `[audit] max_store_mib` ceiling refuses further appends, so
monitor it with `tessera audit status`.

## Back Up the Store

A complete backup preserves the whole audit directory: the active
`events-v2.jsonl`, both checkpoint sidecars (`.checkpoint` and `.key`), the
manifest, and `segments/`. Treat the `.key` file like a credential: it
authenticates the checkpoint and manifest, so store it with the archive but
never in a world-readable location.
