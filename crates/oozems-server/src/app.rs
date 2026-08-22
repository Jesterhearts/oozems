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

#[derive(Clone)]
pub struct AppState {
    pub catalog: Arc<ContentCatalog>,
    pub database: Database,
}

pub fn router(
    database: Database,
    catalog: ContentCatalog,
    public_dir: &Path,
    asset_dir: &Path,
) -> Router {
    let state = AppState {
        catalog: Arc::new(catalog),
        database,
    };
    let api = Router::new()
        .route("/bootstrap", post(crate::api::bootstrap))
        .route("/characters/create", post(crate::api::create_character))
        .route(
            "/characters/sprites",
            post(crate::api::get_character_sprites),
        )
        .route("/gui/get", post(crate::api::get_gui))
        .route("/maps/get", post(crate::api::get_map))
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
