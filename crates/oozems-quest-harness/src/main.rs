use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use clap::Args;
use clap::Parser;
use clap::Subcommand;
use clap::ValueEnum;
use oozems_quest_harness::auth;
use oozems_quest_harness::evidence;
use oozems_quest_harness::evidence::EvidenceBundle;
use oozems_quest_harness::evidence::EvidencePaths;
use oozems_quest_harness::provider;
use oozems_quest_harness::provider::CompletionConfig;
use oozems_quest_harness::provider::Message;
use oozems_quest_harness::provider::MessageRole;
use oozems_quest_harness::provider::ReasoningConfig;
use oozems_quest_harness::provider::ReasoningEffort;
use oozems_quest_harness::script;
use oozems_quest_harness::script::QuestPhase;
use oozems_wz::OpenOptions;
use oozems_wz::Region;
use serde::Serialize;

#[derive(Debug, Parser)]
#[command(
    name = "oozems-quest-harness",
    about = "Infer typed quest-script TOML through an OpenRouter-compatible model",
    version
)]
struct Cli {
    /// Encryption region to use when opening WZ archives.
    #[arg(long, global = true, value_enum, default_value_t)]
    region: RegionArg,

    /// Expected WZ patch version. By default, the version is detected.
    #[arg(long = "wz-version", global = true)]
    wz_version: Option<i16>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Authorize OpenRouter in a browser and store the resulting API key.
    Login,

    /// Remove the API key created by browser login.
    Logout,

    /// List quests that reference an external start or completion script.
    Quests {
        /// Quest.wz archive to inspect.
        quest_wz: PathBuf,

        /// Keep quests whose ID, name, or script name contains this text.
        #[arg(long)]
        search: Option<String>,
    },

    /// Print the automatically assembled model evidence without calling a
    /// model.
    Evidence {
        #[command(flatten)]
        quest: QuestInput,

        /// Write evidence JSON to this file instead of standard output.
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Infer script programs for one quest or every scripted quest.
    Generate {
        #[command(flatten)]
        input: GenerateInput,

        /// OpenRouter or compatible model identifier.
        #[arg(long)]
        model: String,

        /// Write validated TOML to this file instead of standard output.
        #[arg(short, long)]
        output: Option<PathBuf>,

        /// OpenAI-compatible API base URL.
        #[arg(long, default_value = provider::DEFAULT_BASE_URL)]
        base_url: String,

        /// Maximum number of model attempts after validation failures.
        #[arg(long, default_value_t = 2, value_parser = clap::value_parser!(u8).range(1..=5))]
        attempts: u8,

        /// Maximum completion tokens requested from the model.
        /// This budget includes reasoning tokens.
        #[arg(long, default_value_t = 16_384, value_parser = clap::value_parser!(u32).range(1..))]
        max_tokens: u32,

        /// Model reasoning effort. OpenRouter defaults to low; none disables it
        /// when supported.
        #[arg(long, value_enum)]
        reasoning_effort: Option<ReasoningEffortArg>,
    },
}

#[derive(Clone, Debug, Args)]
struct QuestInput {
    /// Quest.wz archive to inspect.
    quest_wz: PathBuf,

    /// Quest ID, exact script name, exact quest name, or unique name substring.
    quest: String,

    /// Restrict generation to one scripted phase.
    #[arg(long, value_enum)]
    phase: Option<PhaseArg>,

    /// Associated Npc.wz. Defaults to Npc.wz beside Quest.wz when present.
    #[arg(long)]
    npc_wz: Option<PathBuf>,

    /// Associated String.wz. Defaults to String.wz beside Quest.wz when
    /// present.
    #[arg(long)]
    string_wz: Option<PathBuf>,

    /// Optional UTF-8 notes to include with the extracted WZ evidence.
    #[arg(long)]
    notes: Option<PathBuf>,
}

#[derive(Clone, Debug, Args)]
struct GenerateInput {
    /// Quest.wz archive to inspect.
    quest_wz: PathBuf,

