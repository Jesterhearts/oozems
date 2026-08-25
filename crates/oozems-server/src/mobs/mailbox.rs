use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use oozems_proto::v1::CombatEvent;
use oozems_proto::v1::Map;
use oozems_proto::v1::PlayerState;
use tokio::sync::mpsc;
use tokio::sync::oneshot;

use super::MobDeath;
use super::MobStoreError;
use super::MobUpdate;
use super::PlayerAttack;
use super::PlayerAttackTransaction;
use super::PlayerRoutes;
use super::Simulation;
use crate::effects::ProjectedEffects;
use crate::gameplay::CombatConfig;
use crate::items::DropStore;
use crate::items::StagedDropGrant;
use crate::jobs::SkillAttackType;
use crate::loot::LootCatalog;
use crate::player_lock::PlayerLocks;
use crate::player_lock::acquire_player;
use crate::skill_formula::FormulaCatalog;

const MAILBOX_CAPACITY: usize = 1_024;
const COMMAND_BATCH_LIMIT: usize = 128;
// Retain drained effects until persistence acknowledges them. This lease
// bounds reservations abandoned by a cancelled request.
const UPDATE_DELIVERY_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) struct PendingUpdate {
    map_id: u32,
    player_id: String,
    combat_events: Vec<CombatEvent>,
    mob_deaths: Vec<MobDeath>,
    staged_drops: Vec<StagedDropGrant>,
    transaction: Option<PlayerAttackTransaction>,
    expires_at: Instant,
}

#[derive(Clone, Copy)]
pub(super) struct UpdateDelivery {
    map_id: u32,
    sequence: u64,
}

pub struct MobStore {
    workers: Box<[mpsc::Sender<Command>]>,
    player_routes: Arc<PlayerRoutes>,
    player_locks: PlayerLocks,
    #[cfg(test)]
    player_enqueue_signals:
        std::sync::Mutex<std::collections::HashMap<(String, u32), std::sync::mpsc::Sender<()>>>,
}

impl MobStore {
    pub fn new(
        rules: CombatConfig,
        formulas: Arc<FormulaCatalog>,
        loot: Arc<LootCatalog>,
        drops: Arc<DropStore>,
    ) -> Self {
        let worker_count = std::thread::available_parallelism().map_or(1, usize::from);
        Self::with_worker_count(rules, formulas, loot, drops, worker_count)
    }

