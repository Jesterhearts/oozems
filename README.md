# Oozems

Oozems is an original old-school side-scrolling RPG foundation for personal
use. It does not include MapleStory code or assets.

## Not Ready For General Use

The current version of the server is not yet ready for general use. Combat is
still limited to player skills, mob contact attacks, and basic mob projectiles.
Features such as player death handling, loot, and quests are not implemented.

When it is ready, a release tag will be posted for a version 0.1. That will
indicate general usage availability, although polish and bug fixes will
likely be needed after such a release.

## Run it

Install Rust, the `wasm32-unknown-unknown` target, and Trunk. Place `Map.wz` in
`./data`, then run:

```sh
make run
```

Open <http://127.0.0.1:3000>. The Make target builds the WASM client into the
server's generated `public` directory before starting the server.

The default data directory is `./data`. It is ignored by Git. These environment
variables override the defaults:

| Variable             | Default                             |
| -------------------- | ----------------------------------- |
| `OOZEMS_BIND`        | `127.0.0.1:3000`                    |
| `OOZEMS_DATA_DIR`    | `./data`                            |
| `OOZEMS_CONFIG_DIR`  | `./config`                          |
| `OOZEMS_PUBLIC_DIR`  | `crates/oozems-server/public`       |
| `OOZEMS_WZ_DIR`      | `./data`                            |

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
  -> POST /api/v1/skills/...      allocate a skill point or use a skill
  -> POST /api/v1/players/recover apply one rate-limited natural recovery tick
  -> GET /wz-assets/...           requested WZ PNG and skill audio assets
  -> POST /api/v1/players/save    key bindings and authoritative session state

server
  -> config/xp-curves.toml        validated game progression rules
  -> config/gameplay.toml         validated item, skill, and movement rules
  -> config/content.toml          WZ content inclusion rules
  -> config/skill-formulas.toml   validated combat formulas
  -> data/Map.wz                  required, lazy WZ map source
  -> data/Npc.wz                  optional NPC placement animation source
  -> data/Mob.wz                  optional mob stats and animation source
  -> data/Character.wz            optional character sprite source
  -> data/UI.wz                   optional GUI sprite source
  -> data/Skill.wz                optional skill data, icons, and effects
  -> data/Sound.wz                optional skill sounds
  -> data/String.wz               optional WZ map names and skill text
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
portals use the animation frames under `MapHelper.img`. The client requests a
sprite when one of its placements first enters the viewport. Each sprite stays
compressed in `Map.wz` until the browser requests its opaque
`/wz-assets/...` URL. The server then decodes that sprite, returns a normal PNG,
and caches it for later requests. WZ files and extracted assets are not added to
the client bundle.

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
server loads each referenced NPC's standing animation when the map is first
requested, places the NPC on its supporting foothold and WZ layer, and includes
only those frame assets in the map response. The client preserves the WZ frame
timing, origin, and facing direction while rendering NPCs. Their PNG data stays
compressed until a frame first enters the viewport.

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

Place `Character.wz` beside the map archives to enable character creation. The
server indexes the available skin, face, and hair styles, then composes idle,
walk, jump, ladder, and rope frames from each sprite's WZ anchor points and z
layer. The browser receives only frame metadata at first. It requests the
individual PNG layers while the preview or game renderer needs them. The
chosen name and appearance are stored with the player in SurrealKV.

Place `UI.wz` beside the other archives to use its classic `StatusBar.img` and
`UIWindow.img` sprites for the in-game HUD. The server sends the layouts
through protobuf. The browser then requests backgrounds, gauges, quick-slot
panels, buttons, and open windows as normal versioned PNG assets. If `UI.wz`
is absent, the client keeps using its built-in fallback HUD.

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
palette contains Jump, Pick Up, Character, Equipment, Inventory, Key Settings,
and Skills. Changes are stored with the player in SurrealKV.

Place `Skill.wz` and its matching `String.wz` beside the other archives to use
the original skill books. New characters receive the configured initial skill
points. Open the Skills window and click the WZ plus button beside a skill to
spend one point. Click a learned skill icon to use it directly. To bind a
learned skill, leave the Skills window open, open Key Settings, and drag the
skill icon onto a key. A skill can have one key assignment, like each built-in
action.

Skill use is server-owned. The server confirms the learned level, reads that
level's WZ properties, checks and spends HP and MP, enforces WZ cooldowns,
applies immediate HP recovery, and returns temporary speed and jump effects to
the client. A damaging skill targets the nearest living mob in front of the
character. The server verifies the target map, foothold layer, facing direction,
horizontal reach, and vertical reach before choosing damage from the calculated
range. It owns mob HP, death, aggro, and respawn state. The client displays the
resulting damage, mob HP bar, attack animation, and projectile state.

