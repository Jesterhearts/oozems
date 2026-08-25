# Oozems

Oozems is an original old-school side-scrolling RPG foundation for personal
use. It does not include MapleStory code or assets.

## Not Ready For General Use

The current version of the server is not yet ready for general use. Combat is
still limited to basic player attacks, player skills, mob contact attacks, and
basic mob projectiles.
Player death handling is not implemented. Quest support currently covers a
typed subset of `Quest.wz`.

When it is ready, a release tag will be posted for a version 0.1. That will
indicate general usage availability, although polish and bug fixes will
likely be needed after such a release.

## Run it

Install Rust, the `wasm32-unknown-unknown` target, and Trunk. Local interaction
definitions require matching `Map.wz`, `Npc.wz`, and `Character.wz` archives in
`./data`. `Quest.wz` is optional. Add `UI.wz` to display the configured native
interaction windows and Cash Shop screen, then run:

```sh
make run
```

Open <http://127.0.0.1:3000>. The Make target builds the WASM client into the
server's generated `public` directory before starting the server.

The default data directory is `./data`. It contains the local WZ archives,
SurrealKV state, and version-specific `cash-shop.toml`, `interactions.toml`,
`loot.toml`, and `quest-scripts.toml` files. It is ignored by Git. These
environment variables override the defaults:

| Variable             | Default                             |
| -------------------- | ----------------------------------- |
| `OOZEMS_BIND`        | `127.0.0.1:3000`                    |
| `OOZEMS_DATA_DIR`    | `./data`                            |
| `OOZEMS_CONFIG_DIR`  | `./config`                          |
| `OOZEMS_PUBLIC_DIR`  | `crates/oozems-server/public`       |
| `OOZEMS_WZ_DIR`      | `./data`                            |

## Inspect and edit WZ archives

The workspace includes `oozems-wz`, a JSON-first CLI for repeatable WZ
inspection and safe PKG1 property edits. It inspects standard PKG1 and PKG2
archives, paginates large node lists, and emits typed values without embedding
large media payloads.

```sh
cargo run --package oozems-wz -- info data/Quest.wz
cargo run --package oozems-wz -- list data/Quest.wz /Act.img --limit 25
cargo run --package oozems-wz -- get data/Quest.wz /Act.img/1000/1/nextQuest
```

Edits always require a separate output path. The tool copies every unchanged
image blob byte-for-byte, rebuilds archive offsets and checksums, validates the
complete output with two independent WZ readers, and then atomically installs
it:

```sh
cargo run --package oozems-wz -- set \
  data/Quest.wz /Act.img/1000/1/nextQuest \
  --value 1002 \
  --output data/Quest.edited.wz
```

See [`crates/oozems-wz/README.md`](crates/oozems-wz/README.md) for path rules,
pagination, JSON fields, supported value types, and safety details.

## Infer quest script replacements

The workspace also includes `oozems-quest-harness`, a CLI that discovers
scripted quests and assembles model evidence directly from `Quest.wz`, `Npc.wz`,
and `String.wz`. It sends that evidence to an OpenRouter-compatible model and
validates the guessed `quest-scripts.toml` programs against the server's
supported schema. OpenRouter login uses a localhost PKCE callback and never
stores the resulting API key in the repository.

```sh
cargo run --package oozems-quest-harness -- login
cargo run --package oozems-quest-harness -- quests \
  data/Quest.wz --search q10272e
cargo run --package oozems-quest-harness -- generate \
  --model openai/gpt-5.2 \
  data/Quest.wz \
  q10272e
```

Pass `--all --output generated-quest-scripts.toml` instead of a quest selector
to generate every unique script referenced by the archive. A complete batch can
make hundreds of paid model requests.

See
[`crates/oozems-quest-harness/README.md`](crates/oozems-quest-harness/README.md)
for input rules, compatible endpoints, credential behavior, and limitations.

## Data flow

```text
browser
  -> GET /                         WASM shell
  -> POST /api/v1/bootstrap       saved player or creation options
  -> POST /api/v1/characters/...  create a character or get sprite metadata
  -> POST /api/v1/gui/get         current GUI layout and asset metadata
  -> POST /api/v1/maps/get        current map protobuf
  -> POST /api/v1/movement/rules server-configured movement constants and caps
  -> POST /api/v1/movement/submit movement correction, combat, and world snapshot
  -> POST /api/v1/movement/portal server-authorized portal transition
  -> POST /api/v1/items/...       equip, unequip, drop, or pick up an item
  -> POST /api/v1/cash-shop/...   list offers or buy an authoritative cash item
  -> POST /api/v1/npcs/interact   open or act on an authoritative NPC interaction
  -> POST /api/v1/combat/...      use a server-authoritative basic attack
  -> POST /api/v1/skills/...      allocate a skill point or use a skill
  -> POST /api/v1/players/recover apply one rate-limited natural recovery tick
  -> GET /wz-assets/...           requested WZ PNG and skill audio assets
  -> POST /api/v1/players/save    key bindings and authoritative session state

server
  -> config/xp-curves.toml        validated game progression rules
  -> config/gameplay.toml         validated world, item, skill, and movement rules
  -> config/content.toml          WZ content inclusion rules
  -> config/skill-formulas.toml   validated combat formulas
  -> data/interactions.toml       version-specific shop stock and taxi routes
  -> data/cash-shop.toml          global Cash Shop offers, prices, and lifetimes
  -> data/loot.toml               version-specific mob item drop rates
  -> data/quest-scripts.toml      version-specific replacements for WZ quest scripts
  -> data/Map.wz                  required, lazy WZ map source
  -> data/Npc.wz                  optional NPC placement animation source
  -> data/Quest.wz                enabled quest conditions, dialog, and rewards
  -> data/Mob.wz                  optional mob stats and animation source
  -> data/Character.wz            optional character sprite source
  -> data/UI.wz                   optional GUI sprite source
  -> data/Skill.wz                optional skill data, icons, and effects
  -> data/Sound.wz                optional skill sounds
  -> data/String.wz               optional map, NPC, and skill text
  -> SurrealDB -> SurrealKV       mutable player state
```

