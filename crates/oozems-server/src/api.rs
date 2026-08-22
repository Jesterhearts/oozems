use axum::body::Bytes;
use axum::extract::Path as AxumPath;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::IntoResponse;
use axum::response::Response;
use oozems_proto::PROTOBUF_CONTENT_TYPE;
use oozems_proto::v1::BootstrapRequest;
use oozems_proto::v1::BootstrapResponse;
use oozems_proto::v1::CreateCharacterRequest;
use oozems_proto::v1::CreateCharacterResponse;
use oozems_proto::v1::DropItemRequest;
use oozems_proto::v1::EquipItemRequest;
use oozems_proto::v1::ErrorResponse;
use oozems_proto::v1::GetCharacterSpritesRequest;
use oozems_proto::v1::GetCharacterSpritesResponse;
use oozems_proto::v1::GetGuiRequest;
use oozems_proto::v1::GetGuiResponse;
use oozems_proto::v1::GetMapRequest;
use oozems_proto::v1::GetMapResponse;
use oozems_proto::v1::GetSkillBookRequest;
use oozems_proto::v1::GetSkillBookResponse;
use oozems_proto::v1::ItemActionResponse;
use oozems_proto::v1::PickUpItemRequest;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::SavePlayerRequest;
use oozems_proto::v1::SavePlayerResponse;
use oozems_proto::v1::UnequipItemRequest;
use oozems_proto::v1::Vec2;
use prost::Message;
use thiserror::Error;

use crate::app::AppState;
use crate::database::CharacterName;
use crate::database::PlayerId;
use crate::database::STARTER_MAP_ID;

