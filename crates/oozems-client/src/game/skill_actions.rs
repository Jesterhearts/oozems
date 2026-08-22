use std::cell::RefCell;
use std::rc::Rc;

use oozems_proto::v1::SkillUseResult;
use wasm_bindgen_futures::spawn_local;

use super::ActiveSkillEffect;
use super::Game;
use crate::api;
use crate::game_gui::GuiAction;
use crate::show_status;

pub(super) fn begin(
    game: Rc<RefCell<Game>>,
    action: GuiAction,
) {
    let in_flight = game.borrow().skill_action_in_flight.clone();
    if in_flight.replace(true) {
        show_status("A skill action is already in progress.", true);
        return;
    }
    let player_id = game.borrow().player.id.clone();
    spawn_local(async move {
        let result = match action {
            GuiAction::AllocateSkill { skill_id } => {
                match api::allocate_skill_point(&player_id, skill_id).await {
                    Ok(response) => install_allocation(&mut game.borrow_mut(), response),
                    Err(error) => Err(format!("Skill point allocation failed: {error}")),
                }
            }
            GuiAction::UseSkill { skill_id } => match api::use_skill(&player_id, skill_id).await {
                Ok(response) => install_use(&mut game.borrow_mut(), response),
                Err(error) => Err(format!("Skill use failed: {error}")),
            },
            _ => unreachable!("non-skill action reached the skill request pipeline"),
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
    response: oozems_proto::v1::UseSkillResponse,
) -> Result<String, String> {
    let player = response
        .player
        .ok_or("skill use response did not contain a player")?;
    let result = response
        .result
        .ok_or("skill use response did not contain a result")?;
    game.player.stats = player.stats;
    install_active_effect(game, &result);
    Ok(use_message(game, &result))
}

fn install_active_effect(
    game: &mut Game,
    result: &SkillUseResult,
) {
    game.active_skill_effects
        .retain(|effect| effect.skill_id != result.skill_id);
    if result.duration_ms == 0 || (result.speed_bonus == 0 && result.jump_bonus == 0) {
        return;
    }
    game.active_skill_effects.push(ActiveSkillEffect {
        skill_id: result.skill_id,
        speed_bonus: result.speed_bonus,
        jump_bonus: result.jump_bonus,
        expires_at_ms: game.frame_time_ms + result.duration_ms as f64,
    });
}

fn use_message(
    game: &Game,
    result: &SkillUseResult,
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
        return format!(
            "Used {name}. Damage: {}-{}.",
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
