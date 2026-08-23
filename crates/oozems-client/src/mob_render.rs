use std::collections::HashMap;

use oozems_proto::v1::CombatEvent;
use oozems_proto::v1::CombatEventKind;
use oozems_proto::v1::Mob;
use oozems_proto::v1::MobProjectile;
use oozems_proto::v1::Vec2;

const COMBAT_TEXT_LIFETIME_MS: f64 = 900.0;

#[derive(Default)]
pub struct MobRenderState {
    transitions: HashMap<String, MobTransition>,
    projectile_transitions: HashMap<String, MobTransition>,
    combat_texts: Vec<CombatText>,
    last_simulation_sequence: u64,
}

#[derive(Clone, Copy)]
struct MobTransition {
    from: Vec2,
    to: Vec2,
    started_at_ms: f64,
    duration_ms: f64,
}

#[derive(Clone, Copy)]
struct CombatText {
    position: Vec2,
    damage: u64,
    player_damage: bool,
    started_at_ms: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderedCombatText {
    pub position: Vec2,
    pub damage: u64,
    pub player_damage: bool,
    pub progress: f32,
}

pub fn new_map_state(simulation_sequence: u64) -> MobRenderState {
    MobRenderState {
        last_simulation_sequence: simulation_sequence,
        ..MobRenderState::default()
    }
}

pub fn install_snapshot(
    state: &mut MobRenderState,
    mobs: &mut Vec<Mob>,
    next_mobs: Vec<Mob>,
    timestamp_ms: f64,
    duration_ms: u64,
) {
    let transitions = next_mobs
        .iter()
        .filter_map(|next| {
            let to = finite_position(next)?;
            let from = mobs
                .iter()
                .find(|current| current.id == next.id)
                .and_then(|current| {
                    if current.current_hp == 0 && next.current_hp > 0 {
                        Some(to)
                    } else {
                        position(state, current, timestamp_ms)
                    }
                })
                .unwrap_or(to);
            Some((
                next.id.clone(),
                MobTransition {
                    from,
                    to,
                    started_at_ms: timestamp_ms,
                    duration_ms: duration_ms.max(1) as f64,
                },
            ))
        })
        .collect();
    *mobs = next_mobs;
    state.transitions = transitions;
}

pub fn accept_simulation_snapshot(
    state: &mut MobRenderState,
    sequence: u64,
) -> bool {
    if sequence <= state.last_simulation_sequence {
        return false;
    }
    state.last_simulation_sequence = sequence;
    true
}

pub fn install_projectile_snapshot(
    state: &mut MobRenderState,
    projectiles: &mut Vec<MobProjectile>,
    next_projectiles: Vec<MobProjectile>,
    timestamp_ms: f64,
    duration_ms: u64,
) {
    let transitions = next_projectiles
        .iter()
        .filter_map(|next| {
            let to = next.position.filter(finite_vector)?;
            let from = projectiles
                .iter()
                .find(|current| current.id == next.id)
                .and_then(|current| projectile_position(state, current, timestamp_ms))
                .unwrap_or(to);
            Some((
                next.id.clone(),
                MobTransition {
                    from,
                    to,
                    started_at_ms: timestamp_ms,
                    duration_ms: duration_ms.max(1) as f64,
                },
            ))
        })
        .collect();
    *projectiles = next_projectiles;
    state.projectile_transitions = transitions;
}

pub fn install_combat_events(
    state: &mut MobRenderState,
    events: Vec<CombatEvent>,
    timestamp_ms: f64,
) {
    state
        .combat_texts
        .retain(|text| timestamp_ms - text.started_at_ms < COMBAT_TEXT_LIFETIME_MS);
    state
        .combat_texts
        .extend(events.into_iter().filter_map(|event| {
            let position = event.position.filter(finite_vector)?;
            let kind = CombatEventKind::try_from(event.kind).ok()?;
            let player_damage = matches!(
                kind,
                CombatEventKind::MobTouchedPlayer | CombatEventKind::MobProjectileHitPlayer
            );
            (event.damage > 0).then_some(CombatText {
                position,
                damage: event.damage,
                player_damage,
                started_at_ms: timestamp_ms,
            })
        }));
}

pub fn projectile_position(
    state: &MobRenderState,
    projectile: &MobProjectile,
    timestamp_ms: f64,
) -> Option<Vec2> {
    let fallback = projectile.position.filter(finite_vector)?;
    let Some(transition) = state.projectile_transitions.get(&projectile.id) else {
        return Some(fallback);
    };
    Some(interpolate(*transition, timestamp_ms))
}

pub fn combat_texts(
    state: &MobRenderState,
    timestamp_ms: f64,
) -> Vec<RenderedCombatText> {
    state
        .combat_texts
        .iter()
        .filter_map(|text| {
            let progress = ((timestamp_ms - text.started_at_ms) / COMBAT_TEXT_LIFETIME_MS) as f32;
            (0.0..1.0)
                .contains(&progress)
                .then_some(RenderedCombatText {
                    position: text.position,
                    damage: text.damage,
                    player_damage: text.player_damage,
                    progress,
                })
        })
        .collect()
}

pub fn position(
    state: &MobRenderState,
    mob: &Mob,
    timestamp_ms: f64,
) -> Option<Vec2> {
    let fallback = finite_position(mob)?;
    let Some(transition) = state.transitions.get(&mob.id) else {
        return Some(fallback);
    };
    Some(interpolate(*transition, timestamp_ms))
}

fn interpolate(
    transition: MobTransition,
    timestamp_ms: f64,
) -> Vec2 {
    let progress =
        ((timestamp_ms - transition.started_at_ms) / transition.duration_ms).clamp(0.0, 1.0) as f32;
    Vec2 {
        x: transition.from.x + (transition.to.x - transition.from.x) * progress,
        y: transition.from.y + (transition.to.y - transition.from.y) * progress,
    }
}

fn finite_vector(position: &Vec2) -> bool {
    position.x.is_finite() && position.y.is_finite()
}

fn finite_position(mob: &Mob) -> Option<Vec2> {
    mob.position.filter(finite_vector)
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::CombatEvent;
    use oozems_proto::v1::CombatEventKind;
    use oozems_proto::v1::Mob;
    use oozems_proto::v1::MobProjectile;
    use oozems_proto::v1::Vec2;

    use super::MobRenderState;
    use super::accept_simulation_snapshot;
    use super::combat_texts;
    use super::install_combat_events;
    use super::install_projectile_snapshot;
    use super::install_snapshot;
    use super::position;
    use super::projectile_position;

    #[test]
    fn authoritative_snapshots_are_interpolated_over_the_poll_interval() {
        let mut state = MobRenderState::default();
        let mut mobs = vec![mob("slime", 100.0, 300.0)];

        install_snapshot(
            &mut state,
            &mut mobs,
            vec![mob("slime", 120.0, 280.0)],
            1_000.0,
            200,
        );

        assert_eq!(
            position(&state, &mobs[0], 1_000.0),
            Some(Vec2 { x: 100.0, y: 300.0 })
        );
        assert_eq!(
            position(&state, &mobs[0], 1_100.0),
            Some(Vec2 { x: 110.0, y: 290.0 })
        );
        assert_eq!(
            position(&state, &mobs[0], 1_300.0),
            Some(Vec2 { x: 120.0, y: 280.0 })
        );
    }

    #[test]
    fn a_new_snapshot_continues_from_the_current_rendered_position() {
        let mut state = MobRenderState::default();
        let mut mobs = vec![mob("slime", 100.0, 300.0)];
        install_snapshot(
            &mut state,
            &mut mobs,
            vec![mob("slime", 120.0, 300.0)],
            1_000.0,
            200,
        );

        install_snapshot(
            &mut state,
            &mut mobs,
            vec![mob("slime", 140.0, 300.0)],
            1_100.0,
            200,
        );

        assert_eq!(
            position(&state, &mobs[0], 1_100.0),
            Some(Vec2 { x: 110.0, y: 300.0 })
        );
    }

    #[test]
    fn respawned_mobs_appear_at_the_authoritative_spawn_position() {
        let mut state = MobRenderState::default();
        let mut mobs = vec![mob_with_hp("slime", 300.0, 300.0, 0)];

        install_snapshot(
            &mut state,
            &mut mobs,
            vec![mob_with_hp("slime", 100.0, 300.0, 50)],
            1_000.0,
            200,
        );

        assert_eq!(
            position(&state, &mobs[0], 1_000.0),
            Some(Vec2 { x: 100.0, y: 300.0 })
        );
    }

    #[test]
    fn older_simulation_snapshots_are_ignored() {
        let mut state = MobRenderState::default();

        assert!(accept_simulation_snapshot(&mut state, 2));
        assert!(!accept_simulation_snapshot(&mut state, 1));
        assert!(!accept_simulation_snapshot(&mut state, 2));
        assert!(accept_simulation_snapshot(&mut state, 3));
    }

    #[test]
    fn projectile_snapshots_are_interpolated_over_the_poll_interval() {
        let mut state = MobRenderState::default();
        let mut projectiles = vec![projectile("bolt", 100.0, 300.0)];

        install_projectile_snapshot(
            &mut state,
            &mut projectiles,
            vec![projectile("bolt", 140.0, 300.0)],
            1_000.0,
            200,
        );

        assert_eq!(
            projectile_position(&state, &projectiles[0], 1_100.0),
            Some(Vec2 { x: 120.0, y: 300.0 })
        );
    }

    #[test]
    fn combat_events_become_temporary_damage_text() {
        let mut state = MobRenderState::default();
        install_combat_events(
            &mut state,
            vec![CombatEvent {
                kind: CombatEventKind::PlayerHitMob as i32,
                damage: 42,
                position: Some(Vec2 { x: 100.0, y: 200.0 }),
                ..CombatEvent::default()
            }],
            1_000.0,
        );

        let visible = combat_texts(&state, 1_450.0);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].damage, 42);
        assert_eq!(visible[0].progress, 0.5);
        assert!(combat_texts(&state, 1_900.0).is_empty());
    }

    fn mob(
        id: &str,
        x: f32,
        y: f32,
    ) -> Mob {
        mob_with_hp(id, x, y, 0)
    }

    fn mob_with_hp(
        id: &str,
        x: f32,
        y: f32,
        current_hp: u64,
    ) -> Mob {
        Mob {
            id: id.to_owned(),
            position: Some(Vec2 { x, y }),
            current_hp,
            ..Mob::default()
        }
    }

    fn projectile(
        id: &str,
        x: f32,
        y: f32,
    ) -> MobProjectile {
        MobProjectile {
            id: id.to_owned(),
            position: Some(Vec2 { x, y }),
            ..MobProjectile::default()
        }
    }
}