    /// Quest ID, exact script name, exact quest name, or unique name substring.
    #[arg(required_unless_present = "all", conflicts_with = "all")]
    quest: Option<String>,

    /// Generate every unique script referenced by the archive.
    #[arg(long, conflicts_with = "quest")]
    all: bool,

    /// Restrict generation to one scripted phase.
    #[arg(long, value_enum)]
    phase: Option<PhaseArg>,

    /// Associated Npc.wz. Defaults to Npc.wz beside Quest.wz when present.
    #[arg(long)]
    npc_wz: Option<PathBuf>,

    /// Associated String.wz. Defaults to String.wz beside Quest.wz when
    /// present.
    #[arg(long)]
    string_wz: Option<PathBuf>,

    /// Optional UTF-8 notes to include with the extracted WZ evidence.
    #[arg(long)]
    notes: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum PhaseArg {
    Start,
    Completion,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ReasoningEffortArg {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl From<ReasoningEffortArg> for ReasoningEffort {
    fn from(value: ReasoningEffortArg) -> Self {
        match value {
            ReasoningEffortArg::None => Self::None,
            ReasoningEffortArg::Minimal => Self::Minimal,
            ReasoningEffortArg::Low => Self::Low,
            ReasoningEffortArg::Medium => Self::Medium,
            ReasoningEffortArg::High => Self::High,
            ReasoningEffortArg::Xhigh => Self::Xhigh,
            ReasoningEffortArg::Max => Self::Max,
        }
    }
}

impl From<PhaseArg> for QuestPhase {
    fn from(value: PhaseArg) -> Self {
        match value {
            PhaseArg::Start => Self::Start,
            PhaseArg::Completion => Self::Completion,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, ValueEnum)]
enum RegionArg {
    #[default]
    Gms,
    Ems,
    Bms,
}

impl From<RegionArg> for Region {
    fn from(value: RegionArg) -> Self {
        match value {
            RegionArg::Gms => Region::Gms,
            RegionArg::Ems => Region::Ems,
            RegionArg::Bms => Region::Bms,
        }
    }
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    let wz_options = OpenOptions {
        region: cli.region.into(),
        version: cli.wz_version,
    };
    match cli.command {
        Command::Login => login(),
        Command::Logout => logout(),
        Command::Quests { quest_wz, search } => {
            list_quests(&quest_wz, search.as_deref(), wz_options)
        }
        Command::Evidence { quest, output } => {
            let evidence = load_evidence(&quest, wz_options)?;
            write_json(output.as_deref(), &evidence)
        }
        Command::Generate {
            input,
            model,
            output,
            base_url,
            attempts,
            max_tokens,
            reasoning_effort,
        } => generate(GenerateOptions {
            input,
            model,
            output,
            base_url,
            attempts,
            max_tokens,
            reasoning_effort,
            wz_options,
        }),
    }
}

fn login() -> Result<()> {
    let client = provider::client()?;
    let key = auth::browser_login(&client)?;
    let path = auth::credential_path()?;
    auth::save_key(&path, &key)?;
    eprintln!("OpenRouter authorization saved to {}", path.display());
    Ok(())
}

fn logout() -> Result<()> {
    let path = auth::credential_path()?;
    if auth::remove_key(&path)? {
        eprintln!("Removed OpenRouter authorization from {}", path.display());
    } else {
        eprintln!("No stored OpenRouter authorization was found");
    }
    Ok(())
}

fn list_quests(
    quest_wz: &Path,
    search: Option<&str>,
    options: OpenOptions,
) -> Result<()> {
    let search = search.map(str::to_lowercase);
    let quests = evidence::catalog(quest_wz, options)?
        .into_iter()
        .filter(|quest| {
            search.as_ref().is_none_or(|search| {
                quest.quest_id.to_string().contains(search)
                    || quest.name.to_lowercase().contains(search)
                    || quest
                        .scripts
                        .iter()
                        .any(|script| script.name.to_lowercase().contains(search))
            })
        })
        .collect::<Vec<_>>();
    write_json(None, &quests)
}

struct GenerateOptions {
    input: GenerateInput,
    model: String,
    output: Option<PathBuf>,
    base_url: String,
    attempts: u8,
    max_tokens: u32,
    reasoning_effort: Option<ReasoningEffortArg>,
    wz_options: OpenOptions,
}

fn generate(options: GenerateOptions) -> Result<()> {
    provider::completion_url(&options.base_url)?;
    if options.model.trim().is_empty() || options.model.trim() != options.model {
        bail!("model identifier cannot be empty or have surrounding whitespace");
    }
    let paths = evidence_paths(
        &options.input.quest_wz,
        options.input.npc_wz.as_ref(),
        options.input.string_wz.as_ref(),
        options.input.notes.as_ref(),
    );
    let source = evidence::open_source(&paths, options.wz_options)?;
    let phase = options.input.phase.map(Into::into);
    let selectors = generation_selectors(
        evidence::source_catalog(&source),
        options.input.quest.as_deref(),
        options.input.all,
        phase,
    )?;
    let total = if options.input.all {
        unique_script_count(evidence::source_catalog(&source), phase)
    } else {
        0
    };
    let api_key = resolve_api_key(&options.base_url)?;
    let client = provider::client()?;
    let config = CompletionConfig {
        base_url: &options.base_url,
        api_key: &api_key,
        model: &options.model,
        maximum_tokens: options.max_tokens,
        reasoning: reasoning_config(&options.base_url, options.reasoning_effort),
    };
    prepare_program_output(options.output.as_deref())?;
    let mut generated = 0;
    let mut seen_names = BTreeSet::<String>::new();
    let mut cache = evidence::EvidenceCache::default();
    for selector in selectors {
        let bundle = evidence::assemble_from_source(&source, &mut cache, &selector, phase)?;
        let evidence_value =
            serde_json::to_value(&bundle).context("failed to encode WZ evidence")?;
        for target in &bundle.scripts {
            if !seen_names.insert(target.name.clone()) {
                continue;
            }
            if options.input.all {
                eprintln!(
                    "Generating script {} of {total}: {} (quest {})",
                    generated + 1,
                    target.name,
                    bundle.quest_id
                );
            }
            let program = generate_program(
                &client,
                &config,
                &bundle,
                target.phase,
                &target.name,
                &evidence_value,
                options.attempts,
            )?;
            append_program(options.output.as_deref(), &program, generated > 0)?;
            generated += 1;
        }
    }
    Ok(())
}

fn generation_selectors(
    catalog: &[evidence::QuestSummary],
    quest: Option<&str>,
    all: bool,
    phase: Option<QuestPhase>,
) -> Result<Vec<String>> {
    if all {
        let selectors = catalog
            .iter()
            .filter(|quest| {
                quest
                    .scripts
                    .iter()
                    .any(|script| phase.is_none_or(|phase| script.phase == phase))
            })
            .map(|quest| quest.quest_id.to_string())
            .collect::<Vec<_>>();
        if selectors.is_empty() {
            bail!("archive has no scripts for the selected phase");
        }
        return Ok(selectors);
    }
    Ok(vec![
        quest
            .context("a quest selector or --all is required")?
            .to_owned(),
    ])
}

fn unique_script_count(
    catalog: &[evidence::QuestSummary],
    phase: Option<QuestPhase>,
) -> usize {
    catalog
        .iter()
        .flat_map(|quest| &quest.scripts)
        .filter(|script| phase.is_none_or(|phase| script.phase == phase))
        .map(|script| script.name.as_str())
        .collect::<BTreeSet<_>>()
        .len()
}

fn reasoning_config(
    base_url: &str,
    requested_effort: Option<ReasoningEffortArg>,
) -> Option<ReasoningConfig> {
    requested_effort
        .map(ReasoningEffort::from)
        .or_else(|| provider::is_default_openrouter(base_url).then_some(ReasoningEffort::Low))
        .map(|effort| ReasoningConfig {
            effort,
            exclude: true,
        })
}

fn generate_program(
    client: &reqwest::blocking::Client,
    config: &CompletionConfig<'_>,
    evidence: &EvidenceBundle,
    phase: QuestPhase,
    script_name: &str,
    evidence_value: &serde_json::Value,
    attempts: u8,
) -> Result<String> {
    let user_prompt = script::user_prompt(evidence.quest_id, phase, script_name, evidence_value)?;
    let mut messages = vec![
        Message {
            role: MessageRole::System,
            content: script::SYSTEM_PROMPT.to_owned(),
        },
        Message {
            role: MessageRole::User,
            content: user_prompt,
        },
    ];
    let mut last_error = None;
    for attempt in 1..=attempts {
        let response = provider::complete(client, config, &messages)?;
        match script::validate_output(&response, script_name, evidence.quest_id) {
            Ok(output) => return Ok(output),
            Err(error) => {
                let message = format!("{error:#}");
                last_error = Some(message.clone());
                if attempt < attempts {
                    messages.push(Message {
                        role: MessageRole::Assistant,
                        content: response,
                    });
                    messages.push(Message {
                        role: MessageRole::User,
                        content: script::correction_prompt(&message),
                    });
                }
            }
        }
    }
    bail!(
        "model did not return valid TOML for script {script_name:?} after {attempts} attempt(s): \
         {}",
        last_error.unwrap_or_else(|| "unknown validation error".to_owned())
    )
}

fn load_evidence(
    input: &QuestInput,
    options: OpenOptions,
) -> Result<EvidenceBundle> {
    let paths = evidence_paths(
        &input.quest_wz,
        input.npc_wz.as_ref(),
        input.string_wz.as_ref(),
        input.notes.as_ref(),
    );
    evidence::assemble(&paths, &input.quest, input.phase.map(Into::into), options)
}

fn evidence_paths(
    quest_wz: &Path,
    npc_wz: Option<&PathBuf>,
    string_wz: Option<&PathBuf>,
    notes: Option<&PathBuf>,
) -> EvidencePaths {
    EvidencePaths {
        quest_wz: quest_wz.to_owned(),
        npc_wz: npc_wz
            .cloned()
            .or_else(|| evidence::default_associated_archive(quest_wz, "Npc.wz")),
        string_wz: string_wz
            .cloned()
            .or_else(|| evidence::default_associated_archive(quest_wz, "String.wz")),
        notes: notes.cloned(),
    }
}

fn resolve_api_key(base_url: &str) -> Result<String> {
    if let Some(key) = env::var_os("OPENROUTER_API_KEY") {
        let key = key
            .into_string()
            .map_err(|_| anyhow::anyhow!("OPENROUTER_API_KEY is not valid UTF-8"))?;
        if key.trim() != key || key.is_empty() {
            bail!("OPENROUTER_API_KEY is empty or has surrounding whitespace");
        }
        return Ok(key);
    }
    if !provider::is_default_openrouter(base_url) {
        bail!(
            "OPENROUTER_API_KEY is required for a custom compatible endpoint; stored OpenRouter \
             browser credentials are not sent to custom servers"
        );
    }
    let path = auth::credential_path()?;
    auth::load_key(&path)?.with_context(|| {
        format!(
            "no OpenRouter credential is available; run `oozems-quest-harness login` or set \
             OPENROUTER_API_KEY (looked in {})",
            path.display()
        )
    })
}

fn write_json(
    path: Option<&Path>,
    value: &impl Serialize,
) -> Result<()> {
    let mut output = serde_json::to_string_pretty(value).context("failed to encode JSON output")?;
    output.push('\n');
    write_text(path, &output)
}

fn write_text(
    path: Option<&Path>,
    output: &str,
) -> Result<()> {
    match path {
        Some(path) => fs::write(path, output)
            .with_context(|| format!("failed to write output to {}", path.display())),
        None => {
            print!("{output}");
            Ok(())
        }
    }
}

fn prepare_program_output(path: Option<&Path>) -> Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    fs::File::create(path)
        .with_context(|| format!("failed to prepare output file {}", path.display()))?;
    Ok(())
}

fn append_program(
    path: Option<&Path>,
    program: &str,
    needs_separator: bool,
) -> Result<()> {
    let separator = if needs_separator { "\n" } else { "" };
    match path {
        Some(path) => {
            let mut file = fs::OpenOptions::new()
                .append(true)
                .open(path)
                .with_context(|| format!("failed to open output file {}", path.display()))?;
            write!(file, "{separator}{program}")
                .with_context(|| format!("failed to write output to {}", path.display()))?;
            file.flush()
                .with_context(|| format!("failed to flush output to {}", path.display()))
        }
        None => {
            print!("{separator}{program}");
            std::io::stdout()
                .flush()
                .context("failed to flush standard output")
        }
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn command_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn generate_requires_one_quest_selector_or_all() {
        assert!(
            Cli::try_parse_from([
                "oozems-quest-harness",
                "generate",
                "Quest.wz",
                "--model",
                "test-model",
            ])
            .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "oozems-quest-harness",
                "generate",
                "Quest.wz",
                "100",
                "--all",
                "--model",
                "test-model",
            ])
            .is_err()
        );
        let cli = Cli::try_parse_from([
            "oozems-quest-harness",
            "generate",
            "Quest.wz",
            "--all",
            "--model",
            "test-model",
        ])
        .expect("all quests command");
        assert!(matches!(
            cli.command,
            Command::Generate {
                input: GenerateInput { all: true, .. },
                max_tokens: 16_384,
                reasoning_effort: None,
                ..
            }
        ));
    }

    #[test]
    fn all_generation_selectors_filter_phases_and_count_unique_scripts() {
        let catalog = vec![
            evidence::QuestSummary {
                quest_id: 100,
                name: "First".to_owned(),
                scripts: vec![
                    evidence::ScriptReference {
                        phase: QuestPhase::Start,
                        name: "shared".to_owned(),
                    },
                    evidence::ScriptReference {
                        phase: QuestPhase::Completion,
                        name: "q100e".to_owned(),
                    },
                ],
            },
            evidence::QuestSummary {
                quest_id: 200,
                name: "Second".to_owned(),
                scripts: vec![evidence::ScriptReference {
                    phase: QuestPhase::Start,
                    name: "shared".to_owned(),
                }],
            },
        ];

        assert_eq!(
            generation_selectors(&catalog, None, true, Some(QuestPhase::Completion))
                .expect("completion selectors"),
            ["100"]
        );
        assert_eq!(unique_script_count(&catalog, None), 2);
        assert_eq!(unique_script_count(&catalog, Some(QuestPhase::Start)), 1);
    }

    #[test]
    fn program_output_is_written_incrementally_as_one_document() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("programs.toml");

        prepare_program_output(Some(&path)).expect("prepare output");
        append_program(Some(&path), "[[scripts]]\nname = \"one\"\n", false).expect("first program");
        append_program(Some(&path), "[[scripts]]\nname = \"two\"\n", true).expect("second program");

        assert_eq!(
            fs::read_to_string(path).expect("generated programs"),
            "[[scripts]]\nname = \"one\"\n\n[[scripts]]\nname = \"two\"\n"
        );
    }

    #[test]
    fn openrouter_defaults_to_low_excluded_reasoning() {
        assert_eq!(
            reasoning_config(provider::DEFAULT_BASE_URL, None),
            Some(ReasoningConfig {
                effort: ReasoningEffort::Low,
                exclude: true,
            })
        );
        assert_eq!(reasoning_config("http://localhost:11434/v1", None), None);
        assert_eq!(
            reasoning_config("http://localhost:11434/v1", Some(ReasoningEffortArg::None)),
            Some(ReasoningConfig {
                effort: ReasoningEffort::None,
                exclude: true,
            })
        );
    }
}
