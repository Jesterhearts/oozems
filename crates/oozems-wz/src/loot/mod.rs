use std::path::Path;

use anyhow::Result;
pub use model::GenerationReport;

use crate::OpenOptions;

mod model;
mod output;
mod policy;
mod source;

pub fn generate_loot(
    wz_data_directory: &Path,
    policy_path: &Path,
    output_path: &Path,
    options: OpenOptions,
    force: bool,
) -> Result<GenerationReport> {
    if output_path.exists() && !force {
        anyhow::bail!(
            "output already exists: {} (use --force to replace it)",
            output_path.display()
        );
    }
    let policy = policy::load(policy_path)?;
    let source = source::load(wz_data_directory, options, &policy)?;
    let mut diagnostics = source.diagnostics;
    diagnostics.warn(
        "Reactor.wz does not establish reactor-to-item relationships; no reactor tables are \
         generated",
    );
    let catalog = model::generate(&source.facts, &policy, &mut diagnostics);
    let counts = model::report_counts(&source.facts, &catalog);
    let contents = output::render(&catalog, options.region, &source.versions);
    output::write_atomic(output_path, &contents, force)?;

    Ok(GenerationReport {
        source_region: options.region,
        requested_wz_version: options.version,
        source_versions: source.versions,
        policy_name: policy.policy_name,
        counts,
        omissions: diagnostics.omissions,
        warnings: diagnostics.warnings,
        output_path: output_path.to_owned(),
    })
}
