use axum::body::Bytes;
use axum::extract::Path as AxumPath;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::http::header;
use axum::response::IntoResponse;
use axum::response::Response;
use oozems_proto::PROTOBUF_CONTENT_TYPE;
use oozems_proto::v1::AllocateSkillPointRequest;
use oozems_proto::v1::AllocateSkillPointResponse;
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
use oozems_proto::v1::GetMorphRequest;
use oozems_proto::v1::GetMorphResponse;
use oozems_proto::v1::GetSkillBookRequest;
use oozems_proto::v1::GetSkillBookResponse;
use oozems_proto::v1::ItemActionResponse;
use oozems_proto::v1::PickUpItemRequest;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::RecoverPlayerRequest;
use oozems_proto::v1::RecoverPlayerResponse;
use oozems_proto::v1::SavePlayerRequest;
use oozems_proto::v1::SavePlayerResponse;
use oozems_proto::v1::UnequipItemRequest;
use oozems_proto::v1::UseSkillRequest;
use oozems_proto::v1::UseSkillResponse;
use oozems_proto::v1::Vec2;
use prost::Message;
use thiserror::Error;

use crate::app::AppState;
use crate::database::CharacterName;
use crate::database::PlayerId;

pub(crate) mod cash_shop;
pub(crate) mod combat;
pub(crate) mod interactions;
pub(crate) mod movement;

