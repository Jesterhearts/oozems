use axum::body::Bytes;
use axum::extract::Path as AxumPath;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::header;
use axum::response::IntoResponse;
use axum::response::Response;
use oozems_proto::v1::AbilityStat;
use oozems_proto::v1::AllocateAbilityPointRequest;
use oozems_proto::v1::AllocateAbilityPointResponse;
use oozems_proto::v1::BootstrapRequest;
use oozems_proto::v1::BootstrapResponse;
use oozems_proto::v1::CharacterStats;
use oozems_proto::v1::CreateCharacterRequest;
use oozems_proto::v1::CreateCharacterResponse;
use oozems_proto::v1::GameplaySessionGrant;
use oozems_proto::v1::GetCharacterSpritesRequest;
use oozems_proto::v1::GetCharacterSpritesResponse;
use oozems_proto::v1::GetGuiRequest;
use oozems_proto::v1::GetGuiResponse;
use oozems_proto::v1::GetMapRequest;
use oozems_proto::v1::GetMapResponse;
use oozems_proto::v1::GetMorphRequest;
use oozems_proto::v1::GetMorphResponse;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::QuestStatus;
use oozems_proto::v1::RecoverPlayerRequest;
use oozems_proto::v1::RecoverPlayerResponse;
use oozems_proto::v1::SavePlayerRequest;
use oozems_proto::v1::SavePlayerResponse;
use oozems_proto::v1::Vec2;

use crate::app::AppState;
use crate::database::CharacterName;
use crate::database::PlayerId;

pub(crate) mod cash_shop;
pub(crate) mod combat;
pub(crate) mod interactions;
pub(crate) mod items;
pub(crate) mod movement;
mod player_mutation;
mod protocol;
pub(crate) mod respawn;
pub(crate) mod skills;

pub(crate) use self::player_mutation::PlayerMutation;
use self::player_mutation::active_buff_state;
pub(crate) use self::player_mutation::advance_automatic_player;
pub(crate) use self::player_mutation::begin_player_mutation;
use self::player_mutation::load_player;
pub(crate) use self::player_mutation::merge_dropped_items;
pub(crate) use self::player_mutation::persist_player_mutation;
pub(crate) use self::player_mutation::prepare_player_mutation;
pub(crate) use self::player_mutation::prepare_simulation_player_effects;
use self::player_mutation::process_automatic_quests;
pub(crate) use self::player_mutation::project_combat_effects;
use self::player_mutation::require_player;
use self::protocol::ApiError;
use self::protocol::Protobuf;
use self::protocol::decode_request;

