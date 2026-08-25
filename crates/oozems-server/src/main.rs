#![forbid(unsafe_code)]

use std::collections::BTreeSet;

mod api;
mod app;
mod attacks;
mod config;
mod content;
mod database;
mod effects;
mod experience;
mod gameplay;
mod interactions;
mod items;
mod jobs;
mod keymap;
mod loot;
mod mobs;
mod monster_book;
mod movement;
mod player_lock;
mod quest_records;
mod quest_scripts;
mod quests;
mod random;
mod recovery;
mod skill_formula;
mod skills;

use anyhow::Context;
use config::Config;
use content::ContentCatalog;
use content::ContentConfig;
use experience::ExperienceCurves;
use gameplay::GameplayConfig;
use interactions::InteractionCatalog;
use loot::LootCatalog;
use quest_scripts::QuestScriptCatalog;
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
        initial_map_id = gameplay.initial_map_id,
        world_id = gameplay.world_id,
        movement_snapshot_interval = %humantime::format_duration(gameplay.movement.snapshot_interval),
        movement_speed_cap = gameplay.movement.speed_cap,
        movement_jump_cap = gameplay.movement.jump_cap,
        combat_disengage_range = gameplay.combat.disengage_range,
        combat_attack_range = gameplay.combat.player_attack_range,
        combat_player_attack_interval = %humantime::format_duration(gameplay.combat.player_attack_interval),
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
    let mut catalog = ContentCatalog::load(&config.wz_dir, &content_config)?;
    if catalog.get_map(gameplay.initial_map_id)?.is_none() {
        anyhow::bail!(
            "configured initial character map {} does not exist",
            gameplay.initial_map_id
        );
    }
    let interactions =
        InteractionCatalog::load(&config.data_dir.join("interactions.toml"), &catalog)?;
    catalog.project_item_definitions(&interactions.item_reference_ids().collect())?;
    let empty_script_references = BTreeSet::new();
    let archive_script_references = catalog
        .quest_script_reference_names()
        .unwrap_or(&empty_script_references);
    let quest_scripts = QuestScriptCatalog::load(
        &config.data_dir.join("quest-scripts.toml"),
        catalog.quest_definitions(),
        archive_script_references,
        &catalog,
    )?;
    catalog.project_item_definitions(quest_scripts.item_reference_ids())?;
    info!(
        program_count = quest_scripts.len(),
        ignored_program_count = quest_scripts.ignored_len(),
        "quest script configuration ready"
    );
    let loot = LootCatalog::load(&config.data_dir.join("loot.toml"), &catalog)?;
    catalog.project_item_definitions(&loot.item_reference_ids().collect())?;
    info!(table_count = loot.len(), "mob loot configuration ready");
    let database = database::open_surreal_kv(&config.data_dir.join("surrealkv")).await?;
    let router = app::router(
        database,
        catalog,
        interactions,
        quest_scripts,
        loot,
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
