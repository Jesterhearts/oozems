# Oozems

Oozems is an original old-school side-scrolling RPG foundation for personal
use. It does not include MapleStory code or assets.

The current vertical slice includes:

- a Rust server using Axum;
- embedded SurrealDB persistence backed by SurrealKV;
- a small Rust WASM client rendered with the browser canvas;
- protobuf request and response bodies over HTTP;
- server-owned map files fetched only when entered;
- optional classic PKG1 WZ map archives parsed lazily by the server;
- a character creation screen with idle, walk, jump, ladder, and rope
  animations composed from `Character.wz`;
- optional in-game status, character-stat, equipment, inventory, and keyboard
  settings windows composed from `UI.wz` sprites;
- persisted equipment and inventory state with WZ item icons and transient
  map drops that characters can pick up;
- persisted, drag-and-drop keyboard bindings for every supported action;
- validated TOML game rules with named, formula-based XP curves;
- server-owned assets fetched only when referenced by the current view; and
- player movement, platforms, jumping, ladder and rope climbing, direct portal
  transitions, and periodic position saves.

## Run it

Install Rust, the `wasm32-unknown-unknown` target, and Trunk. Then run:

```sh
make run
```

Open <http://127.0.0.1:3000>. The Make target builds the WASM client into the
server's generated `public` directory before starting the server.

The default data directory is `./data`. It is ignored by Git. These environment
variables override the defaults:

| Variable | Default |
| --- | --- |
| `OOZEMS_BIND` | `127.0.0.1:3000` |
| `OOZEMS_DATA_DIR` | `./data` |
| `OOZEMS_CONFIG_DIR` | `./config` |
| `OOZEMS_ASSET_DIR` | `crates/oozems-server/assets` |
| `OOZEMS_CONTENT_DIR` | `crates/oozems-server/content/maps` |
| `OOZEMS_PUBLIC_DIR` | `crates/oozems-server/public` |
| `OOZEMS_WZ_DIR` | `./data` |

## Data flow

```text
browser
  -> GET /                         WASM shell
  -> POST /api/v1/bootstrap       saved player or creation options
  -> POST /api/v1/characters/...  create a character or get sprite metadata
  -> POST /api/v1/gui/get         current GUI layout and asset metadata
  -> POST /api/v1/maps/get        current map protobuf
  -> POST /api/v1/items/...       equip, unequip, drop, or pick up an item
  -> GET /assets/...              only bundled assets named by that map
  -> GET /wz-assets/...           requested map, character, and GUI PNG layers
  -> POST /api/v1/players/save    player position and key bindings protobuf

server
  -> config/xp-curves.toml        validated game progression rules
  -> config/gameplay.toml         validated item drop rules
  -> content/maps/*.json          immutable map source
  -> data/Map.wz                  optional, lazy WZ map source
  -> data/Character.wz            optional character sprite source
  -> data/UI.wz                   optional GUI sprite source
  -> data/String.wz               optional WZ map names
  -> assets/**                    immutable source assets
  -> SurrealDB -> SurrealKV       mutable player state
```

The API schema is in
`crates/oozems-proto/proto/oozems.proto`. Image and audio files keep their
native formats instead of being wrapped in protobuf. This lets the browser
stream, cache, and decode them directly. Asset URLs include a SHA-256-derived
version, so changing one file invalidates only that cached file.

## Use classic WZ maps

Place `Map.wz` in `./data`. Place the matching `String.wz` beside it to use the
original map names. The server detects the archive version, indexes map image
entries at startup, and parses each map only when it is requested. A WZ map
overrides a JSON map with the same ID.

The map response contains footholds and references to only the sprite assets
used by that map. It also contains typed ladder, rope, and portal data. Visible
portals use the animation frames under `MapHelper.img`. The client requests a
sprite when one of its placements first enters the viewport. Each sprite stays
compressed in `Map.wz` until the browser requests its opaque
`/wz-assets/...` URL. The server then decodes that sprite, returns a normal PNG,
and caches it for later requests. WZ files and extracted assets are not added to
the client bundle.

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
palette contains Jump, Pick Up, Character, Equipment, Inventory, and Key
Settings. Changes are stored with the player in SurrealKV.

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
```

`items.drop_despawn` controls how long a dropped item remains in a map. It must
be a positive human-readable duration, such as `30s`, `10m`, `2h`, or
`1h 30m`. Restart the server after changing it. Drops are intentionally not
persisted across a server restart.

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

| Syntax | Meaning |
| --- | --- |
| `Level` | The level currently being evaluated. It is case-sensitive. |
| `atLevel(10)` | The XP requirement produced by this curve for level 10. |
| `^` | Exponentiation. It is right-associative. |
| `*`, `/` | Multiplication and division. |
| `+`, `-` | Addition and subtraction, including unary signs. |
| `( ... )` | Explicit grouping. |

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
climb. Press Up while standing at a direct portal to enter it. Arrow keys stay
reserved for movement and interaction. The default action bindings are Space
for Jump, Z for Pick Up, C for Character, E for Equipment, I for Inventory,
and K for Key Settings. Script portals remain inactive because their behavior
belongs to a future server-side scripting system.

## Add a map

Add a JSON file under `crates/oozems-server/content/maps`. The server validates
map dimensions, platform geometry, decoration references, duplicate asset IDs,
and asset paths during startup. A bad map stops the restartable server pipeline
before it can serve inconsistent content.

Assets belong under `crates/oozems-server/assets`. Refer to them with paths
relative to that directory. The client receives no map or game asset in its
WASM bundle.

## Verify it

```sh
make check
```

The first server build is relatively large because embedded SurrealDB includes
its database engine in the server binary.

## Suggested next slices

1. Add account sessions and make the server derive player identity from the
   session instead of a fixed local ID.
2. Add a protobuf WebSocket stream for authoritative movement and other
   players while retaining HTTP for bootstrap and content.
3. Split maps into spatial chunks if maps become much wider than the viewport.
4. Add server-side portal scripts, NPCs, consumable items, and combat as
   separate typed pipelines.