    fn with_worker_count(
        rules: CombatConfig,
        formulas: Arc<FormulaCatalog>,
        loot: Arc<LootCatalog>,
        drops: Arc<DropStore>,
        worker_count: usize,
    ) -> Self {
        let worker_count = worker_count.max(1);
        let worker_capacity = MAILBOX_CAPACITY.div_ceil(worker_count).max(1);
        let player_routes = Arc::new(PlayerRoutes::default());
        let mut workers = Vec::with_capacity(worker_count);
        for worker_index in 0..worker_count {
            let (sender, receiver) = mpsc::channel(worker_capacity);
            let formulas = formulas.clone();
            let loot = loot.clone();
            let drops = drops.clone();
            let player_routes = player_routes.clone();
            std::thread::Builder::new()
                .name(format!("oozems-simulation-{worker_index}"))
                .spawn(move || {
                    run_simulation(
                        receiver,
                        Simulation::new(rules, formulas, loot, drops, player_routes),
                    );
                })
                .expect("failed to spawn a world simulation thread");
            workers.push(sender);
        }
        Self {
            workers: workers.into_boxed_slice(),
            player_routes,
            player_locks: PlayerLocks::default(),
            #[cfg(test)]
            player_enqueue_signals: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    #[cfg(test)]
    pub(super) fn new_with_worker_count(
        rules: CombatConfig,
        formulas: Arc<FormulaCatalog>,
        loot: Arc<LootCatalog>,
        drops: Arc<DropStore>,
        worker_count: usize,
    ) -> Self {
        Self::with_worker_count(rules, formulas, loot, drops, worker_count)
    }

    fn sender_for_map(
        &self,
        map_id: u32,
    ) -> &mpsc::Sender<Command> {
        &self.workers[worker_index(map_id, self.workers.len())]
    }
}

pub async fn map_snapshot(
    store: &MobStore,
    map: &Map,
) -> Result<MobUpdate, MobStoreError> {
    let (response, receiver) = oneshot::channel();
    send_command(
        store.sender_for_map(map.id),
        Command::MapSnapshot {
            map: map.clone(),
            response,
        },
        receiver,
    )
    .await
}

pub async fn observe_player_with_effects(
    store: &MobStore,
    map: &Map,
    player: &PlayerState,
    effects: ProjectedEffects,
) -> Result<MobUpdate, MobStoreError> {
    let (response, receiver) = oneshot::channel();
    send_player_command(
        store,
        map.id,
        &player.id,
        Command::ObservePlayer {
            map: map.clone(),
            player: player.clone(),
            effects,
            response,
        },
        receiver,
    )
    .await
}

pub async fn use_player_attack_with_effects(
    store: &MobStore,
    map: &Map,
    player: &PlayerState,
    attack: PlayerAttack<'_>,
    effects: ProjectedEffects,
) -> Result<MobUpdate, MobStoreError> {
    let (response, receiver) = oneshot::channel();
    send_player_command(
        store,
        map.id,
        &player.id,
        Command::UsePlayerAttack {
            map: map.clone(),
            player: player.clone(),
            attack: OwnedPlayerAttack::from(attack),
            effects,
            response,
        },
        receiver,
    )
    .await
}

pub async fn commit_player_attack(
    store: &MobStore,
    update: &mut MobUpdate,
) -> Result<(), MobStoreError> {
    let Some(delivery_id) = update.delivery_id else {
        return Ok(());
    };
    let (response, receiver) = oneshot::channel();
    send_control_command(
        store.sender_for_map(delivery_id.map_id).clone(),
        Command::CommitPlayerAttack {
            delivery_id,
            response,
        },
        receiver,
    )
    .await?;
    update.delivery_id = None;
    Ok(())
}

pub async fn rollback_player_update(
    store: &MobStore,
    update: &mut MobUpdate,
) -> Result<bool, MobStoreError> {
    let Some(delivery_id) = update.delivery_id else {
        return Ok(false);
    };
    let (response, receiver) = oneshot::channel();
    let rolled_back = send_control_command(
        store.sender_for_map(delivery_id.map_id).clone(),
        Command::RollbackPlayerUpdate {
            delivery_id,
            response,
        },
        receiver,
    )
    .await?;
    update.combat_events.clear();
    update.mob_deaths.clear();
    update.staged_drops.clear();
    update.delivery_id = None;
    Ok(rolled_back)
}

#[cfg(test)]
pub(super) async fn expire_pending_updates(store: &MobStore) -> Result<(), MobStoreError> {
    for sender in &store.workers {
        let (response, receiver) = oneshot::channel();
        send_control_command(
            sender.clone(),
            Command::ExpirePendingUpdates { response },
            receiver,
        )
        .await?;
    }
    Ok(())
}

#[cfg(test)]
pub(super) async fn block_worker_for_map(
    store: &MobStore,
    map_id: u32,
    started: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
) -> Result<(), MobStoreError> {
    let (response, receiver) = oneshot::channel();
    send_command(
        store.sender_for_map(map_id),
        Command::BlockWorker {
            started,
            release,
            response,
        },
        receiver,
    )
    .await
}

#[cfg(test)]
pub(super) async fn map_contains_player(
    store: &MobStore,
    map_id: u32,
    player_id: &str,
) -> Result<bool, MobStoreError> {
    let (response, receiver) = oneshot::channel();
    send_command(
        store.sender_for_map(map_id),
        Command::MapContainsPlayer {
            map_id,
            player_id: player_id.to_owned(),
            response,
        },
        receiver,
    )
    .await
}

#[cfg(test)]
pub(super) fn worker_index_for_map(
    store: &MobStore,
    map_id: u32,
) -> usize {
    worker_index(map_id, store.workers.len())
}

#[cfg(test)]
pub(super) fn signal_next_player_enqueue(
    store: &MobStore,
    player_id: &str,
    map_id: u32,
) -> std::sync::mpsc::Receiver<()> {
    let (sender, receiver) = std::sync::mpsc::channel();
    store
        .player_enqueue_signals
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert((player_id.to_owned(), map_id), sender);
    receiver
}

#[cfg(test)]
pub(super) fn delivery_coordinates(update: &MobUpdate) -> Option<(u32, u64)> {
    update
        .delivery_id
        .map(|delivery| (delivery.map_id, delivery.sequence))
}

async fn send_command<T>(
    sender: &mpsc::Sender<Command>,
    command: Command,
    response: oneshot::Receiver<Result<T, MobStoreError>>,
) -> Result<T, MobStoreError> {
    sender
        .send(command)
        .await
        .map_err(|_| MobStoreError::Unavailable)?;
    response.await.map_err(|_| MobStoreError::Unavailable)?
}

async fn send_player_command<T>(
    store: &MobStore,
    map_id: u32,
    player_id: &str,
    command: Command,
    response: oneshot::Receiver<Result<T, MobStoreError>>,
) -> Result<T, MobStoreError> {
    let player_guard = acquire_player(&store.player_locks, player_id).await?;
    let previous_map_id = store.player_routes.map_for(player_id);
    let previous_worker = previous_map_id
        .filter(|previous| *previous != map_id)
        .map(|previous| (previous, store.sender_for_map(previous).clone()));
    let sender = store.sender_for_map(map_id).clone();
    let player_routes = store.player_routes.clone();
    let player_id = player_id.to_owned();
    #[cfg(test)]
    let enqueue_signal = store
        .player_enqueue_signals
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&(player_id.clone(), map_id));
    // Finish relocation and enqueue as one cancellation-safe operation. The
    // caller retains the response receiver so cancellation still compensates
    // any update staged by the owner thread.
    let enqueue = tokio::spawn(async move {
        let _player_guard = player_guard;
        if let Some((previous_map_id, previous_sender)) = previous_worker {
            match remove_player(previous_sender, previous_map_id, &player_id).await {
                Ok(()) => {}
                Err(MobStoreError::Unavailable) => {
                    player_routes.remove_map(&player_id, previous_map_id);
                }
                Err(error) => return Err(error),
            }
        }
        player_routes.set_map(&player_id, map_id);
        if sender.send(command).await.is_err() {
            player_routes.remove_map(&player_id, map_id);
            return Err(MobStoreError::Unavailable);
        }
        #[cfg(test)]
        if let Some(enqueue_signal) = enqueue_signal {
            let _ = enqueue_signal.send(());
        }
        Ok(())
    })
    .await
    .map_err(|_| MobStoreError::Unavailable)?;
    enqueue?;
    response.await.map_err(|_| MobStoreError::Unavailable)?
}

async fn remove_player(
    sender: mpsc::Sender<Command>,
    map_id: u32,
    player_id: &str,
) -> Result<(), MobStoreError> {
    let (response, receiver) = oneshot::channel();
    send_command(
        &sender,
        Command::RemovePlayer {
            map_id,
            player_id: player_id.to_owned(),
            response,
        },
        receiver,
    )
    .await
}

async fn send_control_command<T: Send + 'static>(
    sender: mpsc::Sender<Command>,
    command: Command,
    response: oneshot::Receiver<Result<T, MobStoreError>>,
) -> Result<T, MobStoreError> {
    // Dropping the HTTP future must not cancel a commit or compensation that
    // belongs to an already-running cross-store transaction.
    tokio::spawn(async move {
        sender
            .send(command)
            .await
            .map_err(|_| MobStoreError::Unavailable)?;
        response.await.map_err(|_| MobStoreError::Unavailable)?
    })
    .await
    .map_err(|_| MobStoreError::Unavailable)?
}

