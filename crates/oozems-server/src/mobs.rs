use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use oozems_proto::v1::CombatEvent;
use oozems_proto::v1::CombatEventKind;
use oozems_proto::v1::Map;
use oozems_proto::v1::Mob;
use oozems_proto::v1::MobDefinition;
use oozems_proto::v1::MobMovementMode;
use oozems_proto::v1::MobProjectile;
use oozems_proto::v1::MobSpawnPoint;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::Vec2;
use shipyard::IntoIter;
use shipyard::UniqueViewMut;
use shipyard::View;
use shipyard::Workload;
use shipyard::World;
use thiserror::Error;

use crate::gameplay::CombatConfig;
use crate::skill_formula::FormulaCatalog;

mod ai;
mod combat;
mod components;

use components::CombatFormulas;
use components::CombatRules;
use components::MobCombat;
use components::MobIdentity;
use components::MobMotion;
use components::PendingEvents;
use components::PlayerPresence;
use components::Position;
use components::Projectile;
use components::ProjectileSpawns;
use components::SimulationErrors;
use components::TargetCache;
use components::Terrain;
use components::Tick;

const BASE_MOVE_SPEED: f32 = 80.0;
const MAX_CATCH_UP: Duration = Duration::from_secs(1);
const PLAYER_PRESENCE_TIMEOUT_MS: u64 = 5_000;
const MOB_TICK_WORKLOAD: &str = "mob simulation tick";

pub struct MobStore {
    maps: Mutex<HashMap<u32, MobMapState>>,
    rules: CombatConfig,
    formulas: Arc<FormulaCatalog>,
}

struct MobMapState {
    world: World,
    player_entities: HashMap<String, shipyard::EntityId>,
    updated_at: Instant,
    clock_ms: u64,
    snapshot_sequence: u64,
}

#[derive(Default)]
pub struct MobUpdate {
    pub mobs: Vec<Mob>,
    pub mob_projectiles: Vec<MobProjectile>,
    pub combat_events: Vec<CombatEvent>,
    pub sequence: u64,
}

#[derive(Clone, Copy)]
pub struct PlayerAttack<'a> {
    pub target_mob_id: &'a str,
    pub facing_left: bool,
    pub minimum_damage: u32,
    pub maximum_damage: u32,
    pub fixed_damage: bool,
}

impl MobUpdate {
    pub fn player_damage(&self) -> u64 {
        self.combat_events
            .iter()
            .filter(|event| {
                matches!(
                    CombatEventKind::try_from(event.kind),
                    Ok(CombatEventKind::MobTouchedPlayer)
                        | Ok(CombatEventKind::MobProjectileHitPlayer)
                )
            })
            .map(|event| event.damage)
            .fold(0, u64::saturating_add)
    }
}

#[derive(Debug, Error)]
pub enum MobStoreError {
    #[error("the mob store lock was poisoned")]
    Lock,
    #[error("the Shipyard simulation could not be built: {message}")]
    Build { message: String },
    #[error("the Shipyard simulation workload failed: {message}")]
    Workload { message: String },
    #[error("the Shipyard simulation borrow failed: {message}")]
    Borrow { message: String },
    #[error("a combat formula failed: {message}")]
    Formula { message: String },
}

impl MobStore {
    pub fn new(
        rules: CombatConfig,
        formulas: Arc<FormulaCatalog>,
    ) -> Self {
        Self {
            maps: Mutex::new(HashMap::new()),
            rules,
            formulas,
        }
    }
}

pub fn map_snapshot(
    store: &MobStore,
    map: &Map,
) -> Result<MobUpdate, MobStoreError> {
    map_snapshot_at(store, map, Instant::now())
}

pub fn observe_player(
    store: &MobStore,
    map: &Map,
    player: &PlayerState,
) -> Result<MobUpdate, MobStoreError> {
    observe_player_at(store, map, player, Instant::now())
}

pub fn use_player_attack(
    store: &MobStore,
    map: &Map,
    player: &PlayerState,
    attack: PlayerAttack<'_>,
) -> Result<MobUpdate, MobStoreError> {
    use_player_attack_at(store, map, player, attack, Instant::now())
}

