use std::collections::HashMap;

use oozems_proto::v1::Map;
use oozems_proto::v1::Reactor;
use oozems_proto::v1::ReactorDefinition;
use oozems_proto::v1::Vec2;

use crate::attacks::AttackReach;
use crate::attacks::DEFAULT_TARGET_VERTICAL_BOUNDS;
use crate::attacks::POINT_VERTICAL_BOUNDS;
use crate::attacks::VerticalBounds;
use crate::attacks::vertical_attack_intersects;

#[derive(Clone, Debug)]
pub(super) struct ReactorRuntime {
    pub(super) id: String,
    pub(super) definition_id: u32,
    pub(super) spawn_id: u32,
    pub(super) position: Vec2,
    pub(super) layer: i32,
    vertical_bounds: VerticalBounds,
    initial_state: u32,
    state: u32,
    active: bool,
    respawn_delay_ms: Option<u64>,
    respawn_at_ms: Option<u64>,
    transitions: HashMap<u32, u32>,
    random_state: u64,
    player_attack_transaction: Option<u64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ReactorAttackState {
    state: u32,
    active: bool,
    respawn_at_ms: Option<u64>,
    random_state: u64,
}

#[derive(Clone, Debug)]
pub(super) struct ReactorAttackResult {
    pub(super) spawn_id: u32,
    pub(super) id: String,
    pub(super) position: Vec2,
    pub(super) layer: i32,
    pub(super) before: ReactorAttackState,
    pub(super) after: ReactorAttackState,
    pub(super) destroyed: bool,
    pub(super) item_ids: Vec<u32>,
}

#[derive(Clone, Debug)]
pub(super) struct RespawnedReactor {
    pub(super) id: String,
    pub(super) position: Vec2,
    pub(super) layer: i32,
}

pub(super) fn spawn(map: &Map) -> Vec<ReactorRuntime> {
    let definitions = map
        .reactor_definitions
        .iter()
        .map(|definition| (definition.id, definition))
        .collect::<HashMap<_, _>>();
    map.reactor_spawn_points
        .iter()
        .filter_map(|spawn| {
            let definition = definitions.get(&spawn.reactor_id).copied()?;
            let position = spawn.position.filter(finite_position)?;
            build_runtime(map.id, spawn, definition, position)
        })
        .collect()
}

fn build_runtime(
    map_id: u32,
    spawn: &oozems_proto::v1::ReactorSpawnPoint,
    definition: &ReactorDefinition,
    position: Vec2,
) -> Option<ReactorRuntime> {
    let initial_state = definition.states.iter().map(|state| state.state).min()?;
    let transitions = definition
        .states
        .iter()
        .filter_map(|state| state.next_state.map(|next| (state.state, next)))
        .collect();
    Some(ReactorRuntime {
        id: format!("{map_id}:reactor:{}", spawn.spawn_id),
        definition_id: definition.id,
        spawn_id: spawn.spawn_id,
        position,
        layer: spawn.layer,
        vertical_bounds: reactor_vertical_bounds(definition),
        initial_state,
        state: initial_state,
        active: true,
        respawn_delay_ms: (spawn.respawn_seconds > 0)
            .then(|| u64::from(spawn.respawn_seconds).saturating_mul(1_000)),
        respawn_at_ms: None,
        transitions,
        random_state: crate::random::map_spawn_seed(map_id, spawn.spawn_id),
        player_attack_transaction: None,
    })
}

pub(super) fn snapshot(reactors: &[ReactorRuntime]) -> Vec<Reactor> {
    reactors
        .iter()
        .map(|reactor| Reactor {
            id: reactor.id.clone(),
            definition_id: reactor.definition_id,
            spawn_id: reactor.spawn_id,
            state: reactor.state,
            active: reactor.active,
        })
        .collect()
}

pub(super) fn nearest_attack_candidate(
    reactors: &[ReactorRuntime],
    player_x: f32,
    player_y: f32,
    player_layer: i32,
    facing_left: bool,
    reach: AttackReach,
    intersects_target_body: bool,
) -> Option<(usize, f32)> {
    reactors
        .iter()
        .enumerate()
        .filter(|(_, reactor)| {
            reactor.active
                && reactor.player_attack_transaction.is_none()
                && reactor.transitions.contains_key(&reactor.state)
                && reactor.layer == player_layer
                && vertical_attack_intersects(
                    player_y,
                    reach,
                    reactor.position.y,
                    if intersects_target_body {
                        reactor.vertical_bounds
                    } else {
                        POINT_VERTICAL_BOUNDS
                    },
                )
                && in_facing_range(reactor.position.x - player_x, facing_left, reach.horizontal)
        })
        .map(|(index, reactor)| (index, (reactor.position.x - player_x).abs()))
        .min_by(|left, right| left.1.total_cmp(&right.1))
}

fn reactor_vertical_bounds(definition: &ReactorDefinition) -> VerticalBounds {
    definition
        .states
        .iter()
        .flat_map(|state| &state.frames)
        .filter_map(|frame| {
            let top = -frame.origin_y;
            let bottom = frame.height - frame.origin_y;
            (frame.height > 0.0 && top.is_finite() && bottom.is_finite() && top <= bottom)
                .then_some(VerticalBounds { top, bottom })
        })
        .reduce(|bounds, frame| VerticalBounds {
            top: bounds.top.min(frame.top),
            bottom: bounds.bottom.max(frame.bottom),
        })
        .unwrap_or(DEFAULT_TARGET_VERTICAL_BOUNDS)
}

pub(super) fn prepare_attack(
    reactors: &mut [ReactorRuntime],
    index: usize,
    now_ms: u64,
    transaction_id: u64,
    loot: &crate::loot::LootCatalog,
    player: &oozems_proto::v1::PlayerState,
) -> Option<ReactorAttackResult> {
    let reactor = reactors.get_mut(index)?;
    if !reactor.active || reactor.player_attack_transaction.is_some() {
        return None;
    }
    let next_state = reactor.transitions.get(&reactor.state).copied()?;
    let before = attack_state(reactor);
    reactor.state = next_state;
    let destroyed = !reactor.transitions.contains_key(&next_state);
    if destroyed {
        reactor.active = false;
        reactor.respawn_at_ms = reactor
            .respawn_delay_ms
            .map(|delay| now_ms.saturating_add(delay));
    }
    let item_ids = if destroyed {
        crate::loot::roll_reactor_items(
            loot,
            reactor.definition_id,
            player,
            &mut reactor.random_state,
        )
    } else {
        Vec::new()
    };
    reactor.player_attack_transaction = Some(transaction_id);
    Some(ReactorAttackResult {
        spawn_id: reactor.spawn_id,
        id: reactor.id.clone(),
        position: reactor.position,
        layer: reactor.layer,
        before,
        after: attack_state(reactor),
        destroyed,
        item_ids,
    })
}

pub(super) fn commit_attack(
    reactors: &mut [ReactorRuntime],
    spawn_id: u32,
    transaction_id: u64,
) -> bool {
    let Some(reactor) = reactors
        .iter_mut()
        .find(|reactor| reactor.spawn_id == spawn_id)
    else {
        return false;
    };
    if reactor.player_attack_transaction != Some(transaction_id) {
        return false;
    }
    reactor.player_attack_transaction = None;
    true
}

pub(super) fn rollback_attack(
    reactors: &mut [ReactorRuntime],
    spawn_id: u32,
    transaction_id: u64,
    before: ReactorAttackState,
    after: ReactorAttackState,
) -> bool {
    let Some(reactor) = reactors
        .iter_mut()
        .find(|reactor| reactor.spawn_id == spawn_id)
    else {
        return false;
    };
    if reactor.player_attack_transaction != Some(transaction_id) || attack_state(reactor) != after {
        return false;
    }
    reactor.state = before.state;
    reactor.active = before.active;
    reactor.respawn_at_ms = before.respawn_at_ms;
    reactor.random_state = before.random_state;
    reactor.player_attack_transaction = None;
    true
}

pub(super) fn advance(
    reactors: &mut [ReactorRuntime],
    now_ms: u64,
) -> Vec<RespawnedReactor> {
    reactors
        .iter_mut()
        .filter_map(|reactor| {
            if reactor.active
                || reactor.player_attack_transaction.is_some()
                || reactor
                    .respawn_at_ms
                    .is_none_or(|deadline| now_ms < deadline)
            {
                return None;
            }
            reactor.state = reactor.initial_state;
            reactor.active = true;
            reactor.respawn_at_ms = None;
            Some(RespawnedReactor {
                id: reactor.id.clone(),
                position: reactor.position,
                layer: reactor.layer,
            })
        })
        .collect()
}

fn attack_state(reactor: &ReactorRuntime) -> ReactorAttackState {
    ReactorAttackState {
        state: reactor.state,
        active: reactor.active,
        respawn_at_ms: reactor.respawn_at_ms,
        random_state: reactor.random_state,
    }
}

fn in_facing_range(
    delta_x: f32,
    facing_left: bool,
    range: f32,
) -> bool {
    delta_x.abs() <= range
        && if facing_left {
            delta_x <= 0.0
        } else {
            delta_x >= 0.0
        }
}

fn finite_position(position: &Vec2) -> bool {
    position.x.is_finite() && position.y.is_finite()
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::Map;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::ReactorDefinition;
    use oozems_proto::v1::ReactorFrame;
    use oozems_proto::v1::ReactorSpawnPoint;
    use oozems_proto::v1::ReactorStateDefinition;
    use oozems_proto::v1::Vec2;

    use super::advance;
    use super::commit_attack;
    use super::nearest_attack_candidate;
    use super::prepare_attack;
    use super::rollback_attack;
    use super::snapshot;
    use super::spawn;
    use crate::attacks::AttackReach;
    use crate::loot::LootCatalog;

    #[test]
    fn hits_advance_until_destruction_then_respawn() {
        let mut reactors = spawn(&map());
        let player = PlayerState::default();

        for (transaction_id, expected_state) in [(1, 1), (2, 2), (3, 3), (4, 4)] {
            let result = prepare_attack(
                &mut reactors,
                0,
                1_000,
                transaction_id,
                &LootCatalog::default(),
                &player,
            )
            .expect("reactor attack");
            assert_eq!(result.destroyed, expected_state == 4);
            assert!(commit_attack(&mut reactors, 0, transaction_id));
            assert_eq!(snapshot(&reactors)[0].state, expected_state);
        }
        assert!(!snapshot(&reactors)[0].active);
        assert!(advance(&mut reactors, 90_999).is_empty());
        assert_eq!(advance(&mut reactors, 91_000).len(), 1);
        assert_eq!(snapshot(&reactors)[0].state, 0);
        assert!(snapshot(&reactors)[0].active);
    }

    #[test]
    fn rollback_restores_the_previous_state() {
        let mut reactors = spawn(&map());
        let result = prepare_attack(
            &mut reactors,
            0,
            1_000,
            7,
            &LootCatalog::default(),
            &PlayerState::default(),
        )
        .expect("reactor attack");

        assert!(rollback_attack(
            &mut reactors,
            result.spawn_id,
            7,
            result.before,
            result.after,
        ));
        assert_eq!(snapshot(&reactors)[0].state, 0);
        assert!(snapshot(&reactors)[0].active);
    }

    #[test]
    fn attack_candidates_respect_asymmetric_weapon_reach() {
        let reactors = spawn(&map());
        let reach = AttackReach {
            horizontal: 88.0,
            top: -62.0,
            bottom: -6.0,
        };

        assert_eq!(
            nearest_attack_candidate(&reactors, 12.0, 38.0, 0, false, reach, true),
            Some((0, 88.0))
        );
        assert_eq!(
            nearest_attack_candidate(&reactors, 12.0, 37.0, 0, false, reach, true),
            None
        );
    }

    fn map() -> Map {
        Map {
            id: 1,
            reactor_spawn_points: vec![ReactorSpawnPoint {
                spawn_id: 0,
                reactor_id: 2_001,
                position: Some(Vec2 { x: 100.0, y: 80.0 }),
                respawn_seconds: 90,
                ..ReactorSpawnPoint::default()
            }],
            reactor_definitions: vec![ReactorDefinition {
                id: 2_001,
                states: (0..=4)
                    .map(|state| ReactorStateDefinition {
                        state,
                        frames: vec![ReactorFrame {
                            height: 48.0,
                            origin_y: 48.0,
                            ..ReactorFrame::default()
                        }],
                        next_state: (state < 4).then_some(state + 1),
                        ..ReactorStateDefinition::default()
                    })
                    .collect(),
            }],
            ..Map::default()
        }
    }
}