pub async fn bootstrap(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<BootstrapResponse>, ApiError> {
    let request: BootstrapRequest = decode_request(&headers, body)?;
    let player_id = PlayerId::parse(&request.player_id)
        .map_err(|error| ApiError::bad_request("invalid_player_id", error.to_string()))?;
    let _player_guard = lock_player(&state, &player_id).await?;
    let player = load_player(&state, &player_id)
        .await?
        .filter(|player| player.appearance.is_some());
    let player = if let Some(player) = player {
        let activity_time_ms = unix_time_ms()?;
        let player = process_automatic_quests(&state, player, activity_time_ms).await?;
        let map = load_map(&state, player.map_id).await?.ok_or_else(|| {
            ApiError::not_found(
                "map_not_found",
                format!("map {} does not exist", player.map_id),
            )
        })?;
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
    Ok(Protobuf(BootstrapResponse {
        player,
        creation_options: Some(state.catalog.character_creation_options()),
        active_buffs,
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
    let _player_guard = lock_player(&state, &player_id).await?;
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

    let initial_map_id = state.gameplay.initial_map_id;
    let map = load_map(&state, initial_map_id).await?.ok_or_else(|| {
        ApiError::not_found(
            "starter_map_not_found",
            format!("starter map {initial_map_id} does not exist"),
        )
    })?;
    let position = starter_position(&map);
    let experience_required =
        crate::experience::required_for_level(state.experience.default_curve(), 1)?;
    let activity_time_ms = unix_time_ms()?;
    let player = crate::database::create_player(
        &state.database,
        &player_id,
        &name,
        appearance,
        initial_map_id,
        position,
        experience_required,
        state.gameplay.initial_skill_points,
        state.gameplay.initial_cash_points,
    )
    .await?;
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
    let mut map = load_map(&state, request.map_id).await?.ok_or_else(|| {
        ApiError::not_found(
            "map_not_found",
            format!("map {} does not exist", request.map_id),
        )
    })?;
    map.dropped_items = crate::items::map_drops(&state.drops, map.id)?;
    let simulation = crate::mobs::map_snapshot(&state.mobs, &map)?;
    map.mobs = simulation.mobs;
    map.mob_projectiles = simulation.mob_projectiles;
    map.simulation_sequence = simulation.sequence;

    Ok(Protobuf(GetMapResponse { map: Some(map) }))
}

pub async fn equip_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<ItemActionResponse>, ApiError> {
    let request: EquipItemRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let _player_guard = lock_player(&state, &player_id).await?;
    let current = require_player(&state, &player_id).await?;
    crate::items::validate_inventory_selection(
        &current,
        request.inventory_index,
        request.expected_item_id,
        request.expected_expires_at_unix_ms,
    )
    .map_err(item_rule_error)?;
    let updated = crate::items::equip_inventory_item(
        current,
        request.inventory_index,
        state.catalog.as_ref(),
    )
    .map_err(item_rule_error)?;
    let activity_time_ms = unix_time_ms()?;
    let player = crate::database::save_player(&state.database, &updated).await?;
    record_recovery_activity(&state, player_id.as_str(), activity_time_ms);

    Ok(Protobuf(ItemActionResponse {
        player: Some(player),
        dropped_item: None,
        picked_up_drop_id: String::new(),
        active_buffs: Some(active_buff_state(
            &state,
            player_id.as_str(),
            activity_time_ms,
        )?),
    }))
}

pub async fn unequip_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<ItemActionResponse>, ApiError> {
    let request: UnequipItemRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let _player_guard = lock_player(&state, &player_id).await?;
    let current = require_player(&state, &player_id).await?;
    let updated = crate::items::unequip_item(current, request.slot, state.catalog.as_ref())
        .map_err(item_rule_error)?;
    let activity_time_ms = unix_time_ms()?;
    let player = crate::database::save_player(&state.database, &updated).await?;
    record_recovery_activity(&state, player_id.as_str(), activity_time_ms);

    Ok(Protobuf(ItemActionResponse {
        player: Some(player),
        dropped_item: None,
        picked_up_drop_id: String::new(),
        active_buffs: Some(active_buff_state(
            &state,
            player_id.as_str(),
            activity_time_ms,
        )?),
    }))
}

pub async fn drop_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<ItemActionResponse>, ApiError> {
    let request: DropItemRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let _player_guard = lock_player(&state, &player_id).await?;
    let current = require_player(&state, &player_id).await?;
    let original = current.clone();
    crate::items::validate_inventory_selection(
        &current,
        request.inventory_index,
        request.expected_item_id,
        request.expected_expires_at_unix_ms,
    )
    .map_err(item_rule_error)?;
    let removed = crate::items::remove_inventory_item(
        current,
        request.inventory_index,
        state.catalog.as_ref(),
    )
    .map_err(item_rule_error)?;
    let activity_time_ms = unix_time_ms()?;
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
    record_recovery_activity(&state, player_id.as_str(), activity_time_ms);

    Ok(Protobuf(ItemActionResponse {
        player: Some(player),
        dropped_item: Some(dropped_item),
        picked_up_drop_id: String::new(),
        active_buffs: Some(active_buff_state(
            &state,
            player_id.as_str(),
            activity_time_ms,
        )?),
    }))
}

pub async fn pick_up_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<ItemActionResponse>, ApiError> {
    let request: PickUpItemRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let _player_guard = lock_player(&state, &player_id).await?;
    let current = require_player(&state, &player_id).await?;
    let position = current
        .position
        .ok_or(crate::movement::MovementError::MissingPlayerPosition)?;
    let picked =
        crate::items::pick_up_nearest(&state.drops, current, position, state.catalog.as_ref())
            .map_err(pick_up_error)?;
    let map_id = picked.player.map_id;
    let activity_time_ms = unix_time_ms()?;
    let effects =
        crate::effects::snapshot(&state.active_effects, player_id.as_str(), activity_time_ms)?;
    let advanced = advance_automatic_player(&state, picked.player, effects, activity_time_ms);
    let active_buffs = crate::effects::state(&advanced.effects, activity_time_ms);
    let player = match crate::database::save_player(&state.database, &advanced.player).await {
        Ok(player) => player,
        Err(error) => {
            if let Err(restore_error) = crate::items::restore_drop(
                &state.drops,
                map_id,
                picked.drop.clone(),
                picked.owner_player_id.clone(),
            ) {
                tracing::error!(%restore_error, "failed to restore an item after a player save error");
            }
            return Err(error.into());
        }
    };
    crate::effects::commit(&state.active_effects, player_id.as_str(), advanced.effects)?;
    record_recovery_activity(&state, player_id.as_str(), activity_time_ms);

    Ok(Protobuf(ItemActionResponse {
        player: Some(player),
        dropped_item: None,
        picked_up_drop_id: picked.drop.id,
        active_buffs: Some(active_buffs),
    }))
}

pub async fn get_gui(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<GetGuiResponse>, ApiError> {
    let request: GetGuiRequest = decode_request(&headers, body)?;
    let player_id = PlayerId::parse(&request.player_id)
        .map_err(|error| ApiError::bad_request("invalid_player_id", error.to_string()))?;
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
    Ok(Protobuf(GetGuiResponse {
        gui: Some(state.catalog.game_gui(&item_ids)?),
    }))
}

pub async fn get_skill_book(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<GetSkillBookResponse>, ApiError> {
    let request: GetSkillBookRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let _player_guard = lock_player(&state, &player_id).await?;
    let player = require_player(&state, &player_id).await?;
    let skill_context = load_skill_book(&state, &player).await?;
    let skill_book =
        crate::skills::personalize_skill_book(skill_context, &player).map_err(skill_rule_error)?;
    let now_unix_ms = unix_time_ms()?;
    let active_buffs = active_buff_state(&state, player_id.as_str(), now_unix_ms)?;

    Ok(Protobuf(GetSkillBookResponse {
        skill_book: Some(skill_book),
        active_buffs: Some(active_buffs),
    }))
}

pub async fn allocate_skill_point(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<AllocateSkillPointResponse>, ApiError> {
    let request: AllocateSkillPointRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let _player_guard = lock_player(&state, &player_id).await?;
    let current = require_player(&state, &player_id).await?;
    let skill_context = load_skill_book(&state, &current).await?;
    let updated = crate::skills::allocate_skill_point(current, &skill_context, request.skill_id)
        .map_err(skill_rule_error)?;
    let activity_time_ms = unix_time_ms()?;
    let effects =
        crate::effects::snapshot(&state.active_effects, player_id.as_str(), activity_time_ms)?;
    let advanced = advance_automatic_player(&state, updated, effects, activity_time_ms);
    let active_buffs = crate::effects::state(&advanced.effects, activity_time_ms);
    let player = crate::database::save_player(&state.database, &advanced.player).await?;
    crate::effects::commit(&state.active_effects, player_id.as_str(), advanced.effects)?;
    record_recovery_activity(&state, player_id.as_str(), activity_time_ms);
    let skill_context = load_skill_book(&state, &player).await?;
    let skill_book =
        crate::skills::personalize_skill_book(skill_context, &player).map_err(skill_rule_error)?;

    Ok(Protobuf(AllocateSkillPointResponse {
        player: Some(player),
        skill_book: Some(skill_book),
        active_buffs: Some(active_buffs),
    }))
}

pub async fn use_skill(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<UseSkillResponse>, ApiError> {
    let request: UseSkillRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let _player_guard = lock_player(&state, &player_id).await?;
    let current = require_player(&state, &player_id).await?;
    let now_ms = unix_time_ms()?;
    let mut effects = crate::effects::snapshot(&state.active_effects, player_id.as_str(), now_ms)?;
    let skill_context = load_skill_book(&state, &current).await?;
    let skill_job_id = skill_context
        .book
        .skills
        .iter()
        .filter_map(|skill| skill.definition.as_ref())
        .find(|definition| definition.skill_id == request.skill_id)
        .map_or_else(
            || current.stats.as_ref().map_or(0, |stats| stats.job_id),
            |definition| definition.job_id,
        );
    let prepared = crate::skills::prepare_skill_use(
        current,
        &skill_context,
        request.skill_id,
        &state.formulas,
        effects.projected(),
    )
    .map_err(skill_rule_error)?;
    if prepared.result.has_damage && effects.attacks_disabled() {
        return Err(ApiError::bad_request(
            "invalid_skill_action",
            "the active morph does not allow attacks",
        ));
    }
    let effect = load_skill_effect(
        &state,
        skill_job_id,
        request.skill_id,
        prepared.result.skill_level,
    )
    .await?;
    let map = load_map(&state, prepared.player.map_id)
        .await?
        .ok_or_else(|| {
            ApiError::not_found(
                "map_not_found",
                format!("map {} does not exist", prepared.player.map_id),
            )
        })?;
    let mut dropped_items = crate::items::map_drops(&state.drops, map.id)?;
    crate::skills::reserve_skill_cooldown(
        &state.skill_cooldowns,
        player_id.as_str(),
        request.skill_id,
        now_ms,
        prepared.cooldown_ms,
    )
    .map_err(skill_rule_error)?;
    crate::effects::apply_skill_effect(&mut effects, &prepared.result, now_ms);
    let mut simulation = match crate::mobs::use_player_attack_with_effects(
        &state.mobs,
        &map,
        &prepared.player,
        crate::mobs::PlayerAttack {
            target_mob_id: &request.target_mob_id,
            source_skill_id: Some(prepared.result.skill_id),
            facing_left: request.facing_left,
            minimum_damage: prepared.result.minimum_damage,
            maximum_damage: prepared.result.maximum_damage,
            fixed_damage: prepared.result.fixed_damage,
            attack_type: prepared.attack_type,
        },
        effects.projected(),
    ) {
        Ok(simulation) => simulation,
        Err(error) => {
            release_skill_reservation(&state, player_id.as_str(), request.skill_id);
            return Err(error.into());
        }
    };
    let persisted = save_simulation_player_effects(
        &state,
        prepared.player,
        &mut simulation,
        effects,
        now_ms,
        true,
    )
    .await?;
    let player = persisted.player;
    let effects = persisted.effects;
    merge_dropped_items(&mut dropped_items, persisted.committed_drops);
    record_recovery_activity(&state, player_id.as_str(), now_ms);
    let active_buffs = crate::effects::state(&effects, now_ms);

    Ok(Protobuf(UseSkillResponse {
        player: Some(player),
        result: Some(prepared.result),
        effect: Some(effect),
        mobs: simulation.mobs,
        mob_projectiles: simulation.mob_projectiles,
        combat_events: simulation.combat_events,
        simulation_sequence: simulation.sequence,
        active_buffs: Some(active_buffs),
        dropped_items,
    }))
}

pub async fn recover_player(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<RecoverPlayerResponse>, ApiError> {
    let request: RecoverPlayerRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let _player_guard = lock_player(&state, &player_id).await?;
    let current = require_player(&state, &player_id).await?;
    let now_ms = unix_time_ms()?;
    let deadline_ms = match crate::recovery::reserve_recovery(
        &state.recovery_timers,
        player_id.as_str(),
        now_ms,
    )? {
        crate::recovery::RecoveryReservation::Waiting { remaining_ms } => {
            return Ok(Protobuf(RecoverPlayerResponse {
                player: Some(current),
                retry_after_ms: remaining_ms,
                active_buffs: Some(active_buff_state(&state, player_id.as_str(), now_ms)?),
                ..RecoverPlayerResponse::default()
            }));
        }
        crate::recovery::RecoveryReservation::Ready { deadline_ms } => deadline_ms,
    };
    let prepared = match crate::recovery::prepare_recovery(current, &state.formulas) {
        Ok(prepared) => prepared,
        Err(error) => {
            release_recovery(&state, player_id.as_str(), deadline_ms);
            return Err(error.into());
        }
    };
    let player = if prepared.hp_restored == 0 && prepared.mp_restored == 0 {
        prepared.player
    } else {
        match crate::database::save_player(&state.database, &prepared.player).await {
            Ok(player) => player,
            Err(error) => {
                release_recovery(&state, player_id.as_str(), deadline_ms);
                return Err(error.into());
            }
        }
    };

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
    let requested = request.player.ok_or_else(|| {
        ApiError::bad_request("missing_player", "request does not contain a player")
    })?;
    crate::keymap::validate_bindings(&requested.key_bindings)
        .map_err(|error| ApiError::bad_request("invalid_key_bindings", error.to_string()))?;

    let player_id = PlayerId::parse(&requested.id)
        .map_err(|error| ApiError::bad_request("invalid_player_id", error.to_string()))?;
    let _player_guard = lock_player(&state, &player_id).await?;
    let current = require_player(&state, &player_id).await?;
    let skill_context = load_skill_book(&state, &current).await?;
    crate::skills::validate_bound_skills(&requested.key_bindings, &current, &skill_context)
        .map_err(skill_rule_error)?;
    let player = crate::database::apply_player_preferences(current, &requested);
    let player = crate::database::save_player(&state.database, &player).await?;
    let now_unix_ms = unix_time_ms()?;
    crate::movement::mark_persisted(&state.movement, player_id.as_str(), now_unix_ms)?;

    Ok(Protobuf(SavePlayerResponse {
        player: Some(player),
        active_buffs: Some(active_buff_state(&state, player_id.as_str(), now_unix_ms)?),
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

async fn load_player(
    state: &AppState,
    player_id: &PlayerId,
) -> Result<Option<PlayerState>, ApiError> {
    let Some(mut player) = crate::database::load_player(&state.database, player_id).await? else {
        return Ok(None);
    };
    let inventory = player
        .inventory
        .as_mut()
        .ok_or(crate::items::ItemRuleError::MissingInventory)
        .map_err(item_rule_error)?;
    let inventory_pruned = crate::items::prune_and_validate_inventory(
        inventory,
        state.catalog.as_ref(),
        unix_time_ms()?,
    )
    .map_err(|error| ApiError::PlayerData(error.to_string()))?;
    let appearance = player
        .appearance
        .as_ref()
        .ok_or_else(|| ApiError::PlayerData("character appearance is missing".to_owned()))?;
    if !state.catalog.supports_character(appearance) {
        return Err(ApiError::PlayerData(
            "character appearance is not available in the current content".to_owned(),
        ));
    }
    let skill_context = load_skill_book(state, &player).await?;
    crate::skills::validate_bound_skills(&player.key_bindings, &player, &skill_context)
        .map_err(|error| ApiError::PlayerData(error.to_string()))?;
    let known_card_ids = state.catalog.monster_book_card_ids();
    if !known_card_ids.is_empty() {
        let unknown_card_ids = player
            .monster_book_cards
            .iter()
            .map(|card| card.card_item_id)
            .filter(|card_item_id| !known_card_ids.contains(card_item_id))
            .collect::<Vec<_>>();
        if !unknown_card_ids.is_empty() {
            tracing::warn!(
                player_id = %player.id,
                ?unknown_card_ids,
                "preserving Monster Book cards absent from the current content catalog"
            );
        }
    }
    player = crate::experience::apply_curve(player, state.experience.default_curve())?;
    if inventory_pruned {
        player = crate::database::save_player(&state.database, &player).await?;
    }
    Ok(Some(player))
}

async fn load_skill_book(
    state: &AppState,
    player: &PlayerState,
) -> Result<crate::content::SkillBookContext, ApiError> {
    let catalog = state.catalog.clone();
    let player = player.clone();
    Ok(tokio::task::spawn_blocking(move || catalog.skill_book_context(&player)).await??)
}

async fn load_skill_effect(
    state: &AppState,
    job_id: u32,
    skill_id: u32,
    level: u32,
) -> Result<oozems_proto::v1::SkillEffect, ApiError> {
    let catalog = state.catalog.clone();
    Ok(
        tokio::task::spawn_blocking(move || catalog.skill_effect(job_id, skill_id, level))
            .await??,
    )
}

fn parse_player_id(value: &str) -> Result<PlayerId, ApiError> {
    PlayerId::parse(value)
        .map_err(|error| ApiError::bad_request("invalid_player_id", error.to_string()))
}

async fn lock_player(
    state: &AppState,
    player_id: &PlayerId,
) -> Result<tokio::sync::OwnedMutexGuard<()>, ApiError> {
    Ok(crate::player_lock::acquire_player(&state.player_locks, player_id.as_str()).await?)
}

async fn require_player(
    state: &AppState,
    player_id: &PlayerId,
) -> Result<PlayerState, ApiError> {
    require_player_at(state, player_id, unix_time_ms()?).await
}

async fn require_player_at(
    state: &AppState,
    player_id: &PlayerId,
    now_unix_ms: u64,
) -> Result<PlayerState, ApiError> {
    let player = load_player(state, player_id)
        .await?
        .filter(|player| player.appearance.is_some())
        .ok_or_else(|| ApiError::not_found("player_not_found", "player does not exist"))?;
    let player = crate::movement::synchronize_player(&state.movement, player)?;
    process_automatic_quests(state, player, now_unix_ms).await
}

pub(crate) fn advance_automatic_player(
    state: &AppState,
    player: PlayerState,
    effects: crate::effects::PlayerEffects,
    now_unix_ms: u64,
) -> crate::quests::AutomaticQuestAdvance {
    let mut definitions = state.catalog.quest_definitions().collect::<Vec<_>>();
    definitions.sort_by_key(|quest| quest.id);
    let consume_effects = state.catalog.consume_effect_definitions();
    let advanced = crate::quests::advance_automatic_quests(
        player,
        effects,
        definitions,
        state.experience.default_curve(),
        state.catalog.item_definition_slice(),
        &consume_effects,
        &state.quest_scripts,
        crate::quests::QuestEnvironment {
            now_unix_ms,
            world_id: state.gameplay.world_id,
        },
    );
    if !advanced.failures.is_empty() {
        tracing::warn!(
            player_id = %advanced.player.id,
            failures = ?advanced.failures,
            "automatic quest transitions were blocked"
        );
    }
    advanced
}

async fn process_automatic_quests(
    state: &AppState,
    player: PlayerState,
    now_unix_ms: u64,
) -> Result<PlayerState, ApiError> {
    let effects = crate::effects::snapshot(&state.active_effects, &player.id, now_unix_ms)?;
    let advanced = advance_automatic_player(state, player, effects, now_unix_ms);
    let player = if advanced.changed {
        crate::database::save_player(&state.database, &advanced.player).await?
    } else {
        advanced.player
    };
    crate::effects::commit(&state.active_effects, &player.id, advanced.effects)?;
    Ok(player)
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

fn skill_rule_error(error: crate::skills::SkillRuleError) -> ApiError {
    match error {
        crate::skills::SkillRuleError::CooldownStore
        | crate::skills::SkillRuleError::Formula { .. } => ApiError::SkillRules(error),
        _ => ApiError::bad_request("invalid_skill_action", error.to_string()),
    }
}

async fn save_simulation_player_effects(
    state: &AppState,
    player: PlayerState,
    simulation: &mut crate::mobs::MobUpdate,
    effects: crate::effects::PlayerEffects,
    now_unix_ms: u64,
    persistence_required: bool,
) -> Result<SimulationPersistence, ApiError> {
    let prepared = prepare_simulation_player_effects(
        state,
        player,
        simulation,
        effects,
        now_unix_ms,
        persistence_required,
    );
    persist_simulation_player_effects(state, prepared, simulation).await
}

fn prepare_simulation_player_effects(
    state: &AppState,
    mut player: PlayerState,
    simulation: &crate::mobs::MobUpdate,
    mut effects: crate::effects::PlayerEffects,
    now_unix_ms: u64,
    persistence_required: bool,
) -> PreparedSimulationPersistence {
    let player_damage = simulation.player_damage();
    if player_damage > 0 {
        let stats = player.stats.get_or_insert_default();
        stats.hp = stats
            .hp
            .saturating_sub(u32::try_from(player_damage).unwrap_or(u32::MAX));
        crate::effects::cancel_damage_morphs(&mut effects, |morph_id| {
            state.catalog.morph_definition(morph_id)
        });
    }
    let mob_kills = simulation
        .mob_deaths
        .iter()
        .map(|death| (death.definition_id, death.source_skill_id))
        .collect::<Vec<_>>();
    let kill_result =
        crate::quests::record_mob_kills(player, &mob_kills, state.catalog.quest_definitions());
    let advanced = advance_automatic_player(state, kill_result.player, effects, now_unix_ms);
    let should_save = persistence_required
        || player_damage > 0
        || !kill_result.changed_quest_ids.is_empty()
        || advanced.changed;
    PreparedSimulationPersistence {
        player: advanced.player,
        effects: advanced.effects,
        should_save,
    }
}

async fn persist_simulation_player_effects(
    state: &AppState,
    prepared: PreparedSimulationPersistence,
    simulation: &mut crate::mobs::MobUpdate,
) -> Result<SimulationPersistence, ApiError> {
    let PreparedSimulationPersistence {
        player: advanced_player,
        effects,
        should_save,
    } = prepared;
    let player = if should_save {
        match crate::database::save_player(&state.database, &advanced_player).await {
            Ok(player) => player,
            Err(error) => {
                crate::mobs::rollback_player_attack(&state.mobs, simulation).unwrap_or_else(
                    |rollback_error| {
                        tracing::error!(
                            %rollback_error,
                            "failed to roll back a player attack after a player save error"
                        );
                        false
                    },
                );
                crate::mobs::restore_player_effects(
                    &state.mobs,
                    advanced_player.map_id,
                    &advanced_player.id,
                    simulation.combat_events.clone(),
                    simulation.mob_deaths.clone(),
                    simulation.staged_drops.clone(),
                )
                .unwrap_or_else(|restore_error| {
                    tracing::error!(%restore_error, "failed to restore combat after a player save error");
                });
                return Err(error.into());
            }
        }
    } else {
        advanced_player
    };
    if let Err(error) = crate::mobs::commit_player_attack(&state.mobs, simulation) {
        tracing::error!(%error, "failed to commit an in-memory player attack transaction");
    }
    let committed_drops = match crate::items::commit_staged_drops(
        &state.drops,
        &simulation.staged_drops,
    ) {
        Ok(()) => simulation
            .staged_drops
            .iter()
            .map(|grant| grant.item().clone())
            .collect(),
        Err(error) => {
            tracing::error!(%error, "failed to commit staged mob drops; the exact grants will retry");
            crate::mobs::restore_player_effects(
                &state.mobs,
                player.map_id,
                &player.id,
                Vec::new(),
                Vec::new(),
                simulation.staged_drops.clone(),
            )
            .unwrap_or_else(|restore_error| {
                tracing::error!(%restore_error, "failed to queue staged mob drops for retry");
            });
            Vec::new()
        }
    };
    if let Err(error) = crate::effects::commit(&state.active_effects, &player.id, effects.clone()) {
        tracing::error!(%error, "failed to commit effects after player persistence");
    }
    Ok(SimulationPersistence {
        player,
        effects,
        committed_drops,
    })
}

struct PreparedSimulationPersistence {
    player: PlayerState,
    effects: crate::effects::PlayerEffects,
    should_save: bool,
}

struct SimulationPersistence {
    player: PlayerState,
    effects: crate::effects::PlayerEffects,
    committed_drops: Vec<oozems_proto::v1::DroppedItem>,
}

fn merge_dropped_items(
    current: &mut Vec<oozems_proto::v1::DroppedItem>,
    additions: Vec<oozems_proto::v1::DroppedItem>,
) {
    for drop in additions {
        if !current.iter().any(|current| current.id == drop.id) {
            current.push(drop);
        }
    }
}

fn release_skill_reservation(
    state: &AppState,
    player_id: &str,
    skill_id: u32,
) {
    if let Err(error) =
        crate::skills::release_skill_cooldown(&state.skill_cooldowns, player_id, skill_id)
    {
        tracing::error!(%error, "failed to release a skill cooldown before an attack was applied");
    }
}

fn active_buff_state(
    state: &AppState,
    player_id: &str,
    now_unix_ms: u64,
) -> Result<oozems_proto::v1::ActiveBuffState, ApiError> {
    let effects = crate::effects::snapshot(&state.active_effects, player_id, now_unix_ms)?;
    Ok(crate::effects::state(&effects, now_unix_ms))
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

fn release_recovery(
    state: &AppState,
    player_id: &str,
    deadline_ms: u64,
) {
    if let Err(error) =
        crate::recovery::release_recovery(&state.recovery_timers, player_id, deadline_ms)
    {
        tracing::error!(%error, "failed to release recovery reservation");
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
    #[error("persisted player data is invalid: {0}")]
    PlayerData(String),
    #[error("game rules could not be applied")]
    GameRules(#[from] crate::experience::ExperienceRuleError),
    #[error("attack rules could not be applied")]
    AttackRules(#[from] crate::attacks::AttackRuleError),
    #[error("dropped-item operation failed")]
    Drops(#[from] crate::items::DropStoreError),
    #[error("mob spawning failed")]
    Mobs(#[from] crate::mobs::MobStoreError),
    #[error("skill rules could not be applied")]
    SkillRules(#[from] crate::skills::SkillRuleError),
    #[error("recovery rules could not be applied")]
    Recovery(#[from] crate::recovery::RecoveryError),
    #[error("movement rules could not be applied")]
    Movement(#[from] crate::movement::MovementError),
    #[error("active effects could not be accessed")]
    Effects(#[from] crate::effects::EffectStoreError),
    #[error("player operations could not be serialized")]
    PlayerLock(#[from] crate::player_lock::PlayerLockError),
    #[error("system time is earlier than the Unix epoch")]
    Clock,
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
            Self::PlayerData(error) => {
                tracing::error!(%error, "persisted player data is invalid");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "player_data_error",
                    "the server could not load valid player data".to_owned(),
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
            Self::AttackRules(error) => {
                tracing::error!(%error, "attack rules could not be applied");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "attack_rules_error",
                    "the server could not apply its attack rules".to_owned(),
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
            Self::Mobs(error) => {
                tracing::error!(%error, "mob spawning failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "mob_store_error",
                    "the server could not access live mobs".to_owned(),
                )
            }
            Self::SkillRules(error) => {
                tracing::error!(%error, "skill rules could not be applied");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "skill_rules_error",
                    "the server could not apply its skill rules".to_owned(),
                )
            }
            Self::Recovery(error) => {
                tracing::error!(%error, "recovery rules could not be applied");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "recovery_rules_error",
                    "the server could not apply its recovery rules".to_owned(),
                )
            }
            Self::Movement(error) => {
                tracing::error!(%error, "movement rules could not be applied");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "movement_rules_error",
                    "the server could not apply its movement rules".to_owned(),
                )
            }
            Self::Effects(error) => {
                tracing::error!(%error, "active effects could not be accessed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "effect_store_error",
                    "the server could not access active effects".to_owned(),
                )
            }
            Self::PlayerLock(error) => {
                tracing::error!(%error, "player operation lock failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "player_lock_error",
                    "the server could not serialize player operations".to_owned(),
                )
            }
            Self::Clock => {
                tracing::error!("system time is earlier than the Unix epoch");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "clock_error",
                    "the server clock is unavailable".to_owned(),
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
