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
    if matches!(action, GuiAction::UseSkill { .. }) {
        let game = game.borrow();
        if game.active_buffs.attacks_disabled {
            show_status("The active morph does not allow attacks.", true);
            return;
        }
        if game.transition_in_flight.get() {
            show_status("A map transition is already in progress.", true);
            return;
        }
    }
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
        let request_started_ms = super::monotonic_time_ms();
        let result = match action {
            GuiAction::AllocateSkill { skill_id } => {
                match api::allocate_skill_point(&player_id, skill_id).await {
                    Ok(response) => {
                        install_allocation(&mut game.borrow_mut(), response, request_started_ms)
                    }
                    Err(error) => Err(format!("Skill point allocation failed: {error}")),
                }
            }
            GuiAction::UseSkill { skill_id } => {
                match api::use_skill(&player_id, skill_id, &target_mob_id, facing_left).await {
                    Ok(response) => {
                        install_use(&mut game.borrow_mut(), response, request_started_ms)
                    }
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
    if game.borrow().active_buffs.attacks_disabled {
        show_status("The active morph does not allow attacks.", true);
        return;
    }
    if game.borrow().transition_in_flight.get() {
        show_status("A map transition is already in progress.", true);
        return;
    }
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
        let request_started_ms = super::monotonic_time_ms();
        let result = match api::use_basic_attack(&player_id, facing_left).await {
            Ok(response) => {
                install_basic_attack(&mut game.borrow_mut(), response, request_started_ms)
            }
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
    mut response: oozems_proto::v1::AllocateSkillPointResponse,
    request_started_ms: f64,
) -> Result<String, String> {
    let player = response
        .player
        .ok_or("skill allocation response did not contain a player")?;
    let skill_book = response
        .skill_book
        .ok_or("skill allocation response did not contain a skill book")?;
    let installed = super::install_full_player_update(game, player);
    if installed.domains.skills {
        game.skill_book = skill_book;
    }
    super::install_active_buffs(
        game,
        response.active_buffs.take().unwrap_or_default(),
        request_started_ms,
    );
    Ok("Skill point allocated.".to_owned())
}

fn install_use(
    game: &mut Game,
    mut response: oozems_proto::v1::UseSkillResponse,
    request_started_ms: f64,
) -> Result<String, String> {
    let player = response
        .player
        .ok_or("skill use response did not contain a player")?;
    let result = response
        .result
        .ok_or("skill use response did not contain a result")?;
    let outcome = install_combat_update(
        game,
        player,
        response.simulation_sequence,
        std::mem::take(&mut response.mobs),
        std::mem::take(&mut response.mob_projectiles),
        std::mem::take(&mut response.combat_events),
        std::mem::take(&mut response.dropped_items),
    )?;
    super::install_active_buffs(
        game,
        response.active_buffs.unwrap_or_default(),
        request_started_ms,
    );
    skill_effects::install(
        game,
        response.effect.unwrap_or_default(),
        outcome.position(),
    );
    Ok(use_message(game, &result, &outcome))
}

fn install_basic_attack(
    game: &mut Game,
    mut response: oozems_proto::v1::BasicAttackResponse,
    request_started_ms: f64,
) -> Result<String, String> {
    let player = response
        .player
        .ok_or("basic attack response did not contain a player")?;
    let outcome = install_combat_update(
        game,
        player,
        response.simulation_sequence,
        std::mem::take(&mut response.mobs),
        std::mem::take(&mut response.mob_projectiles),
        std::mem::take(&mut response.combat_events),
        std::mem::take(&mut response.dropped_items),
    )?;
    super::install_active_buffs(
        game,
        response.active_buffs.take().unwrap_or_default(),
        request_started_ms,
    );
    Ok(match outcome {
        PlayerAttackOutcome::Hit { damage, .. } => {
            format!("Basic attack dealt {damage} damage.")
        }
        PlayerAttackOutcome::Miss { .. } => "Basic attack missed.".to_owned(),
        PlayerAttackOutcome::NoTarget => "Basic attack found no target.".to_owned(),
    })
}

#[derive(Clone, Debug, PartialEq)]
enum PlayerAttackOutcome {
    Hit {
        damage: u64,
        position: Option<oozems_proto::v1::Vec2>,
    },
    Miss {
        position: Option<oozems_proto::v1::Vec2>,
    },
    NoTarget,
}

impl PlayerAttackOutcome {
    fn position(&self) -> Option<oozems_proto::v1::Vec2> {
        match self {
            Self::Hit { position, .. } | Self::Miss { position } => *position,
            Self::NoTarget => None,
        }
    }
}

fn install_combat_update(
    game: &mut Game,
    player: oozems_proto::v1::PlayerState,
    simulation_sequence: u64,
    mobs: Vec<oozems_proto::v1::Mob>,
    mob_projectiles: Vec<oozems_proto::v1::MobProjectile>,
    combat_events: Vec<oozems_proto::v1::CombatEvent>,
    dropped_items: Vec<oozems_proto::v1::DroppedItem>,
) -> Result<PlayerAttackOutcome, String> {
    let response_map_id = player.map_id;
    super::install_full_player_update(game, player);
    validate_combat_map(game.map.id, response_map_id)?;
    let outcome = player_attack_outcome(&combat_events);
    if crate::mob_render::accept_simulation_snapshot(&mut game.mob_render, simulation_sequence) {
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
        game.map.dropped_items = dropped_items;
    }
    crate::mob_render::install_combat_events(
        &mut game.mob_render,
        combat_events,
        game.frame_time_ms,
    );
    Ok(outcome)
}

fn player_attack_outcome(events: &[oozems_proto::v1::CombatEvent]) -> PlayerAttackOutcome {
    if let Some(hit) = events
        .iter()
        .find(|event| CombatEventKind::try_from(event.kind) == Ok(CombatEventKind::PlayerHitMob))
    {
        return PlayerAttackOutcome::Hit {
            damage: hit.damage,
            position: hit.position,
        };
    }
    events
        .iter()
        .find(|event| CombatEventKind::try_from(event.kind) == Ok(CombatEventKind::PlayerMissedMob))
        .map_or(PlayerAttackOutcome::NoTarget, |miss| {
            PlayerAttackOutcome::Miss {
                position: miss.position,
            }
        })
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
    outcome: &PlayerAttackOutcome,
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
        match outcome {
            PlayerAttackOutcome::Hit { damage, .. } => {
                return format!("Used {name}. Dealt {damage} damage.");
            }
            PlayerAttackOutcome::Miss { .. } => return format!("Used {name}. Missed."),
            PlayerAttackOutcome::NoTarget => {}
        }
        return format!(
            "Used {name}. No target. Damage range: {}-{}.",
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
    use oozems_proto::v1::CombatEvent;
    use oozems_proto::v1::CombatEventKind;
    use oozems_proto::v1::Vec2;

    use super::PlayerAttackOutcome;
    use super::player_attack_outcome;
    use super::validate_combat_map;

    #[test]
    fn combat_responses_from_an_old_map_are_rejected() {
        assert!(validate_combat_map(2, 1).is_err());
        validate_combat_map(2, 2).expect("current map response");
    }

    #[test]
    fn combat_status_distinguishes_hits_misses_and_absent_targets() {
        let position = Vec2 { x: 10.0, y: 20.0 };
        let event = |kind, damage| CombatEvent {
            kind: kind as i32,
            damage,
            position: Some(position),
            ..CombatEvent::default()
        };

        assert_eq!(
            player_attack_outcome(&[event(CombatEventKind::PlayerHitMob, 7)]),
            PlayerAttackOutcome::Hit {
                damage: 7,
                position: Some(position),
            }
        );
        assert_eq!(
            player_attack_outcome(&[event(CombatEventKind::PlayerMissedMob, 0)]),
            PlayerAttackOutcome::Miss {
                position: Some(position),
            }
        );
        assert_eq!(player_attack_outcome(&[]), PlayerAttackOutcome::NoTarget);
    }
}
