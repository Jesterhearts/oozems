use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use oozems_proto::v1::BasicAttackRequest;
use oozems_proto::v1::BasicAttackResponse;

use super::ApiError;
use super::Protobuf;
use super::begin_player_mutation;
use super::decode_request;
use super::load_map;
use super::lock_player;
use super::merge_dropped_items;
use super::parse_player_id;
use super::prepare_simulation_player_effects;
use super::project_combat_effects;
use super::quest_indicator_updates;
use super::unix_time_ms;
use crate::app::AppState;

pub async fn use_basic_attack(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Protobuf<BasicAttackResponse>, ApiError> {
    let request: BasicAttackRequest = decode_request(&headers, body)?;
    let player_id = parse_player_id(&request.player_id)?;
    let player_guard = lock_player(&state, &player_id, &headers).await?;
    let now_ms = unix_time_ms()?;
    let mutation = begin_player_mutation(&state, &player_guard, &player_id, now_ms).await?;
    let mut player = mutation.player.clone();
    let effects = mutation.effects.clone();
    super::require_living_player(&player, "attack")?;
    if effects.attacks_disabled() {
        return Err(ApiError::bad_request(
            "invalid_attack_action",
            "the active morph does not allow attacks",
        ));
    }
    let map = load_map(&state, player.map_id).await?.ok_or_else(|| {
        ApiError::not_found(
            "map_not_found",
            format!("map {} does not exist", player.map_id),
        )
    })?;
    let synchronized = super::movement::submit_action_movement(
        &state,
        &player,
        request.movement,
        crate::movement::MovementModifiers {
            speed: effects.projected().modifiers.speed,
            jump: effects.projected().modifiers.jump,
        },
        &map,
        now_ms,
    )?;
    player = synchronized.player;
    let player_layer = synchronized.platform_layer;
    let equipment = crate::items::equipment_stats(&player, state.catalog.as_ref())
        .map_err(super::item_rule_error)?;
    let skill_context = super::load_skill_book(&state, &player).await?;
    let learned = crate::skills::learned_skill_modifiers(&skill_context, &player)
        .map_err(super::skill_rule_error)?;
    let weapon = crate::items::equipped_weapon_type(&player);
    let weapon_family = weapon.map(|weapon| weapon.family);
    let combat_effects =
        project_combat_effects(effects.projected(), equipment, learned, weapon_family);
    let attack_reach = player
        .inventory
        .as_ref()
        .and_then(|inventory| state.catalog.basic_attack_reach(&inventory.equipment));
    let attack_animation_duration = player
        .appearance
        .as_ref()
        .map(|appearance| state.catalog.basic_attack_duration(appearance))
        .transpose()?
        .flatten();
    let attack_interval = crate::attacks::basic_attack_interval(
        state.gameplay.combat.player_attack_interval,
        attack_animation_duration,
    );
    let damage = crate::attacks::calculate_basic_attack(
        &player,
        &state.formulas,
        combat_effects.modifiers.weapon_attack,
        combat_effects.modifiers.strength,
        crate::skills::weapon_mastery(learned, weapon_family),
        weapon,
        combat_effects.modifiers.outgoing_damage_percent,
    )
    .map_err(attack_rule_error)?;
    let mut dropped_items = crate::items::map_drops(&state.drops, map.id)?;
    let attack_reservation = crate::attacks::reserve_basic_attack(
        &state.basic_attack_cooldowns,
        player_id.as_str(),
        now_ms,
        attack_interval,
    )
    .map_err(attack_rule_error)?;
    let mut transaction = crate::player_transaction::new_player_transaction(
        mutation.original,
        player.clone(),
        crate::player_transaction::PlayerPersistence::None,
    );
    crate::player_transaction::stage_basic_attack(&mut transaction, attack_reservation);
    let attack = crate::mobs::PlayerAttack {
        target_mob_id: "",
        source_skill_id: None,
        facing_left: request.facing_left,
        minimum_damage: damage.minimum,
        maximum_damage: damage.maximum,
        fixed_damage: false,
        attack_type: crate::jobs::SkillAttackType::Physical,
    };
    let simulation_result = match attack_reach {
        Some(reach) => {
            crate::mobs::use_player_attack_with_reach(
                &state.mobs,
                &map,
                &player,
                player_layer,
                attack,
                reach,
                combat_effects,
            )
            .await
        }
        None => {
            crate::mobs::use_player_attack_with_effects(
                &state.mobs,
                &map,
                &player,
                player_layer,
                attack,
                combat_effects,
            )
            .await
        }
    };
    let simulation = match simulation_result {
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
    let prepared = prepare_simulation_player_effects(
        &state,
        player,
        &simulation,
        effects,
        now_ms,
        true,
        false,
    );
    crate::player_transaction::replace_staged_player(
        &mut transaction,
        prepared.player,
        if prepared.should_save {
            crate::player_transaction::PlayerPersistence::Full
        } else {
            crate::player_transaction::PlayerPersistence::None
        },
    );
    crate::player_transaction::stage_effects(
        &mut transaction,
        state.active_effects.clone(),
        mutation.original_effects,
        prepared.effects,
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
    let effects = committed.effects.expect("basic attack stages effects");
    merge_dropped_items(&mut dropped_items, committed.committed_drops);
    let simulation = committed
        .mob_update
        .expect("basic attack stages a mob update");
    let quest_indicators = quest_indicator_updates(&state, &map, &player, &effects, now_ms);

    Ok(Protobuf(BasicAttackResponse {
        player: Some(player),
        mobs: simulation.mobs,
        mob_projectiles: simulation.mob_projectiles,
        combat_events: simulation.combat_events,
        simulation_sequence: simulation.sequence,
        dropped_items,
        active_buffs: Some(crate::effects::state(&effects, now_ms)),
        reactors: simulation.reactors,
        quest_indicators,
    }))
}

fn attack_rule_error(error: crate::attacks::AttackRuleError) -> ApiError {
    match error {
        crate::attacks::AttackRuleError::Cooldown { .. } => {
            ApiError::bad_request("invalid_attack_action", error.to_string())
        }
        _ => ApiError::AttackRules(error),
    }
}
