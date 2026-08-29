use oozems_proto::v1::MobAnimation;
use oozems_proto::v1::MobDefinition;
use oozems_proto::v1::MobFrame;
use oozems_proto::v1::MobMovementMode;

use crate::assets;
use crate::game::Game;

const HEALTH_BAR_HEIGHT: f32 = 5.0;
const HEALTH_BAR_GAP: f32 = 3.0;

pub(super) fn draw(
    game: &Game,
    camera_x: f64,
    camera_y: f64,
    layer: i32,
) {
    for mob in &game.world.map.mobs {
        if mob.layer != layer {
            continue;
        }
        let Some(position) =
            crate::mob_render::position(&game.world.mob_render, mob, game.clock.now_ms)
        else {
            continue;
        };
        let Some(definition) = definition(game, mob.definition_id) else {
            continue;
        };
        let Some(preferred) =
            frame_selection(&game.world.mob_render, mob, definition, game.clock.now_ms)
        else {
            continue;
        };
        if !super::sprite_is_visible(
            game,
            frame_x(position.x, preferred, mob.flip_x),
            position.y - preferred.origin_y,
            preferred.width,
            preferred.height,
            camera_x,
            camera_y,
        ) {
            continue;
        }
        let Some(frame) = drawable_frame(game, definition, preferred) else {
            continue;
        };
        let x = frame_x(position.x, frame, mob.flip_x);
        super::draw_sprite(
            game,
            &frame.asset_id,
            x,
            position.y - frame.origin_y,
            frame.width,
            frame.height,
            mob.flip_x,
            camera_x,
            camera_y,
        );
        if mob.current_hp > 0 {
            draw_health_bar(
                game,
                position.x,
                position.y,
                frame.origin_y,
                mob.current_hp,
                definition.max_hp,
                camera_x,
                camera_y,
            );
        }
    }
    for projectile in &game.world.map.mob_projectiles {
        if projectile.layer != layer {
            continue;
        }
        let Some(position) = crate::mob_render::projectile_position(
            &game.world.mob_render,
            projectile,
            game.clock.now_ms,
        ) else {
            continue;
        };
        draw_projectile(game, position.x, position.y, camera_x, camera_y);
    }
}

pub(super) fn draw_combat_texts(
    game: &Game,
    camera_x: f64,
    camera_y: f64,
) {
    let context = &game.surface.context;
    context.save();
    context.set_font("bold 16px monospace");
    context.set_text_align("center");
    context.set_line_width(3.0);
    context.set_stroke_style_str("#111111");
    for text in crate::mob_render::combat_texts(&game.world.mob_render, game.clock.now_ms) {
        let x = f64::from(text.position.x) - camera_x;
        let y = f64::from(text.position.y) - camera_y - 36.0 - f64::from(text.progress) * 24.0;
        let label = if text.missed {
            "MISS".to_owned()
        } else {
            text.damage.to_string()
        };
        context.set_fill_style_str(if text.player_damage {
            "#ff6666"
        } else {
            "#ffcc33"
        });
        let _ = context.stroke_text(&label, x, y);
        let _ = context.fill_text(&label, x, y);
    }
    context.restore();
}

fn draw_health_bar(
    game: &Game,
    x: f32,
    y: f32,
    frame_origin_y: f32,
    current_hp: u64,
    maximum_hp: u64,
    camera_x: f64,
    camera_y: f64,
) {
    if maximum_hp == 0 || current_hp >= maximum_hp {
        return;
    }
    let left = f64::from(x) - camera_x - 20.0;
    let top = f64::from(health_bar_top(y, frame_origin_y)) - camera_y;
    let fill = current_hp as f64 / maximum_hp as f64;
    game.surface.context.set_fill_style_str("#222222");
    game.surface.context.fill_rect(left, top, 40.0, 5.0);
    game.surface.context.set_fill_style_str("#e53935");
    game.surface
        .context
        .fill_rect(left + 1.0, top + 1.0, 38.0 * fill, 3.0);
}

fn health_bar_top(
    anchor_y: f32,
    frame_origin_y: f32,
) -> f32 {
    anchor_y - frame_origin_y - HEALTH_BAR_GAP - HEALTH_BAR_HEIGHT
}

fn draw_projectile(
    game: &Game,
    x: f32,
    y: f32,
    camera_x: f64,
    camera_y: f64,
) {
    let x = f64::from(x) - camera_x;
    let y = f64::from(y) - camera_y - 24.0;
    game.surface.context.begin_path();
    game.surface.context.set_fill_style_str("#f7d94c");
    game.surface.context.set_stroke_style_str("#b55a16");
    game.surface.context.set_line_width(2.0);
    let _ = game
        .surface
        .context
        .arc(x, y, 6.0, 0.0, std::f64::consts::TAU);
    game.surface.context.fill();
    game.surface.context.stroke();
}

