# oozems-wz

Use `oozems-wz` to inspect WZ archives from a terminal or script. You can show
archive information, list nodes, and get one node. You can also change one
existing scalar or vector property in a new archive or generate an independent
loot catalog from local WZ facts and an Oozems-authored policy.

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

## Generate loot

Run the generator against one directory containing matching WZ archives:

```sh
cargo run --package oozems-wz -- \
  --region gms --wz-version 83 \
  generate-loot /srv/maplestory/wz \
  --policy config/loot-policy.toml \
  --output data/loot.toml
```

`generate-loot` requires `String.wz`, `Item.wz`, `Mob.wz`, and `Quest.wz`.
Policy rows with `source_map_id` or `required_skill_id` also require matching
`Map.wz` or `Skill.wz`. Add the matching `Character.wz` to validate equipment
rewards. Without it, the generator reports and omits equipment associations.
`--region` and `--wz-version` are the same global options used by the inspection
commands. The generator does not read `Reactor.wz` or emit reactor tables
because WZ does not establish reactor-to-item relationships.

The destination is written atomically. An existing file is refused unless you
pass `--force`. On success, standard output contains a JSON generation report
with each detected archive version, row and source counts, categorized
omissions, warnings, and the output path. The generated TOML starts with a
provenance header.

### Separate facts from policy

The generator reads only these relationship and metadata facts:

- `String.wz/MonsterBook.img` supplies explicit mob-to-item associations.
- `String.wz/Mob.img` supplies optional display names.
- `Item.wz/Consume/0238.img` supplies exact Monster Book card-to-mob mappings.
- `Mob.wz` supplies level, EXP, and boss metadata.
- `Quest.wz/Check.img` supplies positive completion item and mob requirements.
- `Map.wz` validates explicitly authored source-map mob relationships.
- `Skill.wz` validates explicitly authored killing-skill requirements.
- `Item.wz` and `Character.wz` validate item sources and available `slotMax`.

It does not read another server's loot tables, global relationships, or rates.
`config/loot-policy.toml` owns every probability, boss multiplier, quantity
formula, card formula coefficient, meso formula coefficient, and explicit mob
drop selection. The tracked policy is an independently chosen 1x, v83-inspired
Oozems model. It is not a copy of Cosmic, HeavenMS, or another private-server
rate table.

The item rate classes are equipment, recovery, mobility, scroll, cure, ammo,
rechargeable, common mob material, ore/crafting, miscellaneous, and quest.
Consumable, material, and ammo quantities increase with mob level and are
clamped to a positive WZ `slotMax` when one is present. Equipment, scrolls,
rechargeables, and cards use the policy's single-item quantity rule.

Card chance, meso chance, and meso expected value use this deterministic integer
formula:

```text
min(maximum, base + level * per_level + isqrt(exp) * per_experience_sqrt)
```

Boss ratios are then applied with integer arithmetic. The meso policy converts
expected value to an inclusive minimum and maximum using
`spread_per_thousand`. These formulas are monotonic in level and EXP. Every mob
retained from the WZ associations or a validated card mapping receives one meso
row.

### Understand generated rows

Mob and item rows are sorted by mob ID, item ID, quest ID, and required skill
ID. Duplicate Monster Book associations collapse into one row. A validated card
mapping adds the source mob's card even when the Monster Book reward list omits
it. Any card in that reward list is replaced by the policy card chance.

Items in group `403` are emitted only when both conditions hold: the item has
an explicit Monster Book mob association, and at least one quest requires a
positive quantity at completion. One row is emitted for each requiring quest.
Unresolved quest items are omitted. Install, cash, pet, malformed, and missing
item or mob sources are also omitted and reported.

Tutorial and other progression relationships absent from Monster Book may be
authored as `[[mob_drops]]` policy rows. Each row states a mob, item, chance,
inclusive quantity range, and its WZ evidence. A formal `quest_id` requires
`Quest.wz/Check.img` to positively require both that mob and item. A
`source_map_id` requires `Map.wz` to contain the mob in that map. At least one of
these evidence fields is required. The quantity range must also fit the item's
WZ `slotMax` when one is present.

An optional `required_skill_id` restricts a row to kills made with that
`Skill.wz` skill. This supports the pirate second-job test, where Flash Fist and
Double Shot produce different crystals from the same OctoPirate. Policy-only
mobs do not receive automatic meso rows.

```toml
[[mob_drops]]
mob_id = 9300018
item_id = 4031802
quest_id = 1035
chance_per_million = 1000000
minimum_quantity = 1
maximum_quantity = 1

[[mob_drops]]
mob_id = 9001005
item_id = 4031856
source_map_id = 108000500
required_skill_id = 5001001
chance_per_million = 1000000
minimum_quantity = 1
maximum_quantity = 1
```

This is a conservative starting catalog, not a replacement for independently
authored source relationships that are absent from WZ. Replacing an existing
catalog can remove reactor tables and quest-item sources that WZ cannot prove.
Review the JSON report and record independently researched, WZ-verifiable quest
mob rows in the policy before generation.

The output uses item and meso rows accepted by the runtime:

```toml
[[mobs]]
mob_id = 100100

[[mobs.drops]]
item_id = 4000000
chance_per_million = 200000
minimum_quantity = 1
maximum_quantity = 2

[[mobs.drops]]
chance_per_million = 600000
minimum_mesos = 4
maximum_mesos = 12
```

All ranges are inclusive. An item row may have a `quest_id` or
`required_skill_id`. A meso row cannot.

### Author global drops

The tracked policy intentionally has no global rows because WZ does not prove a
global item relationship. A server owner can add explicit item and meso globals
to the policy:

```toml
[[global_drops]]
item_id = 4000000
chance_per_million = 500
minimum_quantity = 1
maximum_quantity = 1

[[global_drops]]
chance_per_million = 10000
minimum_mesos = 1
maximum_mesos = 5
```

Global item rows may also include `quest_id`. Unknown fields, invalid chances,
partial or inverted ranges, duplicate rows, and unavailable local item sources
are rejected or reported rather than guessed.

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