pub fn restore_player_events(
    store: &MobStore,
    map_id: u32,
    player_id: &str,
    mut combat_events: Vec<CombatEvent>,
) -> Result<(), MobStoreError> {
    if combat_events.is_empty() {
        return Ok(());
    }
    let maps = store.maps.lock().map_err(|_| MobStoreError::Lock)?;
    let Some(state) = maps.get(&map_id) else {
        return Ok(());
    };
    state
        .world
        .run(|mut pending: UniqueViewMut<PendingEvents>| {
            let queued = pending.by_player.entry(player_id.to_owned()).or_default();
            combat_events.append(queued);
            *queued = combat_events;
        });
    Ok(())
}

fn map_snapshot_at(
    store: &MobStore,
    map: &Map,
    now: Instant,
) -> Result<MobUpdate, MobStoreError> {
    let mut maps = store.maps.lock().map_err(|_| MobStoreError::Lock)?;
    let state = ensure_map(&mut maps, map, store, now)?;
    advance_map_to(state, now)?;
    prune_stale_players(state)?;
    snapshot(state, None)
}

fn observe_player_at(
    store: &MobStore,
    map: &Map,
    player: &PlayerState,
    now: Instant,
) -> Result<MobUpdate, MobStoreError> {
    let mut maps = store.maps.lock().map_err(|_| MobStoreError::Lock)?;
    remove_player_from_other_maps(&mut maps, map.id, &player.id);
    let state = ensure_map(&mut maps, map, store, now)?;
    sync_player(state, player)?;
    advance_map_to(state, now)?;
    mark_player_seen(state, &player.id)?;
    prune_stale_players(state)?;
    snapshot(state, Some(&player.id))
}

fn use_player_attack_at(
    store: &MobStore,
    map: &Map,
    player: &PlayerState,
    attack: PlayerAttack<'_>,
    now: Instant,
) -> Result<MobUpdate, MobStoreError> {
    let mut maps = store.maps.lock().map_err(|_| MobStoreError::Lock)?;
    remove_player_from_other_maps(&mut maps, map.id, &player.id);
    let state = ensure_map(&mut maps, map, store, now)?;
    sync_player(state, player)?;
    advance_map_to(state, now)?;
    mark_player_seen(state, &player.id)?;
    prune_stale_players(state)?;
    if attack.maximum_damage > 0 {
        apply_player_attack(
            state,
            &player.id,
            PlayerAttack {
                target_mob_id: attack.target_mob_id,
                facing_left: attack.facing_left,
                minimum_damage: attack.minimum_damage.max(1),
                maximum_damage: attack.maximum_damage.max(attack.minimum_damage).max(1),
                fixed_damage: attack.fixed_damage,
            },
            store.rules,
            &store.formulas,
        )?;
        state.snapshot_sequence = state.snapshot_sequence.saturating_add(1);
    }
    snapshot(state, Some(&player.id))
}

fn ensure_map<'a>(
    maps: &'a mut HashMap<u32, MobMapState>,
    map: &Map,
    store: &MobStore,
    now: Instant,
) -> Result<&'a mut MobMapState, MobStoreError> {
    if let std::collections::hash_map::Entry::Vacant(entry) = maps.entry(map.id) {
        entry.insert(build_map_state(
            map,
            store.rules,
            store.formulas.clone(),
            now,
        )?);
    }
    Ok(maps.get_mut(&map.id).expect("map was inserted"))
}

fn build_map_state(
    map: &Map,
    rules: CombatConfig,
    formulas: Arc<FormulaCatalog>,
    now: Instant,
) -> Result<MobMapState, MobStoreError> {
    let mut world = World::new();
    world.add_unique(Terrain {
        platforms: map.platforms.clone(),
        height: map.height as f32,
    });
    world.add_unique(Tick {
        elapsed_seconds: 0.0,
        now_ms: 0,
    });
    world.add_unique(CombatRules(rules));
    world.add_unique(CombatFormulas(formulas));
    world.add_unique(TargetCache::default());
    world.add_unique(ProjectileSpawns::default());
    world.add_unique(PendingEvents::default());
    world.add_unique(SimulationErrors::default());
    install_tick_workload(&world)?;
    for (identity, position, motion, combat) in spawn_mob_components(map, rules.default_respawn) {
        world.add_entity((identity, position, motion, combat));
    }
    Ok(MobMapState {
        world,
        player_entities: HashMap::new(),
        updated_at: now,
        clock_ms: 0,
        snapshot_sequence: 0,
    })
}

