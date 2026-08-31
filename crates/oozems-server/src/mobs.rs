use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

use oozems_proto::v1::CombatEvent;
use oozems_proto::v1::CombatEventKind;
use oozems_proto::v1::Map;
use oozems_proto::v1::Mob;
use oozems_proto::v1::MobMovementMode;
use oozems_proto::v1::MobProjectile;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::Reactor;
use oozems_proto::v1::Vec2;
use shipyard::IntoIter;
use shipyard::UniqueViewMut;
use shipyard::View;
use shipyard::Workload;
use shipyard::World;
use thiserror::Error;

use crate::attacks::AttackReach;
use crate::effects::ProjectedEffects;
use crate::gameplay::CombatConfig;
use crate::items::DropStore;
use crate::jobs::SkillAttackType;
use crate::jobs::stat_formula_family;
use crate::loot::LootCatalog;
use crate::skill_formula::FormulaCatalog;
use crate::skill_formula::FormulaEvaluationError;
use crate::skill_formula::evaluate_profile_property;

mod ai;
mod combat;
mod components;
mod mailbox;
mod snapshot;
mod spawn;

use components::CombatFormulas;
use components::CombatRules;
use components::MobCombat;
use components::MobHitbox;
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
pub use mailbox::MobStore;
pub use mailbox::commit_player_attack;
pub use mailbox::map_snapshot;
pub use mailbox::observe_player_with_effects;
pub use mailbox::rollback_player_update;
pub use mailbox::use_player_attack_with_effects;
pub use mailbox::use_player_attack_with_reach;
use snapshot::snapshot;
use spawn::spawn_mob_components;

const MAX_CATCH_UP: Duration = Duration::from_secs(1);
const PLAYER_PRESENCE_TIMEOUT_MS: u64 = 5_000;
const MOB_TICK_WORKLOAD: &str = "mob simulation tick";

#[derive(Default)]
struct PlayerRoutes {
    maps: Mutex<HashMap<String, u32>>,
}

struct Simulation {
    maps: HashMap<u32, MobMapState>,
    player_maps: HashMap<String, u32>,
    player_routes: Arc<PlayerRoutes>,
    pending_updates: HashMap<u64, mailbox::PendingUpdate>,
    next_pending_update: u64,
    rules: CombatConfig,
    formulas: Arc<FormulaCatalog>,
    loot: Arc<LootCatalog>,
    drops: Arc<DropStore>,
}

struct MobMapState {
    world: World,
    reactors: Vec<crate::reactors::ReactorRuntime>,
    player_entities: HashMap<String, shipyard::EntityId>,
    updated_at: Instant,
    clock_ms: u64,
    snapshot_sequence: u64,
    next_player_attack_transaction: u64,
}

#[derive(Default)]
pub struct MobUpdate {
    pub mobs: Vec<Mob>,
    pub mob_projectiles: Vec<MobProjectile>,
    pub combat_events: Vec<CombatEvent>,
    pub mob_deaths: Vec<MobDeath>,
    pub staged_drops: Vec<crate::items::StagedDropGrant>,
    pub reactors: Vec<Reactor>,
    pub sequence: u64,
    player_attack_transaction: Option<PlayerAttackTransaction>,
    delivery_id: Option<mailbox::UpdateDelivery>,
}

#[derive(Clone)]
struct PlayerAttackTransaction {
    map_id: u32,
    id: u64,
    target: PlayerAttackTarget,
    generated_combat_events: usize,
    generated_mob_deaths: usize,
    generated_staged_drops: usize,
}

#[derive(Clone)]
enum PlayerAttackTarget {
    Mob {
        entity: shipyard::EntityId,
        before: PlayerAttackMobState,
        after: PlayerAttackMobState,
    },
    Reactor {
        spawn_id: u32,
        before: crate::reactors::ReactorAttackState,
        after: crate::reactors::ReactorAttackState,
    },
}

