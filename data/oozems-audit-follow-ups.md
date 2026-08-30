# Oozems Audit Follow-up Issues

Date: 2026-08-28

## Purpose

This document records issues found while auditing the SurrealDB-to-SQLite and
map-only persistence change. These issues are not part of the current SQLite
migration unless stated otherwise. They are recorded here so that they can be
planned and fixed independently.

## Resolution Status

Resolved: 2026-08-29

| Issue | Resolution |
|---|---|
| Cooldown cancellation | Pending skill and basic-attack reservations now release themselves when their request or transaction is dropped. Successful transactions explicitly commit the reservation. |
| Reconciliation quarantine | Incomplete compensation quarantines the player before releasing the player lock. Later mutations and bootstrap return `player_reconciliation_required`. A server restart remains the documented recovery path until all runtime stores support safe live repair. |
| Relocation request races | Portal, taxi, and respawn requests now occupy the movement lane, which drains existing movement and suppresses new snapshots until relocation finishes. |
| Portal consistency gap | Accepted portal responses include the complete target map. The client no longer performs a second map request after the server commits relocation. |
| Transition installation order | Portal, taxi, and respawn validate player, map, position, buffs, and response identity before using one map-first installation pipeline. Failed committed installations block source-map movement and trigger a rate-limited reload through bootstrap. |
| Stale client authorization | Bootstrap rotates a cryptographically random bearer gameplay-session token. Player-scoped requests revalidate it after acquiring the player lock. |
| Movement map agreement | Initialization, relocation, and existing-session boundaries enforce player, map, and movement-session map agreement before tracker mutation. |
| API module cohesion | Item and skill endpoints moved to `api/items.rs` and `api/skills.rs`; the root production module is below 1,000 lines. |

The current migration plan already covers these related findings:

- Atomic server-side relocation planning and compensation
- Durable map IDs without durable coordinates
- Reconnect spawning and invalid-map repair
- Narrowing `SavePlayerRequest`
- SQLite revision compare-and-swap and durable compensation
- Removing the ineffective database-layer player guard check

## Priority Summary

| Priority | Issue |
|---|---|
| High | Cooldown reservations can leak when request tasks are cancelled |
| High | Failed reconciliation does not quarantine the affected player |
| High | Portal and taxi requests can race with movement requests |
| High | A committed portal transition can leave the client on the source map |
| Medium | Transition responses install client state in inconsistent orders |
| Medium | A stale client remains authorized after another client bootstraps |
| Medium | Movement initialization does not enforce player/map agreement |
| Low | `api.rs` has exceeded the repository's module-size guideline |

## 1. Cooldown Reservations Can Leak On Cancellation

Severity: High

### Evidence

Skill use reserves a cooldown and stages it in a `PlayerTransaction` before it
awaits mob simulation:

- `crates/oozems-server/src/api.rs:777`
- `crates/oozems-server/src/api.rs:785`
- `crates/oozems-server/src/api.rs:797`

Basic attack follows the same pattern:

- `crates/oozems-server/src/api/combat.rs:86`
- `crates/oozems-server/src/api/combat.rs:93`
- `crates/oozems-server/src/api/combat.rs:113`

`PlayerTransaction` compensates reservations only when an explicit commit or
abort path calls `rollback_player_transaction`. Dropping a staged transaction
does not release its cooldown reservations.

### Failure Mode

If an HTTP request is cancelled while mob simulation is pending, the request
future and transaction are dropped. The reservation remains in the cooldown
store even though the attack did not complete. The player can then receive a
false cooldown rejection until the reservation expires.

This is the same ownership pattern that caused the relocation cancellation
window, but it applies to skill and attack reservations rather than movement.

### Recommended Fix

Make reservation ownership cancellation-safe before performing any await after
reservation.

Preferred design:

1. Perform all side-effect-free validation first.
2. Create a transaction plan containing the requested cooldown reservation.
3. Enter a cancellation-shielded coordinator.
4. Reserve the cooldown inside that coordinator.
5. Run or await the staged mob operation.
6. Commit the reservation only when the operation succeeds.
7. Release it on every failure path.