fn install_tick_workload(world: &World) -> Result<(), MobStoreError> {
    Workload::new(MOB_TICK_WORKLOAD)
        .with_system(combat::respawn_mobs)
        .with_barrier()
        .with_system(combat::collect_player_targets)
        .with_system(combat::retain_aggro)
        .with_barrier()
        .with_system(ai::advance_mobs)
        .with_barrier()
        .with_system(combat::apply_touch_damage)
        .with_system(combat::queue_projectile_attacks)
        .with_barrier()
        .with_system(combat::spawn_projectiles)
        .with_barrier()
        .with_system(combat::advance_projectiles)
        .add_to_world(world)
        .map_err(|error| MobStoreError::Build {
            message: error.to_string(),
        })?;
    Ok(())
}

fn advance_map_to(
    state: &mut MobMapState,
    now: Instant,
) -> Result<(), MobStoreError> {
    let wall_elapsed = now
        .checked_duration_since(state.updated_at)
        .unwrap_or_default();
    let elapsed = wall_elapsed.min(MAX_CATCH_UP);
    state.updated_at = now;
    let elapsed_ms = u64::try_from(wall_elapsed.as_millis()).unwrap_or(u64::MAX);
    state.clock_ms = state.clock_ms.saturating_add(elapsed_ms);
    state.world.run(
        |mut tick: UniqueViewMut<Tick>,
         mut errors: UniqueViewMut<SimulationErrors>,
         mut spawns: UniqueViewMut<ProjectileSpawns>| {
            tick.elapsed_seconds = elapsed.as_secs_f32();
            tick.now_ms = state.clock_ms;
            errors.0.clear();
            spawns.0.clear();
        },
    );
    state
        .world
        .run_workload(MOB_TICK_WORKLOAD)
        .map_err(|error| MobStoreError::Workload {
            message: error.to_string(),
        })?;
    remove_impacted_projectiles(state)?;
    let errors = state
        .world
        .run(|errors: shipyard::UniqueView<SimulationErrors>| errors.0.clone());
    if let Some(message) = errors.into_iter().next() {
        return Err(MobStoreError::Formula { message });
    }
    state.snapshot_sequence = state.snapshot_sequence.saturating_add(1);
    Ok(())
}

fn sync_player(
    state: &mut MobMapState,
    player: &PlayerState,
) -> Result<(), MobStoreError> {
    let Some(position) = player.position.filter(finite_position) else {
        return Ok(());
    };
    let stats = player.stats.unwrap_or_default();
    let layer = state.world.run(|terrain: shipyard::UniqueView<Terrain>| {
        ai::nearest_platform(&terrain.platforms, position.x, position.y, None)
            .and_then(|index| terrain.platforms.get(index))
            .map_or(0, |platform| platform.layer)
    });
    let simulation_position = Position {
        x: position.x,
        y: position.y,
        layer,
    };
    if let Some(entity) = state.player_entities.get(&player.id).copied()
        && let Ok((mut stored_position, mut presence)) = state
            .world
            .get::<(&mut Position, &mut PlayerPresence)>(entity)
    {
        **stored_position = simulation_position;
        presence.level = player.level;
        presence.current_hp = stats.hp;
        presence.last_seen_ms = state.clock_ms;
        return Ok(());
    }
    let entity = state.world.add_entity((
        simulation_position,
        PlayerPresence {
            id: player.id.clone(),
            level: player.level,
            current_hp: stats.hp,
            last_seen_ms: state.clock_ms,
            invulnerable_until_ms: 0,
        },
    ));
    state.player_entities.insert(player.id.clone(), entity);
    Ok(())
}