#[derive(Clone)]
struct PlayerAttackMobState {
    current_hp: u64,
    aggro_target: Option<String>,
    dead_until_ms: Option<u64>,
    random_state: u64,
    mode: MobMovementMode,
    attack_until_ms: u64,
    movement_resume_ms: u64,
    movement_resume_mode: Option<MobMovementMode>,
    speed_penalty: i32,
    slow_expires_at_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MobDeath {
    pub public_id: String,
    pub definition_id: u32,
    pub source_skill_id: Option<u32>,
}

#[derive(Clone, Copy)]
pub struct PlayerAttack<'a> {
    pub target_mob_id: &'a str,
    pub source_skill_id: Option<u32>,
    pub facing_left: bool,
    pub minimum_damage: u32,
    pub maximum_damage: u32,
    pub fixed_damage: bool,
    pub attack_type: SkillAttackType,
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
    #[error("the mob simulation mailbox is unavailable")]
    Unavailable,
    #[error(transparent)]
    PlayerLock(#[from] crate::player_lock::PlayerLockError),
    #[error("the Shipyard simulation could not be built: {message}")]
    Build { message: String },
    #[error("the Shipyard simulation workload failed: {message}")]
    Workload { message: String },
    #[error("the Shipyard simulation borrow failed: {message}")]
    Borrow { message: String },
    #[error("the player attack transaction is inconsistent: {message}")]
    PlayerAttackTransaction { message: String },
    #[error("the simulation update delivery is inconsistent: {message}")]
    UpdateDelivery { message: String },
    #[error("a combat formula failed: {message}")]
    Formula { message: String },
    #[error(transparent)]
    Drops(#[from] crate::items::DropStoreError),
}

impl PlayerRoutes {
    fn map_for(
        &self,
        player_id: &str,
    ) -> Option<u32> {
        self.maps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(player_id)
            .copied()
    }

    fn set_map(
        &self,
        player_id: &str,
        map_id: u32,
    ) {
        self.maps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(player_id.to_owned(), map_id);
    }

    fn remove_map(
        &self,
        player_id: &str,
        map_id: u32,
    ) {
        let mut maps = self
            .maps
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if maps.get(player_id) == Some(&map_id) {
            maps.remove(player_id);
        }
    }
}

impl Simulation {
    fn new(
        rules: CombatConfig,
        formulas: Arc<FormulaCatalog>,
        loot: Arc<LootCatalog>,
        drops: Arc<DropStore>,
        player_routes: Arc<PlayerRoutes>,
    ) -> Self {
        Self {
            maps: HashMap::new(),
            player_maps: HashMap::new(),
            player_routes,
            pending_updates: HashMap::new(),
            next_pending_update: 1,
            rules,
            formulas,
            loot,
            drops,
        }
    }
}

fn apply_map_snapshot(
    simulation: &mut Simulation,
    map: &Map,
    now: Instant,
) -> Result<MobUpdate, MobStoreError> {
    ensure_map_state(simulation, map, now)?;
    let (update, stale_players) = {
        let state = simulation
            .maps
            .get_mut(&map.id)
            .expect("the map simulation was just ensured");
        advance_map_to(state, now)?;
        let stale_players = prune_stale_players(state)?;
        (snapshot(state, None), stale_players)
    };
    clean_stale_player_maps(simulation, map.id, &stale_players);
    update
}

fn apply_observe_player(
    simulation: &mut Simulation,
    map: &Map,
    player: &PlayerState,
    player_layer: i32,
    effects: ProjectedEffects,
    now: Instant,
) -> Result<MobUpdate, MobStoreError> {
    ensure_map_state(simulation, map, now)?;
    remove_player_from_previous_map(simulation, map.id, &player.id);
    let formulas = simulation.formulas.clone();
    let (update, player_is_present, stale_players) = {
        let state = simulation
            .maps
            .get_mut(&map.id)
            .expect("the map simulation was just ensured");
        let mut stale_players = Vec::new();
        let update = (|| {
            sync_player(state, player, player_layer, effects, &formulas)?;
            advance_map_to(state, now)?;
            mark_player_seen(state, &player.id)?;
            stale_players = prune_stale_players(state)?;
            snapshot(state, Some(&player.id))
        })();
        let player_is_present = state.player_entities.contains_key(&player.id);
        (update, player_is_present, stale_players)
    };
    record_player_map(simulation, map.id, &player.id, player_is_present);
    clean_stale_player_maps(simulation, map.id, &stale_players);
    update
}

fn apply_use_player_attack(
    simulation: &mut Simulation,
    map: &Map,
    player: &PlayerState,
    player_layer: i32,
    attack: PlayerAttack<'_>,
    attack_reach: Option<AttackReach>,
    effects: ProjectedEffects,
    now: Instant,
) -> Result<MobUpdate, MobStoreError> {
    ensure_map_state(simulation, map, now)?;
    remove_player_from_previous_map(simulation, map.id, &player.id);
    let formulas = simulation.formulas.clone();
    let loot = simulation.loot.clone();
    let drops = simulation.drops.clone();
    let rules = simulation.rules;
    let (attack_reach, intersects_target_body) = attack_reach.map_or_else(
        || {
            (
                AttackReach {
                    horizontal: rules.player_attack_range,
                    top: -rules.attack_vertical_reach,
                    bottom: rules.attack_vertical_reach,
                },
                false,
            )
        },
        |reach| (reach, true),
    );
    let (update, player_is_present, stale_players) = {
        let state = simulation
            .maps
            .get_mut(&map.id)
            .expect("the map simulation was just ensured");
        let mut stale_players = Vec::new();
        let update = (|| {
            sync_player(state, player, player_layer, effects, &formulas)?;
            advance_map_to(state, now)?;
            mark_player_seen(state, &player.id)?;
            stale_players = prune_stale_players(state)?;
            let player_attack_transaction = if attack.maximum_damage > 0 {
                let transaction = prepare_player_attack(
                    state,
                    map.id,
                    player,
                    PlayerAttack {
                        target_mob_id: attack.target_mob_id,
                        source_skill_id: attack.source_skill_id,
                        facing_left: attack.facing_left,
                        minimum_damage: attack.minimum_damage.max(1),
                        maximum_damage: attack.maximum_damage.max(attack.minimum_damage).max(1),
                        fixed_damage: attack.fixed_damage,
                        attack_type: attack.attack_type,
                    },
                    attack_reach,
                    intersects_target_body,
                    &formulas,
                    &loot,
                    &drops,
                )?;
                state.snapshot_sequence = state.snapshot_sequence.saturating_add(1);
                transaction
            } else {
                None
            };
            let mut update = snapshot(state, Some(&player.id))?;
            update.player_attack_transaction = player_attack_transaction;
            Ok(update)
        })();
        let player_is_present = state.player_entities.contains_key(&player.id);
        (update, player_is_present, stale_players)
    };
    record_player_map(simulation, map.id, &player.id, player_is_present);
    clean_stale_player_maps(simulation, map.id, &stale_players);
    update
}

fn apply_restore_player_effects(
    simulation: &mut Simulation,
    map_id: u32,
    player_id: &str,
    mut combat_events: Vec<CombatEvent>,
    mut mob_deaths: Vec<MobDeath>,
    mut staged_drops: Vec<crate::items::StagedDropGrant>,
) -> Result<(), MobStoreError> {
    if combat_events.is_empty() && mob_deaths.is_empty() && staged_drops.is_empty() {
        return Ok(());
    }
    let Some(state) = simulation.maps.get_mut(&map_id) else {
        return Ok(());
    };
    state
        .world
        .run(|mut pending: UniqueViewMut<PendingEvents>| {
            let queued = pending.by_player.entry(player_id.to_owned()).or_default();
            combat_events.append(queued);
            *queued = combat_events;
            let queued_deaths = pending
                .mob_deaths_by_player
                .entry(player_id.to_owned())
                .or_default();
            mob_deaths.append(queued_deaths);
            *queued_deaths = mob_deaths;
            let queued_drops = pending
                .staged_drops_by_player
                .entry(player_id.to_owned())
                .or_default();
            staged_drops.append(queued_drops);
            *queued_drops = staged_drops;
        });
    Ok(())
}

fn apply_commit_player_attack(
    simulation: &mut Simulation,
    transaction: &PlayerAttackTransaction,
) -> Result<(), MobStoreError> {
    let state = simulation
        .maps
        .get_mut(&transaction.map_id)
        .ok_or_else(|| MobStoreError::PlayerAttackTransaction {
            message: format!("map {} no longer exists", transaction.map_id),
        })?;
    match &transaction.target {
        PlayerAttackTarget::Mob { entity, .. } => {
            let mut combat = state
                .world
                .get::<&mut MobCombat>(*entity)
                .map_err(|error| MobStoreError::PlayerAttackTransaction {
                    message: error.to_string(),
                })?;
            if combat.player_attack_transaction != Some(transaction.id) {
                return Err(transaction_target_error(transaction.id));
            }
            combat.player_attack_transaction = None;
        }
        PlayerAttackTarget::Reactor { spawn_id, .. } => {
            if !crate::reactors::commit_attack(&mut state.reactors, *spawn_id, transaction.id) {
                return Err(transaction_target_error(transaction.id));
            }
        }
    }
    Ok(())
}

fn apply_rollback_player_attack(
    simulation: &mut Simulation,
    transaction: &PlayerAttackTransaction,
) -> Result<bool, MobStoreError> {
    let state = simulation
        .maps
        .get_mut(&transaction.map_id)
        .ok_or_else(|| MobStoreError::PlayerAttackTransaction {
            message: format!("map {} no longer exists", transaction.map_id),
        })?;
    match &transaction.target {
        PlayerAttackTarget::Mob {
            entity,
            before,
            after,
        } => {
            let (mut motion, mut combat) = state
                .world
                .get::<(&mut MobMotion, &mut MobCombat)>(*entity)
                .map_err(|error| MobStoreError::PlayerAttackTransaction {
                    message: error.to_string(),
                })?;
            if combat.player_attack_transaction != Some(transaction.id) {
                return Err(transaction_target_error(transaction.id));
            }
            combat.current_hp = before.current_hp;
            combat.dead_until_ms = before.dead_until_ms;
            if combat.aggro_target == after.aggro_target {
                combat.aggro_target = before.aggro_target.clone();
            }
            if motion.random_state == after.random_state {
                motion.random_state = before.random_state;
            }
            if motion.mode == after.mode {
                motion.mode = before.mode;
            }
            if combat.attack_until_ms == after.attack_until_ms {
                combat.attack_until_ms = before.attack_until_ms;
            }
            if combat.movement_resume_ms == after.movement_resume_ms {
                combat.movement_resume_ms = before.movement_resume_ms;
            }
            if combat.movement_resume_mode == after.movement_resume_mode {
                combat.movement_resume_mode = before.movement_resume_mode;
            }
            if motion.speed_penalty == after.speed_penalty {
                motion.speed_penalty = before.speed_penalty;
            }
            if motion.slow_expires_at_ms == after.slow_expires_at_ms {
                motion.slow_expires_at_ms = before.slow_expires_at_ms;
            }
            combat.player_attack_transaction = None;
        }
        PlayerAttackTarget::Reactor {
            spawn_id,
            before,
            after,
        } => {
            if !crate::reactors::rollback_attack(
                &mut state.reactors,
                *spawn_id,
                transaction.id,
                *before,
                *after,
            ) {
                return Err(transaction_target_error(transaction.id));
            }
        }
    }
    state.snapshot_sequence = state.snapshot_sequence.saturating_add(1);
    Ok(true)
}

fn transaction_target_error(transaction_id: u64) -> MobStoreError {
    MobStoreError::PlayerAttackTransaction {
        message: format!("transaction {transaction_id} no longer owns its target"),
    }
}

#[cfg(test)]
fn observe_player_at(
    simulation: &mut Simulation,
    map: &Map,
    player: &PlayerState,
    now: Instant,
) -> Result<MobUpdate, MobStoreError> {
    apply_observe_player(simulation, map, player, 0, ProjectedEffects::default(), now)
}

#[cfg(test)]
fn use_player_attack_at(
    simulation: &mut Simulation,
    map: &Map,
    player: &PlayerState,
    attack: PlayerAttack<'_>,
    now: Instant,
) -> Result<MobUpdate, MobStoreError> {
    use_player_attack_at_on_layer(simulation, map, player, 0, attack, now)
}

#[cfg(test)]
fn use_player_attack_at_on_layer(
    simulation: &mut Simulation,
    map: &Map,
    player: &PlayerState,
    player_layer: i32,
    attack: PlayerAttack<'_>,
    now: Instant,
) -> Result<MobUpdate, MobStoreError> {
    let mut update = apply_use_player_attack(
        simulation,
        map,
        player,
        player_layer,
        attack,
        None,
        ProjectedEffects::default(),
        now,
    )?;
    if let Some(transaction) = update.player_attack_transaction.clone() {
        apply_commit_player_attack(simulation, &transaction)?;
        update.player_attack_transaction = None;
    }
    Ok(update)
}

#[cfg(test)]
fn use_player_attack_at_with_reach(
    simulation: &mut Simulation,
    map: &Map,
    player: &PlayerState,
    attack: PlayerAttack<'_>,
    reach: AttackReach,
    now: Instant,
) -> Result<MobUpdate, MobStoreError> {
    let mut update = apply_use_player_attack(
        simulation,
        map,
        player,
        0,
        attack,
        Some(reach),
        ProjectedEffects::default(),
        now,
    )?;
    if let Some(transaction) = update.player_attack_transaction.clone() {
        apply_commit_player_attack(simulation, &transaction)?;
        update.player_attack_transaction = None;
    }
    Ok(update)
}

#[cfg(test)]
fn use_player_attack_at_uncommitted(
    simulation: &mut Simulation,
    map: &Map,
    player: &PlayerState,
    attack: PlayerAttack<'_>,
    now: Instant,
) -> Result<MobUpdate, MobStoreError> {
    apply_use_player_attack(
        simulation,
        map,
        player,
        0,
        attack,
        None,
        ProjectedEffects::default(),
        now,
    )
}

fn ensure_map_state(
    simulation: &mut Simulation,
    map: &Map,
    now: Instant,
) -> Result<(), MobStoreError> {
    if simulation.maps.contains_key(&map.id) {
        return Ok(());
    }
    let state = build_map_state(map, simulation.rules, simulation.formulas.clone(), now)?;
    simulation.maps.insert(map.id, state);
    Ok(())
}

fn remove_player_from_previous_map(
    simulation: &mut Simulation,
    current_map_id: u32,
    player_id: &str,
) {
    let previous_map_id = simulation.player_maps.get(player_id).copied();
    let Some(previous_map_id) = previous_map_id.filter(|map_id| *map_id != current_map_id) else {
        return;
    };
    apply_remove_player(simulation, previous_map_id, player_id);
}

fn apply_remove_player(
    simulation: &mut Simulation,
    map_id: u32,
    player_id: &str,
) {
    if let Some(state) = simulation.maps.get_mut(&map_id)
        && let Some(entity) = state.player_entities.remove(player_id)
    {
        state.world.delete_entity(entity);
    }
    if simulation.player_maps.get(player_id) == Some(&map_id) {
        simulation.player_maps.remove(player_id);
    }
    simulation.player_routes.remove_map(player_id, map_id);
}

fn record_player_map(
    simulation: &mut Simulation,
    map_id: u32,
    player_id: &str,
    player_is_present: bool,
) {
    if player_is_present {
        simulation.player_maps.insert(player_id.to_owned(), map_id);
        simulation.player_routes.set_map(player_id, map_id);
    } else {
        if simulation.player_maps.get(player_id) == Some(&map_id) {
            simulation.player_maps.remove(player_id);
        }
        simulation.player_routes.remove_map(player_id, map_id);
    }
}

fn clean_stale_player_maps(
    simulation: &mut Simulation,
    map_id: u32,
    stale_player_ids: &[String],
) {
    for player_id in stale_player_ids {
        if simulation.player_maps.get(player_id) == Some(&map_id) {
            simulation.player_maps.remove(player_id);
            simulation.player_routes.remove_map(player_id, map_id);
        }
    }
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
    for (identity, position, hitbox, motion, combat) in
        spawn_mob_components(map, rules.default_respawn)
    {
        world.add_entity((identity, position, hitbox, motion, combat));
    }
    Ok(MobMapState {
        world,
        reactors: crate::reactors::spawn(map),
        player_entities: HashMap::new(),
        updated_at: now,
        clock_ms: 0,
        snapshot_sequence: 0,
        next_player_attack_transaction: 1,
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
    let respawned = crate::reactors::advance(&mut state.reactors, state.clock_ms);
    queue_reactor_respawns(state, respawned);
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

fn queue_reactor_respawns(
    state: &MobMapState,
    respawned: Vec<crate::reactors::RespawnedReactor>,
) {
    if respawned.is_empty() {
        return;
    }
    let player_ids = state.player_entities.keys().cloned().collect::<Vec<_>>();
    state.world.run(|mut events: UniqueViewMut<PendingEvents>| {
        for reactor in &respawned {
            for player_id in &player_ids {
                combat::queue_event(
                    &mut events,
                    player_id,
                    CombatEventKind::ReactorRespawned,
                    &reactor.id,
                    &reactor.id,
                    0,
                    Position {
                        x: reactor.position.x,
                        y: reactor.position.y,
                        layer: reactor.layer,
                    },
                );
            }
        }
    });
}

fn sync_player(
    state: &mut MobMapState,
    player: &PlayerState,
    player_layer: i32,
    effects: ProjectedEffects,
    formulas: &FormulaCatalog,
) -> Result<(), MobStoreError> {
    let Some(position) = player.position.filter(finite_position) else {
        return Ok(());
    };
    let stats = player.stats.unwrap_or_default();
    let variables = [
        ("Dexterity", f64::from(stats.dexterity)),
        ("Intelligence", f64::from(stats.intelligence)),
        ("Luck", f64::from(stats.luck)),
    ];
    let accuracy =
        evaluate_player_stat(formulas, stats.job_id, "accuracy", &variables).map_err(|error| {
            MobStoreError::Formula {
                message: format!("player accuracy failed: {error}"),
            }
        })?;
    let avoidability = evaluate_player_stat(formulas, stats.job_id, "avoidability", &variables)
        .map_err(|error| MobStoreError::Formula {
            message: format!("player avoidability failed: {error}"),
        })?;
    let accuracy = bounded_combat_stat(accuracy).saturating_add(effects.modifiers.accuracy);
    let avoidability =
        bounded_combat_stat(avoidability).saturating_add(effects.modifiers.avoidability);
    let simulation_position = Position {
        x: position.x,
        y: position.y,
        layer: player_layer,
    };
    if let Some(entity) = state.player_entities.get(&player.id).copied()
        && let Ok((mut stored_position, mut presence)) = state
            .world
            .get::<(&mut Position, &mut PlayerPresence)>(entity)
    {
        **stored_position = simulation_position;
        presence.level = player.level;
        presence.current_hp = stats.hp;
        presence.weapon_defense = effects.modifiers.weapon_defense;
        presence.magic_defense = effects.modifiers.magic_defense;
        presence.accuracy = accuracy;
        presence.accuracy_bonus = effects.modifiers.accuracy;
        presence.intelligence = stats.intelligence;
        presence.luck = stats.luck;
        presence.avoidability = avoidability;
        presence.critical_chance = effects.modifiers.critical_chance;
        presence.critical_damage = effects.modifiers.critical_damage;
        presence.enemy_slow = effects.enemy_slow;
        presence.last_seen_ms = state.clock_ms;
        return Ok(());
    }
    let entity = state.world.add_entity((
        simulation_position,
        PlayerPresence {
            id: player.id.clone(),
            level: player.level,
            current_hp: stats.hp,
            weapon_defense: effects.modifiers.weapon_defense,
            magic_defense: effects.modifiers.magic_defense,
            accuracy,
            accuracy_bonus: effects.modifiers.accuracy,
            intelligence: stats.intelligence,
            luck: stats.luck,
            avoidability,
            critical_chance: effects.modifiers.critical_chance,
            critical_damage: effects.modifiers.critical_damage,
            enemy_slow: effects.enemy_slow,
            last_seen_ms: state.clock_ms,
            invulnerable_until_ms: 0,
            contact_attempt_after_ms: 0,
        },
    ));
    state.player_entities.insert(player.id.clone(), entity);
    Ok(())
}

fn evaluate_player_stat(
    formulas: &FormulaCatalog,
    job_id: u32,
    property: &str,
    variables: &[(&str, f64)],
) -> Result<f64, FormulaEvaluationError> {
    let standard = formulas
        .stat_profile("standard")
        .expect("the standard stat profile is validated during startup");
    let family = stat_formula_family(job_id);
    if family == crate::jobs::StatFormulaFamily::Standard {
        return evaluate_profile_property(standard, property, variables);
    }
    let specialized = formulas
        .stat_profile(family.profile_name())
        .expect("named specialized stat profiles are validated during startup");
    match evaluate_profile_property(specialized, property, variables) {
        Err(FormulaEvaluationError::MissingProperty { .. }) => {
            evaluate_profile_property(standard, property, variables)
        }
        result => result,
    }
}

fn bounded_combat_stat(value: f64) -> i32 {
    if !value.is_finite() {
        return 0;
    }
    value
        .trunc()
        .clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32
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

fn prepare_player_attack(
    state: &mut MobMapState,
    map_id: u32,
    player: &PlayerState,
    attack: PlayerAttack<'_>,
    reach: AttackReach,
    intersects_target_body: bool,
    formulas: &FormulaCatalog,
    loot: &LootCatalog,
    drops: &DropStore,
) -> Result<Option<PlayerAttackTransaction>, MobStoreError> {
    let player_id = player.id.as_str();
    let Some(player_entity) = state.player_entities.get(player_id).copied() else {
        return Ok(None);
    };
    let (player_position, player_presence) = state
        .world
        .get::<(&Position, &PlayerPresence)>(player_entity)
        .map(|(position, presence)| (**position, presence.clone()))
        .map_err(|error| borrow_error(error.to_string()))?;
    let mob_target = state.world.run(
        |positions: View<Position>,
         identities: View<MobIdentity>,
         hitboxes: View<MobHitbox>,
         combats: View<MobCombat>| {
            let candidates = (&positions, &identities, &hitboxes, &combats)
                .iter()
                .with_id()
                .filter(|(_, (position, identity, hitbox, combat))| {
                    combat.current_hp > 0
                        && combat.player_attack_transaction.is_none()
                        && valid_attack_target(
                            player_position,
                            **position,
                            if intersects_target_body {
                                hitbox.0
                            } else {
                                crate::attacks::POINT_VERTICAL_BOUNDS
                            },
                            attack.facing_left,
                            reach,
                        )
                        && (attack.target_mob_id.is_empty()
                            || identity.public_id == attack.target_mob_id)
                });
            candidates
                .map(|(entity, (position, _, _, _))| {
                    (entity, (position.x - player_position.x).abs())
                })
                .min_by(|left, right| left.1.total_cmp(&right.1))
        },
    );
    let reactor_target = (attack.source_skill_id.is_none() && attack.target_mob_id.is_empty())
        .then(|| {
            crate::reactors::nearest_attack_candidate(
                &state.reactors,
                player_position.x,
                player_position.y,
                player_position.layer,
                attack.facing_left,
                reach,
                intersects_target_body,
            )
        })
        .flatten();
    if reactor_target.is_some_and(|(_, reactor_distance)| {
        mob_target.is_none_or(|(_, mob_distance)| reactor_distance < mob_distance)
    }) {
        return prepare_reactor_attack(
            state,
            map_id,
            player,
            reactor_target.expect("reactor target was checked").0,
            loot,
            drops,
        );
    }
    let Some((target, _)) = mob_target else {
        return Ok(None);
    };
    let transaction_id = state.next_player_attack_transaction;
    state.next_player_attack_transaction = state.next_player_attack_transaction.saturating_add(1);
    let (
        position,
        target_id,
        target_definition_id,
        damage,
        died,
        missed,
        staggered,
        staged_drops,
        before,
        after,
    ) = {
        let (position, identity, mut motion, mut combat) = state
            .world
            .get::<(&Position, &MobIdentity, &mut MobMotion, &mut MobCombat)>(target)
            .map_err(|error| borrow_error(error.to_string()))?;
        let before = PlayerAttackMobState {
            current_hp: combat.current_hp,
            aggro_target: combat.aggro_target.clone(),
            dead_until_ms: combat.dead_until_ms,
            random_state: motion.random_state,
            mode: motion.mode,
            attack_until_ms: combat.attack_until_ms,
            movement_resume_ms: combat.movement_resume_ms,
            movement_resume_mode: combat.movement_resume_mode,
            speed_penalty: motion.speed_penalty,
            slow_expires_at_ms: motion.slow_expires_at_ms,
        };
        let hit = combat::player_attack_hits(
            formulas,
            attack.attack_type,
            player_presence.accuracy,
            player_presence.accuracy_bonus,
            player_presence.intelligence,
            player_presence.luck,
            combat.avoidability,
            combat.level,
            player_presence.level,
            &mut motion.random_state,
        )
        .map_err(|error| MobStoreError::Formula {
            message: format!(
                "accuracy against mob {} failed: {error}",
                identity.public_id
            ),
        })?;
        let damage = if hit {
            let defense = match attack.attack_type {
                SkillAttackType::Physical => combat.physical_defense,
                SkillAttackType::Magical => combat.magic_defense,
            };
            combat::calculate_player_damage(
                formulas,
                attack.attack_type,
                defense,
                combat.level,
                player_presence.level,
                attack.minimum_damage,
                attack.maximum_damage,
                attack.fixed_damage,
                player_presence.critical_chance,
                player_presence.critical_damage,
                &mut motion.random_state,
            )
            .map_err(|error| MobStoreError::Formula {
                message: format!("damage against mob {} failed: {error}", identity.public_id),
            })?
        } else {
            0
        };
        let died = damage >= combat.current_hp;
        let staged_drops = if hit && died {
            let item_ids = crate::loot::roll_mob_items(
                loot,
                identity.definition_id,
                player,
                &mut motion.random_state,
            );
            crate::items::stage_combat_drops(
                drops,
                map_id,
                position.vector(),
                &item_ids,
                player_id,
            )?
        } else {
            Vec::new()
        };
        let staggered = hit
            && !died
            && combat.stagger_threshold > 0
            && damage >= combat.stagger_threshold
            && combat.stagger_duration_ms > 0;
        if hit {
            combat.current_hp = combat.current_hp.saturating_sub(damage);
        }
        if damage > 0
            && !died
            && let Some(slow) = player_presence.enemy_slow
            && let Some((speed_penalty, expires_at_ms)) =
                roll_enemy_slow(&mut motion.random_state, slow, state.clock_ms)
        {
            motion.speed_penalty = speed_penalty;
            motion.slow_expires_at_ms = expires_at_ms;
        }
        combat.aggro_target = Some(player_id.to_owned());
        if combat.current_hp == 0 {
            combat.dead_until_ms = Some(state.clock_ms.saturating_add(combat.respawn_delay_ms));
            combat.aggro_target = None;
            motion.mode = MobMovementMode::Idle;
        } else if staggered {
            combat.attack_until_ms = 0;
            combat.movement_resume_ms = state.clock_ms.saturating_add(combat.stagger_duration_ms);
            combat.movement_resume_mode = Some(if motion.mode == MobMovementMode::Jumping {
                MobMovementMode::Jumping
            } else {
                MobMovementMode::Idle
            });
            motion.mode = MobMovementMode::Idle;
        }
        combat.player_attack_transaction = Some(transaction_id);
        let after = PlayerAttackMobState {
            current_hp: combat.current_hp,
            aggro_target: combat.aggro_target.clone(),
            dead_until_ms: combat.dead_until_ms,
            random_state: motion.random_state,
            mode: motion.mode,
            attack_until_ms: combat.attack_until_ms,
            movement_resume_ms: combat.movement_resume_ms,
            movement_resume_mode: combat.movement_resume_mode,
            speed_penalty: motion.speed_penalty,
            slow_expires_at_ms: motion.slow_expires_at_ms,
        };
        (
            **position,
            identity.public_id.clone(),
            identity.definition_id,
            damage,
            died,
            !hit,
            staggered,
            staged_drops,
            before,
            after,
        )
    };
    let generated_staged_drops = staged_drops.len();
    let generated_mob_deaths = usize::from(died && !missed);
    let generated_combat_events = 1 + generated_mob_deaths;
    let source_id = player_id.to_owned();
    state.world.run(|mut events: UniqueViewMut<PendingEvents>| {
        combat::queue_event_with_stagger(
            &mut events,
            player_id,
            if missed {
                CombatEventKind::PlayerMissedMob
            } else {
                CombatEventKind::PlayerHitMob
            },
            &source_id,
            &target_id,
            damage,
            position,
            staggered,
        );
        if died && !missed {
            events
                .mob_deaths_by_player
                .entry(player_id.to_owned())
                .or_default()
                .push(MobDeath {
                    public_id: target_id.clone(),
                    definition_id: target_definition_id,
                    source_skill_id: attack.source_skill_id,
                });
            events
                .staged_drops_by_player
                .entry(player_id.to_owned())
                .or_default()
                .extend(staged_drops);
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
    Ok(Some(PlayerAttackTransaction {
        map_id,
        id: transaction_id,
        target: PlayerAttackTarget::Mob {
            entity: target,
            before,
            after,
        },
        generated_combat_events,
        generated_mob_deaths,
        generated_staged_drops,
    }))
}

fn roll_enemy_slow(
    random_state: &mut u64,
    slow: crate::effects::EnemySlowEffect,
    now_ms: u64,
) -> Option<(i32, u64)> {
    if slow.duration_ms == 0
        || slow.speed_penalty == 0
        || slow.chance == 0
        || crate::random::next_u64(random_state) % 100 >= u64::from(slow.chance.min(100))
    {
        return None;
    }
    Some((slow.speed_penalty, now_ms.saturating_add(slow.duration_ms)))
}

fn prepare_reactor_attack(
    state: &mut MobMapState,
    map_id: u32,
    player: &PlayerState,
    target_index: usize,
    loot: &LootCatalog,
    drops: &DropStore,
) -> Result<Option<PlayerAttackTransaction>, MobStoreError> {
    let player_id = player.id.as_str();
    let transaction_id = state.next_player_attack_transaction;
    state.next_player_attack_transaction = state.next_player_attack_transaction.saturating_add(1);
    let Some(result) = crate::reactors::prepare_attack(
        &mut state.reactors,
        target_index,
        state.clock_ms,
        transaction_id,
        loot,
        player,
    ) else {
        return Ok(None);
    };
    let staged_drops = match crate::items::stage_combat_drops(
        drops,
        map_id,
        result.position,
        &result.item_ids,
        player_id,
    ) {
        Ok(staged_drops) => staged_drops,
        Err(error) => {
            let rolled_back = crate::reactors::rollback_attack(
                &mut state.reactors,
                result.spawn_id,
                transaction_id,
                result.before,
                result.after,
            );
            debug_assert!(rolled_back, "prepared reactor attack must be rollbackable");
            return Err(error.into());
        }
    };
    let generated_combat_events = 1 + usize::from(result.destroyed);
    let generated_staged_drops = staged_drops.len();
    state.world.run(|mut events: UniqueViewMut<PendingEvents>| {
        let position = Position {
            x: result.position.x,
            y: result.position.y,
            layer: result.layer,
        };
        combat::queue_event(
            &mut events,
            player_id,
            CombatEventKind::PlayerHitReactor,
            player_id,
            &result.id,
            0,
            position,
        );
        if result.destroyed {
            events
                .staged_drops_by_player
                .entry(player_id.to_owned())
                .or_default()
                .extend(staged_drops);
            combat::queue_event(
                &mut events,
                player_id,
                CombatEventKind::ReactorDestroyed,
                player_id,
                &result.id,
                0,
                position,
            );
        }
    });
    Ok(Some(PlayerAttackTransaction {
        map_id,
        id: transaction_id,
        target: PlayerAttackTarget::Reactor {
            spawn_id: result.spawn_id,
            before: result.before,
            after: result.after,
        },
        generated_combat_events,
        generated_mob_deaths: 0,
        generated_staged_drops,
    }))
}

fn valid_attack_target(
    player: Position,
    target: Position,
    target_bounds: crate::attacks::VerticalBounds,
    facing_left: bool,
    reach: AttackReach,
) -> bool {
    if player.layer != target.layer
        || !crate::attacks::vertical_attack_intersects(player.y, reach, target.y, target_bounds)
    {
        return false;
    }
    let delta_x = target.x - player.x;
    delta_x.abs() <= reach.horizontal
        && if facing_left {
            delta_x <= 0.0
        } else {
            delta_x >= 0.0
        }
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

fn prune_stale_players(state: &mut MobMapState) -> Result<Vec<String>, MobStoreError> {
    let stale_before = state.clock_ms.saturating_sub(PLAYER_PRESENCE_TIMEOUT_MS);
    let stale = state.world.run(|players: View<PlayerPresence>| {
        players
            .iter()
            .with_id()
            .filter(|(_, player)| player.last_seen_ms < stale_before)
            .map(|(entity, player)| (entity, player.id.clone()))
            .collect::<Vec<_>>()
    });
    for (entity, player_id) in &stale {
        state.world.delete_entity(*entity);
        state.player_entities.remove(player_id);
    }
    Ok(stale.into_iter().map(|(_, player_id)| player_id).collect())
}

fn finite_position(position: &Vec2) -> bool {
    position.x.is_finite() && position.y.is_finite()
}

fn borrow_error(message: String) -> MobStoreError {
    MobStoreError::Borrow { message }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;
    use std::time::Instant;

    use oozems_proto::v1::CharacterStats;
    use oozems_proto::v1::CombatEventKind;
    use oozems_proto::v1::ItemDefinition;
    use oozems_proto::v1::Map;
    use oozems_proto::v1::MobAnimation;
    use oozems_proto::v1::MobDefinition;
    use oozems_proto::v1::MobFrame;
    use oozems_proto::v1::MobMovementMode;
    use oozems_proto::v1::MobSpawnPoint;
    use oozems_proto::v1::Platform;
    use oozems_proto::v1::PlayerQuest;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::QuestStatus;
    use oozems_proto::v1::ReactorDefinition;
    use oozems_proto::v1::ReactorFrame;
    use oozems_proto::v1::ReactorSpawnPoint;
    use oozems_proto::v1::ReactorStateDefinition;
    use oozems_proto::v1::Vec2;
    use shipyard::IntoIter;
    use shipyard::View;

    use super::MobDeath;
    use super::MobStore;
    use super::MobStoreError;
    use super::MobUpdate;
    use super::PlayerAttack;
    use super::PlayerRoutes;
    use super::Position;
    use super::Simulation;
    use super::apply_map_snapshot as map_snapshot_at;
    use super::apply_restore_player_effects as restore_player_effects;
    use super::apply_rollback_player_attack;
    use super::commit_player_attack;
    use super::components::MobCombat;
    use super::components::MobMotion;
    use super::evaluate_player_stat;
    use super::mailbox::block_worker_for_map;
    use super::mailbox::delivery_coordinates;
    use super::mailbox::expire_pending_updates;
    use super::mailbox::map_contains_player;
    use super::mailbox::signal_next_player_enqueue;
    use super::roll_enemy_slow;

    #[test]
    fn enemy_slow_roll_applies_penalty_and_exact_deadline() {
        let mut random_state = 1;

        let slow = roll_enemy_slow(
            &mut random_state,
            crate::effects::EnemySlowEffect {
                speed_penalty: -40,
                duration_ms: 20_000,
                chance: 100,
            },
            5_000,
        );
        assert_eq!(slow, Some((-40, 25_000)));
    }
    use super::mailbox::worker_index_for_map;
    use super::map_snapshot as mailbox_map_snapshot;
    use super::observe_player_at;
    use super::observe_player_with_effects;
    use super::rollback_player_update;
    use super::use_player_attack_at;
    use super::use_player_attack_at_on_layer;
    use super::use_player_attack_at_uncommitted;
    use super::use_player_attack_at_with_reach;
    use super::use_player_attack_with_effects;
    use super::use_player_attack_with_reach;
    use super::valid_attack_target;
    use crate::attacks::AttackReach;
    use crate::attacks::POINT_VERTICAL_BOUNDS;
    use crate::attacks::VerticalBounds;
    use crate::content::QuestDefinition;
    use crate::gameplay::CombatConfig;
    use crate::items::DropStore;
    use crate::items::SPARE_BOTTOM_ID;
    use crate::items::SPARE_TOP_ID;
    use crate::items::map_drops;
    use crate::jobs::SkillAttackType;
    use crate::loot::LootCatalog;
    use crate::skill_formula::FormulaCatalog;

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn mailbox_serializes_concurrent_commands_and_routes_each_response() {
        const REQUESTS: usize = 64;

        let store = Arc::new(mailbox_store());
        let map = Arc::new(map());
        let first = mailbox_map_snapshot(&store, &map)
            .await
            .expect("initialize map");
        let barrier = Arc::new(tokio::sync::Barrier::new(REQUESTS + 1));
        let mut tasks = Vec::with_capacity(REQUESTS);
        for _ in 0..REQUESTS {
            let store = store.clone();
            let map = map.clone();
            let barrier = barrier.clone();
            tasks.push(tokio::spawn(async move {
                barrier.wait().await;
                mailbox_map_snapshot(&store, &map)
                    .await
                    .expect("mailbox response")
                    .sequence
            }));
        }
        barrier.wait().await;

        let mut sequences = Vec::with_capacity(REQUESTS);
        for task in tasks {
            sequences.push(task.await.expect("request task"));
        }
        sequences.sort_unstable();

        let first_expected = first.sequence + 1;
        assert_eq!(
            sequences,
            (first_expected..first_expected + REQUESTS as u64).collect::<Vec<_>>()
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mailbox_runs_independent_map_shards_in_parallel() {
        let store = Arc::new(mailbox_store());
        let blocked_map_id = 1;
        let parallel_map_id = map_id_on_different_worker(&store, blocked_map_id);
        let (started_sender, started_receiver) = std::sync::mpsc::channel();
        let (release_sender, release_receiver) = std::sync::mpsc::channel();
        let blocking_store = store.clone();
        let blocking = tokio::spawn(async move {
            block_worker_for_map(
                &blocking_store,
                blocked_map_id,
                started_sender,
                release_receiver,
            )
            .await
        });
        started_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("blocked worker started");

        let mut parallel_map = map();
        parallel_map.id = parallel_map_id;
        let parallel_store = store.clone();
        let (finished_sender, finished_receiver) = std::sync::mpsc::channel();
        let parallel = tokio::spawn(async move {
            let result = mailbox_map_snapshot(&parallel_store, &parallel_map).await;
            let _ = finished_sender.send(());
            result
        });

        finished_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("independent shard completed while the first was blocked");
        release_sender.send(()).expect("release blocked worker");
        blocking
            .await
            .expect("blocking task")
            .expect("blocking command");
        parallel
            .await
            .expect("parallel task")
            .expect("parallel snapshot");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn cancelled_player_request_finishes_enqueued_relocation() {
        let store = Arc::new(mailbox_store());
        let first_map = Arc::new(map());
        let mut second_map = map();
        second_map.id = map_id_on_different_worker(&store, first_map.id);
        let second_map = Arc::new(second_map);
        let (started_sender, started_receiver) = std::sync::mpsc::channel();
        let (release_sender, release_receiver) = std::sync::mpsc::channel();
        let blocking_store = store.clone();
        let blocked_map_id = first_map.id;
        let blocking = tokio::spawn(async move {
            block_worker_for_map(
                &blocking_store,
                blocked_map_id,
                started_sender,
                release_receiver,
            )
            .await
        });
        started_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("blocked worker started");
        let enqueued = signal_next_player_enqueue(&store, "player", first_map.id);

        let cancelled_store = store.clone();
        let cancelled_map = first_map.clone();
        let cancelled = tokio::spawn(async move {
            observe_player_with_effects(
                &cancelled_store,
                &cancelled_map,
                &player(90.0, 100.0),
                0,
                crate::effects::ProjectedEffects::default(),
            )
            .await
        });
        enqueued
            .recv_timeout(Duration::from_secs(1))
            .expect("cancelled player command enqueued");
        cancelled.abort();
        assert!(matches!(cancelled.await, Err(error) if error.is_cancelled()));

        let destination_store = store.clone();
        let destination_map = second_map.clone();
        let destination = tokio::spawn(async move {
            let mut player = player(90.0, 100.0);
            player.map_id = destination_map.id;
            observe_player_with_effects(
                &destination_store,
                &destination_map,
                &player,
                0,
                crate::effects::ProjectedEffects::default(),
            )
            .await
        });
        release_sender.send(()).expect("release blocked worker");
        blocking
            .await
            .expect("blocking task")
            .expect("blocking command");
        let mut destination = destination
            .await
            .expect("destination task")
            .expect("destination observation");
        commit_player_attack(&store, &mut destination)
            .await
            .expect("acknowledge destination observation");

        assert!(
            !map_contains_player(&store, first_map.id, "player")
                .await
                .expect("query first map")
        );
        assert!(
            map_contains_player(&store, second_map.id, "player")
                .await
                .expect("query second map")
        );
    }

    #[tokio::test]
    async fn mailbox_moves_player_presence_between_map_shards() {
        let store = mailbox_store();
        let first_map = map();
        let mut second_map = map();
        second_map.id = map_id_on_different_worker(&store, first_map.id);
        let mut player = player(90.0, 100.0);
        let mut first = observe_player_with_effects(
            &store,
            &first_map,
            &player,
            0,
            crate::effects::ProjectedEffects::default(),
        )
        .await
        .expect("observe first map");
        commit_player_attack(&store, &mut first)
            .await
            .expect("acknowledge first observation");

        player.map_id = second_map.id;
        let mut second = observe_player_with_effects(
            &store,
            &second_map,
            &player,
            0,
            crate::effects::ProjectedEffects::default(),
        )
        .await
        .expect("observe second map");
        commit_player_attack(&store, &mut second)
            .await
            .expect("acknowledge second observation");

        assert!(
            !map_contains_player(&store, first_map.id, &player.id)
                .await
                .expect("query first map")
        );
        assert!(
            map_contains_player(&store, second_map.id, &player.id)
                .await
                .expect("query second map")
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_player_relocations_leave_presence_on_exactly_one_map() {
        let store = Arc::new(mailbox_store());
        let first_map = Arc::new(map());
        let mut second_map = map();
        second_map.id = map_id_on_different_worker(&store, first_map.id);
        let second_map = Arc::new(second_map);
        let first_player = player(90.0, 100.0);
        let mut second_player = first_player.clone();
        second_player.map_id = second_map.id;

        let first_store = store.clone();
        let first_request_map = first_map.clone();
        let first = tokio::spawn(async move {
            observe_player_with_effects(
                &first_store,
                &first_request_map,
                &first_player,
                0,
                crate::effects::ProjectedEffects::default(),
            )
            .await
        });
        let second_store = store.clone();
        let second_request_map = second_map.clone();
        let second = tokio::spawn(async move {
            observe_player_with_effects(
                &second_store,
                &second_request_map,
                &second_player,
                0,
                crate::effects::ProjectedEffects::default(),
            )
            .await
        });
        let mut first = first.await.expect("first task").expect("first observation");
        let mut second = second
            .await
            .expect("second task")
            .expect("second observation");
        commit_player_attack(&store, &mut first)
            .await
            .expect("acknowledge first observation");
        commit_player_attack(&store, &mut second)
            .await
            .expect("acknowledge second observation");

        let first_present = map_contains_player(&store, first_map.id, "player")
            .await
            .expect("query first map");
        let second_present = map_contains_player(&store, second_map.id, "player")
            .await
            .expect("query second map");
        assert_ne!(first_present, second_present);
    }

    #[tokio::test]
    async fn delivery_coordinates_route_worker_local_sequences_to_their_maps() {
        let store = mailbox_store();
        let first_map = map();
        let mut second_map = map();
        second_map.id = map_id_on_different_worker(&store, first_map.id);
        let first_player = player(90.0, 100.0);
        let mut second_player = player(90.0, 100.0);
        second_player.id = "second-player".to_owned();
        second_player.map_id = second_map.id;

        let mut first = mailbox_attack(&store, &first_map, &first_player, 10).await;
        let mut second = mailbox_attack(&store, &second_map, &second_player, 10).await;
        let first_delivery = delivery_coordinates(&first).expect("first delivery");
        let second_delivery = delivery_coordinates(&second).expect("second delivery");
        assert_eq!(first_delivery.1, second_delivery.1);
        assert_ne!(first_delivery.0, second_delivery.0);

        commit_player_attack(&store, &mut first)
            .await
            .expect("commit first map attack");
        assert!(
            rollback_player_update(&store, &mut second)
                .await
                .expect("roll back second map attack")
        );
        let first_snapshot = mailbox_map_snapshot(&store, &first_map)
            .await
            .expect("first map snapshot");
        let second_snapshot = mailbox_map_snapshot(&store, &second_map)
            .await
            .expect("second map snapshot");

        assert_eq!(first_snapshot.mobs[0].current_hp, 90);
        assert_eq!(second_snapshot.mobs[0].current_hp, 100);
    }

    #[tokio::test]
    async fn delivery_remains_routable_after_the_player_changes_shards() {
        let store = mailbox_store();
        let first_map = map();
        let mut second_map = map();
        second_map.id = map_id_on_different_worker(&store, first_map.id);
        let mut player = player(90.0, 100.0);
        let mut attack = mailbox_attack(&store, &first_map, &player, 10).await;

        player.map_id = second_map.id;
        let mut observation = observe_player_with_effects(
            &store,
            &second_map,
            &player,
            0,
            crate::effects::ProjectedEffects::default(),
        )
        .await
        .expect("observe destination map");
        commit_player_attack(&store, &mut observation)
            .await
            .expect("acknowledge destination observation");
        commit_player_attack(&store, &mut attack)
            .await
            .expect("commit source map attack after relocation");

        let source = mailbox_map_snapshot(&store, &first_map)
            .await
            .expect("source map snapshot");
        assert_eq!(source.mobs[0].current_hp, 90);
    }

    #[tokio::test]
    async fn mailbox_rolls_back_an_uncommitted_attack_before_responding() {
        let store = mailbox_store();
        let map = map();
        let player = player(90.0, 100.0);
        let mut update = use_player_attack_with_effects(
            &store,
            &map,
            &player,
            0,
            PlayerAttack {
                target_mob_id: "1:1:0",
                source_skill_id: None,
                facing_left: false,
                minimum_damage: 10,
                maximum_damage: 10,
                fixed_damage: true,
                attack_type: SkillAttackType::Physical,
            },
            crate::effects::ProjectedEffects::default(),
        )
        .await
        .expect("uncommitted attack");
        assert_eq!(update.mobs[0].current_hp, 90);

        assert!(
            rollback_player_update(&store, &mut update)
                .await
                .expect("rollback response")
        );
        let restored = mailbox_map_snapshot(&store, &map)
            .await
            .expect("restored snapshot");

        assert_eq!(restored.mobs[0].current_hp, 100);
        assert!(update.combat_events.is_empty());
    }

    #[tokio::test]
    async fn mailbox_preserves_the_weapon_attack_reach() {
        let store = mailbox_store();
        let map = map();
        let player = player(11.0, 100.0);
        let mut update = use_player_attack_with_reach(
            &store,
            &map,
            &player,
            0,
            PlayerAttack {
                target_mob_id: "",
                source_skill_id: None,
                facing_left: false,
                minimum_damage: 10,
                maximum_damage: 10,
                fixed_damage: true,
                attack_type: SkillAttackType::Physical,
            },
            AttackReach {
                horizontal: 88.0,
                top: -62.0,
                bottom: -6.0,
            },
            crate::effects::ProjectedEffects::default(),
        )
        .await
        .expect("mailbox weapon attack");

        assert_eq!(update.mobs[0].current_hp, 100);
        commit_player_attack(&store, &mut update)
            .await
            .expect("acknowledge mailbox weapon attack");
    }

    #[tokio::test]
    async fn mailbox_restores_abandoned_reactor_destruction_and_loot() {
        let store = mailbox_store_with_loot(&format!(
            "[[reactors]]\nreactor_id = 2001\n[[reactors.drops]]\nitem_id = \
             {SPARE_TOP_ID}\nchance_per_million = 1000000\n[[reactors.drops]]\nitem_id = \
             {SPARE_BOTTOM_ID}\nchance_per_million = 500000\n"
        ));
        let map = reactor_map();
        let player = player(90.0, 100.0);

        for _ in 0..3 {
            let mut update = mailbox_attack(&store, &map, &player, 1).await;
            assert!(update.staged_drops.is_empty());
            commit_player_attack(&store, &mut update)
                .await
                .expect("commit nonlethal reactor hit");
        }

        let mut abandoned = mailbox_attack(&store, &map, &player, 1).await;
        let original_item_ids = abandoned
            .staged_drops
            .iter()
            .map(|grant| grant.item().item_id)
            .collect::<Vec<_>>();
        assert_eq!(original_item_ids, vec![SPARE_TOP_ID, SPARE_BOTTOM_ID]);
        assert!(abandoned.combat_events.iter().any(|event| {
            CombatEventKind::try_from(event.kind) == Ok(CombatEventKind::ReactorDestroyed)
        }));

        expire_pending_updates(&store)
            .await
            .expect("expire abandoned reactor destruction");
        assert!(
            !rollback_player_update(&store, &mut abandoned)
                .await
                .expect("expired reactor rollback is idempotent")
        );
        assert!(abandoned.combat_events.is_empty());
        assert!(abandoned.staged_drops.is_empty());

        let mut retried = mailbox_attack(&store, &map, &player, 1).await;
        let retried_item_ids = retried
            .staged_drops
            .iter()
            .map(|grant| grant.item().item_id)
            .collect::<Vec<_>>();
        assert_eq!(retried_item_ids, original_item_ids);
        assert!(
            rollback_player_update(&store, &mut retried)
                .await
                .expect("roll back retried reactor destruction")
        );
        assert!(retried.combat_events.is_empty());
        assert!(retried.staged_drops.is_empty());

        let restored = mailbox_map_snapshot(&store, &map)
            .await
            .expect("restored reactor snapshot");
        assert!(restored.reactors[0].active);
        assert_eq!(restored.reactors[0].state, 3);
    }

    #[tokio::test]
    async fn mailbox_commits_an_attack_delivery_and_releases_its_reservation() {
        let store = mailbox_store();
        let map = map();
        let player = player(90.0, 100.0);
        let mut first = mailbox_attack(&store, &map, &player, 10).await;

        commit_player_attack(&store, &mut first)
            .await
            .expect("commit attack delivery");
        let mut second = mailbox_attack(&store, &map, &player, 10).await;

        assert_eq!(second.mobs[0].current_hp, 80);
        assert!(
            rollback_player_update(&store, &mut second)
                .await
                .expect("rollback second attack")
        );
    }

    #[tokio::test]
    async fn abandoned_attack_deliveries_expire_and_release_their_reservation() {
        let store = mailbox_store();
        let map = map();
        let player = player(90.0, 100.0);
        let mut abandoned = mailbox_attack(&store, &map, &player, 10).await;
        assert_eq!(abandoned.mobs[0].current_hp, 90);

        expire_pending_updates(&store)
            .await
            .expect("expire abandoned delivery");
        assert!(
            !rollback_player_update(&store, &mut abandoned)
                .await
                .expect("expired rollback is idempotent")
        );
        let restored = mailbox_map_snapshot(&store, &map)
            .await
            .expect("restored snapshot");
        let mut retried = mailbox_attack(&store, &map, &player, 10).await;

        assert_eq!(restored.mobs[0].current_hp, 100);
        assert_eq!(retried.mobs[0].current_hp, 90);
        assert!(
            rollback_player_update(&store, &mut retried)
                .await
                .expect("rollback retried attack")
        );
    }

    #[tokio::test]
    async fn abandoned_observation_deliveries_restore_drained_combat_events() {
        let store = mailbox_store();
        let map = map();
        let player = player(100.0, 100.0);
        let abandoned = observe_player_with_effects(
            &store,
            &map,
            &player,
            0,
            crate::effects::ProjectedEffects::default(),
        )
        .await
        .expect("initial observation");
        let expected_damage = abandoned.player_damage();
        assert!(expected_damage > 0);

        expire_pending_updates(&store)
            .await
            .expect("expire abandoned delivery");
        let mut retried = observe_player_with_effects(
            &store,
            &map,
            &player,
            0,
            crate::effects::ProjectedEffects::default(),
        )
        .await
        .expect("retried observation");

        assert_eq!(retried.player_damage(), expected_damage);
        commit_player_attack(&store, &mut retried)
            .await
            .expect("acknowledge retried observation");
    }

    #[test]
    fn simulation_owns_all_map_worlds_without_map_locks() {
        let mut store = store();
        let first_map = map();
        let mut second_map = map();
        second_map.id = 2;
        let now = Instant::now();
        map_snapshot_at(&mut store, &first_map, now).expect("initialize first map");
        map_snapshot_at(&mut store, &second_map, now).expect("initialize second map");

        assert_eq!(store.maps.len(), 2);
        assert!(store.maps.contains_key(&first_map.id));
        assert!(store.maps.contains_key(&second_map.id));
    }

    #[test]
    fn observing_a_player_on_a_new_map_removes_the_old_presence() {
        let mut store = store();
        let first_map = map();
        let mut second_map = map();
        second_map.id = 2;
        let mut player = player(90.0, 100.0);
        let now = Instant::now();
        observe_player_at(&mut store, &first_map, &player, now).expect("observe first map");

        player.map_id = second_map.id;
        observe_player_at(
            &mut store,
            &second_map,
            &player,
            now + Duration::from_millis(1),
        )
        .expect("observe second map");

        let first_state = store.maps.get(&first_map.id).expect("first map state");
        assert!(!first_state.player_entities.contains_key(&player.id));
        assert_eq!(store.player_maps.get(&player.id), Some(&second_map.id));
    }

    #[test]
    fn stale_player_pruning_releases_the_shared_route() {
        let mut store = store();
        let map = map();
        let player = player(90.0, 100.0);
        let now = Instant::now();
        observe_player_at(&mut store, &map, &player, now).expect("observe player");
        assert_eq!(store.player_routes.map_for(&player.id), Some(map.id));

        map_snapshot_at(&mut store, &map, now + Duration::from_secs(6))
            .expect("advance beyond presence timeout");

        assert_eq!(store.player_routes.map_for(&player.id), None);
    }

    #[test]
    fn an_untargeted_player_attack_hits_an_in_range_mob_and_sets_aggro() {
        let mut store = store();
        let map = map();
        let player = player(90.0, 100.0);
        let now = Instant::now();

        map_snapshot_at(&mut store, &map, now).expect("initialize map");
        let update = use_player_attack_at(
            &mut store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "",
                source_skill_id: None,
                facing_left: false,
                minimum_damage: 10,
                maximum_damage: 10,
                fixed_damage: false,
                attack_type: SkillAttackType::Physical,
            },
            now + Duration::from_millis(1),
        )
        .expect("use attack");

        assert_eq!(update.mobs[0].current_hp, 90);
        assert!(update.combat_events.iter().any(|event| event.damage == 10));
    }

    #[test]
    fn a_weapon_attack_uses_its_authored_reach_instead_of_the_global_envelope() {
        let map = map();
        let attack = PlayerAttack {
            target_mob_id: "",
            source_skill_id: None,
            facing_left: false,
            minimum_damage: 10,
            maximum_damage: 10,
            fixed_damage: true,
            attack_type: SkillAttackType::Physical,
        };
        let reach = AttackReach {
            horizontal: 88.0,
            top: -62.0,
            bottom: -6.0,
        };

        let mut outside_store = store();
        let outside = use_player_attack_at_with_reach(
            &mut outside_store,
            &map,
            &player(11.0, 100.0),
            attack,
            reach,
            Instant::now(),
        )
        .expect("attack beyond weapon reach");
        assert_eq!(outside.mobs[0].current_hp, 100);

        let mut edge_store = store();
        let edge = use_player_attack_at_with_reach(
            &mut edge_store,
            &map,
            &player(12.0, 100.0),
            attack,
            reach,
            Instant::now(),
        )
        .expect("attack at weapon reach");
        assert_eq!(edge.mobs[0].current_hp, 90);
    }

    #[test]
    fn weapon_attacks_intersect_the_visible_target_bounds() {
        let player = Position {
            x: 0.0,
            y: 100.0,
            layer: 0,
        };
        let reach = AttackReach {
            horizontal: 88.0,
            top: -62.0,
            bottom: -6.0,
        };
        let target_bounds = VerticalBounds {
            top: -48.0,
            bottom: 0.0,
        };

        assert!(valid_attack_target(
            player,
            Position {
                x: 50.0,
                y: 38.0,
                layer: 0,
            },
            target_bounds,
            false,
            reach,
        ));
        assert!(!valid_attack_target(
            player,
            Position {
                x: 50.0,
                y: 37.0,
                layer: 0,
            },
            target_bounds,
            false,
            reach,
        ));
        assert!(valid_attack_target(
            player,
            Position {
                x: 50.0,
                y: 100.0,
                layer: 0,
            },
            target_bounds,
            false,
            reach,
        ));
        assert!(valid_attack_target(
            player,
            Position {
                x: 50.0,
                y: 142.0,
                layer: 0,
            },
            target_bounds,
            false,
            reach,
        ));
        assert!(!valid_attack_target(
            player,
            Position {
                x: 50.0,
                y: 143.0,
                layer: 0,
            },
            target_bounds,
            false,
            reach,
        ));

        let fallback = AttackReach {
            horizontal: 220.0,
            top: -90.0,
            bottom: 90.0,
        };
        for y in [10.0, 190.0] {
            assert!(valid_attack_target(
                player,
                Position {
                    x: 50.0,
                    y,
                    layer: 0,
                },
                POINT_VERTICAL_BOUNDS,
                false,
                fallback,
            ));
        }
        for y in [9.0, 191.0] {
            assert!(!valid_attack_target(
                player,
                Position {
                    x: 50.0,
                    y,
                    layer: 0,
                },
                POINT_VERTICAL_BOUNDS,
                false,
                fallback,
            ));
        }
    }

    #[test]
    fn airborne_attacks_use_the_authoritative_launch_layer() {
        let mut map = map();
        map.platforms[0].layer = 3;
        map.platforms.push(Platform {
            id: 2,
            x: 0.0,
            y: 180.0,
            end_x: 500.0,
            end_y: 180.0,
            layer: 7,
        });
        map.mob_spawn_points[0].position = Some(Vec2 { x: 100.0, y: 160.0 });
        map.mob_spawn_points[0].layer = 3;
        let player = player(90.0, 220.0);
        let attack = PlayerAttack {
            target_mob_id: "",
            source_skill_id: None,
            facing_left: false,
            minimum_damage: 10,
            maximum_damage: 10,
            fixed_damage: true,
            attack_type: SkillAttackType::Physical,
        };

        let mut launch_layer_store = store();
        let hit = use_player_attack_at_on_layer(
            &mut launch_layer_store,
            &map,
            &player,
            3,
            attack,
            Instant::now(),
        )
        .expect("airborne attack on launch layer");
        let mut nearby_layer_store = store();
        let miss = use_player_attack_at_on_layer(
            &mut nearby_layer_store,
            &map,
            &player,
            7,
            attack,
            Instant::now(),
        )
        .expect("airborne attack on nearby layer");

        assert_eq!(hit.mobs[0].current_hp, 90);
        assert_eq!(miss.mobs[0].current_hp, 100);
    }

    #[test]
    fn pushed_threshold_pauses_movement_for_the_hit_animation() {
        let now = Instant::now();
        let player = player(50.0, 100.0);
        let attack = PlayerAttack {
            target_mob_id: "",
            source_skill_id: None,
            facing_left: false,
            minimum_damage: 10,
            maximum_damage: 10,
            fixed_damage: true,
            attack_type: SkillAttackType::Physical,
        };
        let vulnerable_map = stagger_map(10);
        let mut resistant_map = stagger_map(11);
        resistant_map.id = 2;
        let mut vulnerable_store = store();
        let mut resistant_store = store();

        let hit =
            use_player_attack_at(&mut vulnerable_store, &vulnerable_map, &player, attack, now)
                .expect("staggering attack");
        let resistant_hit = use_player_attack_at(
            &mut resistant_store,
            &resistant_map,
            &PlayerState {
                map_id: resistant_map.id,
                ..player.clone()
            },
            attack,
            now,
        )
        .expect("non-staggering attack");

        assert!(hit.combat_events.iter().any(|event| event.staggered));
        assert!(
            resistant_hit
                .combat_events
                .iter()
                .all(|event| !event.staggered)
        );
        let held = map_snapshot_at(
            &mut vulnerable_store,
            &vulnerable_map,
            now + Duration::from_millis(599),
        )
        .expect("snapshot during stagger");
        let resumed = map_snapshot_at(
            &mut vulnerable_store,
            &vulnerable_map,
            now + Duration::from_millis(601),
        )
        .expect("snapshot after stagger");
        assert_eq!(held.mobs[0].position.expect("held position").x, 100.0);
        assert!(held.mob_projectiles.is_empty());
        assert!(resumed.mobs[0].position.expect("resumed position").x < 100.0);
    }

    #[test]
    fn basic_attacks_break_and_respawn_a_map_reactor() {
        let mut store = store();
        let map = reactor_map();
        let player = player(90.0, 100.0);
        let now = Instant::now();

        for (hit, expected_state) in (1..=4).zip(1..=4) {
            let update = use_player_attack_at(
                &mut store,
                &map,
                &player,
                PlayerAttack {
                    target_mob_id: "",
                    source_skill_id: None,
                    facing_left: false,
                    minimum_damage: 10,
                    maximum_damage: 10,
                    fixed_damage: true,
                    attack_type: SkillAttackType::Physical,
                },
                now + Duration::from_millis(hit),
            )
            .expect("use basic attack");

            assert_eq!(update.reactors[0].state, expected_state);
            assert!(update.combat_events.iter().any(|event| {
                CombatEventKind::try_from(event.kind) == Ok(CombatEventKind::PlayerHitReactor)
            }));
        }
        let destroyed = map_snapshot_at(&mut store, &map, now + Duration::from_secs(89))
            .expect("destroyed reactor snapshot");
        assert!(!destroyed.reactors[0].active);
        let respawned = map_snapshot_at(&mut store, &map, now + Duration::from_secs(91))
            .expect("respawned reactor snapshot");
        assert!(respawned.reactors[0].active);
        assert_eq!(respawned.reactors[0].state, 0);
    }

    #[test]
    fn a_rolled_back_reactor_hit_restores_its_state() {
        let mut store = store();
        let map = reactor_map();
        let player = player(90.0, 100.0);
        let mut update = use_player_attack_at_uncommitted(
            &mut store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "",
                source_skill_id: None,
                facing_left: false,
                minimum_damage: 10,
                maximum_damage: 10,
                fixed_damage: true,
                attack_type: SkillAttackType::Physical,
            },
            Instant::now(),
        )
        .expect("uncommitted reactor hit");

        assert_eq!(update.reactors[0].state, 1);
        assert!(rollback_player_attack(&mut store, &mut update).expect("rollback reactor hit"));
        let restored =
            map_snapshot_at(&mut store, &map, Instant::now()).expect("restored reactor snapshot");
        assert_eq!(restored.reactors[0].state, 0);
        assert!(restored.reactors[0].active);
    }

    #[test]
    fn reactor_destruction_stages_loot_transactionally() {
        let mut store = store_with_loot(&format!(
            "[[reactors]]\nreactor_id = 2001\n[[reactors.drops]]\nitem_id = \
             {SPARE_TOP_ID}\nchance_per_million = 1000000\n[[reactors.drops]]\nitem_id = \
             {SPARE_BOTTOM_ID}\nchance_per_million = 500000\n"
        ));
        let map = reactor_map();
        let player = player(90.0, 100.0);
        let now = Instant::now();

        for hit in 1..=3 {
            let update = use_player_attack_at(
                &mut store,
                &map,
                &player,
                PlayerAttack {
                    target_mob_id: "",
                    source_skill_id: None,
                    facing_left: false,
                    minimum_damage: 1,
                    maximum_damage: 1,
                    fixed_damage: true,
                    attack_type: SkillAttackType::Physical,
                },
                now + Duration::from_millis(hit),
            )
            .expect("nonlethal reactor hit");
            assert!(update.staged_drops.is_empty());
        }

        let mut destroyed = use_player_attack_at_uncommitted(
            &mut store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "",
                source_skill_id: None,
                facing_left: false,
                minimum_damage: 1,
                maximum_damage: 1,
                fixed_damage: true,
                attack_type: SkillAttackType::Physical,
            },
            now + Duration::from_millis(4),
        )
        .expect("destructive reactor hit");
        let original_item_ids = destroyed
            .staged_drops
            .iter()
            .map(|grant| grant.item().item_id)
            .collect::<Vec<_>>();
        assert_eq!(original_item_ids, vec![SPARE_TOP_ID, SPARE_BOTTOM_ID]);
        assert!(
            map_drops(&store.drops, map.id)
                .expect("uncommitted reactor drops")
                .is_empty()
        );

        assert!(rollback_player_attack(&mut store, &mut destroyed).expect("rollback destruction"));
        assert!(destroyed.staged_drops.is_empty());
        let restored = map_snapshot_at(&mut store, &map, now + Duration::from_millis(4))
            .expect("restored reactor");
        assert!(restored.reactors[0].active);
        assert_eq!(restored.reactors[0].state, 3);

        let retried = use_player_attack_at(
            &mut store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "",
                source_skill_id: None,
                facing_left: false,
                minimum_damage: 1,
                maximum_damage: 1,
                fixed_damage: true,
                attack_type: SkillAttackType::Physical,
            },
            now + Duration::from_millis(5),
        )
        .expect("retried reactor destruction");
        let retried_item_ids = retried
            .staged_drops
            .iter()
            .map(|grant| grant.item().item_id)
            .collect::<Vec<_>>();
        assert_eq!(retried_item_ids, original_item_ids);
        crate::items::commit_staged_drops(&store.drops, &retried.staged_drops)
            .expect("commit reactor drop");
        crate::items::commit_staged_drops(&store.drops, &retried.staged_drops)
            .expect("idempotent reactor drop commit");
        let drops = map_drops(&store.drops, map.id).expect("reactor drops");
        assert_eq!(drops.len(), 2);
        assert_eq!(drops[0].item_id, SPARE_TOP_ID);
        assert_eq!(drops[1].item_id, SPARE_BOTTOM_ID);
    }

    #[test]
    fn quest_loot_uses_the_attacking_player_for_mobs_and_reactors() {
        let quest = quest_definition(7);
        let loot = test_loot_with_quests(
            &format!(
                "[[mobs]]\nmob_id = 100\n[[mobs.drops]]\nitem_id = \
                 {SPARE_TOP_ID}\nchance_per_million = 1000000\nquest_id = \
                 7\n[[reactors]]\nreactor_id = 2001\n[[reactors.drops]]\nitem_id = \
                 {SPARE_TOP_ID}\nchance_per_million = 1000000\nquest_id = 7\n"
            ),
            &[quest],
        );
        let now = Instant::now();
        let inactive_player = player(90.0, 100.0);
        let mut inactive_store = store();
        inactive_store.loot = loot.clone();

        let inactive_kill = use_player_attack_at(
            &mut inactive_store,
            &map(),
            &inactive_player,
            PlayerAttack {
                target_mob_id: "1:1:0",
                source_skill_id: None,
                facing_left: false,
                minimum_damage: 100,
                maximum_damage: 100,
                fixed_damage: true,
                attack_type: SkillAttackType::Physical,
            },
            now,
        )
        .expect("inactive quest mob kill");
        assert!(inactive_kill.staged_drops.is_empty());

        let mut active_player = player(90.0, 100.0);
        active_player.quests.push(PlayerQuest {
            quest_id: 7,
            status: QuestStatus::Started.into(),
            ..PlayerQuest::default()
        });
        let mut active_mob_store = store();
        active_mob_store.loot = loot.clone();
        let active_kill = use_player_attack_at(
            &mut active_mob_store,
            &map(),
            &active_player,
            PlayerAttack {
                target_mob_id: "1:1:0",
                source_skill_id: None,
                facing_left: false,
                minimum_damage: 100,
                maximum_damage: 100,
                fixed_damage: true,
                attack_type: SkillAttackType::Physical,
            },
            now,
        )
        .expect("active quest mob kill");
        assert_eq!(active_kill.staged_drops.len(), 1);
        assert_eq!(active_kill.staged_drops[0].item().item_id, SPARE_TOP_ID);

        let mut active_reactor_store = store();
        active_reactor_store.loot = loot;
        for hit in 1..=4 {
            let update = use_player_attack_at(
                &mut active_reactor_store,
                &reactor_map(),
                &active_player,
                PlayerAttack {
                    target_mob_id: "",
                    source_skill_id: None,
                    facing_left: false,
                    minimum_damage: 1,
                    maximum_damage: 1,
                    fixed_damage: true,
                    attack_type: SkillAttackType::Physical,
                },
                now + Duration::from_millis(hit),
            )
            .expect("active quest reactor hit");
            if hit < 4 {
                assert!(update.staged_drops.is_empty());
            } else {
                assert_eq!(update.staged_drops.len(), 1);
                assert_eq!(update.staged_drops[0].item().item_id, SPARE_TOP_ID);
            }
        }
    }

    #[test]
    fn a_basic_attack_selects_a_closer_breakable_reactor_before_a_mob() {
        let mut store = store();
        let mut map = map();
        add_reactor(&mut map, 95.0);
        let player = player(90.0, 100.0);

        let update = use_player_attack_at(
            &mut store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "",
                source_skill_id: None,
                facing_left: false,
                minimum_damage: 10,
                maximum_damage: 10,
                fixed_damage: true,
                attack_type: SkillAttackType::Physical,
            },
            Instant::now(),
        )
        .expect("use basic attack");

        assert_eq!(update.mobs[0].current_hp, 100);
        assert_eq!(update.reactors[0].state, 1);
    }

    #[test]
    fn an_inert_reactor_does_not_intercept_a_basic_attack() {
        let mut store = store();
        let mut map = map();
        add_reactor(&mut map, 95.0);
        for state in &mut map.reactor_definitions[0].states {
            state.next_state = None;
        }
        let player = player(90.0, 100.0);

        let update = use_player_attack_at(
            &mut store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "",
                source_skill_id: None,
                facing_left: false,
                minimum_damage: 10,
                maximum_damage: 10,
                fixed_damage: true,
                attack_type: SkillAttackType::Physical,
            },
            Instant::now(),
        )
        .expect("use basic attack");

        assert_eq!(update.mobs[0].current_hp, 90);
        assert_eq!(update.reactors[0].state, 0);
    }

    #[test]
    fn untargeted_skills_do_not_hit_a_closer_reactor() {
        let mut store = store();
        let mut map = map();
        add_reactor(&mut map, 95.0);
        let player = player(90.0, 100.0);

        let update = use_player_attack_at(
            &mut store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "",
                source_skill_id: Some(1_000),
                facing_left: false,
                minimum_damage: 10,
                maximum_damage: 10,
                fixed_damage: true,
                attack_type: SkillAttackType::Physical,
            },
            Instant::now(),
        )
        .expect("use targeted skill");

        assert_eq!(update.mobs[0].current_hp, 90);
        assert_eq!(update.reactors[0].state, 0);
    }

    #[test]
    fn inaccurate_player_attacks_emit_misses_without_damaging_the_mob() {
        let mut store = store();
        let mut map = map();
        map.mob_definitions[0].avoidability = 10;
        let player = player(90.0, 100.0);

        let update = use_player_attack_at(
            &mut store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "",
                source_skill_id: None,
                facing_left: false,
                minimum_damage: 10,
                maximum_damage: 10,
                fixed_damage: false,
                attack_type: SkillAttackType::Physical,
            },
            Instant::now(),
        )
        .expect("use inaccurate attack");

        assert_eq!(update.mobs[0].current_hp, 100);
        assert!(update.combat_events.iter().any(|event| {
            CombatEventKind::try_from(event.kind) == Ok(CombatEventKind::PlayerMissedMob)
                && event.damage == 0
        }));
    }

    #[test]
    fn magical_attacks_use_authoritative_intelligence_and_luck() {
        let mut store = store();
        let mut map = map();
        map.mob_definitions[0].avoidability = 10;
        let mut player = player(90.0, 100.0);
        let stats = player.stats.as_mut().expect("stats");
        stats.dexterity = 0;
        stats.intelligence = 100;
        stats.luck = 10;

        let update = use_player_attack_at(
            &mut store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "",
                source_skill_id: Some(2_001_000),
                facing_left: false,
                minimum_damage: 10,
                maximum_damage: 10,
                fixed_damage: true,
                attack_type: SkillAttackType::Magical,
            },
            Instant::now(),
        )
        .expect("use magical attack");

        assert_eq!(update.mobs[0].current_hp, 90);
        assert!(update.combat_events.iter().any(|event| {
            CombatEventKind::try_from(event.kind) == Ok(CombatEventKind::PlayerHitMob)
                && event.damage == 10
        }));
    }

    #[test]
    fn attacks_cannot_damage_a_mob_behind_the_player() {
        let mut store = store();
        let map = map();
        let player = player(140.0, 100.0);

        let update = use_player_attack_at(
            &mut store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "1:1:0",
                source_skill_id: None,
                facing_left: false,
                minimum_damage: 10,
                maximum_damage: 10,
                fixed_damage: false,
                attack_type: SkillAttackType::Physical,
            },
            Instant::now(),
        )
        .expect("use skill");

        assert_eq!(update.mobs[0].current_hp, 100);
        assert!(update.combat_events.is_empty());
    }

    #[test]
    fn body_contact_damages_a_player_once_during_invulnerability() {
        let mut store = store();
        let map = map();
        let player = player(100.0, 100.0);
        let now = Instant::now();

        let first = observe_player_at(&mut store, &map, &player, now).expect("first observation");
        let second = observe_player_at(&mut store, &map, &player, now + Duration::from_millis(100))
            .expect("invulnerable observation");

        assert!(first.player_damage() > 0);
        assert_eq!(second.player_damage(), 0);
    }

    #[test]
    fn inaccurate_body_contact_emits_a_miss_without_damaging_the_player() {
        let mut store = store();
        let map = map();
        let mut player = player(100.0, 100.0);
        player.stats.as_mut().expect("stats").dexterity = 40;

        let update = observe_player_at(&mut store, &map, &player, Instant::now())
            .expect("observe evasive player");

        assert_eq!(update.player_damage(), 0);
        assert!(update.combat_events.iter().any(|event| {
            CombatEventKind::try_from(event.kind) == Ok(CombatEventKind::MobMissedPlayer)
                && event.damage == 0
        }));
        let state = store.maps.get(&map.id).expect("map simulation");
        let (invulnerable_until_ms, contact_attempt_after_ms) =
            state
                .world
                .run(|players: View<super::components::PlayerPresence>| {
                    players
                        .iter()
                        .find(|presence| presence.id == player.id)
                        .map(|presence| {
                            (
                                presence.invulnerable_until_ms,
                                presence.contact_attempt_after_ms,
                            )
                        })
                        .expect("player presence")
                });
        assert_eq!(invulnerable_until_ms, 0);
        assert_eq!(contact_attempt_after_ms, 1_000);
    }

    #[test]
    fn contact_misses_follow_the_attempt_interval_not_request_frequency() {
        let map = map();
        let mut player = player(100.0, 100.0);
        player.stats.as_mut().expect("stats").dexterity = 40;
        let now = Instant::now();

        let mut frequent = store();
        let frequent_events = [0, 100, 200, 999, 1_000]
            .into_iter()
            .map(|offset| {
                observe_player_at(
                    &mut frequent,
                    &map,
                    &player,
                    now + Duration::from_millis(offset),
                )
                .expect("frequent observation")
                .combat_events
                .len()
            })
            .sum::<usize>();
        let mut sparse = store();
        let sparse_events = [0, 1_000]
            .into_iter()
            .map(|offset| {
                observe_player_at(
                    &mut sparse,
                    &map,
                    &player,
                    now + Duration::from_millis(offset),
                )
                .expect("sparse observation")
                .combat_events
                .len()
            })
            .sum::<usize>();

        assert_eq!(frequent_events, 2);
        assert_eq!(sparse_events, 2);
    }

    #[test]
    fn projectile_miss_events_use_the_target_player_position() {
        let mut store = store();
        let mut map = map();
        map.mob_definitions[0].body_attack = false;
        map.mob_definitions[0].magic_attack = 20;
        let mut player = player(300.0, 100.0);
        player.stats.as_mut().expect("stats").dexterity = 40;
        let now = Instant::now();

        use_player_attack_at(
            &mut store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "1:1:0",
                source_skill_id: None,
                facing_left: true,
                minimum_damage: 1,
                maximum_damage: 1,
                fixed_damage: true,
                attack_type: SkillAttackType::Physical,
            },
            now,
        )
        .expect("provoke magic mob");
        let update = observe_player_at(
            &mut store,
            &map,
            &player,
            now + Duration::from_millis(1_600),
        )
        .expect("missed projectile observation");
        let missed = update
            .combat_events
            .iter()
            .find(|event| {
                CombatEventKind::try_from(event.kind) == Ok(CombatEventKind::MobMissedPlayer)
            })
            .expect("projectile miss event");

        assert_eq!(missed.position, player.position);
    }

    #[test]
    fn job_stat_profiles_fall_back_per_absent_property() {
        let formulas = FormulaCatalog::load(Path::new("../../config/skill-formulas.toml"))
            .expect("formula catalog");
        let variables = [("Dexterity", 20.0), ("Intelligence", 10.0), ("Luck", 10.0)];

        let stat = |job_id, property| {
            evaluate_player_stat(&formulas, job_id, property, &variables)
                .expect("player stat formula")
        };
        assert_eq!((stat(0, "accuracy"), stat(0, "avoidability")), (21.0, 10.0));
        assert_eq!(
            (stat(311, "accuracy"), stat(311, "avoidability")),
            (15.0, 10.0)
        );
        assert_eq!(
            (stat(511, "accuracy"), stat(511, "avoidability")),
            (21.0, 35.0)
        );
        assert_eq!(
            (stat(521, "accuracy"), stat(521, "avoidability")),
            (21.0, 7.5)
        );
    }

    #[test]
    fn combat_events_can_be_restored_after_a_persistence_failure() {
        let mut store = store();
        let map = map();
        let player = player(100.0, 100.0);
        let now = Instant::now();

        let first = observe_player_at(&mut store, &map, &player, now).expect("first observation");
        let expected_damage = first.player_damage();
        restore_player_effects(
            &mut store,
            map.id,
            &player.id,
            first.combat_events,
            first.mob_deaths,
            first.staged_drops,
        )
        .expect("restore events");
        let retried =
            observe_player_at(&mut store, &map, &player, now + Duration::from_millis(100))
                .expect("retry observation");

        assert!(expected_damage > 0);
        assert_eq!(retried.player_damage(), expected_damage);
    }

    #[test]
    fn failed_player_attack_transactions_restore_hp_before_retry() {
        let mut store = store();
        let map = map();
        let player = player(90.0, 100.0);
        let now = Instant::now();
        let attack = PlayerAttack {
            target_mob_id: "1:1:0",
            source_skill_id: Some(1_001_000),
            facing_left: false,
            minimum_damage: 10,
            maximum_damage: 10,
            fixed_damage: true,
            attack_type: SkillAttackType::Physical,
        };

        let mut failed = use_player_attack_at_uncommitted(&mut store, &map, &player, attack, now)
            .expect("uncommitted attack");
        let blocked = use_player_attack_at_uncommitted(
            &mut store,
            &map,
            &player,
            attack,
            now + Duration::from_millis(1),
        )
        .expect("attack while persistence is pending");
        assert_eq!(failed.mobs[0].current_hp, 90);
        assert_eq!(blocked.mobs[0].current_hp, 90);
        assert!(blocked.combat_events.is_empty());

        assert!(rollback_player_attack(&mut store, &mut failed).expect("roll back attack"));
        assert!(failed.combat_events.iter().all(|event| {
            CombatEventKind::try_from(event.kind) != Ok(CombatEventKind::PlayerHitMob)
        }));
        let retried = use_player_attack_at(
            &mut store,
            &map,
            &player,
            attack,
            now + Duration::from_millis(2),
        )
        .expect("retry rolled-back attack");

        assert_eq!(retried.mobs[0].current_hp, 90);
        assert_eq!(
            retried
                .combat_events
                .iter()
                .filter(|event| {
                    CombatEventKind::try_from(event.kind) == Ok(CombatEventKind::PlayerHitMob)
                })
                .count(),
            1
        );
    }

    #[test]
    fn failed_staggering_attacks_restore_the_mob_movement_state() {
        let mut store = store();
        let map = stagger_map(10);
        let player = player(50.0, 100.0);
        let now = Instant::now();
        let mut failed = use_player_attack_at_uncommitted(
            &mut store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "1:1:0",
                source_skill_id: None,
                facing_left: false,
                minimum_damage: 10,
                maximum_damage: 10,
                fixed_damage: true,
                attack_type: SkillAttackType::Physical,
            },
            now,
        )
        .expect("uncommitted staggering attack");
        assert!(failed.combat_events.iter().any(|event| event.staggered));

        assert!(rollback_player_attack(&mut store, &mut failed).expect("rollback stagger"));
        let state = store.maps.get(&map.id).expect("map state");
        let (mode, attack_until_ms, movement_resume_ms, movement_resume_mode) =
            state
                .world
                .run(|motions: View<MobMotion>, combats: View<MobCombat>| {
                    (&motions, &combats)
                        .iter()
                        .next()
                        .map(|(motion, combat)| {
                            (
                                motion.mode,
                                combat.attack_until_ms,
                                combat.movement_resume_ms,
                                combat.movement_resume_mode,
                            )
                        })
                        .expect("mob components")
                });
        assert_eq!(mode, MobMovementMode::Idle);
        assert_eq!(attack_until_ms, 0);
        assert_eq!(movement_resume_ms, 0);
        assert_eq!(movement_resume_mode, None);
    }

    #[test]
    fn failed_lethal_attacks_restore_the_mob_and_discard_their_staged_death() {
        let mut store = store_with_guaranteed_loot();
        let map = map();
        let player = player(90.0, 100.0);
        let now = Instant::now();
        let mut failed = use_player_attack_at_uncommitted(
            &mut store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "1:1:0",
                source_skill_id: Some(1_001_000),
                facing_left: false,
                minimum_damage: 100,
                maximum_damage: 100,
                fixed_damage: true,
                attack_type: SkillAttackType::Physical,
            },
            now,
        )
        .expect("uncommitted lethal attack");
        assert_eq!(failed.mobs[0].current_hp, 0);
        assert_eq!(failed.mob_deaths.len(), 1);
        assert_eq!(failed.staged_drops.len(), 1);

        assert!(rollback_player_attack(&mut store, &mut failed).expect("roll back lethal attack"));
        let restored = map_snapshot_at(&mut store, &map, now + Duration::from_millis(1))
            .expect("restored mob snapshot");

        assert_eq!(restored.mobs[0].current_hp, 100);
        assert!(failed.mob_deaths.is_empty());
        assert!(failed.staged_drops.is_empty());
        assert!(
            map_drops(&store.drops, map.id)
                .expect("committed drops")
                .is_empty()
        );
    }

    #[test]
    fn mob_deaths_can_be_restored_after_a_persistence_failure() {
        let mut store = store_with_guaranteed_loot();
        let map = map();
        let player = player(90.0, 100.0);
        let now = Instant::now();
        let killed = use_player_attack_at(
            &mut store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "1:1:0",
                source_skill_id: None,
                facing_left: false,
                minimum_damage: 100,
                maximum_damage: 100,
                fixed_damage: true,
                attack_type: SkillAttackType::Physical,
            },
            now,
        )
        .expect("lethal attack");
        let expected = killed.mob_deaths.clone();
        let expected_drops = killed.staged_drops.clone();

        restore_player_effects(
            &mut store,
            map.id,
            &player.id,
            killed.combat_events,
            killed.mob_deaths,
            killed.staged_drops,
        )
        .expect("restore effects");
        assert!(
            map_drops(&store.drops, map.id)
                .expect("drops after simulated save failure")
                .is_empty()
        );
        let retried = observe_player_at(&mut store, &map, &player, now + Duration::from_millis(1))
            .expect("retry observation");
        let consumed = observe_player_at(&mut store, &map, &player, now + Duration::from_millis(2))
            .expect("consume restored effects");

        assert_eq!(retried.mob_deaths, expected);
        assert_eq!(retried.staged_drops, expected_drops);
        assert!(consumed.mob_deaths.is_empty());
        assert!(consumed.staged_drops.is_empty());
    }

    #[test]
    fn aggroed_mobs_chase_a_nearby_player() {
        let mut store = store();
        let mut map = map();
        map.mob_definitions[0].animations.push(MobAnimation {
            name: "move".to_owned(),
            frames: vec![MobFrame::default()],
        });
        let player = player(250.0, 100.0);
        let now = Instant::now();

        use_player_attack_at(
            &mut store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "1:1:0",
                source_skill_id: None,
                facing_left: true,
                minimum_damage: 1,
                maximum_damage: 1,
                fixed_damage: true,
                attack_type: SkillAttackType::Physical,
            },
            now,
        )
        .expect("provoking attack");
        let update = observe_player_at(&mut store, &map, &player, now + Duration::from_millis(500))
            .expect("aggro observation");

        assert!(update.mobs[0].position.expect("mob position").x > 100.0);
    }

