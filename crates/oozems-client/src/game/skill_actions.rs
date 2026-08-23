use std::cell::RefCell;
use std::rc::Rc;

use oozems_proto::v1::CombatEventKind;
use oozems_proto::v1::SkillUseResult;
use wasm_bindgen_futures::spawn_local;

use super::Game;
use crate::api;
use crate::game_gui::GuiAction;
use crate::show_status;
use crate::skill_effects;

pub(super) fn begin(
    game: Rc<RefCell<Game>>,
    action: GuiAction,
) {
    if super::recovery_actions::is_in_flight(&game.borrow().recovery_state) {
        show_status("Recovery is still being saved.", true);
        return;
    }
    super::recovery_actions::reset(&mut game.borrow_mut().recovery_state);
    let in_flight = game.borrow().skill_action_in_flight.clone();
    if in_flight.replace(true) {
        show_status("A skill action is already in progress.", true);
        return;
    }
    let (player_id, target_mob_id, facing_left) = {
        let game = game.borrow();
        (
            game.player.id.clone(),
            select_target(&game).unwrap_or_default(),
            game.facing_left,
        )
    };
    spawn_local(async move {
        let result = match action {
            GuiAction::AllocateSkill { skill_id } => {
                match api::allocate_skill_point(&player_id, skill_id).await {
                    Ok(response) => install_allocation(&mut game.borrow_mut(), response),
                    Err(error) => Err(format!("Skill point allocation failed: {error}")),
                }
            }
            GuiAction::UseSkill { skill_id } => {
                match api::use_skill(&player_id, skill_id, &target_mob_id, facing_left).await {
                    Ok(response) => install_use(&mut game.borrow_mut(), response),
                    Err(error) => Err(format!("Skill use failed: {error}")),
                }
            }
            _ => unreachable!("non-skill action reached the skill request pipeline"),
        };
        match result {
            Ok(message) => show_status(&message, false),
            Err(error) => show_status(&error, true),
        }
        in_flight.set(false);
    });
}

pub(super) fn begin_basic_attack(game: Rc<RefCell<Game>>) {
    if super::recovery_actions::is_in_flight(&game.borrow().recovery_state) {
        show_status("Recovery is still being saved.", true);
        return;
    }
    super::recovery_actions::reset(&mut game.borrow_mut().recovery_state);
    let in_flight = game.borrow().skill_action_in_flight.clone();
    if in_flight.replace(true) {
        show_status("A combat action is already in progress.", true);
        return;
    }
    super::start_character_attack_animation(&mut game.borrow_mut());
    let (player_id, facing_left) = {
        let game = game.borrow();
        (game.player.id.clone(), game.facing_left)
    };
    spawn_local(async move {
        let result = match api::use_basic_attack(&player_id, facing_left).await {
            Ok(response) => install_basic_attack(&mut game.borrow_mut(), response),
            Err(error) => Err(format!("Basic attack failed: {error}")),
        };
        match result {
            Ok(message) => show_status(&message, false),
            Err(error) => show_status(&error, true),
        }
        in_flight.set(false);
    });
}

fn install_allocation(
    game: &mut Game,
    response: oozems_proto::v1::AllocateSkillPointResponse,
) -> Result<String, String> {
    let player = response
        .player
        .ok_or("skill allocation response did not contain a player")?;
    let skill_book = response
        .skill_book
        .ok_or("skill allocation response did not contain a skill book")?;
    game.player.skill_points = player.skill_points;
    game.player.learned_skills = player.learned_skills;
    game.skill_book = skill_book;
    Ok("Skill point allocated.".to_owned())
}

fn install_use(
    game: &mut Game,
    mut response: oozems_proto::v1::UseSkillResponse,
) -> Result<String, String> {
    let player = response
        .player
        .ok_or("skill use response did not contain a player")?;
    let result = response
        .result
        .ok_or("skill use response did not contain a result")?;
    let (actual_damage, target_position) = install_combat_update(
        game,
        player,
        response.simulation_sequence,
        std::mem::take(&mut response.mobs),
        std::mem::take(&mut response.mob_projectiles),
        std::mem::take(&mut response.combat_events),
    )?;
    super::buffs::install(
        &mut game.active_buffs,
        response.active_buffs.unwrap_or_default(),
        js_sys::Date::now().max(0.0) as u64,
    );
    skill_effects::install(game, response.effect.unwrap_or_default(), target_position);
    Ok(use_message(game, &result, actual_damage))
}

