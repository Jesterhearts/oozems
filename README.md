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
- server-owned assets fetched only when referenced by the current map; and
- player movement, platforms, jumping, and periodic position saves.

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
| `OOZEMS_ASSET_DIR` | `crates/oozems-server/assets` |
| `OOZEMS_CONTENT_DIR` | `crates/oozems-server/content/maps` |
| `OOZEMS_PUBLIC_DIR` | `crates/oozems-server/public` |
| `OOZEMS_WZ_DIR` | `./data` |

## Data flow

```text
browser
  -> GET /                         WASM shell
  -> POST /api/v1/bootstrap       saved player protobuf
  -> POST /api/v1/maps/get        current map protobuf
  -> GET /assets/...              only assets named by that map
  -> POST /api/v1/players/save    player position protobuf

server
  -> content/maps/*.json          immutable map source
  -> data/Map.wz                  optional, lazy WZ map source
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
used by that map. The client requests a sprite when one of its placements first
enters the viewport. Each sprite stays compressed in `Map.wz` until the browser
requests its opaque `/wz-assets/...` URL. The server then decodes that sprite,
returns a normal PNG, and caches it for later requests. WZ files and extracted
assets are not added to the client bundle.

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
4. Add portals, map transitions, NPCs, and inventory as separate typed
   pipelines.