    #[test]
    fn nearby_magic_mobs_do_not_attack_unprovoked() {
        let mut store = store();
        let mut map = map();
        map.mob_definitions[0].body_attack = false;
        map.mob_definitions[0].magic_attack = 20;
        let player = player(300.0, 100.0);
        let now = Instant::now();

        observe_player_at(&mut store, &map, &player, now).expect("first observation");
        let update = observe_player_at(&mut store, &map, &player, now + Duration::from_secs(2))
            .expect("nearby observation");

        assert!(update.mob_projectiles.is_empty());
        assert_eq!(update.player_damage(), 0);
    }

    #[test]
    fn magic_mobs_launch_projectiles_that_damage_their_target() {
        let mut store = store();
        let mut map = map();
        map.mob_definitions[0].body_attack = false;
        map.mob_definitions[0].magic_attack = 20;
        let player = player(300.0, 100.0);
        let now = Instant::now();

        use_player_attack_at(
            &mut store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "1:1:0",
                source_skill_id: None,
                facing_left: true,
                minimum_damage: 1,
                maximum_damage: 1,
                fixed_damage: true,
                attack_type: SkillAttackType::Physical,
            },
            now,
        )
        .expect("provoking attack");
        let damage = (1..=10)
            .map(|step| {
                observe_player_at(
                    &mut store,
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
        let mut store = store();
        let mut map = map();
        map.mob_definitions[0].animations.push(MobAnimation {
            name: "move".to_owned(),
            frames: vec![MobFrame::default()],
        });
        let player = player(250.0, 100.0);
        let now = Instant::now();

        use_player_attack_at(
            &mut store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "1:1:0",
                source_skill_id: None,
                facing_left: true,
                minimum_damage: 1,
                maximum_damage: 1,
                fixed_damage: true,
                attack_type: SkillAttackType::Physical,
            },
            now,
        )
        .expect("provoking attack");
        let moved = observe_player_at(&mut store, &map, &player, now + Duration::from_millis(500))
            .expect("mob movement");
        assert!(moved.mobs[0].position.expect("moved position").x > 100.0);

        let killed = use_player_attack_at(
            &mut store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "1:1:0",
                source_skill_id: None,
                facing_left: true,
                minimum_damage: 100,
                maximum_damage: 100,
                fixed_damage: true,
                attack_type: SkillAttackType::Physical,
            },
            now + Duration::from_millis(501),
        )
        .expect("lethal skill");
        assert_eq!(killed.mobs[0].current_hp, 0);

        let respawned = observe_player_at(&mut store, &map, &player, now + Duration::from_secs(8))
            .expect("respawn observation");
        assert_eq!(respawned.mobs[0].current_hp, 100);
        assert_eq!(
            respawned.mobs[0].position,
            Some(Vec2 { x: 100.0, y: 100.0 })
        );
    }

