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
    reactions: HashMap<String, TimedMobReaction>,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MobReactionKind {
    Hit,
    Death,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MobReactionEvent {
    pub target_id: String,
    pub kind: MobReactionKind,
}

#[derive(Clone, Copy)]
struct TimedMobReaction {
    kind: MobReactionKind,
    started_at_ms: f64,
}

#[derive(Clone, Copy)]
struct CombatText {
    position: Vec2,
    damage: u64,
    player_damage: bool,
    missed: bool,
    started_at_ms: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderedCombatText {
    pub position: Vec2,
    pub damage: u64,
    pub player_damage: bool,
    pub missed: bool,
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
    state
        .reactions
        .retain(|mob_id, _| next_mobs.iter().any(|mob| mob.id == *mob_id));
    for next in &next_mobs {
        if next.current_hp > 0
            && mobs
                .iter()
                .any(|current| current.id == next.id && current.current_hp == 0)
        {
            state.reactions.remove(&next.id);
        }
    }
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
) -> Vec<MobReactionEvent> {
    let reactions = mob_reactions(&events);
    for reaction in &reactions {
        state.reactions.insert(
            reaction.target_id.clone(),
            TimedMobReaction {
                kind: reaction.kind,
                started_at_ms: timestamp_ms,
            },
        );
    }
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
                CombatEventKind::MobTouchedPlayer
                    | CombatEventKind::MobProjectileHitPlayer
                    | CombatEventKind::MobMissedPlayer
            );
            let missed = matches!(
                kind,
                CombatEventKind::PlayerMissedMob | CombatEventKind::MobMissedPlayer
            );
            (event.damage > 0 || missed).then_some(CombatText {
                position,
                damage: event.damage,
                player_damage,
                missed,
                started_at_ms: timestamp_ms,
            })
        }));
    reactions
}

pub(crate) fn reaction(
    state: &MobRenderState,
    mob_id: &str,
    timestamp_ms: f64,
) -> Option<(MobReactionKind, f64)> {
    let reaction = state.reactions.get(mob_id)?;
    Some((
        reaction.kind,
        (timestamp_ms - reaction.started_at_ms).max(0.0),
    ))
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
                    missed: text.missed,
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

fn mob_reactions(events: &[CombatEvent]) -> Vec<MobReactionEvent> {
    let mut reactions = Vec::<MobReactionEvent>::new();
    for event in events {
        let kind = match CombatEventKind::try_from(event.kind) {
            Ok(CombatEventKind::PlayerHitMob) if event.staggered => MobReactionKind::Hit,
            Ok(CombatEventKind::MobDied) => MobReactionKind::Death,
            _ => continue,
        };
        if event.target_id.is_empty() {
            continue;
        }
        if let Some(existing) = reactions
            .iter_mut()
            .find(|reaction| reaction.target_id == event.target_id)
        {
            if kind == MobReactionKind::Death {
                existing.kind = kind;
            }
        } else {
            reactions.push(MobReactionEvent {
                target_id: event.target_id.clone(),
                kind,
            });
        }
    }
    reactions
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::CombatEvent;
    use oozems_proto::v1::CombatEventKind;
    use oozems_proto::v1::Mob;
    use oozems_proto::v1::MobProjectile;
    use oozems_proto::v1::Vec2;

    use super::MobReactionKind;
    use super::MobRenderState;
    use super::accept_simulation_snapshot;
    use super::combat_texts;
    use super::install_combat_events;
    use super::install_projectile_snapshot;
    use super::install_snapshot;
    use super::position;
    use super::projectile_position;
    use super::reaction;

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

    #[test]
    fn a_confirmed_hit_starts_the_target_mob_reaction() {
        let mut state = MobRenderState::default();
        let reactions = install_combat_events(
            &mut state,
            vec![CombatEvent {
                kind: CombatEventKind::PlayerHitMob as i32,
                target_id: "slime".to_owned(),
                staggered: true,
                ..CombatEvent::default()
            }],
            1_000.0,
        );

        assert_eq!(reactions.len(), 1);
        assert_eq!(reactions[0].kind, MobReactionKind::Hit);
        assert_eq!(
            reaction(&state, "slime", 1_150.0),
            Some((MobReactionKind::Hit, 150.0))
        );
    }

    #[test]
    fn a_hit_below_the_stagger_threshold_does_not_start_a_reaction() {
        let mut state = MobRenderState::default();
        let reactions = install_combat_events(
            &mut state,
            vec![CombatEvent {
                kind: CombatEventKind::PlayerHitMob as i32,
                target_id: "slime".to_owned(),
                damage: 42,
                position: Some(Vec2 { x: 100.0, y: 200.0 }),
                ..CombatEvent::default()
            }],
            1_000.0,
        );

        assert!(reactions.is_empty());
        assert_eq!(reaction(&state, "slime", 1_000.0), None);
        assert_eq!(combat_texts(&state, 1_000.0).len(), 1);
    }

    #[test]
    fn a_killing_hit_uses_one_death_reaction() {
        let mut state = MobRenderState::default();
        let reactions = install_combat_events(
            &mut state,
            vec![
                CombatEvent {
                    kind: CombatEventKind::PlayerHitMob as i32,
                    target_id: "slime".to_owned(),
                    ..CombatEvent::default()
                },
                CombatEvent {
                    kind: CombatEventKind::MobDied as i32,
                    target_id: "slime".to_owned(),
                    ..CombatEvent::default()
                },
            ],
            1_000.0,
        );

        assert_eq!(reactions.len(), 1);
        assert_eq!(reactions[0].kind, MobReactionKind::Death);
        assert_eq!(
            reaction(&state, "slime", 1_000.0),
            Some((MobReactionKind::Death, 0.0))
        );
    }

    #[test]
    fn respawning_clears_the_previous_death_reaction() {
        let mut state = MobRenderState::default();
        let mut mobs = vec![mob_with_hp("slime", 300.0, 300.0, 0)];
        install_combat_events(
            &mut state,
            vec![CombatEvent {
                kind: CombatEventKind::MobDied as i32,
                target_id: "slime".to_owned(),
                ..CombatEvent::default()
            }],
            900.0,
        );

        install_snapshot(
            &mut state,
            &mut mobs,
            vec![mob_with_hp("slime", 100.0, 300.0, 50)],
            1_000.0,
            200,
        );

        assert_eq!(reaction(&state, "slime", 1_000.0), None);
    }

    #[test]
    fn miss_events_become_zero_damage_miss_text() {
        let mut state = MobRenderState::default();
        install_combat_events(
            &mut state,
            vec![CombatEvent {
                kind: CombatEventKind::MobMissedPlayer as i32,
                position: Some(Vec2 { x: 100.0, y: 200.0 }),
                ..CombatEvent::default()
            }],
            1_000.0,
        );

        let visible = combat_texts(&state, 1_100.0);
        assert_eq!(visible.len(), 1);
        assert!(visible[0].missed);
        assert!(visible[0].player_damage);
        assert_eq!(visible[0].damage, 0);
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