The API schema is in
`crates/oozems-proto/proto/oozems.proto`. Image and audio files keep their
native formats instead of being wrapped in protobuf. This lets the browser
stream, cache, and decode them directly. Asset URLs include a SHA-256-derived
version, so changing one file invalidates only that cached file.

## Use classic WZ maps

`Map.wz` is required. Place the matching `String.wz` beside it to use the
original map names. The server fails at startup when `Map.wz` is absent. It
detects the archive version, indexes map image entries at startup, and parses
each map only when it is requested.

The map response contains footholds and references to only the sprite assets
used by that map. It also contains typed ladder, rope, and portal data. Visible
portals use the animation frames under `MapHelper.img`. When an animated
placement enters the viewport, the client requests all its frames together and
keeps displaying a ready frame while the others load. Each sprite stays
compressed in `Map.wz` until the browser requests its opaque `/wz-assets/...`
URL. The server then decodes that sprite, returns a normal PNG, and caches it
for later requests. WZ files and extracted assets are not added to the client
bundle.

Place `Mob.wz` beside `Map.wz` to enable mobs. The server reads map-local mob
spawn points, snaps each initial position to its supporting foothold, and
creates the live instances. It loads each distinct mob definition once for the
requested map, including combat stats and all available animation metadata.
The browser requests only the animation frames that it renders. Mob state is
owned by the server and resets when the server restarts. Each map uses a
Shipyard ECS world with separate movement, combat, player-presence, and
projectile components. An ordered workload runs respawn, targeting, aggro,
movement, contact damage, and projectile systems. Mobs randomly idle or move
within the roaming range recorded by the map. They turn at unsafe edges.
A mob with a nonempty WZ `jump` animation can jump toward a nearby higher
foothold that its jump arc can reach. The existing movement heartbeat returns
authoritative mob and projectile snapshots, which the client interpolates
between updates. Mobs remain passive until attacked. Damage makes a mob target
and chase the attacking player. Mobs with WZ body attack data deal contact
damage, while mobs with positive magic attack launch projectiles after they are
provoked.

Place `Npc.wz` beside `Map.wz` to display the map's NPC life entries. The
server loads each referenced NPC's displayable named animations when the map is
first requested, places the NPC on its supporting foothold and WZ layer, and
includes their frame assets in the map response. The client renders `stand`, or
the first nonempty animation when `stand` is absent, while preserving the WZ
frame timing, origin, and facing direction. PNG data stays compressed until the
NPC first enters the viewport. A matching `String.wz` adds NPC names,
functions, and the ambient lines selected by `Npc.wz/info/speak`.

NPC inclusion is controlled by `config/content.toml`:

```toml
[npcs]
allowed_limited_names = []
# allowed_ids = [1012000, 1012003]
```

WZ `limitedname` data normally identifies seasonal or event NPCs. Omit
`allowed_limited_names` to permit every limited name. Set it to a list to permit
only those event scopes, or to an empty list to exclude all limited NPCs. Omit
`allowed_ids` to permit all remaining NPC IDs. Set it to a list to render only
those IDs, or to an empty list to render no NPCs. Both allowlists apply when
both settings are present. If `content.toml` is absent, NPC loading remains
unrestricted. Restart the server after changing these settings.

Double-click a nearby NPC to interact. The server resolves the map-local spawn
ID, checks the authoritative player position, and returns either WZ dialog, a
quest prompt, a shop, or a taxi list. The client uses
`UIWindow.img/UtilDlgEx` and `UIWindow.img/Shop`, so `UI.wz` is required for NPC
interaction windows. Movement, attacks, item pickup, and recovery pause while
one of these modal windows is open, but movement heartbeats continue.

When `Quest.wz` is present, every quest compatible with the implemented typed
mechanics is enabled automatically. Unsupported definitions are skipped and
counted in the startup log. An optional strict allowlist can narrow loading:

```toml
[quests]
allowed_ids = [1009]
```

When an allowlist is present, startup fails if one of its quests is absent or
uses data outside the supported subset. Rain's real `Quest.wz` quiz in Amherst
is one compatible quest. It exercises job and NPC conditions, accept and
decline dialog, list answers, persistent started/completed state, EXP rewards,
and retained next-quest metadata. `autoAccept`, `normalAutoStart`, and
`autoStart` accept a quest when its normal availability checks pass.
`autoComplete` completes a normally ready quest, while
`autoPreComplete` bypasses ordinary objectives but still preflights scripts and
actions. Automatic transitions repeat to a stable state so dependency chains
can advance in one locked request. WZ `ask = 1` list questions are supported in
both start and completion dialogue. Start questions require a human answer and
therefore take precedence over automatic-start metadata. They remain available
through their authoritative start NPC; a start question without an NPC is
rejected as unreachable.