An RAII reservation guard is also viable if releasing a reservation is
synchronous and can safely run from `Drop`. The guard must transfer ownership
to the committed cooldown record explicitly so normal return does not release
it.

### Tests

- Block skill mob simulation after cooldown reservation, cancel the request,
  and verify the same skill can be retried immediately.
- Block basic-attack simulation after reservation, cancel the request, and
  verify the next basic attack is admitted immediately.
- Cancel while explicit compensation is running and verify the coordinator
  finishes before releasing the player lock.
- Verify a successfully committed attack still enforces its cooldown.

## 2. Reconciliation Failure Does Not Quarantine The Player

Severity: High

### Evidence

`PlayerTransactionError::Reconciliation` reports that one or more stores could
not be compensated:

- `crates/oozems-server/src/player_transaction.rs:132`
- `crates/oozems-server/src/player_transaction.rs:387`
- `crates/oozems-server/src/player_transaction.rs:444`

The API logs the error and returns `player_reconciliation_required`:

- `crates/oozems-server/src/api/protocol.rs:237`

No state records that the player is unsafe to mutate. The per-player lock is
released after the request, and a later request can continue using divergent
database, movement, effects, drops, cooldown, recovery, or mob state.

### Failure Mode

A later operation can observe one subset of the failed transaction and make
new changes on top of it. This can turn a recoverable one-operation divergence
into persistent corruption across several stores.

### Recommended Fix

Add a per-player reconciliation registry with explicit states such as:

```text
Healthy
ReconciliationRequired { operation_id, failures }
Rehydrating
```

The transaction coordinator should mark the player before releasing the lock
when compensation is incomplete. Every mutating request and bootstrap should
check this state after acquiring the player lock.

Recovery options should be explicit:

- Rehydrate runtime-only stores from the durable player when that is safe.
- Provide an administrative repair operation for stores that cannot be
  reconstructed automatically.
- Require a server restart only as a documented last resort.

Do not clear quarantine merely because a later request loads successfully.

### Tests

- Force a durable-save compensation conflict and verify the next mutation is
  rejected before touching any store.
- Force movement compensation failure and verify bootstrap cannot silently
  replace the evidence of divergence.
- Complete a successful rehydration and verify the player becomes healthy.
- Verify quarantine is scoped to the affected player.

## 3. Portal And Taxi Requests Can Race With Movement

Severity: High

### Evidence

Client request admission assigns lanes in
`crates/oozems-client/src/game/requests.rs:44`.

The relevant definitions are:

```text
Transition  = PLAYER_MUTATION | TRANSITION
Interaction = PLAYER_MUTATION | INTERACTION
Respawn     = PLAYER_MUTATION | MOVEMENT | RESPAWN
Movement    = MOVEMENT
```

Portal uses `RequestKind::Transition`. Taxi runs through
`RequestKind::Interaction`. Neither occupies the movement lane, while respawn
does.

Movement response sequencing is enforced through
`last_response_sequence` in:

- `crates/oozems-client/src/game/movement_actions.rs:177`
- `crates/oozems-client/src/game/movement_actions.rs:231`

### Failure Mode

A movement request can remain in flight during a portal or taxi request, or a
new movement request can start while relocation is pending. Its response can
advance `last_response_sequence` and cause the committed relocation response
to be classified as superseded.

The server can therefore durably commit the target map while the client keeps
rendering the source map.

### Recommended Fix

All operations capable of relocating a player must occupy the movement lane.

Possible implementation:

- Add a dedicated `Relocation` request kind using
  `PLAYER_MUTATION | MOVEMENT | RELOCATION`.
- Use it for portal, taxi, and respawn.
- Split travel interactions from non-travel NPC interactions when the client
  knows the operation type.
- If that split is not available before the response, make all interaction
  requests occupy `MOVEMENT`; the extra serialization is safer than an
  inconsistent map.

Admission should also stop movement snapshot scheduling while a relocation
permit is held. Existing movement work must either drain before relocation is
sent or be made unable to supersede the relocation response.

