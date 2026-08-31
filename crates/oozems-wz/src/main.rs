use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Context;
use anyhow::Result;
use clap::Parser;
use clap::Subcommand;
use clap::ValueEnum;
use clap::error::ErrorKind;
use oozems_wz::OpenOptions;
use oozems_wz::Region;
use oozems_wz::archive_info;
use oozems_wz::generate_loot;
use oozems_wz::get;
use oozems_wz::list;
use oozems_wz::open_archive;
use oozems_wz::set_value;
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Parser)]
#[command(
    name = "oozems-wz",
    about = "Inspect and safely edit WZ archives",
    version
)]
struct Cli {
    /// Encryption region to use when opening archives.
    #[arg(long, global = true, value_enum, default_value_t)]
    region: RegionArg,

    /// Expected WZ patch version. By default, the version is detected.
    #[arg(long = "wz-version", global = true)]
    wz_version: Option<i16>,

    /// Emit one-line JSON instead of indented JSON.
    #[arg(long, global = true)]
    compact: bool,

    #[command(subcommand)]
    command: Command,
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

#[derive(Debug, Subcommand)]
enum Command {
    /// Show archive format, version, and entry counts.
    Info { archive: PathBuf },

    /// List the immediate children of a directory, image, or property.
    #[command(alias = "ls")]
    List {
        archive: PathBuf,
        #[arg(default_value = "/")]
        path: String,
        /// Zero-based child offset.
        #[arg(long, default_value_t = 0)]
        offset: usize,
        /// Maximum number of children to return.
        #[arg(long, default_value_t = 100)]
        limit: usize,
    },

    /// Show one directory, image, or property.
    Get { archive: PathBuf, path: String },

    /// Set an existing scalar or vector property in a new archive.
    Set {
        archive: PathBuf,
        path: String,
        /// New value encoded as JSON. Strings must include JSON quotes.
        #[arg(long, allow_hyphen_values = true)]
        value: String,
        /// Destination archive. It must differ from the input archive.
        #[arg(short, long)]
        output: PathBuf,
        /// Atomically replace an existing destination file.
        #[arg(long)]
        force: bool,
    },

    /// Generate independent loot definitions from local WZ facts and a policy.
    GenerateLoot {
        /// Directory containing the matching WZ archives used by the policy.
        wz_data_directory: PathBuf,
        /// Oozems-authored rates, formulas, and quantity policy.
        #[arg(long)]
        policy: PathBuf,
        /// Destination loot TOML file.
        #[arg(short, long)]
        output: PathBuf,
        /// Atomically replace an existing destination file.
        #[arg(long)]
        force: bool,
    },
}

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) =>
        {
            return match error.print() {
                Ok(()) => ExitCode::SUCCESS,
                Err(_) => ExitCode::FAILURE,
            };
        }
        Err(error) => {
            eprintln!("{{\"error\":{}}}", json_string(&error.to_string()));
            return ExitCode::from(2);
        }
    };
    let compact = cli.compact;
    match run(cli).and_then(|output| write_json(&output, compact)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let message = format!("{error:#}");
            let fallback = format!("{{\"error\":{}}}", json_string(&message));
            eprintln!("{fallback}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<Value> {
    let options = OpenOptions {
        region: cli.region.into(),
        version: cli.wz_version,
    };

    match cli.command {
        Command::Info { archive } => {
            let archive = open_archive(&archive, options)?;
            serde_json::to_value(archive_info(&archive)).context("failed to encode archive info")
        }
        Command::List {
            archive,
            path,
            offset,
            limit,
        } => {
            let archive = open_archive(&archive, options)?;
            serde_json::to_value(list(&archive, &path, offset, limit)?)
                .context("failed to encode node list")
        }
        Command::Get { archive, path } => {
            let archive = open_archive(&archive, options)?;
            serde_json::to_value(get(&archive, &path)?).context("failed to encode WZ node")
        }
        Command::Set {
            archive,
            path,
            value,
            output,
            force,
        } => {
            let value = serde_json::from_str(&value)
                .with_context(|| format!("--value is not valid JSON: {value}"))?;
            serde_json::to_value(set_value(&archive, &output, &path, value, options, force)?)
                .context("failed to encode edit report")
        }
        Command::GenerateLoot {
            wz_data_directory,
            policy,
            output,
            force,
        } => serde_json::to_value(generate_loot(
            &wz_data_directory,
            &policy,
            &output,
            options,
            force,
        )?)
        .context("failed to encode loot generation report"),
    }
}

fn write_json(
    value: &impl Serialize,
    compact: bool,
) -> Result<()> {
    let stdout = std::io::stdout();
    let mut output = stdout.lock();
    if compact {
        serde_json::to_writer(&mut output, value)?;
    } else {
        serde_json::to_writer_pretty(&mut output, value)?;
    }
    use std::io::Write;
    writeln!(output)?;
    Ok(())
}

fn json_string(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| String::from("\"unknown error\""))
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    #[test]
    fn command_definition_is_valid() {
        Cli::command().debug_assert();
    }

    #[test]
    fn generate_loot_requires_policy_and_output() {
        let missing_policy = Cli::try_parse_from([
            "oozems-wz",
            "generate-loot",
            "data",
            "--output",
            "loot.toml",
        ])
        .expect_err("policy is required");
        assert_eq!(missing_policy.kind(), ErrorKind::MissingRequiredArgument);

        let parsed = Cli::try_parse_from([
            "oozems-wz",
            "--region",
            "gms",
            "--wz-version",
            "83",
            "generate-loot",
            "data",
            "--policy",
            "config/loot-policy.toml",
            "--output",
            "loot.toml",
            "--force",
        ])
        .expect("complete generator command");
        assert!(matches!(parsed.command, Command::GenerateLoot { .. }));
    }
}