    #[test]
    fn a_mob_death_creates_its_loot_exactly_once() {
        let mut store = store_with_guaranteed_loot();
        let map = map();
        let player = player(90.0, 100.0);
        let now = Instant::now();

        let killed = use_player_attack_at(
            &mut store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "1:1:0",
                source_skill_id: Some(1_001_004),
                facing_left: false,
                minimum_damage: 100,
                maximum_damage: 100,
                fixed_damage: true,
                attack_type: SkillAttackType::Physical,
            },
            now,
        )
        .expect("lethal attack");
        assert_eq!(killed.mobs[0].current_hp, 0);
        assert_eq!(
            killed.mob_deaths,
            vec![MobDeath {
                public_id: "1:1:0".to_owned(),
                definition_id: 100,
                source_skill_id: Some(1_001_004),
            }]
        );

        let repeated = use_player_attack_at(
            &mut store,
            &map,
            &player,
            PlayerAttack {
                target_mob_id: "1:1:0",
                source_skill_id: None,
                facing_left: false,
                minimum_damage: 100,
                maximum_damage: 100,
                fixed_damage: true,
                attack_type: SkillAttackType::Physical,
            },
            now + Duration::from_millis(1),
        )
        .expect("attack against dead mob");
        assert!(repeated.mob_deaths.is_empty());