`gameplay.toml` defines one authoritative nonnegative server world ID. The
bundled configuration uses world `0`:

```toml
[world]
id = 0
```

Quest `worldmin` and `worldmax` start checks use this value with inclusive
bounds. Quest fame checks read the authoritative character stats. Completion
mesos, calendar-window, and eligible completed-quest-count checks are evaluated
from current player state. Completed-quest counts exclude quest IDs
`9000..=10999`, QuestInfo area `51`, malformed statuses, and player records for
definitions that are not loaded.

Quest `timeLimit` and `timeLimit2` values are seconds. The server converts both
to checked milliseconds, expires an active quest at its
accepted time plus that duration, and resets it without rewards. An expired
quest cannot be reaccepted during the same automatic transition pass. Mesos,
cash points, and quest state are stored with the player. SurrealKV accepts one
current persisted player schema. Startup backfills only a missing `cash_points`
field as described below. It does not otherwise upgrade or fill missing fields
from older schemas. A record that does not satisfy the current schema fails to
load.

Quest record progress is stored as canonical, typed quest records. Record IDs
are nonzero and unique, entry indices are unique, and both levels are sorted
before persistence. Values are exact ASCII strings of at most 15 bytes, so
leading zeros and case remain significant. `Quest.wz` `infoNumber` redirects a
check to another quest record; direct `info` and `infoex` entries are OR
alternatives against index 0. Equality is exact. Numeric conditions accept only
strict decimal strings. A missing record never satisfies a check.

SurrealKV stores quest records as required nested records and entries. They must
be unique and valid. Missing, malformed, duplicate, or invalid data fails
player load; valid entries are sorted into canonical order.

Quest item-action `period` values are relative lifetimes in minutes. Their
deadline starts when that start, completion, or restoration action executes.
`dateExpire` is an absolute `yyyyMMddHH` civil deadline from the GMS archive.
The server interprets it in `America/Los_Angeles` with Jiff's bundled timezone
database, including PST and PDT transitions. Expiring equipment rewards remain
unsupported because every response path does not yet refresh composed character
sprites authoritatively.

`Quest.wz` contains quest script names but does not contain their executable
script bodies. A script-backed start or completion phase remains unresolved
until `data/quest-scripts.toml` defines a deterministic replacement with the
exact WZ name. If the file is absent, the catalog is empty. Missing replacements
are never treated as successful checks. A configured name must be referenced by
`Quest.wz`, and one exact name may intentionally be shared by multiple quest
definitions. Programs referenced only by quests that the server cannot load are
accepted but ignored.

Quest 10272 retains its archive completion script name, `q10272e`. Runtime
completion returns `ScriptRequired` until a deterministic replacement is added
for the deployed WZ version.

[`examples/v83/quest-scripts.toml`](examples/v83/quest-scripts.toml) is a
project-authored catalog for the matching GMS v83 content. It covers all 663
script names referenced by the 2,766 quest definitions supported by this
server. Fifty-three programs add typed behavior reconstructed from exact local
WZ evidence or behavior facts independently reconciled with the local WZ data.
The other 610 are explicit WZ-only fallbacks: they allow the ordinary WZ
checks, dialogue, and actions to run, but cannot reproduce unavailable or
unsupported script behavior. The catalog is not a copy of the original server
scripts and is not loaded automatically. Review it before using it as
`${OOZEMS_DATA_DIR}/quest-scripts.toml` or with a different archive version.

Each program has ANDed conditions, typed resource actions, and optional
dialogue pages:

```toml
[[scripts]]
name = "exact_wz_script_name"
result_pages = ["This is appended after the WZ result dialogue."]
incomplete_pages = ["This is shown when a condition is not met."]

[[scripts.conditions]]
type = "minimum_level"
level = 10

[[scripts.conditions]]
type = "job_ids"
ids = [100, 110]

[[scripts.conditions]]
type = "map_id"
map_id = 100000000

[[scripts.conditions]]
type = "mesos_at_least"
amount = 500

[[scripts.conditions]]
type = "item_quantity"
item_id = 4000000
quantity = 5

[[scripts.conditions]]
type = "quest_state"
quest_id = 1000
state = "completed"

[[scripts.conditions]]
type = "quest_record_equals"
quest_id = 1000
index = 0
value = "007"

[[scripts.conditions]]
type = "quest_record_at_least"
quest_id = 9000
index = 4
value = "10"

[[scripts.actions]]
type = "item_delta"
item_id = 4000000
delta = -5

[[scripts.actions]]
type = "mesos"
delta = 1000

[[scripts.actions]]
type = "experience"
amount = 100

[[scripts.actions]]
type = "fame"
delta = 1

[[scripts.actions]]
type = "set_record"
quest_id = 1000
index = 0
value = "started"

[[scripts.actions]]
type = "set_quest_status"
quest_id = 1001
state = "completed"
```