fn worker_index(
    map_id: u32,
    worker_count: usize,
) -> usize {
    let mut value = u64::from(map_id).wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^= value >> 31;
    (value as usize) % worker_count
}

fn run_simulation(
    mut receiver: mpsc::Receiver<Command>,
    mut simulation: Simulation,
) {
    let mut batch = Vec::with_capacity(COMMAND_BATCH_LIMIT);
    while let Some(command) = receiver.blocking_recv() {
        batch.push(command);
        while batch.len() < COMMAND_BATCH_LIMIT {
            match receiver.try_recv() {
                Ok(command) => batch.push(command),
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => break,
            }
        }
        for command in batch.drain(..) {
            apply_command(&mut simulation, command);
        }
    }
}

fn apply_command(
    simulation: &mut Simulation,
    command: Command,
) {
    let now = Instant::now();
    reap_expired_updates(simulation, now);
    match command {
        Command::MapSnapshot { map, response } => {
            let _ = response.send(super::apply_map_snapshot(simulation, &map, now));
        }
        Command::ObservePlayer {
            map,
            player,
            effects,
            response,
        } => {
            let result = super::apply_observe_player(simulation, &map, &player, effects, now);
            send_update_response(simulation, map.id, &player.id, response, result, now);
        }
        Command::UsePlayerAttack {
            map,
            player,
            attack,
            effects,
            response,
        } => {
            let result = super::apply_use_player_attack(
                simulation,
                &map,
                &player,
                attack.borrowed(),
                effects,
                now,
            );
            send_update_response(simulation, map.id, &player.id, response, result, now);
        }
        Command::RemovePlayer {
            map_id,
            player_id,
            response,
        } => {
            super::apply_remove_player(simulation, map_id, &player_id);
            let _ = response.send(Ok(()));
        }
        #[cfg(test)]
        Command::BlockWorker {
            started,
            release,
            response,
        } => {
            let _ = started.send(());
            let _ = release.recv();
            let _ = response.send(Ok(()));
        }
        #[cfg(test)]
        Command::MapContainsPlayer {
            map_id,
            player_id,
            response,
        } => {
            let contains_player = simulation
                .maps
                .get(&map_id)
                .is_some_and(|state| state.player_entities.contains_key(&player_id));
            let _ = response.send(Ok(contains_player));
        }
        Command::CommitPlayerAttack {
            delivery_id,
            response,
        } => {
            let _ = response.send(commit_pending_update(simulation, delivery_id));
        }
        Command::RollbackPlayerUpdate {
            delivery_id,
            response,
        } => {
            let _ = response.send(rollback_pending_update(simulation, delivery_id));
        }
        #[cfg(test)]
        Command::ExpirePendingUpdates { response } => {
            reap_expired_updates(simulation, now + UPDATE_DELIVERY_TIMEOUT);
            let _ = response.send(Ok(()));
        }
    }
}