        assert!(
            map_drops(&store.drops, map.id)
                .expect("uncommitted map drops")
                .is_empty()
        );
        crate::items::commit_staged_drops(&store.drops, &killed.staged_drops)
            .expect("commit staged drops");
        crate::items::commit_staged_drops(&store.drops, &killed.staged_drops)
            .expect("idempotent staged drop commit");
        let drops = map_drops(&store.drops, map.id).expect("map drops");
        assert_eq!(drops.len(), 1);
        assert_eq!(drops[0].item_id, SPARE_TOP_ID);
    }

    async fn mailbox_attack(
        store: &MobStore,
        map: &Map,
        player: &PlayerState,
        damage: u32,
    ) -> MobUpdate {
        use_player_attack_with_effects(
            store,
            map,
            player,
            0,
            PlayerAttack {
                target_mob_id: "",
                source_skill_id: None,
                facing_left: false,
                minimum_damage: damage,
                maximum_damage: damage,
                fixed_damage: true,
                attack_type: SkillAttackType::Physical,
            },
            crate::effects::ProjectedEffects::default(),
        )
        .await
        .expect("mailbox attack")
    }

    fn rollback_player_attack(
        simulation: &mut Simulation,
        update: &mut MobUpdate,
    ) -> Result<bool, MobStoreError> {
        let Some(transaction) = update.player_attack_transaction.clone() else {
            return Ok(false);
        };
        if update.combat_events.len() < transaction.generated_combat_events
            || update.mob_deaths.len() < transaction.generated_mob_deaths
            || update.staged_drops.len() < transaction.generated_staged_drops
        {
            return Err(MobStoreError::PlayerAttackTransaction {
                message: format!("transaction {} effects are incomplete", transaction.id),
            });
        }
        let rolled_back = apply_rollback_player_attack(simulation, &transaction)?;
        if rolled_back {
            update
                .combat_events
                .truncate(update.combat_events.len() - transaction.generated_combat_events);
            update
                .mob_deaths
                .truncate(update.mob_deaths.len() - transaction.generated_mob_deaths);
            update
                .staged_drops
                .truncate(update.staged_drops.len() - transaction.generated_staged_drops);
            update.player_attack_transaction = None;
        }
        Ok(rolled_back)
    }

    fn store() -> Simulation {
        let formulas = FormulaCatalog::load(Path::new("../../config/skill-formulas.toml"))
            .expect("formula catalog");
        Simulation::new(
            combat_config(),
            Arc::new(formulas),
            Arc::new(LootCatalog::default()),
            Arc::new(DropStore::new(Duration::from_secs(600))),
            Arc::new(PlayerRoutes::default()),
        )
    }

    fn mailbox_store() -> MobStore {
        let formulas = FormulaCatalog::load(Path::new("../../config/skill-formulas.toml"))
            .expect("formula catalog");
        MobStore::new_with_worker_count(
            combat_config(),
            Arc::new(formulas),
            Arc::new(LootCatalog::default()),
            Arc::new(DropStore::new(Duration::from_secs(600))),
            2,
        )
    }

    fn mailbox_store_with_loot(source: &str) -> MobStore {
        let formulas = FormulaCatalog::load(Path::new("../../config/skill-formulas.toml"))
            .expect("formula catalog");
        MobStore::new_with_worker_count(
            combat_config(),
            Arc::new(formulas),
            test_loot(source),
            Arc::new(DropStore::new(Duration::from_secs(600))),
            2,
        )
    }

    fn map_id_on_different_worker(
        store: &MobStore,
        map_id: u32,
    ) -> u32 {
        let source_worker = worker_index_for_map(store, map_id);
        (map_id + 1..)
            .find(|candidate| worker_index_for_map(store, *candidate) != source_worker)
            .expect("map ID routed to a different worker")
    }

    fn combat_config() -> CombatConfig {
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
        }
    }

    fn store_with_loot(source: &str) -> Simulation {
        let mut store = store();
        store.loot = test_loot(source);
        store
    }

    fn test_loot(source: &str) -> Arc<LootCatalog> {
        test_loot_with_quests(source, &[])
    }

    fn test_loot_with_quests(
        source: &str,
        quest_definitions: &[QuestDefinition],
    ) -> Arc<LootCatalog> {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("loot.toml");
        fs::write(&path, source).expect("write loot configuration");
        let loot = LootCatalog::load(
            &path,
            &[
                ItemDefinition {
                    item_id: SPARE_TOP_ID,
                    ..ItemDefinition::default()
                },
                ItemDefinition {
                    item_id: SPARE_BOTTOM_ID,
                    ..ItemDefinition::default()
                },
            ],
            quest_definitions,
        )
        .expect("loot catalog");
        Arc::new(loot)
    }

    fn quest_definition(id: u32) -> QuestDefinition {
        QuestDefinition {
            id,
            name: format!("Quest {id}"),
            start: Default::default(),
            completion: Default::default(),
            start_actions: Default::default(),
            completion_actions: Default::default(),
            dialogue: Default::default(),
            info: Default::default(),
        }
    }

    fn store_with_guaranteed_loot() -> Simulation {
        store_with_loot(&format!(
            "[[mobs]]\nmob_id = 100\n[[mobs.drops]]\nitem_id = {SPARE_TOP_ID}\nchance_per_million \
             = 1000000\n"
        ))
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

    fn stagger_map(threshold: u64) -> Map {
        let mut map = map();
        let definition = &mut map.mob_definitions[0];
        definition.stagger_threshold = threshold;
        definition.magic_attack = 20;
        definition.animations.extend([
            MobAnimation {
                name: "move".to_owned(),
                frames: vec![MobFrame::default()],
            },
            MobAnimation {
                name: "hit1".to_owned(),
                frames: vec![MobFrame {
                    delay_ms: 600,
                    ..MobFrame::default()
                }],
            },
        ]);
        map
    }

    fn reactor_map() -> Map {
        let mut map = map();
        map.mob_spawn_points.clear();
        map.mob_definitions.clear();
        add_reactor(&mut map, 100.0);
        map
    }

    fn add_reactor(
        map: &mut Map,
        x: f32,
    ) {
        map.reactor_spawn_points.push(ReactorSpawnPoint {
            spawn_id: 0,
            reactor_id: 2_001,
            position: Some(Vec2 { x, y: 100.0 }),
            layer: 0,
            respawn_seconds: 90,
            ..ReactorSpawnPoint::default()
        });
        map.reactor_definitions.push(ReactorDefinition {
            id: 2_001,
            states: (0..=4)
                .map(|state| ReactorStateDefinition {
                    state,
                    frames: vec![ReactorFrame::default()],
                    next_state: (state < 4).then_some(state + 1),
                    ..ReactorStateDefinition::default()
                })
                .collect(),
        });
    }
}
