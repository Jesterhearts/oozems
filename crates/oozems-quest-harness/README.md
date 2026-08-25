# oozems-quest-harness

`oozems-quest-harness` uses an OpenRouter-compatible chat model to infer
deterministic Oozems quest script replacements from local WZ evidence. It writes
typed `quest-scripts.toml` programs that use the subset supported by the Oozems
server.

## Read these warnings first

The `generate` command can make paid API requests. Check the model and
provider's prices before you run it. A validation failure can cause another
request, and `--all` can make hundreds of requests.

Treat every generated program as a draft. The harness checks the TOML shape and
the server's internal constraints, but it cannot confirm historical accuracy.
It also cannot confirm that every referenced item and quest exists in your
target archive. Review every generated guess before you deploy it.

Browser login stores an OpenRouter API key in your user configuration
directory. It does not write the key inside this repository. On Unix, it gives
the directory and key private permissions.

## Follow the shortest workflow

For a known quest selector, first inspect the evidence without logging in or
making an API request:

```sh
cargo run --package oozems-quest-harness -- evidence \
  data/Quest.wz q10272e \
  --output q10272e-evidence.json
```

Then authorize OpenRouter. You can skip this command if you set
`OPENROUTER_API_KEY`:

```sh
cargo run --package oozems-quest-harness -- login
```

Generate a replacement:

```sh
cargo run --package oozems-quest-harness -- generate \
  data/Quest.wz \
  q10272e \
  --model openai/gpt-5.2 \
  --output q10272e.toml
```

Finally, compare `q10272e.toml` with the local evidence. Check all IDs, amounts,
conditions, actions, and dialogue before you add it to `quest-scripts.toml`.

## Choose the WZ archives

Pass the local `Quest.wz` archive to `quests`, `evidence`, or `generate`. The
harness uses the GMS encryption region by default and detects the WZ patch
version. Use the global `--region` option to select `gms`, `ems`, or `bms`. Use
the global `--wz-version` option when you need to provide the expected patch
version.

For a selected quest, the model evidence contains its complete `Check.img`,
`Act.img`, and `QuestInfo.img` branches. It also contains the quest's `Say.img`
branch when one is present.

The harness looks for `Npc.wz` and `String.wz` beside `Quest.wz`. When those
archives are available, it adds metadata and text for referenced NPCs. It also
adds text for referenced items.

Use `--npc-wz` or `--string-wz` if either associated archive is elsewhere. Use
`--notes` to add a UTF-8 evidence file with research that is not in the
archives.

## Find a quest

List every quest that has a `startscript` or `endscript`:

```sh
cargo run --package oozems-quest-harness -- quests data/Quest.wz
```

The command writes a JSON list. Use `--search` to keep entries whose quest ID,
quest name, or script name contains the search text:

```sh
cargo run --package oozems-quest-harness -- quests \
  data/Quest.wz --search q10272e
```

The `evidence` and `generate` commands accept these quest selectors:

- A numeric quest ID.
- An exact script name that identifies one quest.
- An exact quest name.
- A unique, case-insensitive substring of a quest name.

If a selector matches more than one quest, use a numeric quest ID to resolve
the ambiguity.

## Inspect the evidence

Inspect the exact evidence that the harness would send to the model. This
command does not require a credential and does not make an API request:

```sh
cargo run --package oozems-quest-harness -- evidence \
  data/Quest.wz q10272e \
  --output q10272e-evidence.json
```

Use `--phase start` or `--phase completion` to inspect one scripted phase. If
you omit `--output`, the command writes the evidence JSON to standard output.
The archive and notes options described above also apply to this command.

## Authorize OpenRouter

The login command opens your system browser. It uses a localhost callback and
Proof Key for Code Exchange (PKCE). PKCE ties the returned authorization code
to the login process that started it.

```sh
cargo run --package oozems-quest-harness -- login
```

OpenRouter returns a user-controlled API key. The harness stores it under your
user configuration directory.

`OPENROUTER_API_KEY` takes precedence over the stored key. Remove the stored
key with:

```sh
cargo run --package oozems-quest-harness -- logout
```

## Use a compatible endpoint

You can skip this section when you use the default OpenRouter endpoint.

Before you use a custom endpoint, set `OPENROUTER_API_KEY`. The variable is
required even if the endpoint ignores its value. The harness never sends a
browser-created OpenRouter key to a custom URL.

Set `--base-url` to the base URL of an OpenAI-compatible Chat Completions API.
The harness sends `POST /chat/completions` with bearer authentication. The
provider must accept `messages`, `model`, `temperature`, and `max_tokens`. The
request sets `stream` to `false`.

```sh
OPENROUTER_API_KEY=local-token \
  oozems-quest-harness generate \
    --base-url http://localhost:11434/v1 \
    --model local-model \
    data/Quest.wz \
    q10272e
```

For custom endpoints, the harness omits the OpenRouter `reasoning` object unless
you explicitly set `--reasoning-effort`.

## Generate one quest

Check the cost and historical-accuracy warnings above before you make a model
request.

```sh
cargo run --package oozems-quest-harness -- generate \
  data/Quest.wz \
  q10272e \
  --model openai/gpt-5.2 \
  --output q10272e.toml
```

If the quest references both a start script and a completion script, the
harness generates both programs. It combines them into one valid TOML
document. Use `--phase start` or `--phase completion` to generate only one
phase.

Each successful model response is one standalone program. For example:

```toml
[[scripts]]
name = "q10272e"

[[scripts.actions]]
type = "experience"
amount = 100
```

## Generate a batch

Review provider pricing before you use `--all`. The bundled v83 archive
currently contains 680 unique script names, so a complete batch can make
hundreds of paid requests. By default, validation retries can use up to two
model calls for each program.

The server loads quests that reference 663 of those 680 names. Programs for the
remaining unsupported quests are valid configuration entries, but the server
ignores them at runtime.

If the output file already exists, the harness replaces it once after the batch
finishes.

```sh
cargo run --package oozems-quest-harness -- generate \
  data/Quest.wz \
  --all \
  --parallel 4 \
  --model openai/gpt-5.2 \
  --output generated-quest-scripts.toml
```

The batch processes quests in ascending ID order. It generates a shared script
name only once. Use `--phase` to restrict the batch to start scripts or
completion scripts.

`--parallel` sets the maximum number of concurrent model requests. It accepts
values from 1 through 256 and defaults to 1.

The harness writes progress and individual script failures to standard error.
It ignores a failed script and continues with the remaining requests. It merges
successful responses in archive order.

## Control retries and tokens

The harness makes up to two model calls per program by default. If the first
response fails local validation, the second request includes the validation
error and asks for a complete correction. Use `--attempts` to set the limit
from 1 through 5.

`--max-tokens` sets the total completion budget and defaults to 16,384.
Reasoning tokens and visible TOML both use this budget.

For OpenRouter, the harness requests low reasoning effort by default. It
excludes reasoning text from the response because the validator does not use
it. Excluding the text does not reduce token use.

Disable reasoning on models that support this setting when the task does not
need it:

```sh
cargo run --package oozems-quest-harness -- generate \
  data/Quest.wz q10272e \
  --model openai/gpt-5.2 \
  --reasoning-effort none \
  --max-tokens 16384
```

The available efforts are `none`, `minimal`, `low`, `medium`, `high`, `xhigh`,
and `max`. Some models require reasoning and may reject `none`. Use `minimal` or
`low` for those models.

Increase `--max-tokens` if the provider reports that the completion reached its
token limit. You can also reduce `--reasoning-effort`.

## Review the validated output

The harness asks the model for only behavior that is missing from ordinary WZ
checks, dialogue, and actions. It validates every response against the TOML
subset and internal limits that the Oozems server accepts.

This validation proves only that the program has a supported structure. It
does not prove that the model inferred the original external script correctly.
It does not verify referenced IDs against the target archive. Compare the
result with the evidence and other historical sources before deployment.