fn mark_player_seen(
    state: &mut MobMapState,
    player_id: &str,
) -> Result<(), MobStoreError> {
    let Some(entity) = state.player_entities.get(player_id).copied() else {
        return Ok(());
    };
    let mut presence = state
        .world
        .get::<&mut PlayerPresence>(entity)
        .map_err(|error| borrow_error(error.to_string()))?;
    presence.last_seen_ms = state.clock_ms;
    Ok(())
}

fn apply_player_attack(
    state: &mut MobMapState,
    player_id: &str,
    attack: PlayerAttack<'_>,
    rules: CombatConfig,
    formulas: &FormulaCatalog,
) -> Result<(), MobStoreError> {
    let Some(player_entity) = state.player_entities.get(player_id).copied() else {
        return Ok(());
    };
    let (player_position, player_level) = state
        .world
        .get::<(&Position, &PlayerPresence)>(player_entity)
        .map(|(position, presence)| (**position, presence.level))
        .map_err(|error| borrow_error(error.to_string()))?;
    let target = state.world.run(
        |positions: View<Position>, identities: View<MobIdentity>, combats: View<MobCombat>| {
            let candidates = (&positions, &identities, &combats).iter().with_id().filter(
                |(_, (position, identity, combat))| {
                    combat.current_hp > 0
                        && valid_attack_target(
                            player_position,
                            **position,
                            attack.facing_left,
                            rules,
                        )
                        && (attack.target_mob_id.is_empty()
                            || identity.public_id == attack.target_mob_id)
                },
            );
            candidates
                .map(|(entity, (position, _, _))| (entity, (position.x - player_position.x).abs()))
                .min_by(|left, right| left.1.total_cmp(&right.1))
                .map(|(entity, _)| entity)
        },
    );
    let Some(target) = target else {
        return Ok(());
    };
    let (position, target_id, damage, died) = {
        let (position, identity, mut motion, mut combat) = state
            .world
            .get::<(&Position, &MobIdentity, &mut MobMotion, &mut MobCombat)>(target)
            .map_err(|error| borrow_error(error.to_string()))?;
        let damage = combat::calculate_player_damage(
            formulas,
            combat.physical_defense,
            combat.level,
            player_level,
            attack.minimum_damage,
            attack.maximum_damage,
            attack.fixed_damage,
            &mut motion.random_state,
        )
        .map_err(|error| MobStoreError::Formula {
            message: format!("damage against mob {} failed: {error}", identity.public_id),
        })?;
        combat.current_hp = combat.current_hp.saturating_sub(damage);
        combat.aggro_target = Some(player_id.to_owned());
        if combat.current_hp == 0 {
            combat.dead_until_ms = Some(state.clock_ms.saturating_add(combat.respawn_delay_ms));
            combat.aggro_target = None;
            motion.mode = MobMovementMode::Idle;
        }
        (
            **position,
            identity.public_id.clone(),
            damage,
            combat.current_hp == 0,
        )
    };
    let source_id = player_id.to_owned();
    state.world.run(|mut events: UniqueViewMut<PendingEvents>| {
        combat::queue_event(
            &mut events,
            player_id,
            CombatEventKind::PlayerHitMob,
            &source_id,
            &target_id,
            damage,
            position,
        );
        if died {
            combat::queue_event(
                &mut events,
                player_id,
                CombatEventKind::MobDied,
                &source_id,
                &target_id,
                0,
                position,
            );
        }
    });
    Ok(())
}

fn valid_attack_target(
    player: Position,
    target: Position,
    facing_left: bool,
    rules: CombatConfig,
) -> bool {
    if player.layer != target.layer || (player.y - target.y).abs() > rules.attack_vertical_reach {
        return false;
    }
    let delta_x = target.x - player.x;
    delta_x.abs() <= rules.player_attack_range
        && if facing_left {
            delta_x <= 0.0
        } else {
            delta_x >= 0.0
        }
}

