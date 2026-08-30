use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use oozems_proto::v1::AllocateSkillPointRequest;
use oozems_proto::v1::AllocateSkillPointResponse;
use oozems_proto::v1::GetSkillBookRequest;
use oozems_proto::v1::GetSkillBookResponse;
use oozems_proto::v1::UseSkillRequest;
use oozems_proto::v1::UseSkillResponse;

use super::ApiError;
use super::Protobuf;
use super::active_buff_state;
use super::begin_player_mutation;
use super::decode_request;
use super::item_rule_error;
use super::load_map;
use super::load_skill_book;
use super::lock_player;
use super::merge_dropped_items;
use super::parse_player_id;
use super::prepare_player_mutation;
use super::prepare_simulation_player_effects;
use super::project_combat_effects;
use super::quest_indicator_updates;
use super::require_living_player;
use super::require_player;
use super::skill_rule_error;
use super::unix_time_ms;
use crate::app::AppState;

pub async fn get_skill_book(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<GetSkillBookResponse>, ApiError> {
    let request: GetSkillBookRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let player_guard = lock_player(&state, &player_id, &headers).await?;
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
    let player_guard = lock_player(&state, &player_id, &headers).await?;
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

pub async fn use_skill(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<UseSkillResponse>, ApiError> {
    let request: UseSkillRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let player_guard = lock_player(&state, &player_id, &headers).await?;
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
    let synchronized = super::movement::submit_action_movement(
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
    crate::player_transaction::stage_skill_cooldown(&mut transaction, reservation);
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