fn send_update_response(
    simulation: &mut Simulation,
    map_id: u32,
    player_id: &str,
    response: Response<MobUpdate>,
    mut result: Result<MobUpdate, MobStoreError>,
    now: Instant,
) {
    if let Ok(update) = result.as_mut() {
        stage_pending_update(simulation, map_id, player_id, update, now);
    }
    let Err(Ok(update)) = response.send(result) else {
        return;
    };
    if let Some(delivery_id) = update.delivery_id
        && let Err(error) = rollback_pending_update(simulation, delivery_id)
    {
        tracing::error!(%error, "failed to compensate a cancelled simulation update");
    }
}

fn stage_pending_update(
    simulation: &mut Simulation,
    map_id: u32,
    player_id: &str,
    update: &mut MobUpdate,
    now: Instant,
) {
    let transaction = update.player_attack_transaction.take();
    if transaction.is_none()
        && update.combat_events.is_empty()
        && update.mob_deaths.is_empty()
        && update.staged_drops.is_empty()
    {
        return;
    }
    let delivery_sequence = simulation.next_pending_update;
    simulation.next_pending_update = simulation
        .next_pending_update
        .checked_add(1)
        .expect("simulation update delivery IDs exhausted");
    let previous = simulation.pending_updates.insert(
        delivery_sequence,
        PendingUpdate {
            map_id,
            player_id: player_id.to_owned(),
            combat_events: update.combat_events.clone(),
            mob_deaths: update.mob_deaths.clone(),
            staged_drops: update.staged_drops.clone(),
            transaction,
            expires_at: now + UPDATE_DELIVERY_TIMEOUT,
        },
    );
    debug_assert!(previous.is_none());
    update.delivery_id = Some(UpdateDelivery {
        map_id,
        sequence: delivery_sequence,
    });
}