The complete condition capability list is `minimum_level`, `maximum_level`,
`job_ids`, `map_id`, `mesos_at_least`, `mesos_at_most`, `item_quantity`, and
`quest_state`, plus `quest_record_equals`, `quest_record_at_least`, and
`quest_record_at_most`. Quest states are `not_started`, `started`, and
`completed`. Record conditions can read any canonical helper or redirected
record. The
complete action capability list is signed `item_delta`, signed `mesos`,
unsigned `experience`, signed `fame`, `set_record`, and `set_quest_status`.
Script actions are merged with the WZ actions for the server-selected start or
completion phase, then the combined set uses the same atomic in-memory
inventory, mesos, EXP, fame, record, and cross-quest status transform as an
ordinary quest. A cross-quest status action changes only the target's stored
status and timestamps. It does not run the target quest's checks, scripts,
dialogue, actions, or rewards. A script cannot target the quest whose phase is
currently transitioning. `not_started` removes the target quest entry and its
own quest record. `started` replaces stale mob progress and timestamps with a
clean acceptance at the action time. `completed` clears mob progress, completes
at the action time, and preserves a valid existing acceptance time.

The file uses strict tagged records. Unknown fields and capability names,
duplicate or empty script names, names absent from `Quest.wz`, and shape limits
fail startup for every program. Programs referenced by loaded quest definitions
also reject unknown item IDs, zero item quantities or action amounts,
contradictory limits, and numeric combinations that cannot be represented by
the quest action model. Their record IDs must be nonzero, values must meet the
persisted ASCII limit, numeric record predicates must be strictly decimal, and
duplicate or incompatible record operations fail startup. Quest status targets
must be nonzero loaded quest definitions and cannot be duplicated in one merged
action plan. A catalog may contain at most 1,024 programs. One program may
contain at most 64 conditions, actions, and pages in total, with at most 16
result pages and 16 incomplete pages. Each page is limited to 4096 UTF-8 bytes,
and each script name to 256 bytes.

Quest scripts have no filesystem, network, clock, random, loop, callback,
generic NPC script, portal script, mob-kill integration, or dynamic branching
capability. Restart the server after changing `quest-scripts.toml`.

The local archives contain NPC script names but not their executable bodies.
Shop stock, buy prices, taxi destinations, and fares therefore come from
`data/interactions.toml`. The local definitions add Sam's armor shop in Henesys
Weapon Store and the Henesys Regular Cab route to Lith Harbor. Buy prices and
fares are project-authored. Sell prices come from each supported equipment
item's WZ `info/price`; an absent or zero price makes an item unsellable. Buying
and selling one item, paying a taxi fare, changing maps, and claiming a quest
reward are all validated and persisted by the server.

A shop can charge the character's cash-point balance instead of mesos:

```toml
[[shops]]
map_id       = 100000101
npc_spawn_id = 1
currency     = "cash_points"

[[shops.offers]]
item_id   = 5000001
buy_price = 250
```

Replace the map, NPC spawn, and item IDs with entries from the local WZ
archives. A shop without `currency` remains a meso shop. Cash-point shops are
buy-only and do not compare their prices with WZ meso sale prices. Each purchase
grants one permanent item. Packages, gifting, timed purchases, and cash-shop
storage are not part of this pseudo shop. An item with a WZ sale price can still
be sold for mesos at a normal shop.

The status bar's original Cash Shop button opens a separate fixed 800 by 600
screen from `UI.wz/CashShop.img`. It is global and does not require an NPC or a
specific map. Offers come from `data/cash-shop.toml`:

```toml
currency_name = "Ooze"

[[offers]]
offer_id = 1
item_id = 5010000
price = 1200
duration = "30d"

[[offers]]
offer_id = 2
item_id = 5010010
price = 1500
duration = "permanent"
```

The optional `currency_name` controls the premium-currency label in both the
global Cash Shop and cash-point NPC shops. It defaults to `"Ooze"` when omitted
and may contain at most 24 characters. The catalogue may contain at most 10
offers. Each `offer_id` must be unique and positive, each price must be positive,
and every item must exist in the local WZ item catalogue. A duration is either
the exact value `"permanent"` or a positive humantime value such as `"7d"` or
`"12h"`. The browser submits only the stable offer ID. The server resolves its
item, price, and lifetime, computes the expiration deadline at purchase time,
and persists the item and remaining Cash Points together. An absent
`cash-shop.toml` creates an empty Cash Shop that uses the `"Ooze"` label.

The current screen is intentionally limited to listing and buying one item at a
time. It does not implement packages, gifting, wishlists, search, try-on, or
cash storage.

Place `Character.wz` beside the map archives to enable character creation. The
server indexes the available skin, face, and hair styles, then composes idle,
walk, jump, ladder, and rope frames from each sprite's WZ anchor points and z
layer. The browser receives only frame metadata at first. It requests the
individual PNG layers while the preview or game renderer needs them. The
chosen name and appearance are stored with the player in SurrealKV.

Place `UI.wz` beside the other archives to use its classic `StatusBar.img`,
`UIWindow.img`, and `CashShop.img` sprites for the in-game UI. The server sends
the layouts through protobuf. The browser then requests backgrounds, gauges,
quick-slot panels, buttons, and open windows as normal versioned PNG assets. If
`UI.wz` is absent, the client keeps using its built-in fallback HUD.