### Tests

- Delay a movement response, commit a portal transition, then release the
  movement response and verify the target map remains installed.
- Repeat the scenario for taxi and respawn.
- Verify new movement snapshots are not sent while relocation is active.
- Verify ordinary non-travel interactions retain the intended concurrency
  policy.

## 4. Portal Transition Has A Two-Request Consistency Gap

Severity: High

### Evidence

Portal installation performs two network requests in
`crates/oozems-client/src/game/movement_actions.rs:146`:

1. `enter_portal` commits the server-side transition.
2. The client accepts the authoritative target snapshot.
3. `get_map` fetches the target map separately at line 172.
4. `install_map` runs only after that second request succeeds.

Taxi and respawn responses already include their target map and do not have
this exact gap.

### Failure Mode

If `get_map` fails after `enter_portal` succeeds, the server and durable player
are on the target map while the client still renders the source map. Retrying
the portal may fail because the server no longer considers the player to be at
the source portal.

The current error message reports failure but does not perform an authoritative
reload or guarantee an automatic retry.

### Recommended Fix

Preferred option: include the target map in the accepted portal response, as
taxi and respawn already do. Validate the map, player, and authoritative
snapshot together before installing any state.

If response size makes that undesirable, use an explicit transition state:

```text
PendingServerCommit
PendingTargetMap { map_id, authoritative }
Installed
RequiresBootstrap
```

The client must retry the target-map fetch or bootstrap authoritative state. It
must not resume source-map movement after the server has committed the target.

### Tests

- Make `enter_portal` succeed and `get_map` fail; verify the client enters a
  retry or rebootstrap state and does not resume source movement.
- Retry the map fetch and verify installation completes without another portal
  request.
- Verify a stale target-map response cannot overwrite a newer bootstrap.
- Verify a complete portal response installs atomically when the map is
  included directly.

## 5. Transition Responses Install Client State In Different Orders

Severity: Medium

### Evidence

Respawn validates the complete response, installs relocation, then installs
the player and buffs:

- `crates/oozems-client/src/game/respawn_actions.rs:39`
- `crates/oozems-client/src/game/respawn_actions.rs:57`
- `crates/oozems-client/src/game/respawn_actions.rs:60`

NPC interactions install player domains and active buffs before attempting a
taxi relocation:

- `crates/oozems-client/src/game/interaction_actions.rs:203`
- `crates/oozems-client/src/game/interaction_actions.rs:218`
- `crates/oozems-client/src/game/interaction_actions.rs:229`

### Failure Mode

If taxi relocation is incomplete, invalid, or superseded, progression,
inventory, mesos, skills, and buffs can already reflect the committed travel
operation while the rendered map remains the source map.

This creates a partial client install even though the server response describes
one atomic operation.

### Recommended Fix

Parse and validate responses into a prepared installation value before
mutating `Game`:

```text
wire response
-> validate all required fields and cross-field IDs
-> PreparedPlayerUpdate
-> install relocation
-> install player domains
-> install buffs
-> install indicators and UI state
```

Use one shared relocation installation pipeline for portal, taxi, and respawn.
If relocation cannot be installed, do not partially install the remaining
response. Enter the same retry or authoritative-bootstrap path used for portal
map-fetch failure.

### Tests

- Supply a taxi response with mismatched map IDs and verify no player domain is
  changed.
- Supersede a taxi relocation and verify mesos, inventory, buffs, and map all
  retain the previous coherent state until reconciliation.
- Verify portal, taxi, and respawn use the same installation order.

## 6. Bootstrap Does Not Revoke Older Clients

Severity: Medium

### Evidence

Requests identify the player only by `player_id`. Bootstrap replaces the
movement session and resets its sequence to zero:

- `crates/oozems-server/src/api.rs:72`
- `crates/oozems-server/src/movement.rs:204`
- `crates/oozems-server/src/movement.rs:648`

No session generation or token distinguishes the client that performed the
latest bootstrap from an older browser tab or stale client process.

### Failure Mode