fn commit_pending_update(
    simulation: &mut Simulation,
    delivery: UpdateDelivery,
) -> Result<(), MobStoreError> {
    let Some(mut pending) = take_pending_update(simulation, delivery)? else {
        return Err(missing_delivery(delivery));
    };
    let result = pending.transaction.as_ref().map_or(Ok(()), |transaction| {
        super::apply_commit_player_attack(simulation, transaction)
    });
    if let Err(error) = result {
        pending.expires_at = Instant::now() + UPDATE_DELIVERY_TIMEOUT;
        simulation
            .pending_updates
            .insert(delivery.sequence, pending);
        return Err(error);
    }
    Ok(())
}

fn rollback_pending_update(
    simulation: &mut Simulation,
    delivery: UpdateDelivery,
) -> Result<bool, MobStoreError> {
    let Some(mut pending) = take_pending_update(simulation, delivery)? else {
        return Ok(false);
    };
    let result = rollback_and_restore_pending_update(simulation, &pending);
    if let Err(error) = result {
        pending.expires_at = Instant::now() + UPDATE_DELIVERY_TIMEOUT;
        simulation
            .pending_updates
            .insert(delivery.sequence, pending);
        return Err(error);
    }
    result
}

fn rollback_and_restore_pending_update(
    simulation: &mut Simulation,
    pending: &PendingUpdate,
) -> Result<bool, MobStoreError> {
    let mut combat_events = pending.combat_events.clone();
    let mut mob_deaths = pending.mob_deaths.clone();
    let mut staged_drops = pending.staged_drops.clone();
    let rolled_back = if let Some(transaction) = pending.transaction.as_ref() {
        validate_effect_lengths(
            combat_events.len(),
            mob_deaths.len(),
            staged_drops.len(),
            transaction,
        )?;
        let rolled_back = super::apply_rollback_player_attack(simulation, transaction)?;
        combat_events.truncate(combat_events.len() - transaction.generated_combat_events);
        mob_deaths.truncate(mob_deaths.len() - transaction.generated_mob_deaths);
        staged_drops.truncate(staged_drops.len() - transaction.generated_staged_drops);
        rolled_back
    } else {
        false
    };
    super::apply_restore_player_effects(
        simulation,
        pending.map_id,
        &pending.player_id,
        combat_events,
        mob_deaths,
        staged_drops,
    )?;
    Ok(rolled_back)
}

fn reap_expired_updates(
    simulation: &mut Simulation,
    now: Instant,
) {
    let expired = simulation
        .pending_updates
        .iter()
        .filter(|(_, pending)| pending.expires_at <= now)
        .map(|(sequence, pending)| UpdateDelivery {
            map_id: pending.map_id,
            sequence: *sequence,
        })
        .collect::<Vec<_>>();
    for delivery in expired {
        if let Err(error) = rollback_pending_update(simulation, delivery) {
            tracing::error!(
                %error,
                map_id = delivery.map_id,
                delivery_sequence = delivery.sequence,
                "failed to expire a simulation update delivery"
            );
        }
    }
}

