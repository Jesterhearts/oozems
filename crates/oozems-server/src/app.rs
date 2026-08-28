use std::path::Path;
use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::routing::get;
use axum::routing::post;
use tower_http::compression::CompressionLayer;
use tower_http::services::ServeDir;
use tower_http::services::ServeFile;
use tower_http::trace::TraceLayer;

use crate::attacks::BasicAttackCooldowns;
use crate::cash_shop::CashShopCatalog;
use crate::content::ContentCatalog;
use crate::database::Database;
use crate::effects::ActiveEffects;
use crate::experience::ExperienceCurves;
use crate::gameplay::GameplayConfig;
use crate::interactions::InteractionCatalog;
use crate::items::DropStore;
use crate::loot::LootCatalog;
use crate::mobs::MobStore;
use crate::movement::MovementTracker;
use crate::player_lock::PlayerLocks;
use crate::quest_scripts::QuestScriptCatalog;
use crate::recovery::RecoveryTimers;
use crate::skill_formula::FormulaCatalog;
use crate::skills::SkillCooldowns;

#[derive(Clone)]
pub struct AppState {
    pub catalog: Arc<ContentCatalog>,
    pub cash_shop: Arc<CashShopCatalog>,
    pub database: Database,
    pub experience: Arc<ExperienceCurves>,
    pub drops: Arc<DropStore>,
    pub gameplay: GameplayConfig,
    pub interactions: Arc<InteractionCatalog>,
    pub movement: Arc<MovementTracker>,
    pub mobs: Arc<MobStore>,
    pub player_locks: Arc<PlayerLocks>,
    pub quest_scripts: Arc<QuestScriptCatalog>,
    pub recovery_timers: Arc<RecoveryTimers>,
    pub basic_attack_cooldowns: Arc<BasicAttackCooldowns>,
    pub skill_cooldowns: Arc<SkillCooldowns>,
    pub active_effects: Arc<ActiveEffects>,
    pub formulas: Arc<FormulaCatalog>,
}

pub fn router(
    database: Database,
    catalog: ContentCatalog,
    cash_shop: CashShopCatalog,
    interactions: InteractionCatalog,
    quest_scripts: QuestScriptCatalog,
    loot: LootCatalog,
    experience: ExperienceCurves,
    gameplay: GameplayConfig,
    formulas: FormulaCatalog,
    public_dir: &Path,
) -> Router {
    let formulas = Arc::new(formulas);
    let drops = Arc::new(DropStore::new(gameplay.item_drop_despawn));
    let state = AppState {
        catalog: Arc::new(catalog),
        cash_shop: Arc::new(cash_shop),
        database,
        experience: Arc::new(experience),
        drops: drops.clone(),
        gameplay,
        interactions: Arc::new(interactions),
        movement: Arc::new(MovementTracker::default()),
        mobs: Arc::new(MobStore::new(
            gameplay.combat,
            formulas.clone(),
            Arc::new(loot),
            drops,
        )),
        player_locks: Arc::new(PlayerLocks::default()),
        quest_scripts: Arc::new(quest_scripts),
        recovery_timers: Arc::new(RecoveryTimers::default()),
        basic_attack_cooldowns: Arc::new(BasicAttackCooldowns::default()),
        skill_cooldowns: Arc::new(SkillCooldowns::default()),
        active_effects: Arc::new(ActiveEffects::default()),
        formulas,
    };
    let api = Router::new()
        .route("/bootstrap", post(crate::api::bootstrap))
        .route("/cash-shop/get", post(crate::api::cash_shop::get))
        .route("/cash-shop/purchase", post(crate::api::cash_shop::purchase))
        .route("/characters/create", post(crate::api::create_character))
        .route("/morphs/get", post(crate::api::get_morph))
        .route(
            "/characters/sprites",
            post(crate::api::get_character_sprites),
        )
        .route("/gui/get", post(crate::api::get_gui))
        .route(
            "/movement/rules",
            post(crate::api::movement::get_movement_rules),
        )
        .route(
            "/movement/submit",
            post(crate::api::movement::submit_movement),
        )
        .route("/movement/portal", post(crate::api::movement::enter_portal))
        .route("/skills/book", post(crate::api::get_skill_book))
        .route("/skills/allocate", post(crate::api::allocate_skill_point))
        .route(
            "/abilities/allocate",
            post(crate::api::allocate_ability_point),
        )
        .route("/skills/use", post(crate::api::use_skill))
        .route(
            "/combat/basic-attack",
            post(crate::api::combat::use_basic_attack),
        )
        .route("/players/recover", post(crate::api::recover_player))
        .route(
            "/players/respawn",
            post(crate::api::respawn::respawn_player),
        )
        .route("/maps/get", post(crate::api::get_map))
        .route("/items/equip", post(crate::api::equip_item))
        .route("/items/unequip", post(crate::api::unequip_item))
        .route("/items/drop", post(crate::api::drop_item))
        .route("/items/use", post(crate::api::use_item))
        .route("/items/pick-up", post(crate::api::pick_up_item))
        .route("/npcs/interact", post(crate::api::interactions::interact))
        .route("/players/save", post(crate::api::save_player))
        .layer(DefaultBodyLimit::max(64 * 1024));
    let public = ServeDir::new(public_dir)
        .append_index_html_on_directories(true)
        .not_found_service(ServeFile::new(public_dir.join("index.html")));
    Router::new()
        .nest("/api/v1", api)
        .route("/wz-assets/{asset_id}", get(crate::api::get_wz_asset))
        .fallback_service(public)
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