GUI sprite metadata retains the WZ dimensions and origins. Dynamic components
are sent as named sprite templates, while named regions record destinations
that were supplied by the original client rather than stored in the archive.
The Skill window uses its native 141 by 35 row component to size and render the
visible skill rows instead of duplicating that geometry in the browser.

Click the KeySet status-bar button, or press K with the default bindings, to
open the original `UIWindow.img/KeyConfig` keyboard settings window. Drag an
action icon from the lower palette, or from an assigned key, onto another key.
Each action has one assignment, so moving an action removes its previous
assignment and replaces any action already on the target key. The supported
palette contains Basic Attack, Jump, Pick Up, Character, Equipment, Inventory,
Key Settings, and Skills. Basic Attack is assigned to left Control for new
characters. Changes are stored with the player in SurrealKV.

Place `Skill.wz` and its matching `String.wz` beside the other archives to use
the original skill books. New characters receive the configured initial skill
points. Open the Skills window and click the WZ plus button beside a skill to
spend one point. Click a learned skill icon to use it directly. To bind a
learned skill, leave the Skills window open, open Key Settings, and drag the
skill icon onto a key. A skill can have one key assignment, like each built-in
action.

Quest Act skill rewards raise learned and master levels independently without
spending skill points or lowering either value. Their authored job IDs are exact;
beginner-family skill IDs retain the original cross-job bypass. `Skill.wz` is the
authoritative global index, including invisible definitions. An invisible real
skill enters the player's skill book after a positive learned or master-level
unlock, and its positive master level limits later point allocation. An
invisible definition with maximum level zero is different: an authored level-1
record may persist as an acquisition marker for quest checks, but the marker
remains hidden, non-allocatable, non-bindable, and non-usable.

A basic attack selects the nearest living mob in front of the character and
uses the configured bare-hands damage profile. Its result follows the same
server-owned mob HP, defense, aggro, and death pipeline as a damaging skill.
The character plays the composed `swingO1` WZ animation for its configured
frame duration when the attack begins.

Each living-to-dead mob transition rolls the independent item entries in
`data/loot.toml`. The local WZ archives provide some mob-to-item associations
but not ordinary drop probabilities, so the local rates are project-authored.
A rate is expressed per million; `1000000` is guaranteed. Generated items are
temporary, belong to the final attacker, and use the existing server-authorized
pickup and inventory pipeline. Combat and movement responses synchronize the
current map drops so other clients see item creation, pickup, and expiry.

Skill use is server-owned. The server confirms the learned level, reads that
level's WZ properties, checks and spends HP and MP, enforces WZ cooldowns,
applies immediate HP recovery, and returns temporary speed and jump effects to
the client. A damaging skill targets the nearest living mob in front of the
character. The server verifies the target map, foothold layer, facing direction,
horizontal reach, and vertical reach before choosing damage from the calculated
range. It owns mob HP, death, aggro, and respawn state. The client displays the
resulting damage, mob HP bar, attack animation, and projectile state.

Temporary effects from different skill and item sources coexist. Reapplying the
same source replaces its previous holder. Each numeric combat or movement
modifier uses the highest nonzero value among the active holders; values are not
added together. A new morph replaces any active morph from another source.

When `Sound.wz` is present, a successful use also returns the matching
`Skill.img/<skill ID>/Use` sound. The server reads caster `effect`, projectile
`ball`, and target `hit` animation frames from the active skill level in
`Skill.wz`. Only their versioned descriptors are included in the use response.
The browser requests the PNG and MP3 or WAV data on first use, then relies on
its normal cache for later uses. Projectile effects travel in the character's
facing direction, followed by their target effect.

The HP, MP, and EXP gauges use the persisted character values for their fill
levels and display bracketed current and maximum values over the WZ artwork.

New characters start on the map selected by `characters.initial_map_id`. The
bundled configuration selects `Mushroom Town` (map `10000`).

Click the stat button in the status bar to open the `UIWindow.img` character
stat window. Its background, close control, and job label remain unloaded until
the window is first opened. New characters receive server-owned Beginner stats,
which are required in their persisted player records.

Click the equipment or inventory button to open its `UIWindow.img` window.
The inventory uses the native Equip, Use, Setup, Etc, and Cash tabs; pet items
appear under Cash. Left-click an item on the Equip tab to equip it. Left-click
an equipped item to move it back to inventory. Right-click an item on any
inventory tab to drop it at the server-owned player position. Equipment and
inventory changes are persisted in SurrealKV. The browser requests each
equipment icon from `Character.wz` only
when the icon is first visible. Equipping or removing an item also refreshes
the composed character layers. An empty top or bottom slot uses the
gender-specific pajama layers from `Character.wz` instead of leaving the body
unclothed.

Inventory stack deadlines are persisted with the stack. Zero means permanent.
Only matching item IDs and deadlines merge, and item consumption uses the
earliest deadline before permanent stacks. The server removes expired stacks at
the locked player-load boundary and saves a new player revision only when that
pruning changes the inventory.

Dropped items are transient and scoped to their map. Their item ID, position,
normal despawn deadline, and preserved item deadline are sent in the map
protobuf. A drop expires at the earlier active deadline. Expired drops are
removed from the server drop store and stop rendering in the client. The Pick
Up action moves the nearest drop within pickup range into the character's
inventory. The server removes the drop and saves the inventory as one item
action, restoring the drop if the player save fails.

## Configure gameplay rules