fn take_pending_update(
    simulation: &mut Simulation,
    delivery: UpdateDelivery,
) -> Result<Option<PendingUpdate>, MobStoreError> {
    let Some(pending) = simulation.pending_updates.remove(&delivery.sequence) else {
        return Ok(None);
    };
    if pending.map_id != delivery.map_id {
        let actual_map_id = pending.map_id;
        simulation
            .pending_updates
            .insert(delivery.sequence, pending);
        return Err(MobStoreError::UpdateDelivery {
            message: format!(
                "simulation update delivery {} belongs to map {actual_map_id}, not map {}",
                delivery.sequence, delivery.map_id
            ),
        });
    }
    Ok(Some(pending))
}

fn missing_delivery(delivery: UpdateDelivery) -> MobStoreError {
    MobStoreError::UpdateDelivery {
        message: format!(
            "simulation update delivery {} for map {} no longer exists",
            delivery.sequence, delivery.map_id
        ),
    }
}

fn validate_effect_lengths(
    combat_events: usize,
    mob_deaths: usize,
    staged_drops: usize,
    transaction: &PlayerAttackTransaction,
) -> Result<(), MobStoreError> {
    if combat_events < transaction.generated_combat_events
        || mob_deaths < transaction.generated_mob_deaths
        || staged_drops < transaction.generated_staged_drops
    {
        return Err(MobStoreError::PlayerAttackTransaction {
            message: format!("transaction {} effects are incomplete", transaction.id),
        });
    }
    Ok(())
}

enum Command {
    MapSnapshot {
        map: Map,
        response: Response<MobUpdate>,
    },
    ObservePlayer {
        map: Map,
        player: PlayerState,
        effects: ProjectedEffects,
        response: Response<MobUpdate>,
    },
    UsePlayerAttack {
        map: Map,
        player: PlayerState,
        attack: OwnedPlayerAttack,
        effects: ProjectedEffects,
        response: Response<MobUpdate>,
    },
    RemovePlayer {
        map_id: u32,
        player_id: String,
        response: Response<()>,
    },
    #[cfg(test)]
    BlockWorker {
        started: std::sync::mpsc::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
        response: Response<()>,
    },
    #[cfg(test)]
    MapContainsPlayer {
        map_id: u32,
        player_id: String,
        response: Response<bool>,
    },
    CommitPlayerAttack {
        delivery_id: UpdateDelivery,
        response: Response<()>,
    },
    RollbackPlayerUpdate {
        delivery_id: UpdateDelivery,
        response: Response<bool>,
    },
    #[cfg(test)]
    ExpirePendingUpdates { response: Response<()> },
}

type Response<T> = oneshot::Sender<Result<T, MobStoreError>>;

struct OwnedPlayerAttack {
    target_mob_id: String,
    source_skill_id: Option<u32>,
    facing_left: bool,
    minimum_damage: u32,
    maximum_damage: u32,
    fixed_damage: bool,
    attack_type: SkillAttackType,
}

impl OwnedPlayerAttack {
    fn borrowed(&self) -> PlayerAttack<'_> {
        PlayerAttack {
            target_mob_id: &self.target_mob_id,
            source_skill_id: self.source_skill_id,
            facing_left: self.facing_left,
            minimum_damage: self.minimum_damage,
            maximum_damage: self.maximum_damage,
            fixed_damage: self.fixed_damage,
            attack_type: self.attack_type,
        }
    }
}

impl From<PlayerAttack<'_>> for OwnedPlayerAttack {
    fn from(attack: PlayerAttack<'_>) -> Self {
        Self {
            target_mob_id: attack.target_mob_id.to_owned(),
            source_skill_id: attack.source_skill_id,
            facing_left: attack.facing_left,
            minimum_damage: attack.minimum_damage,
            maximum_damage: attack.maximum_damage,
            fixed_damage: attack.fixed_damage,
            attack_type: attack.attack_type,
        }
    }
}
