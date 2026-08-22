use std::path::Path;
use std::sync::Arc;

use axum::Router;
use axum::extract::DefaultBodyLimit;
use axum::http::HeaderValue;
use axum::http::header;
use axum::routing::get;
use axum::routing::post;
use tower::ServiceBuilder;
use tower_http::compression::CompressionLayer;
use tower_http::services::ServeDir;
use tower_http::services::ServeFile;
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::trace::TraceLayer;

use crate::content::ContentCatalog;
use crate::database::Database;
use crate::experience::ExperienceCurves;
use crate::gameplay::GameplayConfig;
use crate::items::DropStore;
use crate::recovery::RecoveryTimers;
use crate::skill_formula::FormulaCatalog;
use crate::skills::SkillCooldowns;

#[derive(Clone)]
pub struct AppState {
    pub catalog: Arc<ContentCatalog>,
    pub database: Database,
    pub experience: Arc<ExperienceCurves>,
    pub drops: Arc<DropStore>,
    pub gameplay: GameplayConfig,
    pub recovery_timers: Arc<RecoveryTimers>,
    pub skill_cooldowns: Arc<SkillCooldowns>,
    pub formulas: Arc<FormulaCatalog>,
}

pub fn router(
    database: Database,
    catalog: ContentCatalog,
    experience: ExperienceCurves,
    gameplay: GameplayConfig,
    formulas: FormulaCatalog,
    public_dir: &Path,
    asset_dir: &Path,
) -> Router {
    let state = AppState {
        catalog: Arc::new(catalog),
        database,
        experience: Arc::new(experience),
        drops: Arc::new(DropStore::new(gameplay.item_drop_despawn)),
        gameplay,
        recovery_timers: Arc::new(RecoveryTimers::default()),
        skill_cooldowns: Arc::new(SkillCooldowns::default()),
        formulas: Arc::new(formulas),
    };
    let api = Router::new()
        .route("/bootstrap", post(crate::api::bootstrap))
        .route("/characters/create", post(crate::api::create_character))
        .route(
            "/characters/sprites",
            post(crate::api::get_character_sprites),
        )
        .route("/gui/get", post(crate::api::get_gui))
        .route("/skills/book", post(crate::api::get_skill_book))
        .route("/skills/allocate", post(crate::api::allocate_skill_point))
        .route("/skills/use", post(crate::api::use_skill))
        .route("/players/recover", post(crate::api::recover_player))
        .route("/maps/get", post(crate::api::get_map))
        .route("/items/equip", post(crate::api::equip_item))
        .route("/items/unequip", post(crate::api::unequip_item))
        .route("/items/drop", post(crate::api::drop_item))
        .route("/items/pick-up", post(crate::api::pick_up_item))
        .route("/players/save", post(crate::api::save_player))
        .layer(DefaultBodyLimit::max(64 * 1024));
    let public = ServeDir::new(public_dir)
        .append_index_html_on_directories(true)
        .not_found_service(ServeFile::new(public_dir.join("index.html")));
    let assets = ServiceBuilder::new()
        .layer(SetResponseHeaderLayer::if_not_present(
            header::CACHE_CONTROL,
            HeaderValue::from_static("public, max-age=31536000, immutable"),
        ))
        .service(ServeDir::new(asset_dir));

    Router::new()
        .nest("/api/v1", api)
        .route("/wz-assets/{asset_id}", get(crate::api::get_wz_asset))
        .nest_service("/assets", assets)
        .fallback_service(public)
        .layer(CompressionLayer::new())
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}
