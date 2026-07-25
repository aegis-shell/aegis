# ADR-0006: FFI soundness discipline for hand-rolled protocol handlers

- Status: Accepted
- Date: 2026-06-18

## Context

The hand-rolled Wayland server in `aegis-compositor` (per
[ADR-0002](0002-hand-rolled-wayland-server.md)) drives libwayland over FFI by
defining `*_interface_impl` structs of function pointers and handing them to
`wl_resource_set_implementation`. libwayland indexes those structs by request
opcode: a request with opcode N reads `impl[N]` and calls it. The struct's
size is therefore load-bearing for memory safety — an under-sized struct lets
the highest opcodes read past the end into adjacent memory.

The first soundness audit found this failure mode in production: the
`wl_data_device_manager_interface_impl` struct carried two slots
(`create_data_source`, `get_data_device`), but the global was bound at v3 and
the protocol added `destroy` as opcode 2 in v2. Any client sending the
manager's `destroy` request would have caused libwayland to read two
function-pointer slots past the end and call whatever it found there. The
struct was correct for v1 and wrong for v3.

The same audit found three other unsafe-code hazards unrelated to vtable
sizing: a permanent leak of `SurfaceRec` boxes (intentional, with a comment
acknowledging the leak); seat device request handlers that left the client's
requested new-id unallocated; and a `params_create_immed` failure path that
violated the protocol by silently doing nothing.

## Decision

The hand-rolled FFI seam adopts four discipline rules, each enforced by code
rather than by vigilance.

1. **Compile-time opcode-count asserts.** Every `*_interface_impl` struct in
   `aegis-compositor/src/ffi.rs` is paired with an
   `assert_impl_opcode_count!(T, N)` invocation that fails the build unless
   `size_of::<T>() == N * size_of::<*const ()>()`. `N` is the request count
   the protocol XML advertises for the version the code binds. Adding a
   request to the protocol XML without adding a matching struct slot becomes
   a hard build failure rather than a latent UB.

2. **Surfaces own their slot index.** `SurfaceRec` carries an `index` field
   and a back-pointer to the server `State`. The destroy notify detaches the
   rec from `state.surfaces` by nulling that slot in O(1), posts
   `wl_buffer.release` on any held dma-buf-backed buffer, and reclaims the
   box with `Box::from_raw`. Iterators filter null slots. `Server::drop`
   reclaims any orphaned boxes left after `wl_display_destroy` has fired its
   own destroy notifys. No surface allocation outlives its resource.

3. **Stub device handlers allocate their new-id.** Even with advertised
   capabilities of zero, the `wl_seat.get_pointer`, `get_keyboard`, and
   `get_touch` handlers create an inert `wl_resource` for the requested id.
   A conforming client never calls them, but a non-conforming one that does
   gets a no-op object rather than a dangling id that libwayland treats as
   protocol-fatal on first use.

4. **`create_immed` failure is fatal, per protocol.** When
   `zwp_linux_buffer_params_v1.create_immed` cannot produce a buffer, the
   handler posts `wl_resource_post_error` with
   `invalid_wl_buffer` (protocol error 7). The async `create` path keeps its
   non-fatal `failed` event; only `create_immed` is fatal, matching the
   protocol contract.

## Alternatives

- **Audit-only discipline, no static asserts.** Rejected: the bug already
  shipped once under review. The class of failure is mechanical and
  recurring, so the prevention must also be mechanical.
- **Generate impl structs from the protocol XML.** Considered and deferred.
  `wayland-scanner` could emit both the interface tables (already shared via
  `aegis-protocols`) and Rust-side impl structs, eliminating the manual size
  bookkeeping entirely. This is the long-term right answer, but it does not
  exist yet, and the static assert costs nothing in the meantime.
- **Use a higher-level binding (`wayland-server-rs`).** Rejected per
  [ADR-0002](0002-hand-rolled-wayland-server.md); the discipline applies to
  the chosen raw-FFI approach.

## Consequences

- Adding or removing a request from any bound protocol version now requires
  editing two places: the `*_interface_impl` struct and the matching
  `assert_impl_opcode_count!` call. The compiler enforces they agree.
- `state.surfaces` may contain null slots; every iterator and lookup must
  filter them. The cost is one branch per slot per iteration, negligible at
  compositor workloads.
- The four unsafe patterns the audit flagged are eliminated. Other unsafe
  patterns the audit flagged as *probably fine in modern libwayland*
  (notably `wl_resource_set_implementation` with a `NULL` impl pointer to
  create a documented inert resource) are left as-is, because the libwayland
  contract explicitly permits `NULL` and the alternative is per-interface
  empty vtables with no behavioral difference.
- This ADR does not address explicit sync, viewport crop/scale application,
  subsurface placement, or input routing. Those are feature work, not
  soundness work, and live in their respective milestones.
