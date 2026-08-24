# oozems-wz

`oozems-wz` is a JSON-first command-line tool for inspecting and safely editing
WZ archives. Its output is deterministic and suitable for terminal use,
scripts, and language-model tools.

The tool inspects standard PKG1 and PKG2 archives and edits PKG1 archives. GMS
encryption is the default; use `--region` for EMS or BMS archives. `List.wz`,
hotfix `Data.wz`, custom IVs, and custom user keys are not supported.

## Run

Run the tool from the workspace root:

```sh
cargo run --package oozems-wz -- info data/Quest.wz
```

Build it once when running several commands:

```sh
cargo build --release --package oozems-wz
target/release/oozems-wz info data/Quest.wz
```

Successful archive commands write one JSON document to standard output. Errors
are written as `{"error":"..."}` to standard error and return a nonzero
status. Help and version output remain plain text. JSON is indented by default.
Add `--compact` to emit one line.

## Inspect

Show the archive format, detected version, encryption region, and entry counts:

```sh
oozems-wz info data/Quest.wz
```

List the archive root or the children of any directory, image, or property:

```sh
oozems-wz list data/Quest.wz /
oozems-wz list data/Quest.wz /Act.img --limit 25
oozems-wz list data/Quest.wz /Act.img --offset 25 --limit 25
```

`list` sorts children by path and returns at most 100 entries by default. Its
JSON contains `total`, `offset`, `count`, and `next_offset`. Omitted
`next_offset` means that the current page is the last page. `ls` is an alias for
`list`.

Show one node without expanding its children:

```sh
oozems-wz get data/Quest.wz /Act.img/1000/1/nextQuest
```

Scalar and vector nodes include a `value`. Parsed container and media nodes
include `child_count` and relevant metadata, but never include large canvas,
sound, or video byte arrays. Images returned by a directory listing omit
`child_count` because `list` does not parse each image; `get` reports it.

## Paths

WZ paths are absolute and start at the archive directory root:

```text
/
/Act.img
/Act.img/1000/1/nextQuest
```

Each slash-delimited segment is an exact, case-sensitive WZ node name. Empty,
`.` and `..` segments are rejected. Node names that contain `/` cannot be
addressed by this version of the tool.

## Edit

`set` changes one existing scalar or vector property and writes a new archive:

```sh
oozems-wz set data/Quest.wz /Act.img/1000/1/nextQuest \
  --value 1002 \
  --output data/Quest.edited.wz
```

Values use JSON syntax. Quote JSON strings separately from the shell:

```sh
oozems-wz set data/String.wz /Npc.img/1000/name \
  --value '"New name"' \
  --output data/String.edited.wz

oozems-wz set data/Map.wz /Map/Map0/000000000.img/info/returnMap \
  --value 100000000 \
  --output data/Map.edited.wz

oozems-wz set data/Some.wz /Example.img/origin \
  --value '{"x":10,"y":-20}' \
  --output data/Some.edited.wz
```

The new value must match the existing WZ type:

| WZ type               | Accepted JSON                         |
| --------------------- | ------------------------------------- |
| `short`, `int`, `long` | Signed integer within the type range |
| `float`, `double`     | Finite number                         |
| `string`, `uol`       | String                                |
| `vector`              | Object with only integer `x` and `y`  |
| `null`                | `null`                                |

Adding or removing nodes and replacing canvas, sound, Lua, raw-data, or video
payloads are not supported. PKG1 editing also requires the independent
`wz_reader` validator to open the archive. That validator cannot open archives
whose first directory entry has a non-ASCII name.

### Safety

Editing follows these rules:

- The output path is required and must differ from the input path. Direct
  in-place editing is rejected.
- Existing output files are rejected unless `--force` is present.
- The edited image is parsed and serialized. Every other image blob is copied
  byte-for-byte from the source archive.
- Directory offsets, image sizes, and checksums are rebuilt.
- The complete output tree is parsed with both `wzlib-rs` and the server's
  `wz_reader`. The edited image property structure and types, complete semantic
  property tree, requested value, checksums, and every unchanged image blob are
  verified before the output is installed.
- The output is written to a temporary file in the destination directory and
  renamed into place only after it has been flushed.

The complete source and rebuilt archive are held in memory while editing.

## Version Options

The bundled archives use GMS encryption, which is the default. Patch versions
are detected automatically. Specify either value when opening other archives:

```sh
oozems-wz --region gms --wz-version 83 info data/Quest.wz
```

`--region` accepts `gms`, `ems`, or `bms`. Global options may appear before or
after the subcommand.