pub async fn bootstrap(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<BootstrapResponse>, ApiError> {
    let request: BootstrapRequest = decode_request(&headers, body)?;
    let player_id = PlayerId::parse(&request.player_id)
        .map_err(|error| ApiError::bad_request("invalid_player_id", error.to_string()))?;
    let player = load_player(&state, &player_id)
        .await?
        .filter(|player| player.appearance.is_some());

    Ok(Protobuf(BootstrapResponse {
        player,
        creation_options: Some(state.catalog.character_creation_options()),
    }))
}

pub async fn create_character(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<CreateCharacterResponse>, ApiError> {
    let request: CreateCharacterRequest = decode_request(&headers, body)?;
    let player_id = PlayerId::parse(&request.player_id)
        .map_err(|error| ApiError::bad_request("invalid_player_id", error.to_string()))?;
    let name = CharacterName::parse(&request.name)
        .map_err(|error| ApiError::bad_request("invalid_character_name", error.to_string()))?;
    let appearance = request.appearance.ok_or_else(|| {
        ApiError::bad_request(
            "missing_appearance",
            "request does not contain an appearance",
        )
    })?;
    if !state.catalog.supports_character(&appearance) {
        return Err(ApiError::bad_request(
            "unsupported_appearance",
            "the selected character appearance is not available",
        ));
    }
    if load_player(&state, &player_id)
        .await?
        .is_some_and(|player| player.appearance.is_some())
    {
        return Err(ApiError::conflict(
            "character_exists",
            "this player already has a character",
        ));
    }

    let map = load_map(&state, STARTER_MAP_ID).await?.ok_or_else(|| {
        ApiError::not_found(
            "starter_map_not_found",
            format!("starter map {STARTER_MAP_ID} does not exist"),
        )
    })?;
    let position = starter_position(&map);
    let experience_required =
        crate::experience::required_for_level(state.experience.default_curve(), 1)?;
    let player = crate::database::create_player(
        &state.database,
        &player_id,
        &name,
        appearance,
        position,
        experience_required,
    )
    .await?;

    Ok(Protobuf(CreateCharacterResponse {
        player: Some(player),
    }))
}

pub async fn get_character_sprites(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<GetCharacterSpritesResponse>, ApiError> {
    let request: GetCharacterSpritesRequest = decode_request(&headers, body)?;
    let appearance = request.appearance.ok_or_else(|| {
        ApiError::bad_request(
            "missing_appearance",
            "request does not contain an appearance",
        )
    })?;
    let equipment = if request.use_starter_equipment {
        crate::items::starter_inventory().equipment
    } else {
        request.equipment
    };
    let catalog = state.catalog.clone();
    let sprites =
        tokio::task::spawn_blocking(move || catalog.get_character_sprites(&appearance, &equipment))
            .await??
            .ok_or_else(|| {
                ApiError::bad_request(
                    "unsupported_appearance",
                    "the selected character appearance is not available",
                )
            })?;

    Ok(Protobuf(GetCharacterSpritesResponse {
        sprites: Some(sprites),
    }))
}

pub async fn get_map(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<GetMapResponse>, ApiError> {
    let request: GetMapRequest = decode_request(&headers, body)?;
    let mut map = load_map(&state, request.map_id).await?.ok_or_else(|| {
        ApiError::not_found(
            "map_not_found",
            format!("map {} does not exist", request.map_id),
        )
    })?;
    map.dropped_items = crate::items::map_drops(&state.drops, map.id)?;

    Ok(Protobuf(GetMapResponse { map: Some(map) }))
}

pub async fn equip_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<ItemActionResponse>, ApiError> {
    let request: EquipItemRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let current = require_player(&state, &player_id).await?;
    let updated = crate::items::equip_inventory_item(
        current,
        request.inventory_index,
        &state.catalog.item_definitions(),
    )
    .map_err(item_rule_error)?;
    let player = crate::database::save_player(&state.database, &updated).await?;

    Ok(Protobuf(ItemActionResponse {
        player: Some(player),
        dropped_item: None,
        picked_up_drop_id: String::new(),
    }))
}

pub async fn unequip_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<ItemActionResponse>, ApiError> {
    let request: UnequipItemRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let current = require_player(&state, &player_id).await?;
    let updated = crate::items::unequip_item(current, request.slot).map_err(item_rule_error)?;
    let player = crate::database::save_player(&state.database, &updated).await?;

    Ok(Protobuf(ItemActionResponse {
        player: Some(player),
        dropped_item: None,
        picked_up_drop_id: String::new(),
    }))
}

pub async fn drop_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<ItemActionResponse>, ApiError> {
    let request: DropItemRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let current = require_player(&state, &player_id).await?;
    let original = current.clone();
    let removed = crate::items::remove_inventory_item(
        current,
        request.inventory_index,
        &state.catalog.item_definitions(),
    )
    .map_err(item_rule_error)?;
    let player = crate::database::save_player(&state.database, &removed.player).await?;
    let dropped_item = match crate::items::create_drop(&state.drops, &removed) {
        Ok(drop) => drop,
        Err(error) => {
            if let Err(rollback_error) =
                crate::database::save_player(&state.database, &original).await
            {
                tracing::error!(%rollback_error, "failed to restore an item after a drop-store error");
            }
            return Err(error.into());
        }
    };

    Ok(Protobuf(ItemActionResponse {
        player: Some(player),
        dropped_item: Some(dropped_item),
        picked_up_drop_id: String::new(),
    }))
}

pub async fn pick_up_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<ItemActionResponse>, ApiError> {
    let request: PickUpItemRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let current = require_player(&state, &player_id).await?;
    if current.map_id != request.map_id {
        return Err(ApiError::bad_request(
            "invalid_item_action",
            "the pickup map does not match the player's map",
        ));
    }
    let position = request.position.ok_or_else(|| {
        ApiError::bad_request(
            "missing_position",
            "pickup request does not contain a position",
        )
    })?;
    let map = load_map(&state, current.map_id).await?.ok_or_else(|| {
        ApiError::not_found(
            "map_not_found",
            format!("map {} does not exist", current.map_id),
        )
    })?;
    if !position.x.is_finite()
        || !position.y.is_finite()
        || position.x < 0.0
        || position.x > map.width as f32
        || position.y < 0.0
        || position.y > map.height as f32
    {
        return Err(ApiError::bad_request(
            "invalid_position",
            "pickup position must be finite and inside the current map",
        ));
    }
    let picked =
        crate::items::pick_up_nearest(&state.drops, current, position).map_err(pick_up_error)?;
    let map_id = picked.player.map_id;
    let player = match crate::database::save_player(&state.database, &picked.player).await {
        Ok(player) => player,
        Err(error) => {
            if let Err(restore_error) =
                crate::items::restore_drop(&state.drops, map_id, picked.drop.clone())
            {
                tracing::error!(%restore_error, "failed to restore an item after a player save error");
            }
            return Err(error.into());
        }
    };

    Ok(Protobuf(ItemActionResponse {
        player: Some(player),
        dropped_item: None,
        picked_up_drop_id: picked.drop.id,
    }))
}

pub async fn get_gui(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<GetGuiResponse>, ApiError> {
    let _: GetGuiRequest = decode_request(&headers, body)?;
    Ok(Protobuf(GetGuiResponse {
        gui: Some(state.catalog.game_gui()),
    }))
}

pub async fn get_skill_book(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<GetSkillBookResponse>, ApiError> {
    let request: GetSkillBookRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let player = require_player(&state, &player_id).await?;
    let job_id = player.stats.as_ref().map_or(0, |stats| stats.job_id);
    let catalog = state.catalog.clone();
    let skill_book = tokio::task::spawn_blocking(move || catalog.skill_book(job_id)).await??;

    Ok(Protobuf(GetSkillBookResponse {
        skill_book: Some(skill_book),
    }))
}

pub async fn save_player(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<SavePlayerResponse>, ApiError> {
    let request: SavePlayerRequest = decode_request(&headers, body)?;
    let requested = request.player.ok_or_else(|| {
        ApiError::bad_request("missing_player", "request does not contain a player")
    })?;
    validate_position(&requested)?;
    crate::keymap::validate_bindings(&requested.key_bindings)
        .map_err(|error| ApiError::bad_request("invalid_key_bindings", error.to_string()))?;

    let player_id = PlayerId::parse(&requested.id)
        .map_err(|error| ApiError::bad_request("invalid_player_id", error.to_string()))?;
    let map = load_map(&state, requested.map_id).await?.ok_or_else(|| {
        ApiError::bad_request(
            "invalid_map",
            format!("map {} does not exist", requested.map_id),
        )
    })?;
    let current = load_player(&state, &player_id)
        .await?
        .filter(|player| player.appearance.is_some())
        .ok_or_else(|| ApiError::not_found("player_not_found", "player does not exist"))?;
    let updated =
        crate::database::apply_player_movement(current, &requested, map.width, map.height);
    let player = crate::database::save_player(&state.database, &updated).await?;

    Ok(Protobuf(SavePlayerResponse {
        player: Some(player),
    }))
}

pub async fn get_wz_asset(
    State(state): State<AppState>,
    AxumPath(requested_id): AxumPath<String>,
) -> Result<Response, ApiError> {
    let version = requested_id
        .strip_suffix(".png")
        .filter(|value| value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| ApiError::not_found("asset_not_found", "asset does not exist"))?;
    let asset_id = format!("wz-{version}");
    let asset = state
        .catalog
        .get_wz_asset(&asset_id)
        .ok_or_else(|| ApiError::not_found("asset_not_found", "asset does not exist"))?;
    let bytes = tokio::task::spawn_blocking(move || asset.png_bytes())
        .await?
        .map_err(crate::content::ContentError::from)?;

    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "public, max-age=31536000, immutable"),
        ],
        Bytes::from_owner(bytes),
    )
        .into_response())
}

async fn load_map(
    state: &AppState,
    map_id: u32,
) -> Result<Option<oozems_proto::v1::Map>, ApiError> {
    let catalog = state.catalog.clone();
    Ok(tokio::task::spawn_blocking(move || catalog.get_map(map_id)).await??)
}

async fn load_player(
    state: &AppState,
    player_id: &PlayerId,
) -> Result<Option<PlayerState>, ApiError> {
    crate::database::load_player(&state.database, player_id)
        .await?
        .map(|player| {
            crate::experience::apply_curve(player, state.experience.default_curve())
                .map_err(ApiError::from)
        })
        .transpose()
}

fn parse_player_id(value: &str) -> Result<PlayerId, ApiError> {
    PlayerId::parse(value)
        .map_err(|error| ApiError::bad_request("invalid_player_id", error.to_string()))
}

async fn require_player(
    state: &AppState,
    player_id: &PlayerId,
) -> Result<PlayerState, ApiError> {
    load_player(state, player_id)
        .await?
        .filter(|player| player.appearance.is_some())
        .ok_or_else(|| ApiError::not_found("player_not_found", "player does not exist"))
}

fn item_rule_error(error: crate::items::ItemRuleError) -> ApiError {
    ApiError::bad_request("invalid_item_action", error.to_string())
}

fn pick_up_error(error: crate::items::PickUpError) -> ApiError {
    match error {
        crate::items::PickUpError::Rule(error) => item_rule_error(error),
        crate::items::PickUpError::Store(error) => error.into(),
    }
}

fn starter_position(map: &oozems_proto::v1::Map) -> Vec2 {
    map.portals
        .iter()
        .find(|portal| portal.kind == 0)
        .map(|portal| Vec2 {
            x: portal.x,
            y: portal.y,
        })
        .unwrap_or(Vec2 {
            x: 160.0_f32.min(map.width as f32),
            y: 420.0_f32.min(map.height as f32),
        })
}

fn decode_request<T>(
    headers: &HeaderMap,
    body: Bytes,
) -> Result<T, ApiError>
where
    T: Message + Default,
{
    let has_protobuf_content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.eq_ignore_ascii_case(PROTOBUF_CONTENT_TYPE));
    if !has_protobuf_content_type {
        return Err(ApiError::bad_request(
            "invalid_content_type",
            format!("Content-Type must be {PROTOBUF_CONTENT_TYPE}"),
        ));
    }

    T::decode(body).map_err(|error| {
        ApiError::bad_request(
            "invalid_protobuf",
            format!("invalid protobuf body: {error}"),
        )
    })
}

fn validate_position(player: &oozems_proto::v1::PlayerState) -> Result<(), ApiError> {
    let position = player.position.as_ref().ok_or_else(|| {
        ApiError::bad_request("missing_position", "player does not contain a position")
    })?;
    if !position.x.is_finite() || !position.y.is_finite() {
        return Err(ApiError::bad_request(
            "invalid_position",
            "position coordinates must be finite",
        ));
    }
    Ok(())
}

pub struct Protobuf<T>(pub T);

impl<T: Message> IntoResponse for Protobuf<T> {
    fn into_response(self) -> Response {
        (
            [(header::CONTENT_TYPE, PROTOBUF_CONTENT_TYPE)],
            self.0.encode_to_vec(),
        )
            .into_response()
    }
}

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("{message}")]
    Client {
        status: StatusCode,
        code: &'static str,
        message: String,
    },
    #[error("database operation failed")]
    Database(#[from] surrealdb::Error),
    #[error("content operation failed")]
    Content(#[from] crate::content::ContentError),
    #[error("content worker failed")]
    Worker(#[from] tokio::task::JoinError),
    #[error("game rules could not be applied")]
    GameRules(#[from] crate::experience::ExperienceRuleError),
    #[error("dropped-item operation failed")]
    Drops(#[from] crate::items::DropStoreError),
}

impl ApiError {
    fn bad_request(
        code: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self::Client {
            status: StatusCode::BAD_REQUEST,
            code,
            message: message.into(),
        }
    }

    fn not_found(
        code: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self::Client {
            status: StatusCode::NOT_FOUND,
            code,
            message: message.into(),
        }
    }

    fn conflict(
        code: &'static str,
        message: impl Into<String>,
    ) -> Self {
        Self::Client {
            status: StatusCode::CONFLICT,
            code,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code, message) = match self {
            Self::Client {
                status,
                code,
                message,
            } => (status, code, message),
            Self::Database(error) => {
                tracing::error!(%error, "database request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "database_error",
                    "the server could not access player data".to_owned(),
                )
            }
            Self::Content(error) => {
                tracing::error!(%error, "content request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "content_error",
                    "the server could not load game content".to_owned(),
                )
            }
            Self::Worker(error) => {
                tracing::error!(%error, "content worker failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "content_worker_error",
                    "the server could not load game content".to_owned(),
                )
            }
            Self::GameRules(error) => {
                tracing::error!(%error, "game rules could not be applied");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "game_rules_error",
                    "the server could not apply its game rules".to_owned(),
                )
            }
            Self::Drops(error) => {
                tracing::error!(%error, "dropped-item operation failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "drop_store_error",
                    "the server could not access dropped items".to_owned(),
                )
            }
        };

        (
            status,
            Protobuf(ErrorResponse {
                code: code.to_owned(),
                message,
            }),
        )
            .into_response()
    }
}