Gameplay rules are configured in `config/gameplay.toml`:

```toml
# See README.md for configuration reference.

[items]
drop_despawn = "10m"

[skills]
initial_points = 3

[characters]
initial_map_id      = 10000
initial_cash_points = 10000

[world]
id = 0

[combat]
disengage_range = 520.0
player_attack_range = 220.0
attack_vertical_reach = 90.0
player_attack_interval = "600ms"
touch_horizontal_reach = 28.0
touch_vertical_reach = 48.0
projectile_range = 420.0
projectile_speed = 240.0
projectile_hit_reach = 18.0
mob_attack_interval = "1500ms"
player_invulnerability = "1s"
default_respawn = "7s"

[movement]
walk_speed = 220.0
climb_speed = 135.0
gravity = 1150.0
jump_speed = 480.0
speed_cap = 200
jump_cap = 200
snapshot_interval = "200ms"
maximum_snapshot_gap = "1s"
persistence_interval = "2s"
position_tolerance = 24.0
ground_tolerance = 8.0
platform_edge_tolerance = 20.0
ladder_reach = 32.0
ladder_end_reach = 20.0
portal_horizontal_reach = 48.0
portal_vertical_reach = 64.0
```

Every section and field shown above is required. Missing or unknown sections and
fields stop server startup instead of loading defaults from an older layout.

`items.drop_despawn` controls how long a dropped item remains in a map. It must
be a positive human-readable duration, such as `30s`, `10m`, `2h`, or
`1h 30m`. Restart the server after changing it. Drops are intentionally not
persisted across a server restart.

`skills.initial_points` is the number of unspent skill points assigned to a new
character. Learned levels and later point changes are persisted and are not
replaced when this setting changes.

`characters.initial_map_id` selects the WZ map used for newly created
characters. The server verifies the map during startup and places each new
character at its first spawn portal. Changing it does not move existing
characters.

`characters.initial_cash_points` sets the cash-point balance for new
characters. It must fit the persisted signed 64-bit range. When this field is
first introduced to an existing database, startup also assigns its value to
players that do not yet have a `cash_points` field. Later setting changes do not
replace existing balances.

Combat distances are measured in map pixels. Mobs acquire an aggro target when
that player damages them. `disengage_range` controls how far a mob can remain
interested in that target. `player_attack_range` and `attack_vertical_reach` are
the server-authoritative basic attack and skill target envelope.
`player_attack_interval` limits how often each player can use Basic Attack. The
two touch reach values form the mob contact box.

`projectile_range` controls when a magic-attacking mob can launch a projectile.
`projectile_speed` is measured in map pixels per second, and
`projectile_hit_reach` is its collision radius. `mob_attack_interval` controls
how often a mob can launch one. `player_invulnerability` prevents overlapping
contact and projectile hits from applying on every movement heartbeat.
`default_respawn` applies when a WZ spawn point does not define `mobTime`. All
combat numbers must be finite and positive. Combat durations accept the same
human-readable syntax as item despawn durations.

Movement speeds are measured in map pixels per second, and gravity is measured
in map pixels per second squared. `speed_cap` and `jump_cap` are percentage
stats with 100 as the unmodified value. The default cap of 200 therefore allows
at most twice the configured base speed or jump impulse. Timed WZ skill values,
including Haste-style `speed` and `jump` bonuses, use the highest nonzero active
value before these caps are applied.

The client submits an ordered movement snapshot every `snapshot_interval`.
The server uses its own receipt time to calculate a reachable horizontal and
vertical envelope. `maximum_snapshot_gap` limits the time included in that
calculation, so an absent client cannot accumulate an unlimited movement
budget. `persistence_interval` controls partial SurrealKV position writes;
these writes cannot overwrite character stats, skills, or inventory.

If the character lands, grabs a ladder, or drops through a platform between
heartbeats, the next snapshot includes that brief support contact without
replacing the current position. The server verifies the foothold, ladder, and
full path through the contact before it resets airborne time or accepts a
drop-through transition.

`position_tolerance` provides latency and floating-point tolerance around the
physical envelope. `ground_tolerance` controls how close a grounded snapshot
must be vertically to a parsed foothold. `platform_edge_tolerance` preserves
ground contact while the character moves just beyond a foothold end.
`ladder_reach` controls horizontal ladder and rope snapping, while
`ladder_end_reach` controls attachment just above or below an endpoint. The two
portal reach settings control how close the authoritative position must be
before the server permits a transition. All numeric movement values and
durations must be positive, and `maximum_snapshot_gap` must not be shorter than
`snapshot_interval`. Restart the server after changing these rules.

Rejected snapshots return the last authoritative server position and the WASM
client resynchronizes to it. Purely visual movement inside a modified client is
not observable, but it cannot change the server position used for recovery,
pickups, drops, portals, persistence, or later gameplay calculations.

## Configure formula profiles

Combat formulas are configured in `config/skill-formulas.toml`. The bundled
file records its source and groups formulas into reusable profiles. Each
selector table maps a stable game identifier to one profile:

