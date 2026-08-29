# IPC Reference

The aegis IPC is protocol version 31, carried as length-framed JSON over the
owner-only Unix socket at `$XDG_RUNTIME_DIR/aegis.sock`. Every connection starts
with `Hello`; commands are accepted only after capability and scope checks.
JSON messages are limited to 16 MiB. Large immutable capture and frame
payloads use a separate sealed-file-descriptor transfer described under
[Capture](#capture). The server admits at most 256 concurrent connections,
requires `Hello` within 10 seconds, and applies a 30-second write timeout.
Each connection has a bounded 64-item writer inbox. Request threads
backpressure on that inbox; compositor event/journal producers never block.
A subscriber that cannot keep up is disconnected instead of accumulating
memory or silently missing an invalidation/audit event.

## Capabilities

| Capability | Authority | Default |
|------------|-----------|---------|
| `query` | Read snapshots and subscribe to events or the journal. | Always granted. |
| `control` | Mutate windows, workspaces, live-system state, layout, and notifications. | Server policy. |
| `input` | Inject bounded actions into a target window. | Named scope or paired agent required. |
| `session` | Quit, persist compositor settings, or perform other session-level operations. | Server policy. |
| `interaction_domain` | Create, configure, capture, pause, transfer, launch into, and revoke Interaction Domains. | Named scope or paired agent with the operation in its ceiling. |

Absent `input` and `interaction_domain` fields are `false`. An anonymous connection never
receives `input`; Interaction Domain operations also require an explicit operation in the
connection's ceiling. `[agent] lockdown` defaults on and strips every
privileged capability from unpaired, unnamed connections. First-party owner,
Interaction Domain, agent-administration, and portal clients use separate built-in scopes.
These scope names are trusted-component selectors on the owner-only socket,
not cryptographic identity against a compromised Unix account.

An Agent Interaction Domain's controlling principal carries the authenticated registry
subject that created it. Every Interaction Domain mutation, capture, launch, and directed
input request rechecks that binding. The human Interaction Domain may be one side of an
authority transfer, but an agent cannot target or recover another subject's
Interaction Domain. Existing connections refresh their live ceiling before use, so a
forgotten principal or narrowed ceiling fails closed immediately.

For an authenticated Actor, `query` is only the transport capability. Each
observation family also requires an explicit operation in the Actor's
ceiling, and returned snapshots are filtered by its resource allowlists.
Observation never follows from an action capability, and action never follows
from an observation capability.

Every privileged connection supplies `lease: { ttl_ms }` in `Hello`.
Omitting it strips `control`, `input`, `session`, and `interaction_domain`. The allowed
duration is 1,000 through 86,400,000 milliseconds. `RenewLease { ttl_ms }`
renews a live connection-bound lease; an expired lease cannot be renewed.
The reference client requests 900,000 milliseconds by default.

Every connection also receives a distinct `ActorSession` in `Hello`. It is
bound to the connection and authenticated principal, has independent TTL and
idle deadlines, and carries hard pending-action and live-observation quotas.
Session expiry, EOF, or principal removal cascades to observation tokens,
resource grants, and semantic-provider queues. A durable principal can
survive; its execution session cannot.

## Queries

| Request | Response | Capability |
|---------|----------|------------|
| `GetWindows` | `Windows` | `query`; Actor: `ObserveWindows` |
| `GetWorkspaces` | `Workspaces` | `query`; Actor: `ObserveWorkspaces` |
| `GetNotifications` | `Notifications` | `query`; Actor: `ObserveNotifications` |
| `GetOutputs` | `Outputs` | `query`; Actor: `ObserveOutputs` |
| `EnumerateOutputs` | `Outputs` (lean: connector, primary, physical rect only) | `query`; Actor: `ObserveOutputs` (protocol 29) |
| `GetJournal { since }` | `Journal` | `query`; Actor: `ObserveJournal` |
| `GetInteractionDomains` | `InteractionDomains` | `query`; Actor: `ObserveInteractionDomains` |
| `GetSettings` | `Settings` with a revisioned snapshot | `query`; Actor: `ObserveSettings` |
| `GetSystemStatus` | `SystemStatus` with a live snapshot | `query`; Actor: `ObserveSystem` |
| `InteractionDomain { action }` | `InteractionDomain` with a commit receipt | `interaction_domain` + explicit scope op |
| `Settings { expected_revision, action }` | `SettingsApplied` with a commit receipt | `session` |
| `CaptureOutput` | `CaptureOutput` | `control` + explicit scope op |
| `CaptureInteractionDomain { interaction_domain, region }` | `CaptureInteractionDomain` | `interaction_domain` + `CaptureInteractionDomain` scope op |
| `ObserveInteractionDomain { interaction_domain }` | `InteractionDomainObserved` | `query` + `ObserveInteractionDomain` scope op |
| `ActInInteractionDomain { intent }` | `ActorActionCommitted` | `input` + `InjectInteractionDomainInput` scope op |
| `RequestResourceGrant { resource, ttl_ms, uses }` | `ResourceGranted` | matching Actor capability; exact-resource confirmation when gated or payment |
| `ConsumeResourceGrant { id, resource }` | `ResourceGrantConsumed` | owning live Actor session and exact resource |
| `RevokeResourceGrant { id }` | `ResourceGrantRevoked` | owning live Actor session |
| `GetAccessibilityWindows` | `AccessibilityWindows` | authenticated system provider + `ObserveWindows` + `PublishAccessibilityTree`; never ordinary observation |
| `Observe` | `Observed` with a multi-class snapshot and journal cursor | `query`; every class gated by the live scope like the matching `Get*` (protocol 28) |
| `GetConnectionState` | `ConnectionState` with the connection's own caps, re-resolved scope, lease, and Actor session | always; a paired agent reads its registry ceiling without reconnecting (protocol 28) |
| `Transact { expected_journal_seq?, expected_interaction_domain_revision?, ops }` | `Transact` with a commit receipt or a precondition conflict | `control`; every op authorized like its `Command` (protocol 28) |
| `PublishAccessibilityTree { update }` | `AccessibilityTreePublished` | authenticated provider + `PublishAccessibilityTree` |
| `NextAccessibilityAction { timeout_ms }` | `AccessibilityAction` | authenticated provider + `DispatchAccessibilityAction` |
| `CompleteAccessibilityAction { request_id, success, message }` | `AccessibilityActionCompleted` | owning provider session and pending request |
| `StreamOutputStart { max_fps, target, dmabuf, cursor }` | `StreamOutputStarted` | `control` + `StreamOutput` scope op |
| `StreamOutputStop { stream_id }` | `StreamOutputStopped` | `control` + `StreamOutput` scope op |
| `SetIdleInhibit { inhibit }` | `IdleInhibitSet { inhibited }` | `control` + `IdleInhibit` scope op |

`Observe` (protocol 28, ADR-0125; the Observe primitive) reads windows,
workspaces, outputs, notifications, Interaction Domains, and the journal
cursor in a single round trip. Each class is independently gated by
the connection's live scope: a refused class is `null` in the snapshot,
exactly as if the matching `Get*` request had been refused; permitted
classes are scope-filtered like the individual queries. The journal cursor
is the baseline for `GetJournal` and the precondition for `Transact`.

`Subscribe` enables coarse events:

- `WindowsChanged`, `WorkspaceChanged`, `InteractionDomainsChanged { revision }`,
  `SettingsChanged { revision }`, and `SystemStatusChanged`
  invalidate the corresponding snapshots.
- `SpaceUseChanged { state }` reports the strongest visible output-space
  consumer. `state` is `available`, `maximized`, or `fullscreen`; fullscreen
  has precedence over maximized.
- `InteractionDomainDamaged { interaction_domain, sequence, revision, damage }` reports that an active
  Interaction Domain's directed scene changed. `damage` contains at most 64 rectangles in
  virtual-output logical coordinates. Surface commits conservatively
  invalidate the complete Interaction Domain-local window placement; mapping, removal,
  transfer, observer, and output-layout changes may invalidate the complete
  output. Pixels remain pull-based through `CaptureInteractionDomain`.

Anonymous broadcast lanes are unfiltered. Principal-bound agent lanes are
filtered per event at broadcast time (protocol 28, ADR-0125): a coarse
event is delivered only when the lane's live scope pregrants the matching
observation capability, and a `Journal` event passes through the same
subject and resource projection as `GetJournal`. The scope is re-resolved
at every broadcast, so a revoked named scope or a forgotten principal
silently stops delivery. Lanes remain bounded and fail-closed: a full lane
shuts the connection down rather than dropping an invalidation.

`SubscribeJournal` additionally enables one ordered `Journal` event per
mutation decision. Each `JournalEntry` contains an `effect` and one tagged
`mutation`. Trusted components use `Origin::Ipc { conn_id }`; authenticated
Actors use `Origin::Actor { conn_id, principal }`:

- `Command { cmd: AuditedCommand }`;
- `InteractionDomain { action, before_revision, after_revision }`;
- `Settings { action, before_revision, after_revision }`;
- `ActorAction { action_id, interaction_domain, target, window, actions, actions_truncated,
  authority_revision }`;
- `AgentAuth { principal, action }`;
- `ActorSession { session, principal, action }`; or
- `ResourceGrant { session, principal, capability, resource_kind, action }`; or
- `ResourceGrantAttempt { session, principal, action, capability?, resource_kind? }`; or
- `CapabilityUse { session, principal, capability, action }`; or
- `ScopeClaim { scope }`.

An `ActorAction` never stores its observation bearer token, typed text,
values, key codes, or pointer coordinates. `AuditedCommand` similarly removes
notification text/external ids, screenshot paths/regions, and synthetic-input
coordinates and codes. These projections retain only target ids, action
shapes, UTF-8 byte counts, and low-level action counts. Resource events retain
only the resource category, never an exact path, network origin, secret
purpose, payee, amount, or grant bearer id. Every durable refusal reason is
reduced to a fixed mutation category before persistence; arbitrary downstream
error strings never enter the log. Refused issue/consume/revoke operations
retain only the attempted operation plus capability/resource categories. `action_id` and
`authority_revision` are present after a committed main-loop action. Actor
journal queries exclude other principals' Actor events and apply live window
and Interaction Domain allowlists.

`CapabilityUse` covers explicit high-risk endpoints that have no richer
command or semantic-action event: accessibility publication/dispatch,
capture, semantic observation, stream start/stop, idle-inhibit enable/disable,
interactive picks/prompts, and wallpaper application. It retains the precise
operation category but not the endpoint payload. Routine snapshot polling is
not durably logged. `ScopeClaim` records built-in scope claims refused by
the peer-identity gate at the handshake
([ADR-0128](../adr/0128-peer-identity-bound-built-in-scopes-and-capture-indicator.md));
it retains only the claimed scope name — never the peer's executable path
or pid — and principal-bound lanes never observe it.

Actor session ids and IPC connection ids are separate namespaces. The server
binds them explicitly and re-resolves that binding at effect boundaries;
clients must not infer one id from the other. Long-running capture and stream
delivery also carry a live scope binding, so principal or named-scope
revocation, lease expiry, and window-allowlist narrowing take effect before
pixels are delivered.

Capability, lease, validation, and scope refusals are journaled even when the
mutation never reaches the compositor main loop. Interaction Domain actions
rejected by live state carry the unchanged revision in both revision fields.

Routine capability polling is not durably audited
([ADR-0135](../adr/0135-routine-capability-polling-is-not-durably-audited.md)).
A timed-out `NextAccessibilityAction` long-poll (`Ok(None)`) and a
successful `GetAccessibilityWindows` scan query decide nothing and journal
nothing; action delivery, handler errors, and authorization refusals remain
durable records, so steady-state audit growth is independent of session
length.

The live journal is backed by
`$XDG_DATA_HOME/aegis/audit/events-v2.jsonl` (or the equivalent default data
directory). The owner-only append store synchronizes successful writes and
verifies monotonic sequence numbers, record bounds, and a SHA-256 hash chain.
An owner-only local key authenticates an atomic replay checkpoint at
`events-v2.jsonl.checkpoint`; the key is
`events-v2.jsonl.key` ([ADR-0136](../adr/0136-authenticated-bounded-audit-replay-and-storage-guards.md)).
Normal startup authenticates the checkpoint and synchronously verifies only
the bounded live projection plus its uncheckpointed tail. A worker still
verifies the complete older prefix from genesis, and no new authority record
may be appended until that scan succeeds. An existing stream without a
checkpoint pays one complete streaming scan to establish it; replay memory
never grows beyond the live projection plus one record.

An open store holds an exclusive advisory lock for its lifetime, so a second
live compositor fails fast instead of interleaving appends from stale
sequence state. Hash mismatches, unsafe ownership/mode, symlinks, malformed
JSON, sequence gaps, an incomplete trailing record, or an invalid checkpoint
fail closed. Creation and atomic checkpoint replacement synchronize the
containing directory, so reported durable state cannot depend on an
uncommitted directory entry. This is a durable decision/audit log, not a
promise to resurrect external Wayland client state after compositor restart.

`XDG_DATA_HOME` or `HOME` must resolve at startup; the production runtime does
not downgrade the audit log to memory. Security lifecycle events and
authorization refusals are persisted before their IPC response is returned.
An append, flush, or sync failure fail-stops the entire compositor, including
when detected on a connection worker.

Aegis never deletes or silently rotates this authority history. Before each
append it enforces the `[audit]` hard history ceiling (2048 MiB by default)
and filesystem reserve (512 MiB by default). When the active stream reaches
`[audit] segment_max_mib` it is sealed into a compressed immutable segment;
an HMAC-authenticated manifest records every sealed segment's chain
identity, and startup verifies each segment against it
([ADR-0137](../adr/0137-audit-segment-manifest-and-retention.md)).
Retention is explicit: pruning requires a configured `retain_segments`
plus an export acknowledgement recorded by `aegis audit export`, and every
removal is preserved in the manifest's pruned history. Production
deployments must monitor those bounds and provide an operator-controlled,
lossless archive/export policy; archival must preserve complete records,
chain order, the manifest, and both checkpoint sidecars. If the active
store cannot accept another durable record, fail-stop is intentional. The
hash chain and local checkpoint HMAC detect accidental or blind edits under
the owner trust model, but they are not proof against an attacker who
controls the log, key, and every trusted verification copy. Deployments
that require hostile-owner tamper evidence must continuously export records
or independently signed anchors to a separately administered system.

## Commands

| Command | Connection capability | Actor capability | Target |
|---------|------------|-----------------|--------|
| `Focus { id, reveal }` | `control` | `Focus` | Window |
| `Minimize { id }` | `control` | `Minimize` | Window |
| `Close { id }` | `control` | `Close` | Window |
| `Move { id }` | `control` | `Move` | Window |
| `SetWindowGeometry { id, rect }` | `control` | `SetWindowGeometry` | Window |
| `SetAlwaysOnTop { id, on_top }` | `control` | `SetWindowGeometry` | Window |
| `SetFullscreen { id, fullscreen }` | `control` | `SetWindowGeometry` | Window |
| `InjectInput { id, actions }` | `input` | `InjectInput` | Window |
| `LaunchInInteractionDomain { interaction_domain, desktop_id }` | `interaction_domain` | `LaunchInInteractionDomain` | Interaction Domain |
| `LaunchApp { desktop_id, placement }` | `control` | `LaunchApp` | Workspace, when the placement names one |
| `Cycle { forward }` | `control` | `Cycle` | — |
| `SwitchWorkspace { dir }` | `control` | `SwitchWorkspace` | Focused output |
| `SwitchWorkspaceTo { id }` | `control` | `SwitchWorkspaceTo` | Workspace |
| `MoveToWorkspace { window, workspace }` | `control` | `MoveToWorkspace` | Window and workspace |
| `System { action }` | `control` | `SystemControl` | Live host or compositor-owned session state |
| `ToggleOverview` | `control` | `ToggleOverview` | — |
| `Notify { summary, body, app_id, external_id }` | `control` | `Notify` | — |
| `DismissNotification { id }` | `control` | `DismissNotification` | Notification |
| `Screenshot { path }` | `control` | `Screenshot` | Focused output |
| `Quit` | `session` | — | Session |

`Do` returns `Ok` after most commands are queued, not after they are applied.
`System { action }` is the exception: its reply is an authoritative main-loop
receipt, and an apply refusal returns `Error`. Read the next snapshot or
journal entry to observe other commands, or use `Transact` (below) when the
caller needs a commit receipt.
Protocol 21 removes the unguarded `Command::InjectInteractionDomainInput` shape entirely;
Interaction Domain input exists only as the observation-bound `ActInInteractionDomain` request.
Window-targeted physical commands are reauthorized on the compositor thread.
If the human Interaction Domain is only an observer, focus, minimize, close, move,
geometry, and workspace mutations produce `Effect::Refused` and do not reach
the client. Its mirror also blocks physical hit-testing, so a refused click
cannot fall through to an unrelated window underneath.

## Transactions

`Transact { expected_journal_seq?, ops }` (protocol 28, ADR-0125) is the
Transact primitive: an ordered batch of `TransactOp`s drawn from the
`Command` vocabulary — window focus/minimize/maximize/fullscreen/always-on-top/
close/
geometry, workspace switch and move, tiling, and notification post/dismiss.
Interactive, input, capture, launch, session, and shell-owned commands stay
outside the transaction vocabulary and keep their own request paths.

Every op is authorized and validated exactly like the `Command` it mirrors
(op allowlists, ask-grant prompts, and resource axes apply per op), and the
batch preflights as a unit: when any op refuses, no op applies and only the
first refusing op is journaled. A committed batch applies in order on the
compositor main loop through the same chokepoint as `Do`, journaling each
op individually. The reply is the authoritative receipt: `before_seq` and
`after_seq` journal cursors plus each op's sequence number and effect. The
batch is not rolled back; a refused op is reported in its receipt entry.

With `expected_journal_seq`, the batch commits only when the journal's
`latest_seq` still equals it at the commit boundary; with
`expected_interaction_domain_revision`, only when the Interaction Domain
authority revision still equals it. Both currencies may be specified; the
reply names the one that failed. A conflict is
`PreconditionConflict { precondition, expected, actual }`: nothing applies
and nothing is journaled. Read the journal cursor from `Observe`'s
`journal_cursor` and the revision from any Interaction Domain snapshot, and
retry at fresh values after a conflict.

`LaunchApp` launches an enumerated desktop entry directly on the desktop —
no Interaction Domain sandbox — and optionally directs its first root
toplevel to a workspace at map time (protocol 27,
[ADR-0118](../adr/0118-launch-placement-and-workspace-isolation.md)).
`placement` is either `Workspace { id }`, an existing workspace by durable
id (the default current-workspace placement applies if it is gone by map
time), or `FreshWorkspace { label }`, a workspace created directly after
the current one on the window's output. A placement never switches the
user's view: the window opens on its target workspace even while hidden
and never takes keyboard focus on map. The compositor matches the first
mapped root toplevel by exact client pid, falling back to a case-sensitive
app_id FIFO, and expires unmatched entries after 60 seconds; a placement
beats `[[window_rule]]` and remembered workspace state, and transients
always follow their parent's workspace instead. With `reveal = false`,
`Focus` raises such a hidden window within its own workspace without
moving the user's view or granting keyboard focus.

## Live System Controls

`GetSystemStatus` returns one normalized observation shared by status-bar
chrome and external IPC clients:

| Field | Type | Meaning |
|-------|------|---------|
| `volume` | optional percentage | Default audio-sink volume; absent when unavailable. |
| `muted` | boolean | Default audio-sink mute state. |
| `network` | `Offline`, `Wifi`, or `Wired` | Coarse active connectivity. |
| `battery` | optional `{ percent, charging }` | Battery state when a battery is present. |
| `wifi_enabled` | optional boolean | Wi-Fi radio state; absent when its service is unavailable. |
| `bluetooth_enabled` | optional boolean | Bluetooth radio state; absent when its service is unavailable. |
| `brightness` | optional percentage | Backlight level; absent without a controllable backlight. |
| `do_not_disturb` | boolean | Current notification suppression state. |
| `tiled` | boolean | Layout mode for the current workspace. |
| `idle_inhibited` | boolean | Derived mirror of the session power mode: true exactly when the mode disarms the automatic lock stage (`awake`). Compositor-owned; survives host status samples. |
| `power_mode` | `balanced`, `awake`, or `secure` | Session power mode (ADR-0140): which idle stages stay armed. Compositor-owned; additive with a `balanced` default. |
| `input`, `display` | status objects | Host-probe data shared with settings surfaces; persistent editors should use `GetSettings` for revisioned state. |

`System { action }` accepts one immediate action:

| Action | Payload | Bounds or effect |
|--------|---------|------------------|
| `ToggleMute` | — | Toggle the default audio sink. |
| `StepVolume` | `delta` | Signed percentage step from -100 through 100. |
| `SetVolume` | `level` | Percentage from 0 through 100. |
| `SetBrightness` | `level` | Percentage from 1 through 100. |
| `SetWifi` | `enabled` | Enable or disable the Wi-Fi radio. |
| `SetBluetooth` | `enabled` | Unblock or block Bluetooth radios. |
| `SetDoNotDisturb` | `enabled` | Change notification suppression. |
| `SetTiling` | `enabled` | Set the current workspace layout mode. |
| `SetOutputPower` | `powered` | Power all physical outputs on or off. Power-off is accepted only after a secure lock frame is confirmed; wake is always safe. Used by `aegis-idle`. |
| `SetIdleInhibit` | `inhibit` | Legacy single-bit shape (ADR-0140): maps onto the session power mode — `true` selects `awake`, `false` selects `balanced`. |
| `SetPowerMode` | `mode` | Select the session power mode (`balanced`, `awake`, `secure`; ADR-0140): which idle stages stay armed. Session runtime state, not persisted; manual locking and lock-before-sleep are unaffected. |

The command requires `control`, a live privileged lease, and permission for
the `SystemControl` operation when the connection's scope restricts `ops`. The server
validates bounds before dispatch and returns only after the compositor main
loop applies or refuses the action. Host-service commands are still spawned
without blocking for the external service's eventual state change. A
successful apply publishes an optimistic snapshot and reconciles it through
the host status poller. `SystemStatusChanged` tells subscribers to re-query;
it carries no partial snapshot. These actions do not write the revisioned
compositor configuration.

## Persistent Settings

`GetSettings` returns one coherent snapshot:

| Field | Type | Meaning |
|-------|------|---------|
| `revision` | unsigned integer | Monotonic settings revision. |
| `input` | `InputStatus` | Effective keyboard profile plus touchpad and mouse status: profile, detected devices, capabilities, and configurability. |
| `display` | `DisplayStatus` | Connected outputs, advertised modes, configurability, and the last apply error. |
| `preferences` | `DesktopPreferences` | Complete effective desktop profile after configuration defaults and explicit startup overrides. |
| `idle` | `IdleSettings` | Complete validated inactivity, lock, output-power, and suspend policy. |

`DesktopPreferences` contains:

| Field | Type | Bounds or values |
|-------|------|------------------|
| `color_scheme` | enum | `system`, `dark`, or `light` |
| `accent_color` | optional RGB object | `{ red, green, blue }`, each 0–255 |
| `contrast` | enum | `normal` or `high` |
| `reduced_motion` | boolean | Desktop and toolkit animation preference |
| `font_name`, `monospace_font_name` | string | Non-empty, at most 256 bytes |
| `text_scale` | float | 0.5–3.0 |
| `icon_theme`, `cursor_theme` | string | Non-empty, at most 256 bytes |
| `cursor_size` | unsigned integer | 8–128 logical pixels |

`IdleSettings` contains:

| Field | Type | Bounds or values |
|-------|------|------------------|
| `enabled` | boolean | Whether inactivity may trigger configured stages |
| `dim_after_seconds` | unsigned integer | `0` or 1–604800 |
| `lock_after_seconds` | unsigned integer | `0` or 1–604800 |
| `display_off_after_seconds` | unsigned integer | `0` or 1–604800; requires nonzero locking |
| `suspend_after_seconds` | unsigned integer | `0` or 1–604800; requires nonzero locking |
| `dim_percent` | unsigned integer | 1–100 |

Nonzero stage times must be strictly increasing in the order shown.

`Settings` submits one tagged action with an optional `expected_revision`:

| Action | Payload | Effect |
|--------|---------|--------|
| `SetInput` | complete `InputConfig` (keyboard, mouse, touchpad) | Validate, persist `[input]`, apply the mouse and touchpad profiles to live libinput devices, and advertise the keyboard repeat rate to clients. |
| `SetDisplay` | connector, mode, scale, position, and primary flag | Validate, atomically persist the output entry, and reconcile the live direct-DRM output. |
| `SetDesktopPreferences` | complete `DesktopPreferences` | Validate, atomically persist the `[appearance]` and preference-related `[ui]` fields, apply chrome and cursor policy, and refresh application icons. |
| `SetIdle` | complete `IdleSettings` | Validate and atomically persist `[idle]`, then replace the supervised idle policy client. |
| `SetDock` | complete `DockSettings` | Validate, atomically persist the `[dock]` presentation fields, and apply the minimize flight style to the compositor. |

The operation requires `session` plus a live privileged lease. It is refused
while the session is locked. When `expected_revision` does not match the
current revision, the complete action is refused without changing state.

`SettingsApplied { receipt: { revision } }` is a confirmation, not a queue
acknowledgement. The server sends it only after the compositor main loop has
validated, persisted, and applied the action. A successful mutation increments
the revision, publishes the replacement snapshot, broadcasts
`SettingsChanged`, and records the action and before/after revisions in the
mutation journal.

Display, input (keyboard repeat, mouse and touchpad motion and scrolling),
desktop appearance, and idle power policy are settings domains in the current
snapshot. Accounts and window-rule modules remain unavailable until their
authoritative services expose typed state and actions.
See the [Settings Reference](settings.md#modules) and
[ADR-0072](../adr/0072-desktop-preference-authority-and-toolkit-compatibility.md).

## Interaction Domain Authority

An Interaction Domain is an interaction and presentation authority domain. Interaction Domain `1` is
the physical human desktop. Each agent Interaction Domain owns an independent `wl_seat`,
a directed virtual output, and private mount-scoped launch portals.

`GetInteractionDomains` returns one `InteractionDomainSnapshot` with:

- `revision`;
- principals and Interaction Domains;
- seats and their enabled state;
- connected Wayland clients and observed multi-seat support; and
- interaction groups, their controlling Interaction Domain, observing Interaction Domains, and windows.

`InteractionDomain { action }` is synchronous. It returns only after the compositor main
loop commits or rejects the operation and records that decision in the
mutation journal.

| Action | Actor capability | Result |
|--------|-----------------|--------|
| `Create { label, capabilities, output }` | `CreateInteractionDomain` | `Created { bundle }` |
| `Transact { expected_revision, mutations }` | `TransactInteractionDomain` | `TransactionCommitted { receipt }` |
| `Revoke { interaction_domain, fallback, expected_revision }` | `RevokeInteractionDomain` | `Revoked { receipt }` |

A transaction contains 1–64 mutations and commits all or none:

| Mutation | Effect |
|----------|--------|
| `TransferWindow { window, target, retain_source_as_observer }` | Transfers the complete interaction group containing `window`. |
| `SetObserver { group, interaction_domain, observe }` | Adds or removes a read-only presentation Interaction Domain. |
| `ConfigureOutput { interaction_domain, output }` | Changes a virtual output. |
| `SetState { interaction_domain, state }` | Pauses or resumes an Interaction Domain. Permanent revocation is a separate action. |

Scope authorization expands `TransferWindow` and `SetObserver` to the
complete interaction group before commit. If any affected sibling window is
outside `scope.windows`, the whole action is refused; allowlisting one
toplevel cannot smuggle another toplevel on the same client connection.

`expected_revision` is optional on the wire. When present, a stale value
rejects the complete operation. The reference shell and CLI always supply
the revision they observed.

Virtual output dimensions are logical pixels. `scale_milli` is scale times
1,000 and `refresh_mhz` is millihertz. Width and height are each limited to
16,384, scale to 0.25–8.0, refresh to 1–1,000 Hz, and one physical RGBA frame
to 256 MiB.

`ActInInteractionDomain` uses semantic-object-local coordinates and the Interaction Domain's
independent seat. It never changes physical pointer or keyboard focus and
does not execute compositor shortcuts. See
[Observation-bound Interaction Domain Actions](#observation-bound-interaction-domain-actions).

`LaunchInInteractionDomain` accepts an enumerated desktop-entry id. The compositor
launches it through a private mount-scoped Wayland listener and a fail-closed
Linux namespace sandbox. The randomized host socket path is removed and
pre-gate connections are dropped before application code runs. One sandbox
may open several Wayland connections without exposing a reusable host
pathname. Network and host filesystem access are denied unless
`[interaction_domain_sandbox]` policy explicitly grants them.

Every managed launch receives mandatory cgroup memory, process-count, and CPU
weight controls. Interaction Domain pause, session lock, and inactive VT freeze the
complete cgroup; revocation terminates and reaps it. Missing bubblewrap,
cgroup v2, controller delegation, or portal setup refuses the launch.

## Window Geometry

`SetWindowGeometry` uses compositor-global logical pixels. `rect.size.w` and
`rect.size.h` must each be between `1` and `32768`. The compositor:

- changes the window to floating layout;
- clears maximized and fullscreen state;
- clamps size to the client's minimum and maximum hints;
- preserves the requested origin; and
- exposes the resulting rectangle through `GetWindows`.

```json
{
  "type": "Do",
  "cmd": {
    "type": "SetWindowGeometry",
    "id": 7,
    "rect": {
      "origin": { "x": 120, "y": 80 },
      "size": { "w": 1280, "h": 720 }
    }
  }
}
```

## Synthetic Input

`InjectInput` requires the operation in the connection's ceiling — a named
scope or a paired agent's approved set — and the target window id in the
resource allowlist. The operation must be listed explicitly: an omitted `ops`
field does not grant synthetic input. Coordinates are logical pixels relative
to the target window's top-left corner.

| Action | Fields | Effect |
|--------|--------|--------|
| `PointerMove` | `position` | Move the logical pointer. |
| `Click` | `position`, `button` | Move, press, and release. |
| `Scroll` | `position`, `dx`, `dy` | Move and deliver a smooth scroll. |
| `KeyPress` | `code` | Press and release one evdev key. |

Validation limits:

- `actions` contains 1–64 entries.
- Pointer positions must be inside the live, visible target and must hit that
  target rather than an overlapping window.
- Shell chrome must not cover any pointer position; keyboard-owning chrome
  rejects key input.
- Click buttons are Linux codes `0x110` through `0x117`.
- Key codes are at most `0x2ff`.
- Scroll deltas are finite and have an absolute value no greater than `1000`.
- Injected keys are refused while a physical modifier is held.
- All input is refused while a physical modifier, button grab, drag, or window
  move/resize is active.

```json
{
  "type": "Do",
  "cmd": {
    "type": "InjectInput",
    "id": 7,
    "actions": [
      {
        "type": "Click",
        "position": { "x": 40, "y": 32 },
        "button": 272
      }
    ]
  }
}
```

Input commands bypass compositor global key bindings. A live-state rejection
after queuing appears as `Effect::Refused` in the mutation journal.

### Observation-bound Interaction Domain Actions

`ObserveInteractionDomain { interaction_domain }` returns a `SemanticObservation` without framebuffer
pixels. Its semantic snapshot contains the Interaction Domain authority revision and
compositor-owned objects. Window roots are guaranteed and use durable window
ids as semantic object ids. Application descendants use
`{ window, nonzero_local_id }`, so provider ids can never collide across
windows. Each object carries its source, parent, role, optional name,
description and bounded value, application id, Interaction Domain-output
bounds, target-local extent, state, declared actions, and content revision.
The compositor does not infer semantic nodes from pixels.

In a production direct session, the compositor supervises `aegis-atspi` as a
separate process with a compositor-lifetime principal. Its credential crosses
an inherited stdin pipe, not argv, environment, or disk. The adapter maps
AT-SPI trees into complete bounded revisions; `aegis-semantic` rejects an
invalid graph, oversized text/tree, escaping geometry, stale revision, or
provider takeover. Nested sessions do not attach to the host AT-SPI bus,
because that could confuse outer-desktop objects with inner windows. The
compositor resolves the adapter only as a sibling of its own executable and
rejects a symlink, non-regular or non-executable file, owner mismatch, or
group/world-writable binary or parent directory. The child starts with a
cleared environment and receives only `XDG_RUNTIME_DIR`, the session and
AT-SPI bus addresses, locale, and `RUST_LOG`; credentials and the ambient
`PATH` are never forwarded.
Before mapping a tree, the adapter requires the AT-SPI D-Bus Unix process id
to equal the kernel credential captured from the still-live Wayland client,
then uses an exact non-empty title to select one toplevel. PID-bearing
bindings are available only to an authenticated provider holding both
`ObserveWindows` and `PublishAccessibilityTree`; `GetWindows` never exposes
them. Missing or ambiguous correlation fails closed.
`PublishAccessibilityTree` and `DispatchAccessibilityAction` are reserved for
the compositor-provisioned ephemeral system principal: ordinary Agent
pairing and administrator registration cannot grant them. The adapter also
refuses unsupervised startup.

Secret prompt values and Agent credentials are never journaled. IPC framing
zeroizes raw JSON serialization/deserialization buffers, and the server
zeroizes its typed secret or newly issued credential copy immediately after
the response write. `SecretPromptResult` also clears its value on drop;
callers should still call `zeroize()` immediately after downstream use so the
lifetime is explicit and minimal.

The response includes an opaque, random observation token and an informational
15,000 ms TTL. The compositor binds the authoritative snapshot to the
connection, authenticated principal, and Interaction Domain. The token is single-use and
is revoked on the owning Actor's first action attempt, connection disconnect,
expiration, session lock, inactive seat, or Interaction Domain lifecycle invalidation. An
attempt from a different Actor neither uses nor revokes the owning Actor's
token.

`ActInInteractionDomain { intent }` names the Interaction Domain, semantic target, observation token,
and bounded actions. Compositor-owned roots accept 1–64 prepared fallback
actions. Accessibility targets accept exactly one semantic action so a
provider can never partially apply a batch. On the compositor main loop it
atomically checks:

- the token's connection, principal, Interaction Domain, and expiry;
- the Actor's live capability ceiling, runtime grant, and resource allowlists;
- unchanged Interaction Domain authority and complete semantic target state;
- an active Interaction Domain seat and current interaction authority;
- that the target declares every requested semantic action; and
- that pointer positions remain inside the target-local extent.

The complete batch is prepared before delivery. Accessibility actions are
then queued to their owning provider, which immediately re-reads live role,
state, bounds, names, value fingerprint, and declared actions before invoking
AT-SPI. Password contents are never requested or published, and password text
actions are not declared because polling cannot prove an unchanged
same-length secret. Provider rejection, disconnect, queue saturation, stale
state, or timeout produces a refused audit event and no commit receipt. Any
mismatch aborts. A
successful `ActorActionCommitted` receipt contains `action_id`, `interaction_domain`,
`target`, the compositor-resolved owning `window`, `authority_revision`,
`actions_applied`, and `committed_mono_ms`.
The transaction closes the observation-to-dispatch race; it does not roll
back application business state after the application receives an event.

For `CaptureInteractionDomain`, the compositor refreshes the internal observation lease
only when the encoded capture passes its delivery-time lock and authority
checks. GPU readback and PNG encoding therefore do not consume the advertised
15-second client action window, and a disconnected Actor cannot refresh it.

## Capture

aegis exposes pixel capture through fail-closed operations that share one
same-frame presentation readback path (ADR-0037), plus per-window offscreen
capture. All are refused while the
session is locked or the seat is inactive. The request copies the exact frame
being submitted; later client commits, animations, or wallpaper frames cannot
change the detached snapshot. Captures include the overview grid while
overview mode is active.

`Command::Screenshot { path, region }` is a journaled `control` command that writes
the focused output as a PNG file; `aegis display capture` is its reference
frontend. `Request::CaptureOutput` is a synchronous query returning
`Response::CaptureOutput { width, height, png_bytes }` followed by one sealed
PNG `memfd` transferred with `SCM_RIGHTS`. The request requires the `control`
capability and an explicit `CaptureOutput` entry in the connection's scope
`ops`; like `InjectInput`, the operation is never inherited through the
unrestricted default.

The receiver must read the descriptor immediately after the JSON response,
check that its file length equals `png_bytes`, and require
`F_SEAL_SEAL | F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_WRITE`. Capture blobs are
limited to 288 MiB independently of the 16 MiB JSON frame limit.

Capture regions use compositor logical pixels. The returned PNG and
`width`/`height` use physical output pixels, so a region captured at 200%
scale has twice the logical width and height.

Continuous physical-output frame streaming reuses the same scale-aware
frame-copy path; see [Streaming](#streaming). Interaction Domain observers do not need to
poll: `InteractionDomainDamaged` tells them when a directed scene should be recaptured.

`CaptureInteractionDomain` reads only the selected Interaction Domain's directed virtual output. It
does not contain physical-desktop chrome, the cursor, or another Interaction Domain. The
response contains `capture` metadata with `interaction_domain`, physical `width` and
`height`, `scale_milli`, the logical `region`, `placements`, `png_bytes`, the
authority `revision`, and a semantic `observation`, followed by the sealed PNG
descriptor. Each placement contains a window id, its rectangle in
virtual-output logical coordinates, and the target-local surface size. The
observation token must be supplied to `ActInInteractionDomain`; pixels and coordinates
alone carry no authority.

`Request::CaptureWindow { window }` (protocol 26, ADR-0117) captures one
window's real content wherever it lives: the compositor renders the window's
complete surface tree into a fresh offscreen target, so foreground, occluded,
minimized, and foreign-workspace windows all capture their true pixels. The
image's origin is the toplevel's logical origin; popups extending past the
toplevel bounds are clipped, mirroring `StreamTarget::Window` semantics. The
reply `Response::CaptureWindow { capture }` carries `window`, physical
`width`/`height`, `scale_milli`, the toplevel's logical `rect` at capture
time, and `png_bytes`, followed by one sealed PNG `memfd` under the same
sealed-blob rules as `CaptureOutput`. `scale_milli` comes from the output the
window is currently visible on, falling back to the primary output and then
to 1000. Authorization is fail-closed like `CaptureOutput`: the `control`
capability, a live lease, and an explicit `CaptureWindow` scope decision for
that exact window — the scope's `windows` axis bounds which windows may be
captured, and the operation is never inherited through the unrestricted
default. Agents may request `CaptureWindow` at pairing; the first use always
asks the user through a runtime grant. The writer rechecks the live scope,
the lock/VT gate, the lease, and that the window still exists immediately
before sending the sealed descriptor.

A security generation invalidates in-flight pixels across session-lock,
inactive-seat, pause, and revocation boundaries, including a lock followed by
a quick unlock. Encoding runs in a bounded worker. The compositor main thread
checks scope, lease, security generation, and Interaction Domain state before queuing the
result; the sole IPC writer checks the live scope, lock/VT gate, Interaction Domain state,
authority revision, and lease again immediately before it sends the sealed
descriptor.

## Streaming

`Request::StreamOutputStart { max_fps, target, dmabuf, cursor }` opens a
continuous frame stream (ADR-0052, pacing and renegotiation per
[ADR-0126](../adr/0126-damage-driven-stream-pacing-and-geometry-renegotiation.md),
window rendering and cursor compositing per
[ADR-0127](../adr/0127-occlusion-safe-window-streams-and-cursor-compositing.md)).
Authorization matches
`CaptureOutput`: the `control` capability plus an explicit `StreamOutput`
entry in the connection's scope `ops`, never inherited through the
unrestricted default. The reply `Response::StreamOutputStarted { stream_id,
width, height, format }` fixes the negotiated geometry until the stream is
restarted. `max_fps` throttles delivery; it defaults to 30 and is clamped
to 1–240 since protocol 29 (1–60 before).

`target` selects what the stream captures: the whole desktop frame
(`{"type":"Output"}`, the default), one connector's physical rectangle of
it (`{"type":"Output","output":"HDMI-A-1"}`, protocol 29; an unknown
connector is refused, and `Request::EnumerateOutputs` reports the
selectable connectors with their rectangles), or one window
(`{"type":"Window","window":...}`, protocol 6). A window stream renders
the window's complete surface tree into its own offscreen target
([ADR-0127](../adr/0127-occlusion-safe-window-streams-and-cursor-compositing.md)),
independent of the desktop frame: an occluded, minimized, or
foreign-workspace window keeps streaming its real content, and no foreign
pixels can leak into the stream. `cursor`
(protocol 29) negotiates the cursor mode: `hidden` (the default) keeps the
compositor's theme cursor out of the frames, `embedded` composites it
wherever its position falls inside the captured region — drawn on the GPU
for dmabuf streams and for window streams, blended into the readback for
SHM output streams. Only the theme cursor is negotiated; a client-provided
cursor surface is scene content and appears in output streams regardless
of mode, and on the software-cursor fallback (nested or degraded direct
display) the presented frame already contains the cursor, so `hidden`
cannot subtract it there. A window target may opt
into the dmabuf transport since protocol 29; the runtime honors it
wherever an exportable capture surface is available and falls back to SHM
with a warning otherwise.

Streams pace presentation at their negotiated cadence (ADR-0130): an
output stream whose `max_fps` interval has elapsed forces a presentation
when nothing else produces one, so a consumer receives frames at that
rate even on a static screen — including cursor-only motion, which the
hardware cursor plane would otherwise hide from the stream. Every
damage-driven composite additionally serves every due stream between
forced frames. Direct scanout is disqualified while an output stream
lives, so fullscreen content is composited — and therefore captured —
for the stream's lifetime. Window streams render their own target instead
of capturing from composites: a window stream renders when its surface
tree committed since the last capture and its `max_fps` interval elapsed,
re-rendering the clean tree at a one-second liveness tick (ADR-0127).

Each captured frame arrives as `Event::StreamFrame { stream_id, sequence,
width, height, stride, format, damage, dropped, byte_len }` followed
immediately by one sealed memfd of `byte_len` tightly packed pixels
(`height` rows of `stride` bytes), transferred with the same sealed-blob
rules as one-shot captures. `format` is `Bgra8` today; `damage` describes,
in the stream's own coordinate space, the regions that changed since the
last frame the consumer received. It is conservative: a forced
(cadence-driven) frame, a moved crop origin, or damage that never
intersected the target all report one full-frame rectangle. `dropped`
is the cumulative count of frames lost to backpressure since the stream
started: delivery runs over a bounded two-frame lane per stream, and excess
frames are dropped rather than queued.

`Request::StreamOutputStop { stream_id }` ends a stream owned by the calling
connection and answers `Response::StreamOutputStopped`. The server ends a
stream with `Event::StreamEnded { stream_id, reason }` when the connection's
scope is revoked or narrowed, its lease expires, the streamed window
closes, the streamed connector disappears, or the compositor shuts down.
A pure geometry change — desktop resize, a mode change on the streamed
connector, or a window resize — instead freezes the stream with
`Event::StreamGeometryChanged { stream_id, width, height }` (protocol 29):
the stream stays registered but produces no further frames until the
client restarts it with `StreamOutputStop` plus a fresh
`StreamOutputStart`, which re-negotiates at the new geometry. The event
goes only to connections that negotiated protocol 29; an older client
simply observes frames stop. Session lock and an inactive VT pause
delivery instead of ending the stream; resuming restarts it transparently.
Disconnecting the connection stops every stream it owned. Frame events,
lease-renewal replies, and end events interleave on the streaming
connection, so streaming clients read one continuous message stream instead
of one reply per request.

Frame readback uses the shared CPU path for SHM-transport consumers.
Connections that negotiate the dmabuf transport (protocol 25 and later, and
`dmabuf: true` on the start request) receive frames as GPU-copied slot
references instead: the compositor blits each presented frame into a fixed
per-stream slot ring, delivers the slot once with JSON metadata only, and
recycles it on `StreamBufferRelease`. Both transports apply the same
per-frame authorization and lock/VT delivery gates, and dmabuf frames that
crossed a security boundary between blit and delivery are dropped rather
than delivered. See [ADR-0055](../adr/0055-zero-copy-dmabuf-frame-export.md)
for the superseded design and the portal repository's ADR-0005 for the
shipped slot-ring contract.

## Idle Inhibition

`Request::SetIdleInhibit { inhibit }` sets or clears the calling
connection's global, surfaceless idle inhibitor
([ADR-0075](../adr/0075-independent-portal-package-and-backend-contract.md)),
built for the portal backend's Inhibit interface. Authorization matches
`CaptureOutput`:
the `control` capability, a live lease, and an explicit `IdleInhibit` entry
in the connection's scope `ops`, never inherited through the unrestricted
default. While any connection holds an inhibitor, ext-idle-notify
notifications stay resumed, exactly as if a visible per-surface
`zwp_idle_inhibit_v1` inhibitor were active; a locked session suppresses
its effect the same way. The reply `Response::IdleInhibitSet { inhibited }`
confirms the state the connection now holds. The inhibitor is
connection-scoped: disconnecting releases it, so a crashed holder can never
keep the session out of idle.

## Interactive Picking

The pick requests ask the user to choose something in compositor chrome;
the connection blocks (bounded by a compositor interaction timeout) until
the user confirms or cancels. They share one authorization shape, fail-closed
exactly like `SetIdleInhibit`: the `control` capability, a live lease, an
explicit entry in the connection's scope `ops` — never inherited — plus a
lock/VT gate, and a scope+lease re-check before the result is delivered.
One interactive pick at a time compositor-wide, shared across all kinds; a
concurrent request is refused. `PickTarget` freezes the screen and reads
user-approved screen content; the others are ordinary modal chrome over
the live scene and capture no screen content.

| Request | Reply | Scope op | Purpose |
|---------|-------|----------|---------|
| `PickTarget { kind }` | `Picked { result }` | `PickTarget` | Region, pixel, window, or output picking for Screenshot and ScreenCast ([ADR-0054](../adr/0054-interactive-target-picking.md)) |
| `PickApp { choices, subject, last_choice }` | `AppPicked { result }` | `PickApp` | AppChooser portal: one application out of the candidates (protocol 14) |
| `PromptSecret { title, reason }` | `SecretPrompted { result }` | `PromptSecret` | Reserved masked credential prompt; both ends zeroize their copies. The portal's vault unlock is Portal-owned and does not use it ([ADR-0112](../adr/0112-native-portal-secret-with-portal-owned-prompts.md)) (protocol 15) |
| `PickConfirm { title, body, accept_label }` | `ConfirmPicked { result }` | `PickConfirm` | Yes/no consent dialogs (Account, DynamicLauncher, Wallpaper, future Access) (protocol 16) |

`PickTarget` kinds and results: `Region` answers the dragged rectangle,
`Pixel` the clicked point and its colour, and `Window` the clicked
toplevel — or, on Enter or a click on empty desktop, the whole output as
the bare `Output` result (no connector). `Output` (protocol 29,
[ADR-0128](../adr/0128-peer-identity-bound-built-in-scopes-and-capture-indicator.md))
highlights the output under the cursor and answers
`Output { connector }` with the clicked connector; Escape cancels every
kind. The bare `Output` shape is unchanged: `connector` is additive and
omitted when absent, so pre-29 replies and requests round-trip exactly as
before.

## Scopes and Agent Authorization

Named scopes survive only as hardcoded trusted-component grants (ADR-0090);
config-declared `[[agent.scope]]` entries were removed in protocol 18.

A built-in scope name is not authority by itself
([ADR-0128](../adr/0128-peer-identity-bound-built-in-scopes-and-capture-indicator.md)).
At accept the server reads the peer's `SO_PEERCRED` and refuses any
connection whose uid differs from the compositor's — defense in depth
behind the owner-only `0600` socket. When `Hello` names a built-in scope,
the peer's canonicalized `/proc/<pid>/exe` must appear in that scope's
executable allowlist, or the claim is refused with
`scope 'X' is not available to this process` and journaled as a
`ScopeClaim` refusal; an unreadable identity fails closed. The same match
governs the `[agent] lockdown` exemption, so the exemption can never be
name-only. Compiled-in allowlists map `aegis-portal` to
`xdg-desktop-portal-aegis` and the admin scopes to the `aegis` CLI in the
usual install prefixes; the additive `[ipc.scope_executables]` config
table replaces them per scope (see the
[configuration reference](config.md#ipc)). Anonymous connections and
paired agents are unaffected: peer identity binds platform components,
never agents (ADR-0088).

Native `aegis` commands use separate `aegis-owner-admin`,
`aegis-interaction-domain-admin`, and `aegis-agent-admin` scopes for ordinary
owner mutations, Interaction Domain recovery, and agent-registry administration.

`xdg-desktop-portal-aegis` uses the built-in owner-only `aegis-portal`
scope, which grants exactly these operations: `CaptureOutput` for Screenshot,
`StreamOutput` for ScreenCast, `IdleInhibit` for Inhibit, `PickTarget` for
user-confirmed Screenshot and ScreenCast selection, `PickApp` for AppChooser,
`Notify` and `DismissNotification`
for Notification, `PickConfirm` for the consent dialogs, plus `SetWallpaper`
for the Wallpaper portal's decode-and-swap mutation (protocol 17). It grants
no general compositor control. The portal boundary is recorded in
[ADR-0075](../adr/0075-independent-portal-package-and-backend-contract.md)
and its extension in
[ADR-0099](../adr/0099-resource-authority-and-out-of-process-file-chooser.md).
Secret password input is Portal-owned and does not cross compositor IPC
([ADR-0112](../adr/0112-native-portal-secret-with-portal-owned-prompts.md));
the `PromptSecret` operation is a reserved, runtime-gated capability with no
production grantee. The built-in high-risk scopes are fail-closed explicit
allowlists.

Protocol 20 removes the former `PickFile` request, response, types, and scope
operation. FileChooser and its path data now stay inside the independent
portal package; Aegis participates only through xdg-foreign-v2 window
parenting.

Protocol 21 separates authenticated Actor observation families, filters query
resources, adds semantic `ObserveInteractionDomain` and observation-bound `ActInInteractionDomain`,
records authenticated Actor origins, and rejects unguarded Interaction Domain input. See
[ADR-0102](../adr/0102-actor-scoped-semantic-observation-and-transactional-actions.md).

Protocol 22 adopts the `InteractionDomain`, `ActorCapability`,
`AuthorizationDecision`, and `ConnectionCapabilities` vocabulary and moves
capability and observation-transaction policy into the authority module of
`aegis-security`. It is a major-version boundary; older clients are refused
at `Hello`.

Protocol 23 adds explicit Actor sessions, dynamic exact-resource grants, and
the authenticated accessibility tree/action adapter. Resource handles are
opaque 256-bit ids bound to one session, optional principal, exact normalized
resource, capability, monotonic TTL, and use count. Filesystem and origin
grants are authority for a compatible resource broker; they do not mutate a
sandbox or voluntarily grant ambient process access. Secret prompting already
consumes a one-use exact grant. Payment requests always require fresh exact
human confirmation.

Protocol 24 adds `GetAccessibilityWindows`, a provider-only process-bound
window seam. It prevents the trusted adapter from assigning an untrusted
AT-SPI tree to a Wayland window based only on spoofable application metadata.

Protocol 27 adds workspace-directed application launching (`LaunchApp`
with an optional `LaunchPlacement`) and the additive `reveal` flag on
`Focus`, serde-defaulted to `true` for older peers. See
[ADR-0118](../adr/0118-launch-placement-and-workspace-isolation.md).

**Agent authorization** (protocol 19, ADR-0090) replaces configured scopes
for agents. `Hello.agent` carries a self-declaration: a cosmetic `label`,
the `requested` operation families, and an optional `credential` from an
earlier pairing. Unrecognized agents are paired interactively through
compositor chrome; the user-approved set becomes the principal's ceiling,
and `Hello.agent` in the reply carries the issued `principal` and a new
`credential` to persist. The connection's effective scope is synthesized
from the ceiling: ordinary approved operations are pregranted, and the
platform dangerous set (`Close`, `InjectInteractionDomainInput`, `CreateInteractionDomain`,
`TransactInteractionDomain`, `RevokeInteractionDomain`, `CaptureInteractionDomain`, `ObserveInteractionDomain`,
`LaunchInInteractionDomain`, `LaunchApp`) lands in
`ask_ops`, where every use routes through an interactive runtime grant
(Deny / Allow once / Allow session / Always allow) before dispatch.

Handshake scope names, labels, credentials, and capability lists are bounded
and validated before registry lookup or pairing; duplicate requested
capabilities are rejected. The principal/grant stores accept only bounded,
semantically valid owner-only regular files with one link and refuse
symlinks, unsafe modes, oversized files, duplicate identities, malformed
ceilings, and component-only capability injection. Credential digests are
compared in constant time. Cleartext handshake, issued-credential, secret,
and identity-file buffers are zeroized after use, and sensitive `Debug`
representations are redacted.

Principal and grant management requires the agent-admin scope (plus
`control` and a live lease for mutations):

| Request | Response | Purpose |
|---------|----------|---------|
| `GetAgentPrincipals` | `AgentPrincipals { principals }` | Agent-admin scope: list paired principals with their ceilings. |
| `GetAgentGrants { principal? }` | `AgentGrants { grants }` | Agent-admin scope: list recorded grants, optionally for one principal. |
| `RenameAgentPrincipal { principal, label? }` | `Ok` | Rename the cosmetic display label. |
| `ForgetAgentPrincipal { principal }` | `Ok` | Kill a credential and drop its grants. |
| `SetAgentCeiling { principal, pregranted, gated }` | `Ok` | Replace a principal's approved ceiling. |
| `RegisterAgent { label?, pregranted, gated }` | `AgentRegistered { principal, credential }` | Pre-provision a principal; plant the credential in the agent. |
| `RevokeAgentGrant { principal, op }` | `Ok` | Drop one recorded grant; the next use asks again. |

See the [Configuration Reference](config.md#agent-authorization) for the
`[agent]` policy table, and the [aegis-mcp Bridge Reference](aegis-mcp.md)
for the agent-side contract.