fn snapshot(
    state: &MobMapState,
    player_id: Option<&str>,
) -> Result<MobUpdate, MobStoreError> {
    let mobs = state.world.run(
        |positions: View<Position>,
         identities: View<MobIdentity>,
         motions: View<MobMotion>,
         combats: View<MobCombat>| {
            (&positions, &identities, &motions, &combats)
                .iter()
                .map(|(position, identity, motion, combat)| Mob {
                    id: identity.public_id.clone(),
                    definition_id: identity.definition_id,
                    position: Some(position.vector()),
                    flip_x: motion.flip_x,
                    layer: position.layer,
                    current_hp: combat.current_hp,
                    spawn_id: identity.spawn_id,
                    movement_mode: motion.mode as i32,
                })
                .collect()
        },
    );
    let mob_projectiles =
        state
            .world
            .run(|positions: View<Position>, projectiles: View<Projectile>| {
                (&positions, &projectiles)
                    .iter()
                    .filter(|(_, projectile)| !projectile.impacted)
                    .map(|(position, projectile)| MobProjectile {
                        id: projectile.public_id.clone(),
                        source_mob_id: projectile.source_mob_id.clone(),
                        target_player_id: projectile.target_player_id.clone(),
                        position: Some(position.vector()),
                        layer: position.layer,
                    })
                    .collect()
            });
    let combat_events = player_id.map_or_else(Vec::new, |player_id| {
        state.world.run(|mut events: UniqueViewMut<PendingEvents>| {
            events.by_player.remove(player_id).unwrap_or_default()
        })
    });
    Ok(MobUpdate {
        mobs,
        mob_projectiles,
        combat_events,
        sequence: state.snapshot_sequence,
    })
}

fn remove_impacted_projectiles(state: &mut MobMapState) -> Result<(), MobStoreError> {
    let impacted = state.world.run(|projectiles: View<Projectile>| {
        projectiles
            .iter()
            .with_id()
            .filter(|(_, projectile)| projectile.impacted)
            .map(|(entity, _)| entity)
            .collect::<Vec<_>>()
    });
    for entity in impacted {
        state.world.delete_entity(entity);
    }
    Ok(())
}

fn prune_stale_players(state: &mut MobMapState) -> Result<(), MobStoreError> {
    let stale_before = state.clock_ms.saturating_sub(PLAYER_PRESENCE_TIMEOUT_MS);
    let stale = state.world.run(|players: View<PlayerPresence>| {
        players
            .iter()
            .with_id()
            .filter(|(_, player)| player.last_seen_ms < stale_before)
            .map(|(entity, player)| (entity, player.id.clone()))
            .collect::<Vec<_>>()
    });
    for (entity, player_id) in stale {
        state.world.delete_entity(entity);
        state.player_entities.remove(&player_id);
    }
    Ok(())
}

fn remove_player_from_other_maps(
    maps: &mut HashMap<u32, MobMapState>,
    current_map_id: u32,
    player_id: &str,
) {
    for (map_id, state) in maps {
        if *map_id == current_map_id {
            continue;
        }
        if let Some(entity) = state.player_entities.remove(player_id) {
            state.world.delete_entity(entity);
        }
    }
}

fn spawn_mob_components(
    map: &Map,
    default_respawn: Duration,
) -> Vec<(MobIdentity, Position, MobMotion, MobCombat)> {
    let definitions = map
        .mob_definitions
        .iter()
        .map(|definition| (definition.id, definition))
        .collect::<HashMap<_, _>>();
    map.mob_spawn_points
        .iter()
        .filter_map(|spawn| {
            spawn_mob(
                map,
                spawn,
                definitions.get(&spawn.mob_id).copied(),
                default_respawn,
            )
        })
        .collect()
}