```toml
# See README.md for configuration reference.

source_url = "https://ayumilovemaple.wordpress.com/2009/09/06/maplestory-formula-compilation/"

[weapon_profiles.one_handed_sword]
minimum = "(PrimaryStat * 0.9 * Mastery + SecondaryStat) * WeaponAttack / 100"
maximum = "(PrimaryStat + SecondaryStat) * WeaponAttack / 100"
primary_stat = "Strength"
secondary_stat = "Dexterity"
primary_modifier = 4.0
swing_modifier = 4.4
stab_modifier = 3.2

[weapons.one_handed_sword]
profile = "one_handed_sword"
```

The profile contains the formulas directly. There is no separate flat formula
catalog and no naming convention that implicitly connects formulas to game
content. The selector contains only the chosen profile name, so multiple game
identifiers can share a profile.

Skills use the same shape. For example, this intentionally makes Double Stab
use the Lucky Seven formulas:

```toml
[skill_profiles.lucky_seven]
minimum = "Luck * 2.5 * WeaponAttack / 100"
maximum = "Luck * 5.0 * WeaponAttack / 100"

[skills."1111111"]
profile = "lucky_seven"
```

Quoted skill keys must be canonical decimal `u32` values without leading zeroes.
A skill mapping takes priority over the automatic Pirate bare-hands profile.
If a skill has no mapping, the server keeps the automatic Pirate behavior and
does not guess a profile from the skill's display name.

Summons also have independent profiles. A profile can expose properties other
than damage, such as durability:

```toml
[summon_profiles.battleship]
durability = "(BattleshipLevel * 2 + (CharacterLevel - 120)) * 200"

[summon."22222222"]
profile = "battleship"
```

Use the selector only when that skill ID exists in the loaded WZ version. If the
archives do not contain Battleship, the default file keeps this profile
available without selecting it.

The supported profile and selector table pairs are:

| Profile table         | Selector table | Selector key                                                    |
| --------------------- | -------------- | --------------------------------------------------------------- |
| `weapon_profiles`     | `weapons`      | Lowercase identifier such as `one_handed_sword` or `bare_hands` |
| `skill_profiles`      | `skills`       | Quoted numeric WZ skill ID                                      |
| `summon_profiles`     | `summon`       | Quoted numeric WZ skill ID                                      |
| `defense_profiles`    | `defenses`     | Lowercase identifier                                            |
| `accuracy_profiles`   | `accuracy`     | Lowercase identifier                                            |
| `experience_profiles` | `experience`   | Lowercase identifier                                            |
| `stat_profiles`       | `stats`        | Lowercase identifier                                            |
| `recovery_profiles`   | `recovery`     | Lowercase identifier                                            |

Profile names, property names, and identifier selector keys must contain only
lowercase ASCII letters, digits, and underscores. Every profile must contain at
least one property formula. Every selector must name a profile from its paired
profile table. `weapons.bare_hands` is required, and its selected profile must
define `attack`, `minimum`, and `maximum`.

A property can be an expression string or a numeric TOML constant. Numeric
constants are useful for properties such as `primary_modifier`,
`swing_modifier`, and `stab_modifier`; they use the same evaluation path as
expressions.

Basic attacks read `attack`, `minimum`, and `maximum` from the profile selected
by `weapons.bare_hands`. Non-Pirate jobs use a standard `JobMultiplier` of 4.0;
Pirate jobs retain their existing job-specific multipliers. The skill damage
pipeline reads `minimum` and `maximum` from a selected skill profile, applies
the skill level's WZ `damage` percentage, and truncates the final values. When
either attack reaches a mob, non-fixed damage passes through
`defenses.physical` using the mob's WZ physical defense and the player and mob
levels. WZ fixed damage bypasses defense. The current equipment model does not
include weapons, so `WeaponAttack` is read from the `attack` property of the
profile selected by `weapons.bare_hands`. This provides one clear input point
for real weapon stats when weapon equipment is added later. Other profile
categories are parsed, validated, and routed now so their combat pipelines can
consume the same configuration model later.

Natural recovery uses the same profile pipeline:

```toml
[recovery_profiles.base]
hp = 10
mp = 3

[recovery_profiles.mage]
hp = 10
mp = "CharacterLevel * SkillLevel / 10"

[recovery.base]
profile = "base"

[recovery.mage]
profile = "mage"
```

The client polls for recovery only while the character appears idle. The poll
does not claim that the idle interval has elapsed. The server owns the recovery
deadline, initializes it when the character is loaded, and restarts it after
accepted movement, jumping, climbing, portal entry, item actions, or skill use.
An early poll restores nothing and tells the client how long to wait. An
eligible poll restores and persists one tick, then starts the next ten-second
interval. Jobs from 200 through 299 select `recovery.mage`; other jobs select
`recovery.base`. A missing mage selection falls back to base. `CharacterLevel`
is the current character level, and `SkillLevel` is learned skill `2000000`.
Recovery results are truncated to whole points and capped by maximum HP and MP.
Every selected recovery profile must define both `hp` and `mp`.

Profile formulas use decimal arithmetic and support these elements:

| Syntax             | Meaning                                          |
| ------------------ | ------------------------------------------------ |
| `^`                | Exponentiation. It is right-associative.         |
| `*`, `/`           | Multiplication and division.                     |
| `+`, `-`           | Addition and subtraction, including unary signs. |
| `( ... )`          | Explicit grouping.                               |
| `floor(value)`     | Round down to an integer value.                  |
| `trunc(value)`     | Discard the fractional part.                     |
| `min(left, right)` | Select the smaller value.                        |
| `max(left, right)` | Select the larger value.                         |