fn definition(
    game: &Game,
    definition_id: u32,
) -> Option<&MobDefinition> {
    game.world
        .map
        .mob_definitions
        .iter()
        .find(|definition| definition.id == definition_id)
}

fn movement_animation(
    definition: &MobDefinition,
    mode: MobMovementMode,
) -> Option<&MobAnimation> {
    let names = match mode {
        MobMovementMode::Attacking => ["attack1", "attack2", "stand"],
        MobMovementMode::Walking => ["move", "fly", "stand"],
        MobMovementMode::Jumping => ["jump", "move", "fly"],
        MobMovementMode::Unspecified | MobMovementMode::Idle => ["stand", "move", "fly"],
    };
    names
        .into_iter()
        .find_map(|name| {
            definition
                .animations
                .iter()
                .find(|animation| animation.name == name && !animation.frames.is_empty())
        })
        .or_else(|| {
            definition
                .animations
                .iter()
                .find(|animation| !animation.frames.is_empty())
        })
}

fn frame_selection<'a>(
    state: &crate::mob_render::MobRenderState,
    mob: &oozems_proto::v1::Mob,
    definition: &'a MobDefinition,
    timestamp_ms: f64,
) -> Option<&'a MobFrame> {
    if let Some((kind, elapsed_ms)) = crate::mob_render::reaction(state, &mob.id, timestamp_ms)
        && (kind != crate::mob_render::MobReactionKind::Death || mob.current_hp == 0)
    {
        let name = match kind {
            crate::mob_render::MobReactionKind::Hit => "hit1",
            crate::mob_render::MobReactionKind::Death => "die1",
        };
        if let Some(animation) = named_animation(definition, name) {
            if let Some(frame) = animation_frame_once(animation, elapsed_ms) {
                return Some(frame);
            }
            if kind == crate::mob_render::MobReactionKind::Death {
                return None;
            }
        } else if kind == crate::mob_render::MobReactionKind::Death {
            return None;
        }
    }
    if mob.current_hp == 0 {
        return None;
    }
    let mode = MobMovementMode::try_from(mob.movement_mode).unwrap_or(MobMovementMode::Idle);
    movement_animation(definition, mode)
        .and_then(|animation| animation_frame(animation, timestamp_ms))
}

fn named_animation<'a>(
    definition: &'a MobDefinition,
    name: &str,
) -> Option<&'a MobAnimation> {
    definition
        .animations
        .iter()
        .find(|animation| animation.name == name && !animation.frames.is_empty())
}

fn animation_frame(
    animation: &MobAnimation,
    timestamp_ms: f64,
) -> Option<&MobFrame> {
    let index = crate::animation::frame_index(
        animation.frames.iter().map(|frame| frame.delay_ms),
        timestamp_ms,
        crate::animation::Playback::Loop,
    )?;
    animation.frames.get(index)
}

fn animation_frame_once(
    animation: &MobAnimation,
    elapsed_ms: f64,
) -> Option<&MobFrame> {
    let index = crate::animation::frame_index(
        animation.frames.iter().map(|frame| frame.delay_ms),
        elapsed_ms,
        crate::animation::Playback::Once,
    )?;
    animation.frames.get(index)
}

fn drawable_frame<'a>(
    game: &Game,
    definition: &'a MobDefinition,
    preferred: &MobFrame,
) -> Option<&'a MobFrame> {
    let preferred_index = definition
        .animations
        .iter()
        .flat_map(|animation| &animation.frames)
        .position(|frame| std::ptr::eq(frame, preferred))?;
    let index = assets::ready_or_fallback_index(
        &game.surface.images,
        definition
            .animations
            .iter()
            .flat_map(|animation| &animation.frames)
            .map(|frame| frame.asset_id.as_str()),
        preferred_index,
    )?;
    definition
        .animations
        .iter()
        .flat_map(|animation| &animation.frames)
        .nth(index)
}