fn spawn_mob(
    map: &Map,
    spawn: &MobSpawnPoint,
    definition: Option<&MobDefinition>,
    default_respawn: Duration,
) -> Option<(MobIdentity, Position, MobMotion, MobCombat)> {
    let definition = definition.filter(|definition| {
        definition
            .animations
            .iter()
            .any(|animation| !animation.frames.is_empty())
    })?;
    let source_position = spawn.position.filter(finite_position)?;
    let position = Position {
        x: source_position.x,
        y: source_position.y,
        layer: spawn.layer,
    };
    let (roam_left, roam_right) = roam_bounds(map, spawn, position.x);
    let spawn_support = map
        .platforms
        .iter()
        .position(|platform| spawn.foothold_id != 0 && platform.id == spawn.foothold_id)
        .or_else(|| {
            ai::nearest_platform(&map.platforms, position.x, position.y, Some(position.layer))
        });
    let flies = has_animation(definition, "fly");
    let can_move = has_animation(definition, "move") || flies;
    let respawn_delay_ms = if spawn.respawn_seconds == 0 {
        u64::try_from(default_respawn.as_millis()).unwrap_or(u64::MAX)
    } else {
        u64::from(spawn.respawn_seconds).saturating_mul(1_000)
    };
    Some((
        MobIdentity {
            public_id: format!("{}:{}:0", map.id, spawn.spawn_id),
            definition_id: definition.id,
            spawn_id: spawn.spawn_id,
        },
        position,
        MobMotion {
            spawn_position: position,
            spawn_support,
            support: spawn_support,
            roam_left,
            roam_right,
            move_speed: movement_speed(definition.speed),
            can_move,
            can_jump: definition.can_jump,
            flies,
            flip_x: spawn.flip_x,
            direction: 0,
            velocity_y: 0.0,
            decision_seconds: 0.0,
            random_state: random_seed(map.id, spawn.spawn_id),
            mode: MobMovementMode::Idle,
        },
        MobCombat {
            level: definition.level,
            maximum_hp: definition.max_hp.max(1),
            current_hp: definition.max_hp.max(1),
            physical_attack: definition.physical_attack,
            physical_defense: definition.physical_defense,
            magic_attack: definition.magic_attack,
            body_attack: definition.body_attack,
            aggro_target: None,
            next_attack_ms: 0,
            attack_until_ms: 0,
            movement_resume_ms: 0,
            dead_until_ms: None,
            respawn_delay_ms,
        },
    ))
}

fn roam_bounds(
    map: &Map,
    spawn: &MobSpawnPoint,
    spawn_x: f32,
) -> (f32, f32) {
    let map_right = map.width as f32;
    let left = spawn.roam_left.min(spawn.roam_right);
    let right = spawn.roam_left.max(spawn.roam_right);
    if !left.is_finite() || !right.is_finite() || left >= right {
        let x = spawn_x.clamp(0.0, map_right);
        return (x, x);
    }
    (left.clamp(0.0, map_right), right.clamp(0.0, map_right))
}

fn movement_speed(wz_speed: i32) -> f32 {
    let percentage = (100_i64 + i64::from(wz_speed)).clamp(0, 200) as f32;
    BASE_MOVE_SPEED * percentage / 100.0
}

fn has_animation(
    definition: &MobDefinition,
    name: &str,
) -> bool {
    definition
        .animations
        .iter()
        .any(|animation| animation.name == name && !animation.frames.is_empty())
}

fn finite_position(position: &Vec2) -> bool {
    position.x.is_finite() && position.y.is_finite()
}

fn random_seed(
    map_id: u32,
    spawn_id: u32,
) -> u64 {
    let seed = (u64::from(map_id) << 32) | u64::from(spawn_id);
    seed ^ 0x9e37_79b9_7f4a_7c15
}