When `Sound.wz` is present, a successful use also returns the matching
`Skill.img/<skill ID>/Use` sound. The server reads caster `effect`, projectile
`ball`, and target `hit` animation frames from the active skill level in
`Skill.wz`. Only their versioned descriptors are included in the use response.
The browser requests the PNG and MP3 or WAV data on first use, then relies on
its normal cache for later uses. Projectile effects travel in the character's
facing direction, followed by their target effect.

The HP, MP, and EXP gauges use the persisted character values for their fill
levels and display bracketed current and maximum values over the WZ artwork.

Click the stat button in the status bar to open the `UIWindow.img` character
stat window. Its background, close control, and job label remain unloaded until
the window is first opened. New characters receive server-owned Beginner stats,
and existing SurrealDB records receive the same defaults when their older
records do not contain stat fields.

Click the equipment or inventory button to open its `UIWindow.img` window.
Left-click an inventory item to equip it. Left-click an equipped item to move
it back to inventory. Right-click an inventory item to drop it at the
server-owned player position. Equipment and inventory changes are persisted in
SurrealKV. The browser requests each equipment icon from `Character.wz` only
when the icon is first visible. Equipping or removing an item also refreshes
the composed character layers. An empty top or bottom slot uses the
gender-specific pajama layers from `Character.wz` instead of leaving the body
unclothed.

Dropped items are transient and scoped to their map. Their item ID, position,
and server-issued expiry time are sent in the map protobuf. Expired drops are
removed from the server drop store and stop rendering in the client. The Pick
Up action moves the nearest drop within pickup range into the character's
inventory. The server removes the drop and saves the inventory as one item
action, restoring the drop if the player save fails.

## Configure gameplay rules

Item rules are configured in `config/gameplay.toml`:

```toml
# See README.md for configuration reference.

[items]
drop_despawn = "10m"

[skills]
initial_points = 3

[combat]
disengage_range = 520.0
player_attack_range = 220.0
attack_vertical_reach = 90.0
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

`items.drop_despawn` controls how long a dropped item remains in a map. It must
be a positive human-readable duration, such as `30s`, `10m`, `2h`, or
`1h 30m`. Restart the server after changing it. Drops are intentionally not
persisted across a server restart.

`skills.initial_points` is the number of unspent skill points assigned to a new
character. It is also used when an older SurrealKV player record has no skill
point field. Learned levels and later point changes are persisted and are not
replaced when this setting changes.

Combat distances are measured in map pixels. Mobs acquire an aggro target when
that player damages them. `disengage_range` controls how far a mob can remain
interested in that target. `player_attack_range` and `attack_vertical_reach` are
the server-authoritative skill target envelope. The two touch reach values form
the mob contact box.

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
including Haste-style `speed` and `jump` bonuses, are summed by the client and
server before these caps are applied.

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

The current skill damage pipeline reads `minimum` and `maximum` from a selected
skill profile. It applies the skill level's WZ `damage` percentage afterward
and truncates the final values. When the attack reaches a mob, non-fixed damage
then passes through `defenses.physical` using the mob's WZ physical defense and
the player and mob levels. WZ fixed damage bypasses defense. The current
equipment model does not include
weapons, so `WeaponAttack` is read from the `attack` property of the profile
selected by `weapons.bare_hands`. This provides one clear input point for real
weapon stats when weapon equipment is added later. Other profile categories
are parsed, validated, and routed now so their combat pipelines can consume the
same configuration model later.

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

The current skill and recovery profile pipelines supply `CharacterLevel`,
`PlayerLevel`, `Strength`, `Dexterity`, `Intelligence`, `Luck`, `SkillDamage`,
`SkillLevel`, and `WeaponAttack`. It also supplies `JobMultiplier` for Pirate
jobs. The other accepted variables belong to formula pipelines that will be
connected as their combat inputs are implemented. Selecting a profile that
needs an unavailable variable returns an explicit skill-use error instead of
substituting a value.

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
interaction. The default action bindings are Space for Jump, Z for Pick Up, C
for Character, E for Equipment, I for Inventory, K for Key Settings, and S for
Skills. Script portals remain inactive because their behavior belongs to a
future server-side scripting system.

## Verify it

```sh
make check
```

The first server build is relatively large because embedded SurrealDB includes
its database engine in the server binary.