fn frame_x(
    anchor_x: f32,
    frame: &MobFrame,
    flip_x: bool,
) -> f32 {
    if flip_x {
        anchor_x - (frame.width - frame.origin_x)
    } else {
        anchor_x - frame.origin_x
    }
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::CombatEvent;
    use oozems_proto::v1::CombatEventKind;
    use oozems_proto::v1::Mob;
    use oozems_proto::v1::MobAnimation;
    use oozems_proto::v1::MobDefinition;
    use oozems_proto::v1::MobFrame;
    use oozems_proto::v1::MobMovementMode;

    use super::animation_frame;
    use super::frame_selection;
    use super::frame_x;
    use super::health_bar_top;
    use super::movement_animation;

    #[test]
    fn stand_animation_is_preferred_for_idle_mobs() {
        let definition = MobDefinition {
            animations: vec![animation("move", 100), animation("stand", 200)],
            ..MobDefinition::default()
        };

        assert_eq!(
            movement_animation(&definition, MobMovementMode::Idle)
                .expect("animation")
                .name,
            "stand"
        );
    }

    #[test]
    fn movement_mode_selects_walk_and_jump_animations() {
        let definition = MobDefinition {
            animations: vec![
                animation("stand", 100),
                animation("move", 100),
                animation("jump", 100),
            ],
            ..MobDefinition::default()
        };

        assert_eq!(
            movement_animation(&definition, MobMovementMode::Walking)
                .expect("walk animation")
                .name,
            "move"
        );
        assert_eq!(
            movement_animation(&definition, MobMovementMode::Jumping)
                .expect("jump animation")
                .name,
            "jump"
        );
    }

    #[test]
    fn mob_animation_uses_frame_delays() {
        let animation = MobAnimation {
            name: "stand".to_owned(),
            frames: vec![
                MobFrame {
                    asset_id: "first".to_owned(),
                    delay_ms: 100,
                    ..MobFrame::default()
                },
                MobFrame {
                    asset_id: "second".to_owned(),
                    delay_ms: 200,
                    ..MobFrame::default()
                },
            ],
        };

        assert_eq!(
            animation_frame(&animation, 99.0).expect("frame").asset_id,
            "first"
        );
        assert_eq!(
            animation_frame(&animation, 100.0).expect("frame").asset_id,
            "second"
        );
        assert_eq!(
            animation_frame(&animation, 300.0).expect("frame").asset_id,
            "first"
        );
    }

    #[test]
    fn a_hit_animation_plays_once_before_movement_resumes() {
        let definition = MobDefinition {
            animations: vec![animation("stand", 100), animation("hit1", 200)],
            ..MobDefinition::default()
        };
        let mob = Mob {
            id: "slime".to_owned(),
            current_hp: 50,
            movement_mode: MobMovementMode::Idle as i32,
            ..Mob::default()
        };
        let mut state = crate::mob_render::MobRenderState::default();
        crate::mob_render::install_combat_events(
            &mut state,
            vec![CombatEvent {
                kind: CombatEventKind::PlayerHitMob as i32,
                target_id: mob.id.clone(),
                staggered: true,
                ..CombatEvent::default()
            }],
            1_000.0,
        );

        assert_eq!(
            frame_selection(&state, &mob, &definition, 1_199.0)
                .expect("hit frame")
                .asset_id,
            "hit1"
        );
        assert_eq!(
            frame_selection(&state, &mob, &definition, 1_200.0)
                .expect("stand frame")
                .asset_id,
            "stand"
        );
    }

    #[test]
    fn a_dead_mob_is_hidden_after_its_death_animation() {
        let definition = MobDefinition {
            animations: vec![animation("stand", 100), animation("die1", 200)],
            ..MobDefinition::default()
        };
        let mob = Mob {
            id: "slime".to_owned(),
            current_hp: 0,
            ..Mob::default()
        };
        let mut state = crate::mob_render::MobRenderState::default();
        crate::mob_render::install_combat_events(
            &mut state,
            vec![CombatEvent {
                kind: CombatEventKind::MobDied as i32,
                target_id: mob.id.clone(),
                ..CombatEvent::default()
            }],
            1_000.0,
        );

        assert_eq!(
            frame_selection(&state, &mob, &definition, 1_199.0)
                .expect("death frame")
                .asset_id,
            "die1"
        );
        assert!(frame_selection(&state, &mob, &definition, 1_200.0).is_none());
    }

    #[test]
    fn a_stale_death_reaction_does_not_hide_a_living_mob() {
        let definition = MobDefinition {
            animations: vec![animation("stand", 100), animation("die1", 200)],
            ..MobDefinition::default()
        };
        let mob = Mob {
            id: "slime".to_owned(),
            current_hp: 50,
            ..Mob::default()
        };
        let mut state = crate::mob_render::MobRenderState::default();
        crate::mob_render::install_combat_events(
            &mut state,
            vec![CombatEvent {
                kind: CombatEventKind::MobDied as i32,
                target_id: mob.id.clone(),
                ..CombatEvent::default()
            }],
            1_000.0,
        );

        assert_eq!(
            frame_selection(&state, &mob, &definition, 1_100.0)
                .expect("stand frame")
                .asset_id,
            "stand"
        );
    }

    #[test]
    fn flipping_keeps_the_frame_origin_on_the_mob_anchor() {
        let frame = MobFrame {
            width: 40.0,
            origin_x: 15.0,
            ..MobFrame::default()
        };

        assert_eq!(frame_x(100.0, &frame, false), 85.0);
        assert_eq!(frame_x(100.0, &frame, true), 75.0);
    }

    #[test]
    fn health_bar_sits_above_the_sprite_bounds() {
        assert_eq!(health_bar_top(300.0, 40.0), 252.0);
    }

    fn animation(
        name: &str,
        delay_ms: u32,
    ) -> MobAnimation {
        MobAnimation {
            name: name.to_owned(),
            frames: vec![MobFrame {
                asset_id: name.to_owned(),
                delay_ms,
                ..MobFrame::default()
            }],
        }
    }
}
