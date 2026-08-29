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
use oozems_proto::v1::AllocateSkillPointRequest;
use oozems_proto::v1::AllocateSkillPointResponse;
use oozems_proto::v1::BootstrapRequest;
use oozems_proto::v1::BootstrapResponse;
use oozems_proto::v1::CreateCharacterRequest;
use oozems_proto::v1::CreateCharacterResponse;
use oozems_proto::v1::DropItemRequest;
use oozems_proto::v1::EquipItemRequest;
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
use oozems_proto::v1::ItemCategory;
use oozems_proto::v1::PickUpItemRequest;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::QuestStatus;
use oozems_proto::v1::RecoverPlayerRequest;
use oozems_proto::v1::RecoverPlayerResponse;
use oozems_proto::v1::SavePlayerRequest;
use oozems_proto::v1::SavePlayerResponse;
use oozems_proto::v1::UnequipItemRequest;
use oozems_proto::v1::UseItemRequest;
use oozems_proto::v1::UseSkillRequest;
use oozems_proto::v1::UseSkillResponse;
use oozems_proto::v1::Vec2;

use crate::app::AppState;
use crate::database::CharacterName;
use crate::database::PlayerId;

pub(crate) mod cash_shop;
pub(crate) mod combat;
pub(crate) mod interactions;
pub(crate) mod movement;
mod player_mutation;
mod protocol;
pub(crate) mod respawn;

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
    let player_guard = lock_player(&state, &player_id).await?;
    let player = load_player(&state, &player_id)
        .await?
        .filter(|loaded| loaded.player.appearance.is_some());
    let player = if let Some(loaded) = player {
        let activity_time_ms = unix_time_ms()?;
        let player =
            process_automatic_quests(&state, &player_guard, loaded, activity_time_ms).await?;
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
    let player_guard = lock_player(&state, &player_id).await?;
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
    let position = starter_position(&map);
    let experience_required =
        crate::experience::required_for_level(state.experience.default_curve(), 1)?;
    let activity_time_ms = unix_time_ms()?;
    let player = crate::database::create_player(
        &state.database,
        &player_guard,
        &player_id,
        &name,
        appearance,
        inventory,
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
    let player_guard = lock_player(&state, &player_id).await?;
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

pub async fn equip_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<ItemActionResponse>, ApiError> {
    let request: EquipItemRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let player_guard = lock_player(&state, &player_id).await?;
    let activity_time_ms = unix_time_ms()?;
    let mutation =
        begin_player_mutation(&state, &player_guard, &player_id, activity_time_ms).await?;
    crate::items::validate_inventory_selection(
        &mutation.player,
        request.inventory_index,
        request.expected_item_id,
        request.expected_expires_at_unix_ms,
    )
    .map_err(item_rule_error)?;
    let updated = crate::items::equip_inventory_item(
        mutation.player.clone(),
        request.inventory_index,
        state.catalog.as_ref(),
    )
    .map_err(item_rule_error)?;
    let committed =
        persist_player_mutation(&state, &player_guard, mutation, updated, true, true).await?;
    let player = committed.player;
    let effects = committed
        .effects
        .expect("equip item transaction stages active effects");
    let quest_indicators =
        current_map_quest_indicators(&state, &player, &effects, activity_time_ms).await;

    Ok(Protobuf(ItemActionResponse {
        player: Some(player),
        dropped_item: None,
        picked_up_drop_id: String::new(),
        active_buffs: Some(crate::effects::state(&effects, activity_time_ms)),
        used_setup_item_id: 0,
        quest_indicators,
    }))
}

pub async fn unequip_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<ItemActionResponse>, ApiError> {
    let request: UnequipItemRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let player_guard = lock_player(&state, &player_id).await?;
    let activity_time_ms = unix_time_ms()?;
    let mutation =
        begin_player_mutation(&state, &player_guard, &player_id, activity_time_ms).await?;
    let updated = crate::items::unequip_item(
        mutation.player.clone(),
        request.slot,
        state.catalog.as_ref(),
    )
    .map_err(item_rule_error)?;
    let committed =
        persist_player_mutation(&state, &player_guard, mutation, updated, true, true).await?;
    let player = committed.player;
    let effects = committed
        .effects
        .expect("unequip item transaction stages active effects");
    let quest_indicators =
        current_map_quest_indicators(&state, &player, &effects, activity_time_ms).await;

    Ok(Protobuf(ItemActionResponse {
        player: Some(player),
        dropped_item: None,
        picked_up_drop_id: String::new(),
        active_buffs: Some(crate::effects::state(&effects, activity_time_ms)),
        used_setup_item_id: 0,
        quest_indicators,
    }))
}

pub async fn drop_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<ItemActionResponse>, ApiError> {
    let request: DropItemRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let player_guard = lock_player(&state, &player_id).await?;
    let activity_time_ms = unix_time_ms()?;
    let mutation =
        begin_player_mutation(&state, &player_guard, &player_id, activity_time_ms).await?;
    crate::items::validate_inventory_selection(
        &mutation.player,
        request.inventory_index,
        request.expected_item_id,
        request.expected_expires_at_unix_ms,
    )
    .map_err(item_rule_error)?;
    let removed = crate::items::remove_inventory_item(
        mutation.player.clone(),
        request.inventory_index,
        state.catalog.as_ref(),
    )
    .map_err(item_rule_error)?;
    let staged_drop = crate::items::stage_inventory_drop(&state.drops, &removed)?;
    let dropped_item = staged_drop.item().clone();
    let (mut transaction, _) =
        prepare_player_mutation(&state, mutation, removed.player, true, true);
    crate::player_transaction::stage_drops(&mut transaction, state.drops.clone(), [staged_drop])?;
    let committed = crate::player_transaction::commit_player_transaction(
        &state.database,
        &player_guard,
        transaction,
    )
    .await?;
    let player = committed.player;
    let effects = committed
        .effects
        .expect("drop item transaction stages active effects");
    let quest_indicators =
        current_map_quest_indicators(&state, &player, &effects, activity_time_ms).await;

    Ok(Protobuf(ItemActionResponse {
        player: Some(player),
        dropped_item: Some(dropped_item),
        picked_up_drop_id: String::new(),
        active_buffs: Some(crate::effects::state(&effects, activity_time_ms)),
        used_setup_item_id: 0,
        quest_indicators,
    }))
}

pub async fn use_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<ItemActionResponse>, ApiError> {
    let request: UseItemRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let player_guard = lock_player(&state, &player_id).await?;
    let activity_time_ms = unix_time_ms()?;
    let mut mutation =
        begin_player_mutation(&state, &player_guard, &player_id, activity_time_ms).await?;
    require_living_player(&mutation.player, "use items")?;
    crate::items::validate_inventory_selection(
        &mutation.player,
        request.inventory_index,
        request.expected_item_id,
        request.expected_expires_at_unix_ms,
    )
    .map_err(item_rule_error)?;
    let used = crate::items::use_inventory_item(
        mutation.player.clone(),
        request.inventory_index,
        state.catalog.as_ref(),
    )
    .map_err(item_rule_error)?;
    let consumes_inventory = used.category == ItemCategory::Consume;
    let used_setup_item_id = if used.category == ItemCategory::Install {
        used.item_id
    } else {
        0
    };
    let updated = if used.category == ItemCategory::Consume {
        let definition = state
            .catalog
            .consume_effect_definition(used.item_id)
            .ok_or(crate::items::ItemRuleError::UnusableItem {
                item_id: used.item_id,
            })
            .map_err(item_rule_error)?;
        crate::effects::apply_consume_effect(
            used.player,
            &mut mutation.effects,
            definition,
            activity_time_ms,
        )
    } else {
        used.player
    };
    let (transaction, _) =
        prepare_player_mutation(&state, mutation, updated, consumes_inventory, true);
    let committed = crate::player_transaction::commit_player_transaction(
        &state.database,
        &player_guard,
        transaction,
    )
    .await?;
    let player = committed.player;
    let effects = committed
        .effects
        .expect("use item transaction stages active effects");
    let quest_indicators =
        current_map_quest_indicators(&state, &player, &effects, activity_time_ms).await;

    Ok(Protobuf(ItemActionResponse {
        player: Some(player),
        dropped_item: None,
        picked_up_drop_id: String::new(),
        active_buffs: Some(crate::effects::state(&effects, activity_time_ms)),
        used_setup_item_id,
        quest_indicators,
    }))
}

pub async fn pick_up_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<ItemActionResponse>, ApiError> {
    let request: PickUpItemRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let player_guard = lock_player(&state, &player_id).await?;
    let activity_time_ms = unix_time_ms()?;
    let mutation =
        begin_player_mutation(&state, &player_guard, &player_id, activity_time_ms).await?;
    require_living_player(&mutation.player, "pick up items")?;
    let position = mutation
        .player
        .position
        .ok_or(crate::movement::MovementError::MissingPlayerPosition)?;
    let picked = crate::items::pick_up_nearest(
        &state.drops,
        mutation.player.clone(),
        position,
        state.catalog.as_ref(),
    )
    .map_err(pick_up_error)?;
    let map_id = picked.player.map_id;
    let picked_up_drop_id = picked.drop.id.clone();
    let (mut transaction, _) =
        prepare_player_mutation(&state, mutation, picked.player.clone(), true, true);
    crate::player_transaction::stage_pickup(&mut transaction, state.drops.clone(), map_id, &picked);
    let committed = crate::player_transaction::commit_player_transaction(
        &state.database,
        &player_guard,
        transaction,
    )
    .await?;
    let player = committed.player;
    let effects = committed
        .effects
        .expect("pick up item transaction stages active effects");
    let quest_indicators =
        current_map_quest_indicators(&state, &player, &effects, activity_time_ms).await;

    Ok(Protobuf(ItemActionResponse {
        player: Some(player),
        dropped_item: None,
        picked_up_drop_id,
        active_buffs: Some(crate::effects::state(&effects, activity_time_ms)),
        used_setup_item_id: 0,
        quest_indicators,
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

pub async fn get_skill_book(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<GetSkillBookResponse>, ApiError> {
    let request: GetSkillBookRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let player_guard = lock_player(&state, &player_id).await?;
    let player = require_player(&state, &player_guard, &player_id).await?;
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
    let player_guard = lock_player(&state, &player_id).await?;
    let activity_time_ms = unix_time_ms()?;
    let mutation =
        begin_player_mutation(&state, &player_guard, &player_id, activity_time_ms).await?;
    let skill_context = load_skill_book(&state, &mutation.player).await?;
    let updated = crate::skills::allocate_skill_point(
        mutation.player.clone(),
        &skill_context,
        request.skill_id,
    )
    .map_err(skill_rule_error)?;
    let (transaction, active_buffs) =
        prepare_player_mutation(&state, mutation, updated, true, true);
    let committed = crate::player_transaction::commit_player_transaction(
        &state.database,
        &player_guard,
        transaction,
    )
    .await?;
    let player = committed.player;
    let skill_context = load_skill_book(&state, &player).await?;
    let skill_book =
        crate::skills::personalize_skill_book(skill_context, &player).map_err(skill_rule_error)?;

    Ok(Protobuf(AllocateSkillPointResponse {
        player: Some(player),
        skill_book: Some(skill_book),
        active_buffs: Some(active_buffs),
    }))
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
    let player_guard = lock_player(&state, &player_id).await?;
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

pub async fn use_skill(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<UseSkillResponse>, ApiError> {
    let request: UseSkillRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let player_guard = lock_player(&state, &player_id).await?;
    let now_ms = unix_time_ms()?;
    let mutation = begin_player_mutation(&state, &player_guard, &player_id, now_ms).await?;
    require_living_player(&mutation.player, "use skills")?;
    let mut effects = mutation.effects.clone();
    let map = load_map(&state, mutation.player.map_id)
        .await?
        .ok_or_else(|| {
            ApiError::not_found(
                "map_not_found",
                format!("map {} does not exist", mutation.player.map_id),
            )
        })?;
    let synchronized = movement::submit_action_movement(
        &state,
        &mutation.player,
        request.movement,
        crate::movement::MovementModifiers {
            speed: effects.projected().modifiers.speed,
            jump: effects.projected().modifiers.jump,
        },
        &map,
        now_ms,
    )?;
    let player = synchronized.player;
    let player_layer = synchronized.platform_layer;
    let equipment =
        crate::items::equipment_stats(&player, state.catalog.as_ref()).map_err(item_rule_error)?;
    let projected_effects = project_combat_effects(effects.projected(), equipment);
    let skill_context = load_skill_book(&state, &player).await?;
    let skill_job_id = skill_context
        .book
        .skills
        .iter()
        .filter_map(|skill| skill.definition.as_ref())
        .find(|definition| definition.skill_id == request.skill_id)
        .map_or_else(
            || {
                mutation
                    .player
                    .stats
                    .as_ref()
                    .map_or(0, |stats| stats.job_id)
            },
            |definition| definition.job_id,
        );
    let prepared = crate::skills::prepare_skill_use(
        player,
        &skill_context,
        request.skill_id,
        &state.formulas,
        projected_effects,
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
    let mut dropped_items = crate::items::map_drops(&state.drops, map.id)?;
    let reservation = crate::skills::reserve_skill_cooldown(
        &state.skill_cooldowns,
        player_id.as_str(),
        request.skill_id,
        now_ms,
        prepared.cooldown_ms,
    )
    .map_err(skill_rule_error)?;
    let mut transaction = crate::player_transaction::new_player_transaction(
        mutation.original,
        prepared.player.clone(),
        crate::player_transaction::PlayerPersistence::Full,
    );
    crate::player_transaction::stage_skill_cooldown(
        &mut transaction,
        state.skill_cooldowns.clone(),
        reservation,
    );
    crate::effects::apply_skill_effect(&mut effects, &prepared.result, now_ms);
    let combat_effects = project_combat_effects(effects.projected(), equipment);
    let simulation = match crate::mobs::use_player_attack_with_effects(
        &state.mobs,
        &map,
        &prepared.player,
        player_layer,
        crate::mobs::PlayerAttack {
            target_mob_id: &request.target_mob_id,
            source_skill_id: Some(prepared.result.skill_id),
            facing_left: request.facing_left,
            minimum_damage: prepared.result.minimum_damage,
            maximum_damage: prepared.result.maximum_damage,
            fixed_damage: prepared.result.fixed_damage,
            attack_type: prepared.attack_type,
        },
        combat_effects,
    )
    .await
    {
        Ok(simulation) => simulation,
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
    let prepared_persistence = prepare_simulation_player_effects(
        &state,
        prepared.player,
        &simulation,
        effects,
        now_ms,
        true,
    );
    crate::player_transaction::replace_staged_player(
        &mut transaction,
        prepared_persistence.player,
        crate::player_transaction::PlayerPersistence::Full,
    );
    crate::player_transaction::stage_effects(
        &mut transaction,
        state.active_effects.clone(),
        mutation.original_effects,
        prepared_persistence.effects,
    );
    crate::player_transaction::stage_mob_update(
        &mut transaction,
        state.mobs.clone(),
        state.drops.clone(),
        simulation,
    )?;
    crate::player_transaction::stage_activity(
        &mut transaction,
        state.recovery_timers.clone(),
        player_id.as_str().to_owned(),
        now_ms,
    );
    let committed = crate::player_transaction::commit_player_transaction(
        &state.database,
        &player_guard,
        transaction,
    )
    .await?;
    let player = committed.player;
    let effects = committed.effects.expect("skill transaction stages effects");
    merge_dropped_items(&mut dropped_items, committed.committed_drops);
    let simulation = committed
        .mob_update
        .expect("skill transaction stages a mob update");
    let active_buffs = crate::effects::state(&effects, now_ms);
    let quest_indicators = quest_indicator_updates(&state, &map, &player, &effects, now_ms);

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
        reactors: simulation.reactors,
        quest_indicators,
    }))
}

pub async fn recover_player(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<RecoverPlayerResponse>, ApiError> {
    let request: RecoverPlayerRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let player_guard = lock_player(&state, &player_id).await?;
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
    let requested = request.player.ok_or_else(|| {
        ApiError::bad_request("missing_player", "request does not contain a player")
    })?;
    crate::keymap::validate_bindings(&requested.key_bindings)
        .map_err(|error| ApiError::bad_request("invalid_key_bindings", error.to_string()))?;

    let player_id = PlayerId::parse(&requested.id)
        .map_err(|error| ApiError::bad_request("invalid_player_id", error.to_string()))?;
    let player_guard = lock_player(&state, &player_id).await?;
    let now_unix_ms = unix_time_ms()?;
    let mutation = begin_player_mutation(&state, &player_guard, &player_id, now_unix_ms).await?;
    let skill_context = load_skill_book(&state, &mutation.player).await?;
    crate::skills::validate_bound_skills(&requested.key_bindings, &mutation.player, &skill_context)
        .map_err(skill_rule_error)?;
    let player = crate::database::apply_player_preferences(mutation.player.clone(), &requested);
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
    crate::player_transaction::stage_movement_persistence(
        &mut transaction,
        state.movement.clone(),
        player_id.as_str().to_owned(),
        now_unix_ms,
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
) -> Result<crate::player_lock::PlayerGuard, ApiError> {
    Ok(crate::player_lock::acquire_player(&state.player_locks, player_id.as_str()).await?)
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