fn install_basic_attack(
    game: &mut Game,
    response: oozems_proto::v1::BasicAttackResponse,
) -> Result<String, String> {
    let player = response
        .player
        .ok_or("basic attack response did not contain a player")?;
    let (actual_damage, _) = install_combat_update(
        game,
        player,
        response.simulation_sequence,
        response.mobs,
        response.mob_projectiles,
        response.combat_events,
    )?;
    Ok(actual_damage.map_or_else(
        || "Basic attack did not hit a target.".to_owned(),
        |damage| format!("Basic attack dealt {damage} damage."),
    ))
}

fn install_combat_update(
    game: &mut Game,
    player: oozems_proto::v1::PlayerState,
    simulation_sequence: u64,
    mobs: Vec<oozems_proto::v1::Mob>,
    mob_projectiles: Vec<oozems_proto::v1::MobProjectile>,
    combat_events: Vec<oozems_proto::v1::CombatEvent>,
) -> Result<(Option<u64>, Option<oozems_proto::v1::Vec2>), String> {
    validate_combat_map(game.map.id, player.map_id)?;
    let hit = combat_events
        .iter()
        .find(|event| CombatEventKind::try_from(event.kind) == Ok(CombatEventKind::PlayerHitMob));
    let actual_damage = hit.map(|event| event.damage);
    let target_position = hit.and_then(|event| event.position);
    if crate::mob_render::accept_simulation_snapshot(&mut game.mob_render, simulation_sequence) {
        game.player.stats = player.stats;
        crate::mob_render::install_snapshot(
            &mut game.mob_render,
            &mut game.map.mobs,
            mobs,
            game.frame_time_ms,
            game.movement_rules.snapshot_interval_ms,
        );
        crate::mob_render::install_projectile_snapshot(
            &mut game.mob_render,
            &mut game.map.mob_projectiles,
            mob_projectiles,
            game.frame_time_ms,
            game.movement_rules.snapshot_interval_ms,
        );
    }
    crate::mob_render::install_combat_events(
        &mut game.mob_render,
        combat_events,
        game.frame_time_ms,
    );
    Ok((actual_damage, target_position))
}

fn validate_combat_map(
    current_map_id: u32,
    response_map_id: u32,
) -> Result<(), String> {
    if response_map_id == current_map_id {
        Ok(())
    } else {
        Err(format!(
            "ignored combat response for map {response_map_id}; current map is {current_map_id}"
        ))
    }
}

fn select_target(game: &Game) -> Option<String> {
    let player = game.player.position?;
    game.map
        .mobs
        .iter()
        .filter(|mob| mob.current_hp > 0 && mob.layer == game.motion.platform_layer)
        .filter_map(|mob| {
            let position = crate::mob_render::position(&game.mob_render, mob, game.frame_time_ms)?;
            let delta_x = position.x - player.x;
            let in_front = if game.facing_left {
                delta_x <= 0.0
            } else {
                delta_x >= 0.0
            };
            (in_front && (position.y - player.y).abs() <= 90.0)
                .then_some((mob.id.clone(), delta_x.abs()))
        })
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(mob_id, _)| mob_id)
}

fn use_message(
    game: &Game,
    result: &SkillUseResult,
    actual_damage: Option<u64>,
) -> String {
    let name = game
        .skill_book
        .skills
        .iter()
        .filter_map(|skill| skill.definition.as_ref())
        .find(|definition| definition.skill_id == result.skill_id)
        .map_or_else(
            || format!("Skill {}", result.skill_id),
            |definition| definition.name.clone(),
        );
    if result.has_damage {
        if let Some(damage) = actual_damage {
            return format!("Used {name}. Dealt {damage} damage.");
        }
        return format!(
            "Used {name}. No target hit. Damage range: {}-{}.",
            result.minimum_damage, result.maximum_damage
        );
    }
    if result.hp_restored > 0 && result.mp_restored > 0 {
        return format!(
            "Used {name}. Restored {} HP and {} MP.",
            result.hp_restored, result.mp_restored
        );
    }
    if result.hp_restored > 0 {
        return format!("Used {name}. Restored {} HP.", result.hp_restored);
    }
    if result.mp_restored > 0 {
        return format!("Used {name}. Restored {} MP.", result.mp_restored);
    }
    format!("Used {name}.")
}

#[cfg(test)]
mod tests {
    use super::validate_combat_map;

    #[test]
    fn combat_responses_from_an_old_map_are_rejected() {
        assert!(validate_combat_map(2, 1).is_err());
        validate_combat_map(2, 2).expect("current map response");
    }
}