fn borrow_error(message: String) -> MobStoreError {
    MobStoreError::Borrow { message }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;
    use std::time::Instant;

    use oozems_proto::v1::CharacterStats;
    use oozems_proto::v1::Map;
    use oozems_proto::v1::MobAnimation;
    use oozems_proto::v1::MobDefinition;
    use oozems_proto::v1::MobFrame;
    use oozems_proto::v1::MobSpawnPoint;
    use oozems_proto::v1::Platform;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::Vec2;

    use super::MobStore;
    use super::PlayerAttack;
    use super::map_snapshot_at;
    use super::observe_player_at;
    use super::restore_player_events;
    use super::use_player_attack_at;
    use crate::gameplay::CombatConfig;
    use crate::skill_formula::FormulaCatalog;

    #[test]
    fn an_untargeted_player_attack_hits_an_in_range_mob_and_sets_aggro() {
        let store = store();
        let map = map();
        let player = player(90.0, 100.0);
        let now = Instant::now();

        map_snapshot_at(&store, &map, now).expect("initialize map");
        let update = use_player_attack_at(
            &store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "",
                facing_left: false,
                minimum_damage: 10,
                maximum_damage: 10,
                fixed_damage: false,
            },
            now + Duration::from_millis(1),
        )
        .expect("use attack");

        assert_eq!(update.mobs[0].current_hp, 90);
        assert!(update.combat_events.iter().any(|event| event.damage == 10));
    }

    #[test]
    fn attacks_cannot_damage_a_mob_behind_the_player() {
        let store = store();
        let map = map();
        let player = player(140.0, 100.0);

        let update = use_player_attack_at(
            &store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "1:1:0",
                facing_left: false,
                minimum_damage: 10,
                maximum_damage: 10,
                fixed_damage: false,
            },
            Instant::now(),
        )
        .expect("use skill");

        assert_eq!(update.mobs[0].current_hp, 100);
        assert!(update.combat_events.is_empty());
    }

    #[test]
    fn body_contact_damages_a_player_once_during_invulnerability() {
        let store = store();
        let map = map();
        let player = player(100.0, 100.0);
        let now = Instant::now();

        let first = observe_player_at(&store, &map, &player, now).expect("first observation");
        let second = observe_player_at(&store, &map, &player, now + Duration::from_millis(100))
            .expect("invulnerable observation");

        assert!(first.player_damage() > 0);
        assert_eq!(second.player_damage(), 0);
    }

    #[test]
    fn combat_events_can_be_restored_after_a_persistence_failure() {
        let store = store();
        let map = map();
        let player = player(100.0, 100.0);
        let now = Instant::now();

        let first = observe_player_at(&store, &map, &player, now).expect("first observation");
        let expected_damage = first.player_damage();
        restore_player_events(&store, map.id, &player.id, first.combat_events)
            .expect("restore events");
        let retried = observe_player_at(&store, &map, &player, now + Duration::from_millis(100))
            .expect("retry observation");

        assert!(expected_damage > 0);
        assert_eq!(retried.player_damage(), expected_damage);
    }

    #[test]
    fn aggroed_mobs_chase_a_nearby_player() {
        let store = store();
        let mut map = map();
        map.mob_definitions[0].animations.push(MobAnimation {
            name: "move".to_owned(),
            frames: vec![MobFrame::default()],
        });
        let player = player(250.0, 100.0);
        let now = Instant::now();

        use_player_attack_at(
            &store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "1:1:0",
                facing_left: true,
                minimum_damage: 1,
                maximum_damage: 1,
                fixed_damage: true,
            },
            now,
        )
        .expect("provoking attack");
        let update = observe_player_at(&store, &map, &player, now + Duration::from_millis(500))
            .expect("aggro observation");

        assert!(update.mobs[0].position.expect("mob position").x > 100.0);
    }

    #[test]
    fn nearby_magic_mobs_do_not_attack_unprovoked() {
        let store = store();
        let mut map = map();
        map.mob_definitions[0].body_attack = false;
        map.mob_definitions[0].magic_attack = 20;
        let player = player(300.0, 100.0);
        let now = Instant::now();

        observe_player_at(&store, &map, &player, now).expect("first observation");
        let update = observe_player_at(&store, &map, &player, now + Duration::from_secs(2))
            .expect("nearby observation");

        assert!(update.mob_projectiles.is_empty());
        assert_eq!(update.player_damage(), 0);
    }

    #[test]
    fn magic_mobs_launch_projectiles_that_damage_their_target() {
        let store = store();
        let mut map = map();
        map.mob_definitions[0].body_attack = false;
        map.mob_definitions[0].magic_attack = 20;
        let player = player(300.0, 100.0);
        let now = Instant::now();

        use_player_attack_at(
            &store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "1:1:0",
                facing_left: true,
                minimum_damage: 1,
                maximum_damage: 1,
                fixed_damage: true,
            },
            now,
        )
        .expect("provoking attack");
        let damage = (1..=10)
            .map(|step| {
                observe_player_at(
                    &store,
                    &map,
                    &player,
                    now + Duration::from_millis(step * 100),
                )
                .expect("projectile observation")
                .player_damage()
            })
            .find(|damage| *damage > 0);

        assert!(damage.is_some());
    }

    #[test]
    fn dead_mobs_return_after_their_respawn_deadline() {
        let store = store();
        let mut map = map();
        map.mob_definitions[0].animations.push(MobAnimation {
            name: "move".to_owned(),
            frames: vec![MobFrame::default()],
        });
        let player = player(250.0, 100.0);
        let now = Instant::now();

        use_player_attack_at(
            &store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "1:1:0",
                facing_left: true,
                minimum_damage: 1,
                maximum_damage: 1,
                fixed_damage: true,
            },
            now,
        )
        .expect("provoking attack");
        let moved = observe_player_at(&store, &map, &player, now + Duration::from_millis(500))
            .expect("mob movement");
        assert!(moved.mobs[0].position.expect("moved position").x > 100.0);

        let killed = use_player_attack_at(
            &store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "1:1:0",
                facing_left: true,
                minimum_damage: 100,
                maximum_damage: 100,
                fixed_damage: true,
            },
            now + Duration::from_millis(501),
        )
        .expect("lethal skill");
        assert_eq!(killed.mobs[0].current_hp, 0);

        let respawned = observe_player_at(&store, &map, &player, now + Duration::from_secs(8))
            .expect("respawn observation");
        assert_eq!(respawned.mobs[0].current_hp, 100);
        assert_eq!(
            respawned.mobs[0].position,
            Some(Vec2 { x: 100.0, y: 100.0 })
        );
    }

    fn store() -> MobStore {
        let formulas = FormulaCatalog::load(Path::new("../../config/skill-formulas.toml"))
            .expect("formula catalog");
        MobStore::new(
            CombatConfig {
                disengage_range: 520.0,
                player_attack_range: 220.0,
                attack_vertical_reach: 90.0,
                touch_horizontal_reach: 28.0,
                touch_vertical_reach: 48.0,
                projectile_range: 420.0,
                projectile_speed: 240.0,
                projectile_hit_reach: 18.0,
                player_attack_interval: Duration::from_millis(600),
                mob_attack_interval: Duration::from_millis(1_500),
                player_invulnerability: Duration::from_secs(1),
                default_respawn: Duration::from_secs(7),
            },
            Arc::new(formulas),
        )
    }

    fn player(
        x: f32,
        y: f32,
    ) -> PlayerState {
        PlayerState {
            id: "player".to_owned(),
            level: 10,
            map_id: 1,
            position: Some(Vec2 { x, y }),
            stats: Some(CharacterStats {
                hp: 50,
                max_hp: 50,
                ..CharacterStats::default()
            }),
            ..PlayerState::default()
        }
    }

    fn map() -> Map {
        Map {
            id: 1,
            width: 500,
            height: 300,
            platforms: vec![Platform {
                id: 1,
                x: 0.0,
                y: 100.0,
                end_x: 500.0,
                end_y: 100.0,
                layer: 0,
                ..Platform::default()
            }],
            mob_spawn_points: vec![MobSpawnPoint {
                spawn_id: 1,
                mob_id: 100,
                position: Some(Vec2 { x: 100.0, y: 100.0 }),
                roam_left: 0.0,
                roam_right: 500.0,
                foothold_id: 1,
                ..MobSpawnPoint::default()
            }],
            mob_definitions: vec![MobDefinition {
                id: 100,
                level: 10,
                max_hp: 100,
                physical_attack: 20,
                body_attack: true,
                animations: vec![MobAnimation {
                    name: "stand".to_owned(),
                    frames: vec![MobFrame::default()],
                }],
                ..MobDefinition::default()
            }],
            ..Map::default()
        }
    }
}
