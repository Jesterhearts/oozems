#![forbid(unsafe_code)]

mod api;
mod app;
mod config;
mod content;
mod database;
mod experience;
mod gameplay;
mod items;
mod keymap;
mod mobs;
mod movement;
mod player_lock;
mod recovery;
mod skill_formula;
mod skills;

use anyhow::Context;
use config::Config;
use content::ContentCatalog;
use content::ContentConfig;
use experience::ExperienceCurves;
use gameplay::GameplayConfig;
use skill_formula::FormulaCatalog;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();

    let config = Config::from_env()?;
    std::fs::create_dir_all(&config.data_dir).with_context(|| {
        format!(
            "failed to create data directory {}",
            config.data_dir.display()
        )
    })?;

    let experience = ExperienceCurves::load(&config.config_dir.join("xp-curves.toml"))?;
    let gameplay = GameplayConfig::load(&config.config_dir.join("gameplay.toml"))?;
    let content_config = ContentConfig::load(&config.config_dir.join("content.toml"))?;
    let formulas = FormulaCatalog::load(&config.config_dir.join("skill-formulas.toml"))?;
    info!(
        curve = experience.default_curve().name(),
        max_level = experience.default_curve().max_level(),
        "XP curve configuration ready"
    );
    info!(
        item_drop_despawn = %humantime::format_duration(gameplay.item_drop_despawn),
        initial_skill_points = gameplay.initial_skill_points,
        movement_snapshot_interval = %humantime::format_duration(gameplay.movement.snapshot_interval),
        movement_speed_cap = gameplay.movement.speed_cap,
        movement_jump_cap = gameplay.movement.jump_cap,
        combat_disengage_range = gameplay.combat.disengage_range,
        combat_attack_range = gameplay.combat.player_attack_range,
        combat_respawn = %humantime::format_duration(gameplay.combat.default_respawn),
        "gameplay configuration ready"
    );
    info!(
        formula_count = formulas.len(),
        profile_count = formulas.profile_count(),
        mapping_count = formulas.mapping_count(),
        source = formulas.source_url(),
        "formula profile configuration ready"
    );
    let catalog = ContentCatalog::load(&config.wz_dir, &content_config)?;
    let database = database::open_surreal_kv(&config.data_dir.join("surrealkv")).await?;
    let router = app::router(
        database,
        catalog,
        experience,
        gameplay,
        formulas,
        &config.public_dir,
    );
    let listener = tokio::net::TcpListener::bind(config.bind).await?;

    info!(address = %config.bind, "oozems server ready");
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("oozems_server=info,tower_http=info"));

    tracing_subscriber::fmt().with_env_filter(filter).init();
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to install shutdown signal handler");
    }
}