pub async fn bootstrap(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<BootstrapResponse>, ApiError> {
    let request: BootstrapRequest = decode_request(&headers, body)?;
    let player_id = PlayerId::parse(&request.player_id)
        .map_err(|error| ApiError::bad_request("invalid_player_id", error.to_string()))?;
    let player_guard = lock_player_unchecked(&state, &player_id).await?;
    let player = load_player(&state, &player_id)
        .await?
        .filter(|loaded| loaded.player.appearance.is_some());
    let player = if let Some(mut loaded) = player {
        let activity_time_ms = unix_time_ms()?;
        let (map, position, repaired) =
            resolve_reconnect_destination(&state, loaded.player.map_id).await?;
        loaded.player.map_id = map.id;
        loaded.player.position = Some(position);
        loaded.changed |= repaired;
        let player =
            process_automatic_quests(&state, &player_guard, loaded, activity_time_ms).await?;
        crate::movement::initialize_player(
            &state.movement,
            &player,
            &map,
            state.gameplay.movement,
            activity_time_ms,
        )?;
        record_recovery_activity(&state, player_id.as_str(), activity_time_ms);
        Some(crate::movement::synchronize_player(
            &state.movement,
            player,
        )?)
    } else {
        None
    };

    let active_buffs = player
        .as_ref()
        .map(|player| -> Result<_, ApiError> {
            let now_unix_ms = unix_time_ms()?;
            let effects = crate::effects::snapshot(&state.active_effects, &player.id, now_unix_ms)?;
            Ok(crate::effects::state(&effects, now_unix_ms))
        })
        .transpose()?;
    let token =
        crate::gameplay_session::issue_session(&state.gameplay_sessions, player_id.as_str())?;
    Ok(Protobuf(BootstrapResponse {
        player,
        creation_options: Some(state.catalog.character_creation_options()),
        active_buffs,
        gameplay_session: Some(GameplaySessionGrant {
            player_id: player_id.as_str().to_owned(),
            token,
        }),
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
    let _player_guard = lock_player(&state, &player_id, &headers).await?;
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
    let inventory =
        crate::items::selected_starter_inventory(&request.equipment).map_err(item_rule_error)?;
    if load_player(&state, &player_id)
        .await?
        .is_some_and(|loaded| loaded.player.appearance.is_some())
    {
        return Err(ApiError::conflict(
            "character_exists",
            "this player already has a character",
        ));
    }

    let initial_map_id = state.gameplay.initial_map_id;
    let map = load_map(&state, initial_map_id).await?.ok_or_else(|| {
        ApiError::not_found(
            "starter_map_not_found",
            format!("starter map {initial_map_id} does not exist"),
        )
    })?;
    let position = crate::movement::default_spawn_position(&map)?;
    let experience_required =
        crate::experience::required_for_level(state.experience.default_curve(), 1)?;
    let activity_time_ms = unix_time_ms()?;
    let player = PlayerState {
        id: player_id.as_str().to_owned(),
        name: name.as_str().to_owned(),
        level: 1,
        map_id: initial_map_id,
        position: Some(position),
        appearance: Some(appearance),
        stats: Some(starter_character_stats(experience_required)),
        inventory: Some(inventory),
        key_bindings: crate::keymap::default_bindings(),
        skill_points: state.gameplay.initial_skill_points,
        learned_skills: Vec::new(),
        mesos: 0,
        quests: Vec::new(),
        revision: 0,
        quest_records: Vec::new(),
        monster_book_cards: Vec::new(),
        cash_points: state.gameplay.initial_cash_points,
    };
    let player = crate::database::create_player(&state.database, &player).await?;
    crate::movement::initialize_player(
        &state.movement,
        &player,
        &map,
        state.gameplay.movement,
        activity_time_ms,
    )?;
    record_recovery_activity(&state, player_id.as_str(), activity_time_ms);

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
        crate::items::default_starter_equipment()
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

pub async fn get_morph(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<GetMorphResponse>, ApiError> {
    let request: GetMorphRequest = decode_request(&headers, body)?;
    let morph = state
        .catalog
        .morph_definition(request.morph_id)
        .ok_or_else(|| ApiError::not_found("morph_not_found", "morph does not exist"))?;
    Ok(Protobuf(GetMorphResponse { morph: Some(morph) }))
}

pub async fn get_map(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<GetMapResponse>, ApiError> {
    let request: GetMapRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let player_guard = lock_player(&state, &player_id, &headers).await?;
    let now_unix_ms = unix_time_ms()?;
    let loaded = load_player(&state, &player_id)
        .await?
        .ok_or_else(|| ApiError::not_found("player_not_found", "player does not exist"))?;
    if request.map_id != loaded.player.map_id {
        return Err(ApiError::bad_request(
            "invalid_map_request",
            "the requested map is not the player's current map",
        ));
    }
    let mut map = load_map(&state, request.map_id).await?.ok_or_else(|| {
        ApiError::not_found(
            "map_not_found",
            format!("map {} does not exist", request.map_id),
        )
    })?;
    let player = loaded.player;
    let effects = crate::effects::snapshot(&state.active_effects, &player.id, now_unix_ms)?;
    drop(player_guard);

    let quest_definitions = state.catalog.quest_definitions().collect::<Vec<_>>();
    let environment = crate::quests::QuestEnvironment {
        now_unix_ms,
        world_id: state.gameplay.world_id,
    };
    crate::quests::project_npc_quest_indicators(
        &mut map,
        &player,
        &effects,
        &quest_definitions,
        state.catalog.item_definition_slice(),
        &state.quest_scripts,
        environment,
    );
    map.dropped_items = crate::items::map_drops(&state.drops, map.id)?;
    let simulation = crate::mobs::map_snapshot(&state.mobs, &map).await?;
    map.mobs = simulation.mobs;
    map.mob_projectiles = simulation.mob_projectiles;
    map.reactors = simulation.reactors;
    map.simulation_sequence = simulation.sequence;

    Ok(Protobuf(GetMapResponse { map: Some(map) }))
}

pub async fn get_gui(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<GetGuiResponse>, ApiError> {
    let request: GetGuiRequest = decode_request(&headers, body)?;
    let player_id = PlayerId::parse(&request.player_id)
        .map_err(|error| ApiError::bad_request("invalid_player_id", error.to_string()))?;
    let _player_guard = lock_player(&state, &player_id, &headers).await?;
    let player = crate::database::load_player(&state.database, &player_id)
        .await?
        .ok_or_else(|| {
            ApiError::not_found("player_not_found", "the requested player does not exist")
        })?;
    let inventory = player
        .inventory
        .as_ref()
        .ok_or_else(|| ApiError::PlayerData("inventory is missing".to_owned()))?;
    let item_ids = inventory
        .stacks
        .iter()
        .map(|stack| stack.item_id)
        .chain(inventory.equipment.iter().map(|equipped| equipped.item_id))
        .chain(request.observed_item_ids)
        .collect();
    let now_unix_ms = unix_time_ms()?;
    let effects = crate::effects::snapshot(&state.active_effects, player_id.as_str(), now_unix_ms)?;
    let quest_definitions = state.catalog.quest_definitions().collect::<Vec<_>>();
    let active_quest_ids = player
        .quests
        .iter()
        .filter(|quest| QuestStatus::try_from(quest.status) == Ok(QuestStatus::Started))
        .map(|quest| quest.quest_id)
        .collect::<std::collections::BTreeSet<_>>();
    let mob_ids = quest_definitions
        .iter()
        .filter(|quest| active_quest_ids.contains(&quest.id))
        .flat_map(|quest| {
            quest
                .completion
                .mobs
                .iter()
                .map(|objective| objective.mob_id)
        })
        .collect();
    let mob_definitions = state.catalog.mob_definitions(&mob_ids);
    let mut gui = state.catalog.game_gui(&item_ids)?;
    gui.quest_tracker = crate::quests::quest_tracker(
        &player,
        &effects,
        &quest_definitions,
        state.catalog.item_definition_slice(),
        &mob_definitions,
        &state.quest_scripts,
        crate::quests::QuestEnvironment {
            now_unix_ms,
            world_id: state.gameplay.world_id,
        },
    );
    Ok(Protobuf(GetGuiResponse { gui: Some(gui) }))
}

pub async fn allocate_ability_point(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<AllocateAbilityPointResponse>, ApiError> {
    let request: AllocateAbilityPointRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let stat = AbilityStat::try_from(request.stat)
        .ok()
        .filter(|stat| *stat != AbilityStat::Unspecified)
        .ok_or_else(|| {
            ApiError::bad_request(
                "invalid_ability_stat",
                "the selected ability stat is invalid",
            )
        })?;
    let player_guard = lock_player(&state, &player_id, &headers).await?;
    let activity_time_ms = unix_time_ms()?;
    let mutation =
        begin_player_mutation(&state, &player_guard, &player_id, activity_time_ms).await?;
    let updated = crate::abilities::allocate_ability_point(mutation.player.clone(), stat)
        .map_err(ability_rule_error)?;
    let committed =
        persist_player_mutation(&state, &player_guard, mutation, updated, true, true).await?;

    Ok(Protobuf(AllocateAbilityPointResponse {
        player: Some(committed.player),
    }))
}

pub async fn recover_player(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<RecoverPlayerResponse>, ApiError> {
    let request: RecoverPlayerRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let player_guard = lock_player(&state, &player_id, &headers).await?;
    let now_ms = unix_time_ms()?;
    let mutation = begin_player_mutation(&state, &player_guard, &player_id, now_ms).await?;
    require_living_player(&mutation.player, "recover")?;
    let reservation = match crate::recovery::reserve_recovery(
        &state.recovery_timers,
        player_id.as_str(),
        now_ms,
    )? {
        crate::recovery::RecoveryReservation::Waiting { remaining_ms } => {
            let active_buffs = crate::effects::state(&mutation.effects, now_ms);
            return Ok(Protobuf(RecoverPlayerResponse {
                player: Some(mutation.player),
                retry_after_ms: remaining_ms,
                active_buffs: Some(active_buffs),
                ..RecoverPlayerResponse::default()
            }));
        }
        crate::recovery::RecoveryReservation::Ready(reservation) => reservation,
    };
    let mut transaction = crate::player_transaction::new_player_transaction(
        mutation.original,
        mutation.player.clone(),
        crate::player_transaction::PlayerPersistence::None,
    );
    crate::player_transaction::stage_recovery(
        &mut transaction,
        state.recovery_timers.clone(),
        reservation,
    );
    let prepared = match crate::recovery::prepare_recovery(mutation.player, &state.formulas) {
        Ok(prepared) => prepared,
        Err(error) => {
            crate::player_transaction::abort_player_transaction(
                &state.database,
                &player_guard,
                transaction,
                error.to_string(),
            )
            .await?;
            return Err(error.into());
        }
    };
    crate::player_transaction::replace_staged_player(
        &mut transaction,
        prepared.player,
        if prepared.hp_restored == 0 && prepared.mp_restored == 0 {
            crate::player_transaction::PlayerPersistence::None
        } else {
            crate::player_transaction::PlayerPersistence::Full
        },
    );
    crate::player_transaction::stage_effects(
        &mut transaction,
        state.active_effects.clone(),
        mutation.original_effects,
        mutation.effects,
    );
    let player = crate::player_transaction::commit_player_transaction(
        &state.database,
        &player_guard,
        transaction,
    )
    .await?
    .player;

    Ok(Protobuf(RecoverPlayerResponse {
        player: Some(player),
        hp_restored: prepared.hp_restored,
        mp_restored: prepared.mp_restored,
        retry_after_ms: crate::recovery::RECOVERY_INTERVAL_MS,
        active_buffs: Some(active_buff_state(&state, player_id.as_str(), now_ms)?),
    }))
}

pub async fn save_player(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<SavePlayerResponse>, ApiError> {
    let request: SavePlayerRequest = decode_request(&headers, body)?;
    crate::keymap::validate_bindings(&request.key_bindings)
        .map_err(|error| ApiError::bad_request("invalid_key_bindings", error.to_string()))?;

    let player_id = PlayerId::parse(&request.player_id)
        .map_err(|error| ApiError::bad_request("invalid_player_id", error.to_string()))?;
    let player_guard = lock_player(&state, &player_id, &headers).await?;
    let now_unix_ms = unix_time_ms()?;
    let mutation = begin_player_mutation(&state, &player_guard, &player_id, now_unix_ms).await?;
    let skill_context = load_skill_book(&state, &mutation.player).await?;
    crate::skills::validate_bound_skills(&request.key_bindings, &mutation.player, &skill_context)
        .map_err(skill_rule_error)?;
    let mut player = mutation.player.clone();
    player.key_bindings = request.key_bindings;
    let active_buffs = crate::effects::state(&mutation.effects, now_unix_ms);
    let mut transaction = crate::player_transaction::new_player_transaction(
        mutation.original,
        player,
        crate::player_transaction::PlayerPersistence::Full,
    );
    crate::player_transaction::stage_effects(
        &mut transaction,
        state.active_effects.clone(),
        mutation.original_effects,
        mutation.effects,
    );
    let player = crate::player_transaction::commit_player_transaction(
        &state.database,
        &player_guard,
        transaction,
    )
    .await?
    .player;
    Ok(Protobuf(SavePlayerResponse {
        player: Some(player),
        active_buffs: Some(active_buffs),
    }))
}

pub async fn get_wz_asset(
    State(state): State<AppState>,
    AxumPath(requested_id): AxumPath<String>,
) -> Result<Response, ApiError> {
    let (version, requested_extension) = requested_id
        .rsplit_once('.')
        .filter(|(value, _)| {
            value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        .ok_or_else(|| ApiError::not_found("asset_not_found", "asset does not exist"))?;
    let asset_id = format!("wz-{version}");
    let asset = state
        .catalog
        .get_wz_asset(&asset_id)
        .ok_or_else(|| ApiError::not_found("asset_not_found", "asset does not exist"))?;
    if asset.extension() != requested_extension {
        return Err(ApiError::not_found(
            "asset_not_found",
            "asset does not exist",
        ));
    }
    let content_type = asset.content_type();
    let bytes = tokio::task::spawn_blocking(move || asset.asset_bytes())
        .await?
        .map_err(crate::content::ContentError::from)?;

    Ok((
        [
            (header::CONTENT_TYPE, content_type),
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

enum SavedMapResolution {
    Use(Vec2),
    RepairUnavailable,
    RepairMissingSpawn,
}

fn inspect_saved_map(
    map: Option<&oozems_proto::v1::Map>
) -> Result<SavedMapResolution, crate::movement::MovementError> {
    let Some(map) = map else {
        return Ok(SavedMapResolution::RepairUnavailable);
    };
    match crate::movement::default_spawn_position(map) {
        Ok(position) => Ok(SavedMapResolution::Use(position)),
        Err(crate::movement::MovementError::MissingDefaultSpawn) => {
            Ok(SavedMapResolution::RepairMissingSpawn)
        }
        Err(error) => Err(error),
    }
}

pub(super) async fn resolve_reconnect_destination(
    state: &AppState,
    saved_map_id: u32,
) -> Result<(oozems_proto::v1::Map, Vec2, bool), ApiError> {
    let saved_map = load_map(state, saved_map_id).await?;
    match inspect_saved_map(saved_map.as_ref())? {
        SavedMapResolution::Use(position) => {
            return Ok((saved_map.expect("inspected saved map"), position, false));
        }
        SavedMapResolution::RepairMissingSpawn => {
            let saved_map = saved_map.expect("inspected saved map");
            let return_town_map_id =
                respawn::respawn_map_id(&saved_map, state.gameplay.initial_map_id);
            tracing::warn!(
                saved_map_id,
                return_town_map_id,
                "saved player map has no usable spawn; reconnecting at its return town"
            );
            let (map, position) = respawn::load_respawn_target(state, return_town_map_id).await?;
            return Ok((map, position, true));
        }
        SavedMapResolution::RepairUnavailable => {
            tracing::warn!(
                saved_map_id,
                fallback_map_id = state.gameplay.initial_map_id,
                "saved player map is unavailable; repairing to the initial map"
            );
        }
    }

    let fallback_map_id = state.gameplay.initial_map_id;
    let map = load_map(state, fallback_map_id).await?.ok_or_else(|| {
        ApiError::not_found(
            "initial_map_not_found",
            format!("initial map {fallback_map_id} does not exist"),
        )
    })?;
    let position = crate::movement::default_spawn_position(&map)?;
    Ok((map, position, saved_map_id != fallback_map_id))
}

async fn current_map_quest_indicators(
    state: &AppState,
    player: &PlayerState,
    effects: &crate::effects::PlayerEffects,
    now_unix_ms: u64,
) -> Vec<oozems_proto::v1::NpcQuestIndicatorUpdate> {
    match load_map(state, player.map_id).await {
        Ok(Some(map)) => quest_indicator_updates(state, &map, player, effects, now_unix_ms),
        Ok(None) => {
            tracing::warn!(
                player_id = %player.id,
                map_id = player.map_id,
                "could not refresh NPC quest indicators because the current map is unavailable"
            );
            Vec::new()
        }
        Err(error) => {
            tracing::warn!(
                player_id = %player.id,
                map_id = player.map_id,
                %error,
                "could not refresh NPC quest indicators"
            );
            Vec::new()
        }
    }
}

fn quest_indicator_updates(
    state: &AppState,
    map: &oozems_proto::v1::Map,
    player: &PlayerState,
    effects: &crate::effects::PlayerEffects,
    now_unix_ms: u64,
) -> Vec<oozems_proto::v1::NpcQuestIndicatorUpdate> {
    let quest_definitions = state.catalog.quest_definitions().collect::<Vec<_>>();
    crate::quests::npc_quest_indicator_updates(
        map,
        player,
        effects,
        &quest_definitions,
        state.catalog.item_definition_slice(),
        &state.quest_scripts,
        crate::quests::QuestEnvironment {
            now_unix_ms,
            world_id: state.gameplay.world_id,
        },
    )
}

async fn load_skill_book(
    state: &AppState,
    player: &PlayerState,
) -> Result<crate::content::SkillBookContext, ApiError> {
    let catalog = state.catalog.clone();
    let player = player.clone();
    Ok(tokio::task::spawn_blocking(move || catalog.skill_book_context(&player)).await??)
}

fn parse_player_id(value: &str) -> Result<PlayerId, ApiError> {
    PlayerId::parse(value)
        .map_err(|error| ApiError::bad_request("invalid_player_id", error.to_string()))
}

pub(crate) fn require_living_player(
    player: &PlayerState,
    action: &str,
) -> Result<(), ApiError> {
    let stats = player
        .stats
        .as_ref()
        .ok_or_else(|| ApiError::PlayerData("character stats are missing".to_owned()))?;
    if stats.hp == 0 {
        return Err(ApiError::bad_request(
            "player_dead",
            format!("a dead player cannot {action}"),
        ));
    }
    Ok(())
}

async fn lock_player(
    state: &AppState,
    player_id: &PlayerId,
    headers: &HeaderMap,
) -> Result<crate::player_lock::PlayerGuard, ApiError> {
    let guard = lock_player_unchecked(state, player_id).await?;
    require_current_gameplay_session(&state.gameplay_sessions, player_id.as_str(), headers)?;
    Ok(guard)
}

async fn lock_player_unchecked(
    state: &AppState,
    player_id: &PlayerId,
) -> Result<crate::player_lock::PlayerGuard, ApiError> {
    Ok(crate::player_lock::acquire_player(&state.player_locks, player_id.as_str()).await?)
}

fn bearer_token(headers: &HeaderMap) -> Result<&str, ApiError> {
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ApiError::Client {
            status: axum::http::StatusCode::UNAUTHORIZED,
            code: "invalid_gameplay_session",
            message: "the request does not contain a gameplay session".to_owned(),
        })?;
    authorization
        .strip_prefix("Bearer ")
        .filter(|token| !token.is_empty())
        .ok_or_else(|| ApiError::Client {
            status: axum::http::StatusCode::UNAUTHORIZED,
            code: "invalid_gameplay_session",
            message: "the request contains an invalid gameplay session".to_owned(),
        })
}

fn require_current_gameplay_session(
    sessions: &crate::gameplay_session::GameplaySessions,
    player_id: &str,
    headers: &HeaderMap,
) -> Result<(), ApiError> {
    let token = bearer_token(headers)?;
    if crate::gameplay_session::is_current_session(sessions, player_id, token)? {
        return Ok(());
    }
    Err(ApiError::Client {
        status: axum::http::StatusCode::UNAUTHORIZED,
        code: "invalid_gameplay_session",
        message: "the gameplay session is no longer current".to_owned(),
    })
}

fn item_rule_error(error: crate::items::ItemRuleError) -> ApiError {
    ApiError::bad_request("invalid_item_action", error.to_string())
}

fn skill_rule_error(error: crate::skills::SkillRuleError) -> ApiError {
    match error {
        crate::skills::SkillRuleError::CooldownStore
        | crate::skills::SkillRuleError::Formula { .. } => ApiError::SkillRules(error),
        _ => ApiError::bad_request("invalid_skill_action", error.to_string()),
    }
}

fn ability_rule_error(error: crate::abilities::AbilityRuleError) -> ApiError {
    match error {
        crate::abilities::AbilityRuleError::NoAbilityPoints
        | crate::abilities::AbilityRuleError::MaximumStat => {
            ApiError::bad_request("invalid_ability_allocation", error.to_string())
        }
        crate::abilities::AbilityRuleError::MissingStats => ApiError::PlayerData(error.to_string()),
    }
}

fn unix_time_ms() -> Result<u64, ApiError> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .map_err(|_| ApiError::Clock)
}

fn record_recovery_activity(
    state: &AppState,
    player_id: &str,
    now_ms: u64,
) {
    if let Err(error) =
        crate::recovery::delay_recovery_after_activity(&state.recovery_timers, player_id, now_ms)
    {
        tracing::error!(%error, "failed to delay recovery after player activity");
    }
}

fn starter_character_stats(experience_required: u64) -> CharacterStats {
    CharacterStats {
        job_id: 0,
        hp: 50,
        max_hp: 50,
        mp: 5,
        max_mp: 5,
        experience: 0,
        experience_required,
        fame: 0,
        ability_points: 9,
        strength: 4,
        dexterity: 4,
        intelligence: 4,
        luck: 4,
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderMap;
    use axum::http::HeaderValue;
    use axum::http::header;
    use oozems_proto::v1::Map;
    use oozems_proto::v1::Portal;
    use oozems_proto::v1::Vec2;

    use super::SavedMapResolution;
    use super::bearer_token;
    use super::inspect_saved_map;
    use super::require_current_gameplay_session;

    #[test]
    fn gameplay_session_header_requires_a_nonempty_bearer_token() {
        let missing = HeaderMap::new();
        assert!(matches!(
            bearer_token(&missing),
            Err(super::ApiError::Client { .. })
        ));

        let mut malformed = HeaderMap::new();
        malformed.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Basic token"),
        );
        assert!(matches!(
            bearer_token(&malformed),
            Err(super::ApiError::Client { .. })
        ));

        let mut valid = HeaderMap::new();
        valid.insert(
            header::AUTHORIZATION,
            HeaderValue::from_static("Bearer token"),
        );
        assert_eq!(bearer_token(&valid).expect("bearer token"), "token");
    }

    #[test]
    fn rotating_a_gameplay_session_rejects_the_previous_bearer_token() {
        let sessions = crate::gameplay_session::GameplaySessions::default();
        let first = crate::gameplay_session::issue_session(&sessions, "player")
            .expect("first gameplay session");
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Bearer {first}")).expect("authorization header"),
        );
        require_current_gameplay_session(&sessions, "player", &headers)
            .expect("first session is current");

        crate::gameplay_session::issue_session(&sessions, "player")
            .expect("replacement gameplay session");

        assert!(matches!(
            require_current_gameplay_session(&sessions, "player", &headers),
            Err(super::ApiError::Client {
                status: axum::http::StatusCode::UNAUTHORIZED,
                code: "invalid_gameplay_session",
                ..
            })
        ));
    }

    #[test]
    fn missing_saved_map_requires_initial_map_repair() {
        assert!(matches!(
            inspect_saved_map(None).expect("inspect missing map"),
            SavedMapResolution::RepairUnavailable
        ));
    }

    #[test]
    fn spawnless_saved_map_requires_return_town_repair() {
        let map = Map {
            id: 100,
            return_map_id: Some(200),
            ..Map::default()
        };

        assert!(matches!(
            inspect_saved_map(Some(&map)).expect("inspect map without spawn"),
            SavedMapResolution::RepairMissingSpawn
        ));
        assert_eq!(super::respawn::respawn_map_id(&map, 300), 200);
    }

    #[test]
    fn saved_map_with_a_default_spawn_is_used_directly() {
        let map = Map {
            id: 100,
            width: 800,
            height: 600,
            portals: vec![Portal {
                kind: 0,
                x: 125.0,
                y: 200.0,
                ..Portal::default()
            }],
            ..Map::default()
        };
        let SavedMapResolution::Use(position) =
            inspect_saved_map(Some(&map)).expect("inspect usable saved map")
        else {
            panic!("usable saved map must not be repaired");
        };
        assert_eq!(position, Vec2 { x: 125.0, y: 200.0 });
    }
}
