# oozems-quest-harness

`oozems-quest-harness` asks an OpenRouter-compatible chat model to infer typed
`quest-scripts.toml` programs directly from a Quest.wz archive. It discovers the
quest ID, scripted phase, and exact WZ script name, assembles related archive
evidence, and validates every model response against the subset accepted by the
Oozems server before writing TOML.

For a selected quest, the harness includes its complete `Check.img`, `Act.img`,
`Say.img`, and `QuestInfo.img` branches. When `Npc.wz` and `String.wz` are beside
`Quest.wz`, it also includes metadata and text for referenced NPCs and text for
referenced items. Paths for those associated archives can be overridden.

## OpenRouter login

Authorize through OpenRouter using a localhost PKCE callback:

```sh
cargo run --package oozems-quest-harness -- login
```

The command opens the system browser and stores the resulting user-controlled
API key under the user's configuration directory with private file permissions
on Unix. The key is not written inside the repository. Remove it with:

```sh
cargo run --package oozems-quest-harness -- logout
```

`OPENROUTER_API_KEY` takes precedence over the stored key.

## Find a quest

List every quest with a `startscript` or `endscript`:

```sh
cargo run --package oozems-quest-harness -- quests data/Quest.wz
```

Narrow the JSON list by quest ID, quest name, or script name:

```sh
cargo run --package oozems-quest-harness -- quests \
  data/Quest.wz --search q10272e
```

The selector accepted by `evidence` and `generate` can be a numeric quest ID,
an exact script name, an exact quest name, or a unique case-insensitive quest
name substring.

## Inspect evidence

Inspect exactly what a model would receive without logging in or making an API
request:

```sh
cargo run --package oozems-quest-harness -- evidence \
  data/Quest.wz q10272e \
  --output q10272e-evidence.json
```

`Npc.wz` and `String.wz` are found beside `Quest.wz` by default. Use `--npc-wz`
or `--string-wz` when they are elsewhere. Use `--notes` to add a UTF-8 evidence
file containing research that is not present in the archives.

## Generate programs

```sh
cargo run --package oozems-quest-harness -- generate \
  data/Quest.wz \
  q10272e \
  --model openai/gpt-5.2 \
  --output q10272e.toml
```

If the selected quest references both a start and completion script, the
harness generates both programs and combines them into one valid TOML document.
Use `--phase start` or `--phase completion` to select only one.

Generate every unique script referenced by the archive with `--all`:

```sh
cargo run --package oozems-quest-harness -- generate \
  data/Quest.wz \
  --all \
  --model openai/gpt-5.2 \
  --output generated-quest-scripts.toml
```

Quests are processed in ascending ID order. Shared script names are generated
only once, and `--phase` can restrict the batch to start or completion scripts.
Progress is written to standard error. Each validated program is appended to
the output immediately, so programs completed before a later failure remain in
the output file. The output file is replaced when a new run starts.

`--all` can make hundreds of paid requests. The bundled v83 archive currently
contains 680 unique script names. The server loads quests that reference 663 of
those names; generated programs for the remaining unsupported quests are valid
configuration entries but are ignored at runtime. Review the model and
provider's pricing before starting a complete batch.

The harness makes up to two model calls per program by default. If the first
response is invalid, the second call includes the validation error and asks for
a complete correction. `--attempts` changes this limit.

### Token and reasoning budgets

`--max-tokens` controls the total completion budget and defaults to 16,384.
Reasoning tokens and visible TOML both consume this budget. For OpenRouter, the
harness requests low reasoning effort by default and excludes reasoning text
from the response because the validator does not use it. Excluding the text does
not reduce token usage.

Disable reasoning on models that support it when the task does not need it:

```sh
cargo run --package oozems-quest-harness -- generate \
  data/Quest.wz q10272e \
  --model openai/gpt-5.2 \
  --reasoning-effort none \
  --max-tokens 16384
```

Available efforts are `none`, `minimal`, `low`, `medium`, `high`, `xhigh`, and
`max`. Some models require reasoning and may reject `none`; use `minimal` or
`low` for those models. Increase `--max-tokens` when the provider reports that
the completion reached its token limit.

The output is one standalone program:

```toml
[[scripts]]
name = "q10272e"

[[scripts.actions]]
type = "experience"
amount = 100
```

Review generated guesses before adding them to a deployed
`quest-scripts.toml`. Local validation can enforce the supported shape and
internal constraints, but it cannot establish that guessed behavior is
historically correct or that every referenced item and quest exists in the
target archive.

## Compatible endpoints

Set `--base-url` to an OpenAI-compatible Chat Completions API base URL. The
provider must accept `POST /chat/completions`, bearer authentication,
`messages`, `model`, `temperature`, and `max_tokens`.

```sh
OPENROUTER_API_KEY=local-token \
  oozems-quest-harness generate \
    --base-url http://localhost:11434/v1 \
    --model local-model \
    data/Quest.wz \
    q10272e
```

For custom endpoints, `OPENROUTER_API_KEY` is required even if the endpoint
ignores its value. The browser-created OpenRouter key is deliberately never
sent to a custom URL. The harness omits the OpenRouter `reasoning` object for
custom endpoints unless `--reasoning-effort` is explicitly supplied.
