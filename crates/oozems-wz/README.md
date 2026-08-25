# oozems-wz

Use `oozems-wz` to inspect WZ archives from a terminal or script. You can show
archive information, list nodes, and get one node. You can also change one
existing scalar or vector property in a new archive.

The tool produces deterministic JSON for scripts and other tools.

## Check archive support

You can inspect standard PKG1 and PKG2 archives. You can edit only PKG1
archives.

| Operation | PKG1 | PKG2 |
| --------- | ---- | ---- |
| `info`    | Yes  | Yes  |
| `list`    | Yes  | Yes  |
| `get`     | Yes  | Yes  |
| `set`     | Yes  | No   |

GMS encryption is the default. Use `--region ems` or `--region bms` for EMS or
BMS archives. The tool detects the patch version by default. Use `--wz-version`
when you need to provide the expected version:

```sh
oozems-wz --region gms --wz-version 83 info data/Quest.wz
```

`--region` accepts `gms`, `ems`, or `bms`. Global options can appear before or
after the subcommand.

The tool does not support `List.wz`, hotfix `Data.wz`, custom IVs, or custom
user keys.

## Run the tool

Run the tool from the workspace root:

```sh
cargo run --package oozems-wz -- info data/Quest.wz
```

Build it once when running several commands:

```sh
cargo build --release --package oozems-wz
target/release/oozems-wz info data/Quest.wz
```

## Read command output

Each successful archive command writes one JSON document to standard output.
JSON is indented by default. Add `--compact` to write the document on one line.

Errors write `{"error":"..."}` to standard error and return a nonzero status.
Help and version output use plain text.

## Inspect an archive

### Check archive information

Show the archive format, detected version, encryption region, and entry counts:

```sh
oozems-wz info data/Quest.wz
```

### Use valid WZ paths

WZ paths are absolute and start at the archive directory root:

```text
/
/Act.img
/Act.img/1000/1/nextQuest
```

Each segment between slashes must exactly match the case-sensitive WZ node
name. The tool rejects empty segments, `.`, and `..`. It also rejects a trailing
slash. This version cannot address a node whose name contains `/`.

### List child nodes

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

### Get one node

Show one node without expanding its children:

```sh
oozems-wz get data/Quest.wz /Act.img/1000/1/nextQuest
```

Scalar and vector nodes include `value`. Parsed properties include
`child_count`. Container and media nodes include relevant metadata. The output
does not include canvas, sound, Lua, raw-data, or video payload bytes.

Images in a directory listing omit `child_count` because `list` does not parse
each image. Use `get` to report that count.

## Review edit safety

Read these constraints before you edit an archive:

- `set` supports PKG1 archives only.
- You must provide an output path that differs from the input path. The tool
  rejects direct in-place edits.
- The output directory must already exist.
- The tool rejects an existing output file unless you use `--force`.
- The input archive path must use valid UTF-8 for the independent `wz_reader`
  validator.
- The independent validator must be able to open the input archive. It cannot
  open an archive whose first directory entry has a non-ASCII name.
- The tool holds the complete source and rebuilt archives in memory while it
  edits.

The tool cannot add or remove nodes. It cannot replace canvas, sound, Lua,
raw-data, or video payloads.

## Set a property

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

## Understand edit validation

The tool validates an edit before it installs the output:

- It opens the input with `wzlib-rs` and the independent `wz_reader` validator.
- It checks the complete directory and image entry tree with both readers.
- It parses and serializes the edited image. It copies every other image blob
  byte-for-byte from the source archive.
- It rebuilds directory offsets, image sizes, and checksums.
- It reparses the rebuilt archive with `wzlib-rs`. It checks the edited image's
  complete semantic property tree, the requested type and value, every image
  checksum, and every unchanged image blob.
- It opens the rebuilt archive with `wz_reader`. It checks the complete entry
  tree and confirms that the edited image kept its property structure and
  types.
- It writes the output to a temporary file in the destination directory. It
  flushes and validates that file before it renames the file into place.