After a new client reconnects and receives a fresh spawn, an older client can
continue submitting movement and mutation requests for the same `player_id`.
The per-player lock serializes the requests but does not determine which client
is authoritative.

This weakens the reconnect guarantee and can cause movement sequence resets,
unexpected relocations, or changes submitted from a stale UI.

### Recommended Fix

Add a random session token or monotonically increasing session generation:

1. Bootstrap creates and stores a new token for the player.
2. Bootstrap returns that token to the client.
3. Every subsequent movement and mutation request includes the token.
4. The server rejects tokens that are not current.
5. A new bootstrap atomically invalidates the previous token.

A common request header can avoid adding a field to every protobuf message. If
the token is sent in protobuf, use a shared envelope rather than duplicating
validation in every endpoint.

The token is a gameplay session identity, not a substitute for authentication.

### Tests

- Bootstrap client A, then client B, and verify A's movement is rejected.
- Verify A cannot submit inventory, skill, taxi, portal, or preference changes
  after B bootstraps.
- Verify B continues normally.
- Verify reconnect rotation is atomic with movement-session replacement.

## 7. Movement Initialization Does Not Enforce Map Agreement

Severity: Medium

### Evidence

`movement::initialize_player` receives both `PlayerState` and `Map`:

- `crates/oozems-server/src/movement.rs:204`

It registers the supplied map but creates the movement session with
`player.map_id` at line 217. There is no release-mode check that
`player.map_id == map.id`.

### Failure Mode

A caller can create a movement session whose map ID names one map while its
terrain, bounds, platforms, and initial motion were calculated from another.
Subsequent movement validation can then use inconsistent geometry or fail in a
non-local way.

### Recommended Fix

Validate the invariant at the movement boundary and return a typed error:

```text
player.map_id must equal map.id
```

Do the same for relocation plans and any helper that receives a player plus a
source map. Do not rely on `debug_assert` for cross-store identity checks.

### Tests

- Attempt initialization with mismatched IDs and verify no map or player
  session is registered.
- Attempt relocation with a source map that differs from the player's session
  and verify the tracker remains unchanged.

## 8. API Module Has Lost Cohesion

Severity: Low

### Evidence

`crates/oozems-server/src/api.rs` is currently about 1,227 lines, excluding its
submodules. It contains bootstrap, character creation, skills, abilities,
recovery, preference saves, asset delivery, helpers, and route-level error
translation.

This exceeds the repository guideline that modules remain under 1,000 lines
exclusive of tests.

### Impact

The file mixes unrelated endpoint pipelines and makes broad changes, such as
runtime hydration or protocol updates, harder to review. It also increases the
chance that helpers intended for one endpoint become implicit shared policy.

### Recommended Fix

Split by cohesive behavior rather than by arbitrary size:

- `api/bootstrap.rs` for bootstrap, character creation, and runtime hydration
- `api/skills.rs` for skill book, allocation, and skill use
- `api/recovery.rs` for recovery endpoints
- `api/preferences.rs` for key-binding persistence
- Keep shared decoding, player locking, time, and error adapters in the parent
  module

Perform this after the SQLite migration unless extracting bootstrap is needed
to keep that implementation readable. Avoid combining a broad mechanical move
with behavioral changes in the same commit.

### Tests

No new behavior tests are required solely for module extraction. Run the full
server test suite before and after the move and compare route registration.

## Suggested Work Order

1. Fix cooldown reservation ownership because it can affect ordinary combat
   without any map transition.
2. Add reconciliation quarantine before introducing more compensated stores.
3. Serialize relocation and movement request lanes.
4. Remove the portal two-request gap or add mandatory transition recovery.
5. Unify transition response validation and installation.
6. Add session generation if multiple tabs or reconnect replacement must be
   strictly enforced.
7. Add movement map-ID boundary checks.
8. Split `api.rs` when nearby behavioral work has settled.

## Scope Notes

The issues in this document should not be silently folded into the SQLite
migration, except where the migration's approved atomic relocation work
directly touches the same code. Each issue changes a separate concurrency,
recovery, protocol, or client-state contract and deserves focused tests and a
reviewable change set.
