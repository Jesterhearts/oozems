# Oozems

Oozems is an original old-school side-scrolling RPG foundation for personal
use. It does not include MapleStory code or assets.

> **Status: not ready for general use.** Combat is limited to basic player
> attacks, player skills, mob contact attacks, and basic mob projectiles. Player
> death uses the native animation and revives through timed recovery. Quest
> support covers a typed subset of `Quest.wz`.

A future version 0.1 release tag will mark the first version intended for
general use. That release will probably still need polish and bug fixes.

## Contents

- [Quick start](#quick-start)
- [Run and play](#run-and-play)
- [Choose WZ content](#choose-wz-content)
- [Configure content and interactions](#configure-content-and-interactions)
- [Configure gameplay rules](#configure-gameplay-rules)
- [Configure formula profiles](#configure-formula-profiles)
- [Configure XP curves](#configure-xp-curves)
- [Use the workspace tools](#use-the-workspace-tools)
- [Dump GUI rendering](#dump-gui-rendering)
- [Understand the architecture](#understand-the-architecture)
- [Verify changes](#verify-changes)

## Quick start

### Install the tools

Install these tools before you build Oozems:

- Rust
- The Rust `wasm32-unknown-unknown` target
- Trunk

### Add the game data

Place `Map.wz` in `./data`. The server requires this archive and stops at
startup if it is missing.

If you use local definitions in `data/interactions.toml`, add matching `Npc.wz`
and `Character.wz` archives beside `Map.wz`. See
[Configure shops and taxi routes](#configure-shops-and-taxi-routes). `Quest.wz`
is optional. Add `UI.wz` if you want the configured native interaction windows
and Cash Shop screen.

> **Existing player data:** Oozems stores players in the normalized SQLite
> database `data/oozems.sqlite3`. This is a clean storage change: the server
> neither imports nor modifies an existing `data/surrealkv` directory. Remove or
> move an incompatible SQLite database before starting this version. Invalid
> normalized player rows fail to load instead of being repaired silently.

See [Choose WZ content](#choose-wz-content) for the behavior that each optional
archive enables.

### Start the server

From the workspace root, run:

```sh
make run
```

Then open <http://127.0.0.1:3000>.

The Make target builds the WASM client in the server's generated `public`
directory before it starts the server. SQLite is compiled into the server
binary, so no separate database service is required.

## Run and play

### Use the controls

| Action | Default control |
| --- | --- |
| Walk | Left Arrow or Right Arrow |
| Climb a ladder or rope | Up Arrow or Down Arrow |
| Enter a direct portal | Up Arrow while standing at the portal |
| Drop through a platform | When another foothold is below you, hold Down Arrow and press the configured Jump key |
| Basic Attack | Left Control |
| Jump | Left Alt |
| Pick Up | Z |
| Open Character | C |
| Open Equipment | E |
| Open Inventory | I |
| Open Key Settings | K |
| Open Skills | S |
| Interact with an NPC | Double-click a nearby NPC |

Arrow keys remain reserved for movement and interaction. Script portals remain
inactive because their behavior requires a future server-side scripting system.

### Change key bindings

Click the KeySet status-bar button, or press K with the default bindings, to
open the original `UIWindow.img/KeyConfig` keyboard settings window. Drag an
action icon from the lower palette, or from an assigned key, onto another key.

Each action can have one assignment. Moving an action removes its previous
assignment and replaces any action on the target key. The palette contains
Basic Attack, Jump, Pick Up, Character, Equipment, Inventory, Key Settings, and
Skills. New characters have Basic Attack assigned to left Control. Oozems stores
changes with the player in SQLite.

### Change runtime paths

The default data directory is `./data`. Git ignores this directory. It contains
the local WZ archives, `oozems.sqlite3`, and these version-specific files:

- `cash-shop.toml`
- `interactions.toml`
- `loot.toml`
- `quest-scripts.toml`
- `skill-semantics.toml`

Set these environment variables to override the default runtime locations:

| Variable | Default |
| --- | --- |
| `OOZEMS_BIND` | `127.0.0.1:3000` |
| `OOZEMS_DATA_DIR` | `./data` |
| `OOZEMS_CONFIG_DIR` | `./config` |
| `OOZEMS_GUI_LAYOUT_DIR` | `${OOZEMS_CONFIG_DIR}/gui` |
| `OOZEMS_PUBLIC_DIR` | `crates/oozems-server/public` |
| `OOZEMS_WZ_DIR` | `./data` |

## Choose WZ content

`Map.wz` is the only archive required for startup. Each archive enables this
content:

| Archive | Purpose |
| --- | --- |
| `Map.wz` | Required map, foothold, ladder, rope, portal, and sprite source |
| `String.wz` | Map, NPC, and skill text |
| `Mob.wz` | Mob stats, combat data, and animations |
| `Npc.wz` | NPC placement animations and ambient speech references |
| `Quest.wz` | Supported quest conditions, dialogue, and rewards |
| `Character.wz` | Character creation choices, composed sprites, and equipment icons |
| `UI.wz` | Classic HUD, windows, controls, and Cash Shop screen |
| `Skill.wz` | Skill books, properties, icons, and effects |
| `Sound.wz` | Map BGM, gameplay sound effects, and skill sounds |

### Maps and assets

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

### Mobs and combat

Place `Mob.wz` beside `Map.wz` to enable mobs. The server reads map-local spawn
points, snaps each initial position to its supporting foothold, and creates the
live instances. It loads each distinct mob definition once for the requested
map, including combat stats and all available animation metadata. The browser
requests only the animation frames that it renders.

The server owns mob state, which resets when the server restarts. It distributes
map worlds across a fixed set of owner threads based on the available
parallelism. Commands for one map remain ordered on one owner. Independent maps
on different owners can simulate concurrently.

Each map uses a Shipyard ECS world with separate movement, combat,
player-presence, and projectile components. An ordered workload runs respawn,
targeting, aggro, movement, contact damage, and projectile systems.

Mobs randomly idle or move within the roaming range recorded by the map. They
turn at unsafe edges. A mob with a nonempty WZ `jump` animation can jump toward
a nearby higher foothold if its jump arc can reach it.

The movement heartbeat returns authoritative mob and projectile snapshots. The
client interpolates between these updates. Mobs remain passive until attacked.
Damage makes a mob target and chase the attacking player. Mobs with WZ body
attack data deal contact damage. Mobs with positive magic attack launch
projectiles after they are provoked.

A basic attack selects the nearest living mob in front of the character and
uses the configured bare-hands damage profile. It follows the same server-owned
mob HP, defense, aggro, and death pipeline as a damaging skill. The character
plays the composed `swingO1` WZ animation for its configured frame duration when
the attack begins.

Each living-to-dead mob transition rolls the independent item entries in
`data/loot.toml`. See [Configure loot](#configure-loot) for the rate format.

Generated items are temporary and belong to the final attacker. They use the
server-authorized pickup and inventory pipeline. Combat and movement responses
synchronize the current map drops so that other clients see item creation,
pickup, and expiry.

### NPCs

Place `Npc.wz` beside `Map.wz` to display the map's NPC life entries. When a map
is first requested, the server loads each referenced NPC's displayable named
animations. It places the NPC on its supporting foothold and WZ layer, then
includes the frame assets in the map response.

The client renders `stand`, or the first nonempty animation if `stand` is
absent. It preserves the WZ frame timing, origin, and facing direction. PNG data
stays compressed until the NPC first enters the viewport. A matching `String.wz`
adds NPC names, functions, and the ambient lines selected by
`Npc.wz/info/speak`.

Double-click a nearby NPC to interact. The server resolves the map-local spawn
ID and checks the authoritative player position. It returns WZ dialogue, a quest
prompt, a shop, or a taxi list.

The client uses `UIWindow.img/UtilDlgEx` and `UIWindow.img/Shop`. You need
`UI.wz` to display these NPC interaction windows. Movement, attacks, item
pickup, and recovery pause while a modal window is open. Movement heartbeats
continue.

### Character appearance

Place `Character.wz` beside the map archives to enable character creation. The
server indexes the available skin, face, and hair styles. It then composes idle,
walk, jump, ladder, and rope frames from each sprite's WZ anchor points and z
layer.

The browser initially receives only frame metadata. It requests individual PNG
layers when the preview or game renderer needs them. Oozems stores the chosen
name and appearance with the player in SQLite.

### User interface

Place `UI.wz` beside the other archives to use its classic `StatusBar.img`,
`UIWindow.img`, and `CashShop.img` sprites. The server sends the layouts through
protobuf. The browser then requests backgrounds, gauges, quick-slot panels,
buttons, and open windows as normal versioned PNG assets. Without `UI.wz`, the
client uses its built-in fallback HUD.

GUI sprite metadata retains the WZ dimensions and origins. Dynamic components
are sent as named sprite templates. Named regions record destinations that the
original client supplied instead of storing in the archive. The Skill window
uses its native 141 by 35 row component to size and render visible skill rows.
The browser does not duplicate that geometry.

The server loads authored window definitions from `config/gui/*.textproto` when
`UI.wz` is present. These files contain stable paths inside `UI.wz`, named
regions, fixed sprite positions, and window positions. The server resolves each
WZ path to its current dimensions and content-addressed asset ID at startup. A
malformed definition or a region outside its background stops startup instead
of silently changing hit testing.

Edit these files with the local desktop application from the workspace root:

```sh
make ui-editor
```

The editor reads `data/UI.wz` and `config/gui` by default. It opens all supported
windows and renders their real WZ artwork. Existing textproto files are loaded
as authored. Missing files appear as `(new)` layouts synthesized from the active
archive and are not written until you save them. You can drag sprites, move or
resize named regions, and edit exact coordinates and canvas dimensions. In the
representative Skill window, drag any skill-point arrow or edit its signed
offsets to move every arrow state and its click target. Use `Ctrl+S` to save.
The editor does not expose routes or write access through the game server.

Use explicit paths for another archive or layout set:

```sh
cargo run --package oozems-ui-editor -- \
  --wz /srv/maplestory/UI.wz \
  --layouts /srv/oozems/gui
```

The bundled `config/gui/skills.textproto` is the initial authored window. Saving
one of the generated layouts creates the corresponding textproto file. Until a
window has a saved definition, the server retains its built-in composition.

The HP, MP, and EXP gauges use persisted character values for their fill levels.
They display bracketed current and maximum values over the WZ artwork.

New characters start on the map selected by `characters.initial_map_id`. The
bundled configuration selects `Mushroom Town` (map `10000`).

Click the stat button in the status bar to open the `UIWindow.img` character
stat window. Its background, close control, and job label remain unloaded until
you first open the window. New characters receive server-owned Beginner stats,
which their persisted player records require.

### Skills and audio

When `Sound.wz` is present, each map's `info/bgm` reference selects its looping
background music. Moving between maps changes the track only when the reference
changes. The browser also uses the archive's `Game.img` cues for jumping,
portals, item pickup, item drop and use, death, level-up, and quest completion.
Browser autoplay rules can defer the initial BGM until the first keyboard or
pointer input.

When `Effect.wz` is present, leveling up also plays the native
`BasicEff.img/LevelUp` animation as a full-canvas overlay anchored to the
character.

Place `Skill.wz` and its matching `String.wz` beside the other archives to use
the original skill books. New characters receive the configured initial skill
points. Open the Skills window and click the WZ plus button beside a skill to
spend one point. Click a learned skill icon to use it directly.

To bind a learned skill, leave the Skills window open and open Key Settings.
Drag the skill icon onto a key. A skill can have one key assignment, like each
built-in action.

Quest Act skill rewards raise learned and master levels independently. They do
not spend skill points or lower either value. Their authored job IDs are exact.
Beginner-family skill IDs retain the original cross-job bypass.

`Skill.wz` is the authoritative global index, including invisible definitions.
An invisible real skill enters the player's skill book after a positive learned
or master-level unlock. Its positive master level limits later point allocation.

An invisible definition with maximum level zero behaves differently. An
authored level-1 record may persist as an acquisition marker for quest checks.
The marker remains hidden, non-allocatable, non-bindable, and non-usable.

The server owns skill use. It confirms the learned level, reads that level's WZ
properties, checks and spends HP and MP, enforces WZ cooldowns, and applies
immediate HP recovery. It returns temporary speed and jump effects to the
client.

A damaging skill targets the nearest living mob in front of the character. The
server verifies the target map, foothold layer, facing direction, horizontal
reach, and vertical reach. It then chooses damage from the calculated range.
The server owns mob HP, death, aggro, and respawn state. The client displays the
resulting damage, mob HP bar, attack animation, and projectile state.

Temporary effects from different skill and item sources coexist. Reapplying the
same source replaces its previous holder. Each numeric combat or movement
modifier uses the highest nonzero value among the active holders. Oozems does
not add these values together. A new morph replaces any active morph from
another source.

Successful skill use also returns the matching `Sound.wz`
`Skill.img/<skill ID>/Use` sound when it is available. The server reads caster
`effect`, projectile `ball`, and target `hit` animation frames from the active
skill level in `Skill.wz`. The use response includes only their versioned
descriptors.

The browser requests the PNG and MP3 or WAV data on first use. It then relies on
its normal cache. Projectile effects travel in the character's facing direction
and are followed by their target effect.

### Items and inventory

Click the equipment or inventory button to open its `UIWindow.img` window. The
inventory uses the native Equip, Use, Setup, Etc, and Cash tabs. Pet items appear
under Cash.

Left-click an item on the Equip tab to equip it. Left-click an equipped item to
move it back to inventory. Right-click an item on any inventory tab to drop it
at the server-owned player position. Double-click a supported item on the Use
tab to consume one and apply its recovery or temporary effect. Double-click a
supported chair on the Setup tab to sit on it without consuming it. Movement,
combat, death, and map changes end the seated state. Oozems persists equipment
and inventory changes in SQLite.

The browser requests an equipment icon from `Character.wz` only when the icon
first becomes visible. Equipping or removing an item refreshes the composed
character layers. An empty top or bottom slot uses the gender-specific pajama
layers from `Character.wz` instead of leaving the body unclothed.

Oozems persists inventory stack deadlines with each stack. Zero means
permanent. Only matching item IDs and deadlines merge. Item consumption uses
the earliest deadline before permanent stacks.

The server removes expired stacks at the locked player-load boundary. It saves
a new player revision only if pruning changes the inventory.

Dropped items are transient and scoped to their map. The map protobuf includes
their item ID, position, normal despawn deadline, and preserved item deadline. A
drop expires at the earlier active deadline. Expired drops are removed from the
server drop store and stop rendering in the client.

The Pick Up action moves the nearest drop within pickup range into the
character's inventory. The server removes the drop and saves the inventory as
one item action. It restores the drop if the player save fails.

## Configure content and interactions

Project-wide configuration files are under `config`. Configuration tied to a
specific WZ version belongs in the runtime data directory, which is `./data` by
default.

### Configure overloaded skill properties

`Skill.wz` uses the property names `x`, `y`, and `z` for different purposes in
different skills. The archive does not encode a universal meaning for those
names. An optional `data/skill-semantics.toml` file assigns meanings for the
specific `Skill.wz` in use. Conventional properties such as `acc`, `eva`,
`damage`, and `time` do not need mappings.

Each rule names one or more skill IDs from that archive, a direct level
property, its editor label, and any normalized server stats:

```toml
schema_version = 1

[[level_properties]]
skill_ids = [4000000]
property = "x"
label = "Accuracy"
normalized_stats = ["accuracy"]
transform = { type = "numeric" }
```

Numeric transforms can include an `offset`. A label-only interpretation uses
`transform = { type = "preserve" }` and omits `normalized_stats`. The server
keeps the original `x`, `y`, or `z` value in either case. A conventional named
property takes precedence if it and an overloaded alias target the same stat.

Missing mapping files are treated as empty, so unknown overloaded properties
remain raw and are never guessed. The server reads the mapping from the same
directory as `Skill.wz`, including when `OOZEMS_WZ_DIR` overrides the default
location. If a mapping file is present, `Skill.wz` must also be present. Both
the server and WZ editor reject stale skill IDs, properties missing from any
direct skill level, and numeric transforms that produce nonnumeric or
out-of-range stat values. Review mappings whenever `Skill.wz` changes.
`examples/v83/skill-semantics.toml` contains mappings audited for the
repository's v83 example data.

### Configure loot

`data/loot.toml` defines the independent item entries rolled when a mob dies or
a reactor is destroyed. The local WZ archives provide some source-to-item
associations, but they do not provide ordinary drop probabilities. The
configured rates are therefore project-authored. A rate is expressed per
million, and `1000000` is guaranteed.

A drop can include an optional `quest_id`. Quest drops roll only while that
quest is started. If the quest has a completion requirement for the item, the
drop stops rolling once the character owns the required quantity. Referenced
quest IDs are validated against the loaded quest catalog during startup.

### Limit NPC content

NPC inclusion is controlled by `config/content.toml`:

```toml
[npcs]
allowed_limited_names = []
# allowed_ids = [1012000, 1012003]
```

WZ `limitedname` data normally identifies seasonal or event NPCs. Use the
settings as follows:

- Omit `allowed_limited_names` to permit every limited name.
- List `allowed_limited_names` to permit only those event scopes.
- Set `allowed_limited_names = []` to exclude all limited NPCs.
- Omit `allowed_ids` to permit all remaining NPC IDs.
- List `allowed_ids` to render only those IDs.
- Set `allowed_ids = []` to render no NPCs.

Both allowlists apply when both settings are present. If `content.toml` is
absent, NPC loading remains unrestricted. Restart the server after changing
these settings.

### Configure quests

#### Limit loaded quests

When `Quest.wz` is present, every quest compatible with the implemented typed
mechanics is enabled automatically. Unsupported definitions are skipped and
counted in the startup log. An optional strict allowlist can narrow loading:

```toml
[quests]
allowed_ids = [1009]
```

When an allowlist is present, startup fails if one of its quests is absent or
uses data outside the supported subset.

Rain's real `Quest.wz` quiz in Amherst is one compatible quest. It exercises job
and NPC conditions, accept and decline dialogue, list answers, persistent
started/completed state, EXP rewards, and retained next-quest metadata.

#### Understand automatic transitions

`autoAccept`, `normalAutoStart`, and `autoStart` accept a quest when its normal
availability checks pass.

`autoComplete` completes a normally ready quest, while
`autoPreComplete` bypasses ordinary objectives but still preflights scripts and
actions. Automatic transitions repeat to a stable state so dependency chains
can advance in one locked request. WZ `ask = 1` list questions are supported in
both start and completion dialogue. Start questions require a human answer and
therefore take precedence over automatic-start metadata. They remain available
through their authoritative start NPC; a start question without an NPC is
rejected as unreachable.

#### Understand conditions and persistence

Quest checks use the authoritative world ID from `gameplay.toml`. See
[Configure the world ID](#configure-the-world-id).

Quest `worldmin` and `worldmax` start checks use this value with inclusive
bounds. Quest fame checks read the authoritative character stats. Completion
mesos, calendar-window, and eligible completed-quest-count checks are evaluated
from current player state. Completed-quest counts exclude quest IDs
`9000..=10999`, QuestInfo area `51`, malformed statuses, and player records for
definitions that are not loaded.

#### Apply quest time limits

Quest `timeLimit` and `timeLimit2` values are seconds. The server converts both
to checked milliseconds. It expires an active quest at its accepted time plus
that duration and resets the quest without rewards. An expired quest cannot be
reaccepted during the same automatic transition pass. Oozems stores mesos, cash
points, and quest state with the player.

#### Store quest records

Quest record progress is stored as canonical, typed quest records. Record IDs
are nonzero and unique, entry indices are unique, and both levels are sorted
before persistence. Values are exact ASCII strings of at most 15 bytes, so
leading zeros and case remain significant. `Quest.wz` `infoNumber` redirects a
check to another quest record; direct `info` and `infoex` entries are OR
alternatives against index 0. Equality is exact. Numeric conditions accept only
strict decimal strings. A missing record never satisfies a check.

SQLite stores quest records and their entries in normalized child rows. They
must be unique and valid. Missing, malformed, duplicate, or invalid data fails
player load; valid entries are sorted into canonical order.

#### Apply reward deadlines

Quest item-action `period` values are relative lifetimes in minutes. Their
deadline starts when that start, completion, or restoration action executes.
`dateExpire` is an absolute `yyyyMMddHH` civil deadline from the GMS archive.
The server interprets it in `America/Los_Angeles` with Jiff's bundled timezone
database, including PST and PDT transitions. Expiring equipment rewards remain
unsupported because every response path does not yet refresh composed character
sprites authoritatively.

#### Replace quest scripts

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

> **WZ version warning:** Review the example before you use it as
> `${OOZEMS_DATA_DIR}/quest-scripts.toml` or with a different archive version.

[`examples/v83/quest-scripts.toml`](examples/v83/quest-scripts.toml) is a
project-authored catalog for the matching GMS v83 content. It covers all 663
script names referenced by the 2,766 quest definitions supported by this
server. Fifty-three programs add typed behavior reconstructed from exact local
WZ evidence or behavior facts independently reconciled with the local WZ data.
The other 610 are explicit WZ-only fallbacks: they allow the ordinary WZ
checks, dialogue, and actions to run, but cannot reproduce unavailable or
unsupported script behavior. The catalog is not a copy of the original server
scripts and is not loaded automatically.

Every condition in a program must pass. A program can also contain typed
resource actions and optional dialogue pages:

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

The configuration supports these capabilities:

| Kind | Capabilities |
| --- | --- |
| Conditions | `minimum_level`, `maximum_level`, `job_ids`, `map_id`, `mesos_at_least`, `mesos_at_most`, `item_quantity`, `quest_state`, `quest_record_equals`, `quest_record_at_least`, `quest_record_at_most` |
| Actions | Signed `item_delta`, signed `mesos`, unsigned `experience`, signed `fame`, `set_record`, `set_quest_status` |

Quest states are `not_started`, `started`, and `completed`. Record conditions
can read any canonical helper or redirected record.

Script actions are merged with the WZ actions for the server-selected start or
completion phase, then the combined set uses the same atomic in-memory
inventory, mesos, EXP, fame, record, and cross-quest status transform as an
ordinary quest.

A cross-quest status action changes only the target's stored status and
timestamps. It does not run the target quest's checks, scripts, dialogue,
actions, or rewards. A script cannot target the quest whose phase is currently
transitioning.

Cross-quest states have these effects:

| State | Effect |
| --- | --- |
| `not_started` | Removes the target quest entry and its own quest record |
| `started` | Replaces stale mob progress and timestamps with a clean acceptance at the action time |
| `completed` | Clears mob progress, completes at the action time, and preserves a valid existing acceptance time |

The file uses strict tagged records. These errors stop startup for every
program:

- Unknown fields or capability names
- Duplicate or empty script names
- Script names absent from `Quest.wz`
- Values that exceed the shape limits

Programs referenced by loaded quest definitions also reject these errors:

- Unknown item IDs
- Zero item quantities or action amounts
- Contradictory limits
- Numeric combinations that the quest action model cannot represent
- Zero record IDs
- Record values that exceed the persisted ASCII limit
- Numeric record predicates that are not strictly decimal
- Duplicate or incompatible record operations
- Quest status targets that are zero, unloaded, or duplicated in one merged
  action plan

A catalog may contain at most 1,024 programs. One program may contain at most
64 conditions, actions, and pages in total. It may have at most 16 result pages
and 16 incomplete pages. Each page is limited to 4096 UTF-8 bytes. Each script
name is limited to 256 bytes.

Quest scripts have no filesystem, network, clock, random, loop, callback,
generic NPC script, portal script, mob-kill integration, or dynamic branching
capability. Restart the server after changing `quest-scripts.toml`.

### Configure shops and taxi routes

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

### Configure the Cash Shop

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
and may contain at most 24 characters.

The catalog has these constraints:

- It may contain at most 10 offers.
- Each `offer_id` must be unique and positive.
- Each price must be positive.
- Each item must exist in the local WZ item catalog.
- Each duration must be `"permanent"` or a positive humantime value such as
  `"7d"` or `"12h"`.

The browser submits only the stable offer ID. The server resolves its item,
price, and lifetime. It computes the expiration deadline at purchase time and
persists the item and remaining Cash Points together.

An absent `cash-shop.toml` creates an empty Cash Shop that uses the `"Ooze"`
label.

The current screen is intentionally limited to listing and buying one item at a
time. It does not implement packages, gifting, wishlists, search, try-on, or
cash storage.

## Configure gameplay rules

Gameplay rules are configured in `config/gameplay.toml`.

> **Required fields:** Every section and field shown below is required. A
> missing or unknown section or field stops server startup. The server does not
> load defaults from an older layout.

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
position_tolerance = 24.0
ground_tolerance = 8.0
platform_edge_tolerance = 20.0
ladder_reach = 32.0
ladder_end_reach = 20.0
portal_horizontal_reach = 48.0
portal_vertical_reach = 64.0
```

### Configure item lifetime

`items.drop_despawn` controls how long a dropped item remains in a map. It must
be a positive human-readable duration, such as `30s`, `10m`, `2h`, or
`1h 30m`. Restart the server after changing it. Drops are intentionally not
persisted across a server restart.

### Configure new characters

`skills.initial_points` is the number of unspent skill points assigned to a new
character. Learned levels and later point changes are persisted and are not
replaced when this setting changes.

`characters.initial_map_id` selects the WZ map used for newly created
characters. The server verifies the map during startup and places each new
character at its first spawn portal. Changing it does not move existing
characters.

`characters.initial_cash_points` sets the cash-point balance for new
characters. It must fit the persisted signed 64-bit range. Later setting
changes do not replace existing balances.

### Configure the world ID

The `[world]` section defines one authoritative nonnegative server world ID.
The bundled configuration uses world `0`. Quest `worldmin` and `worldmax` start
checks use this value with inclusive bounds.

### Configure combat

Combat distances are measured in map pixels. Mobs acquire an aggro target when
that player damages them. `disengage_range` controls how far a mob can remain
interested in that target. Basic attacks use the authored WZ swing bounds for
the equipped weapon or bare hands. `player_attack_range` and
`attack_vertical_reach` define the skill target envelope and the fallback when
character WZ geometry is unavailable.
`player_attack_interval` is the minimum delay between Basic Attacks. When WZ
character animation data is available, a longer attack animation extends this
delay so another attack cannot start before it finishes. The two touch reach
values form the mob contact box.

`projectile_range` controls when a magic-attacking mob can launch a projectile.
`projectile_speed` is measured in map pixels per second, and
`projectile_hit_reach` is its collision radius. `mob_attack_interval` controls
how often a mob can launch one. `player_invulnerability` prevents overlapping
contact and projectile hits from applying on every movement heartbeat.
`default_respawn` applies when a WZ spawn point does not define `mobTime`. All
combat numbers must be finite and positive. Combat durations accept the same
human-readable syntax as item despawn durations.

### Configure movement

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
budget. Movement coordinates are session state and are never persisted. SQLite
stores only the current map ID. On reconnect, the server places the player at
that map's default spawn. An unavailable saved map repairs directly to
`characters.initial_map_id`. A saved map without a default spawn instead uses
its WZ return town, with the initial map as the final fallback.

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

### Understand movement authority

Rejected snapshots return the last authoritative server position and the WASM
client resynchronizes to it. Purely visual movement inside a modified client is
not observable, but it cannot change the server position used for recovery,
pickups, drops, portals, persistence, or later gameplay calculations.

## Configure formula profiles

Combat formulas are configured in `config/skill-formulas.toml`.

> **Startup validation:** The server parses and validates every configured
> formula before it serves players. An invalid formula or selector stops
> startup. Restart the server after changing this file.

### Define profiles and selectors

The bundled file records its source and groups formulas into reusable profiles.
Each selector table maps a stable game identifier to one profile:

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

### Use the supported tables

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

### Understand current formula use

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

### Configure natural recovery

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

### Write profile formulas

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

### Handle formula errors

The server parses every configured formula and rejects unknown identifiers,
unknown functions, invalid syntax, non-ASCII text, invalid numeric IDs, invalid
profile or property names, empty profiles, and selectors that name unknown
profiles before it starts serving players. Evaluation also rejects missing
properties, missing inputs, division by zero, and non-finite results.

The defaults group formulas from the linked 2009 Ayumilove compilation by the
game concepts that consume them while preserving its constants and caps. This
is a historical community source, so a server owner can replace any profile or
expression when targeting a different version or interpretation.

## Configure XP curves

XP curves are configured in `config/xp-curves.toml`.

> **Startup validation:** The server parses and validates every curve at
> startup, including curves that are not selected. An invalid curve stops
> startup.

### Define a curve

Every XP curve configuration file must start with a comment that directs
readers to this reference. The bundled configuration has this shape:

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
named curves may be defined for future game modes.

Each `curves.ranges` entry defines an inclusive level range. Ranges within one
curve must start at level 1, remain contiguous, and must not overlap. The
highest supported configured level is 10,000. A character level outside the
selected curve is treated as a server configuration error.

### Write XP formulas

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
startup with a configuration error.

### Change the selected curve

Restart the server after changing a curve.
Changing the selected curve does not recalculate character levels or discard
accumulated XP. It replaces only the requirement for advancing from the
character's current level.

## Use the workspace tools

### Dump GUI rendering

The WASM client installs `window.oozemsDumpGui` after the game canvas starts.
Use it from browser developer tools or browser automation to download an exact
crop of the current backing canvas as a PNG:

```js
await window.oozemsDumpGui(
  "inventory-window",
  7,
  50,
  140,
  180,
  "inventory-items.png",
);
```

The arguments after the element name are crop `x`, `y`, `width`, and `height`.
They use backing-canvas pixels relative to the selected element. They are not
CSS pixels or absolute game-canvas coordinates. Each value must be a whole,
nonnegative number. The crop must have a positive size and fit within the
visible part of the element. The last argument is a PNG download filename, not
a filesystem path. The promise resolves after PNG encoding starts the download.
Browser automation can capture the result as a normal download and write it to
its chosen output directory.

The supported element names are:

| Element | Availability |
| --- | --- |
| `game` | Entire current game canvas |
| `status-bar` | Main game view |
| `stat-window` | Character window is open |
| `equipment-window` | Equipment window is open |
| `inventory-window` | Inventory window is open |
| `key-config-window` | Key Settings window is open |
| `skill-window` | Skills window is open |
| `npc-dialog-window` | NPC dialogue or taxi window is open |
| `shop-window` | NPC shop is open |
| `cash-shop-window` | Cash Shop is open |

The dumper rejects an unavailable, invalid, or hidden element instead of
capturing unrelated pixels at its expected position. Cash Shop crops account
for the screen's active centering and scale transform. The Rust library also
exports the same function as `dumpGui` from its generated WASM module.

### Edit WZ definitions

The workspace includes `oozems-wz-editor`, a native editor for `Quest.wz`,
`Skill.wz`, and `quest-scripts.toml`. The unified Quest tab resolves each
selected quest's start and completion script references automatically, so its
WZ definition and linked script forms are available after one search. The
Skills tab provides a separate searchable skill list and typed WZ property
controls. Every skill container has an **Add property** form for adding scalar,
vector, null, or nested container nodes. Existing properties have staged remove
and undo controls. This supports effects that the original definition omitted,
such as adding a typed `jump` value to each level of Nimble Feet.

Place matching `Quest.wz`, `Skill.wz`, and `String.wz` files in `data`. If the
archive has an accompanying `skill-semantics.toml`, place that file there too,
then run:

```sh
make wz-editor
```

The editor never overwrites a source WZ archive. It writes
`data/Quest.edited.wz` or `data/Skill.edited.wz`, preserving the source archive
until you review and replace it. Quest script saves update the configured TOML
file atomically.

The Skills tab initially selects Nimble Feet when it is present. Expand its
`level` nodes and set each typed **Duration (time)** property to **Permanent**,
then save the edited archive. The WZ boundary represents a permanent skill
duration as `-1`. The server converts this sentinel to an explicit permanent
lifetime, and the client displays the active buff as `PERM` instead of starting
a countdown. The buff lasts until it is replaced or the player's server session
ends; it is not persisted across server restarts. Skill levels also show their
`String.wz` effect summaries, and known WZ property names are labelled with the
stats they affect. The `hs` property is labelled as a level-description
selector: a value such as `h3` selects the corresponding `String.wz` text
template and does not affect a character stat. Labels for overloaded `x`, `y`,
and `z` properties come from the validated `skill-semantics.toml` associated
with the open archive. Use `--skill-semantics <PATH>` when that file is outside
the selected `--data` directory.

### Inspect WZ archives from the command line

The workspace includes `oozems-wz`, a JSON-first CLI for repeatable WZ
inspection and safe PKG1 property edits. It inspects standard PKG1 and PKG2
archives, paginates large node lists, and emits typed values without embedding
large media payloads.

Run these commands from the workspace root:

```sh
cargo run --package oozems-wz -- info data/Quest.wz
cargo run --package oozems-wz -- list data/Quest.wz /Act.img --limit 25
cargo run --package oozems-wz -- get data/Quest.wz /Act.img/1000/1/nextQuest
```

### Edit a WZ archive

> **Archive safety:** An edit requires a separate output path. The tool never
> overwrites the input archive. It copies every unchanged image blob
> byte-for-byte, rebuilds archive offsets and checksums, validates the complete
> output with two independent WZ readers, and then installs it atomically.

```sh
cargo run --package oozems-wz -- set \
  data/Quest.wz /Act.img/1000/1/nextQuest \
  --value 1002 \
  --output data/Quest.edited.wz
```

See [`crates/oozems-wz/README.md`](crates/oozems-wz/README.md) for path rules,
pagination, JSON fields, supported value types, and safety details.

### Infer quest script replacements

The workspace also includes `oozems-quest-harness`. This CLI discovers scripted
quests and assembles model evidence directly from `Quest.wz`, `Npc.wz`, and
`String.wz`. It sends the evidence to an OpenRouter-compatible model. It then
validates the guessed `quest-scripts.toml` programs against the schema that the
server supports.

OpenRouter login uses a localhost PKCE callback. It never stores the resulting
API key in the repository.

Log in, find a quest, and generate one replacement:

```sh
cargo run --package oozems-quest-harness -- login
cargo run --package oozems-quest-harness -- quests \
  data/Quest.wz --search q10272e
cargo run --package oozems-quest-harness -- generate \
  --model openai/gpt-5.2 \
  data/Quest.wz \
  q10272e
```

> **Potential cost:** A complete batch can make hundreds of paid model
> requests. Review the model and provider pricing before you start one.

To generate every unique script referenced by the archive, pass
`--all --output generated-quest-scripts.toml` instead of a quest selector.

See
[`crates/oozems-quest-harness/README.md`](crates/oozems-quest-harness/README.md)
for input rules, compatible endpoints, credential behavior, and limitations.

## Understand the architecture

### Follow the data flow

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
  -> POST /api/v1/players/save    key bindings

server
  -> config/xp-curves.toml        validated game progression rules
  -> config/gameplay.toml         validated world, item, skill, and movement rules
  -> config/content.toml          WZ content inclusion rules
  -> config/skill-formulas.toml   validated combat formulas
  -> data/interactions.toml       version-specific shop stock and taxi routes
  -> data/cash-shop.toml          global Cash Shop offers, prices, and lifetimes
  -> data/loot.toml               version-specific mob item drop rates
  -> data/quest-scripts.toml      version-specific replacements for WZ quest scripts
  -> data/skill-semantics.toml    version-specific x, y, and z skill meanings
  -> data/Map.wz                  required, lazy WZ map source
  -> data/Npc.wz                  optional NPC placement animation source
  -> data/Quest.wz                enabled quest conditions, dialogue, and rewards
  -> data/Mob.wz                  optional mob stats and animation source
  -> data/Character.wz            optional character sprite source
  -> data/UI.wz                   optional GUI sprite source
  -> data/Skill.wz                optional skill data, icons, and effects
  -> data/Sound.wz                optional skill sounds
  -> data/String.wz               optional map, NPC, and skill text
  -> data/oozems.sqlite3          normalized mutable player state
```

The API schema is in `crates/oozems-proto/proto/oozems.proto`. Image and audio
files keep their native formats instead of being wrapped in protobuf. This lets
the browser stream, cache, and decode them directly.

Asset URLs include a SHA-256-derived version. Changing one file therefore
invalidates only that cached file.

## Verify changes

Run the workspace formatting checks, Rust checks, tests, and WASM client check:

```sh
make check
```

The formatting step uses the Rust nightly toolchain. The other checks cover all
workspace targets and the `wasm32-unknown-unknown` client target.