Identifiers are case-sensitive. The accepted variables are grouped below.
Each calculation supplies only the variables relevant to that formula. Using a
variable that is not supplied produces an explicit formula evaluation error.

| Group                   | Variables                                                                                                                                           |
| ----------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------- |
| Player                  | `CharacterLevel`, `PlayerLevel`, `Strength`, `Dexterity`, `Intelligence`, `Luck`, `Accuracy`, `Avoidability`, `Magic`                               |
| Attack                  | `PrimaryStat`, `SecondaryStat`, `WeaponAttack`, `BasicAttack`, `AttackRate`, `Mastery`, `SkillDamage`, `SkillLevel`, `SpellAttack`, `JobMultiplier` |
| Target                  | `MonsterLevel`, `MonsterHealth`, `MonsterExperience`, `WeaponDefense`, `MagicDefense`, `DamageBeforeDefense`, `TargetCount`, `TargetMultiplier`     |
| Multi-hit and modifiers | `HitNumber`, `TotalHits`, `Orbs`, `ComboLevel`, `AdvancedComboDamage`, `ChargeLevel`, `AmpBulletDamage`                                             |
| Accuracy and recovery   | `AccuracyRatio`, `HealLevel`, `BattleshipLevel`                                                                                                     |
| Economy and parties     | `Mesos`, `DamageDealt`, `TotalPartyLevel`, `PartyExperiencePortion`, `PartyBonus`                                                                   |

The current basic attack, skill, and recovery profile pipelines supply
`CharacterLevel`, `PlayerLevel`, `Strength`, `Dexterity`, `Intelligence`,
`Luck`, and `WeaponAttack` as needed. Skills also supply `SkillDamage` and
`SkillLevel`. Basic attacks supply `JobMultiplier` for every job, while skills
supply it for Pirate jobs. The other accepted variables belong to formula
pipelines that will be connected as their combat inputs are implemented.
Selecting a profile that needs an unavailable variable returns an explicit
request error instead of substituting a value.

The server parses every configured formula and rejects unknown identifiers,
unknown functions, invalid syntax, non-ASCII text, invalid numeric IDs, invalid
profile or property names, empty profiles, and selectors that name unknown
profiles before it starts serving players. Evaluation also rejects missing
properties, missing inputs, division by zero, and non-finite results. Restart
the server after changing the file.

The defaults group formulas from the linked 2009 Ayumilove compilation by the
game concepts that consume them while preserving its constants and caps. This
is a historical community source, so a server owner can replace any profile or
expression when targeting a different version or interpretation.

## Configure XP curves

XP curves are configured in `config/xp-curves.toml`. Every configuration file
must start with a comment directing readers to this reference:

```toml
# See README.md for configuration reference.
```

The bundled configuration has this shape:

```toml
# See README.md for configuration reference.

default_curve = "default"

[[curves]]
name = "default"

[[curves.ranges]]
start = 1
end = 10
formula = "15 * Level ^ 2"

[[curves.ranges]]
start = 11
end = 200
formula = "atLevel(10) + (Level - 10) * 500"
```

`default_curve` selects the curve applied to all current characters. Additional
named curves may be defined for future game modes. The server parses and
validates every curve at startup, including curves that are not selected.

Each `curves.ranges` entry defines an inclusive level range. Ranges within one
curve must start at level 1, remain contiguous, and must not overlap. The
highest supported configured level is 10,000. A character level outside the
selected curve is treated as a server configuration error.

Formulas contain integer literals and these elements:

| Syntax        | Meaning                                                    |
| ------------- | ---------------------------------------------------------- |
| `Level`       | The level currently being evaluated. It is case-sensitive. |
| `atLevel(10)` | The XP requirement produced by this curve for level 10.    |
| `^`           | Exponentiation. It is right-associative.                   |
| `*`, `/`      | Multiplication and division.                               |
| `+`, `-`      | Addition and subtraction, including unary signs.           |
| `( ... )`     | Explicit grouping.                                         |

Exponentiation is evaluated before unary signs, then multiplication and
division, then addition and subtraction. Division truncates toward zero.
Exponents must be non-negative 32-bit integers. Every final level result must
be a positive 64-bit integer.

`atLevel(...)` accepts one positive integer level and refers to another result
in the same curve. References may point forward or backward and may cross
range boundaries. The server resolves all references at startup. A missing
level, arithmetic error, overflow, or direct or indirect reference cycle stops
startup with a configuration error. Restart the server after changing a curve.
Changing the selected curve does not recalculate character levels or discard
accumulated XP. It replaces only the requirement for advancing from the
character's current level.

Use the left and right arrow keys to walk. Use the up and down arrow keys to
climb. Hold Down and press the configured Jump key to drop through a platform
when another foothold is below the character. Press Up while standing at a
direct portal to enter it. Arrow keys stay reserved for movement and
interaction. The default action bindings are left Alt for Jump, Z for Pick Up, C
for Character, E for Equipment, I for Inventory, K for Key Settings, and S for
Skills. Script portals remain inactive because their behavior belongs to a
future server-side scripting system. Double-click an NPC to open its dialog.

## Verify it

```sh
make check
```

The first server build is relatively large because embedded SurrealDB includes
its database engine in the server binary.
