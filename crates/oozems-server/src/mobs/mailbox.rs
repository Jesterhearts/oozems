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
use super::Simulation;
use crate::effects::ProjectedEffects;
use crate::gameplay::CombatConfig;
use crate::items::DropStore;
use crate::items::StagedDropGrant;
use crate::jobs::SkillAttackType;
use crate::loot::LootCatalog;
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

pub struct MobStore {
    sender: mpsc::Sender<Command>,
}

impl MobStore {
    pub fn new(
        rules: CombatConfig,
        formulas: Arc<FormulaCatalog>,
        loot: Arc<LootCatalog>,
        drops: Arc<DropStore>,
    ) -> Self {
        let (sender, receiver) = mpsc::channel(MAILBOX_CAPACITY);
        std::thread::Builder::new()
            .name("oozems-simulation".to_owned())
            .spawn(move || {
                run_simulation(receiver, Simulation::new(rules, formulas, loot, drops));
            })
            .expect("failed to spawn the world simulation thread");
        Self { sender }
    }
}

pub async fn map_snapshot(
    store: &MobStore,
    map: &Map,
) -> Result<MobUpdate, MobStoreError> {
    let (response, receiver) = oneshot::channel();
    send_command(
        store,
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
    send_command(
        store,
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
    send_command(
        store,
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
        store,
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
        store,
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
    let (response, receiver) = oneshot::channel();
    send_control_command(store, Command::ExpirePendingUpdates { response }, receiver).await
}

async fn send_command<T>(
    store: &MobStore,
    command: Command,
    response: oneshot::Receiver<Result<T, MobStoreError>>,
) -> Result<T, MobStoreError> {
    store
        .sender
        .send(command)
        .await
        .map_err(|_| MobStoreError::Unavailable)?;
    response.await.map_err(|_| MobStoreError::Unavailable)?
}

async fn send_control_command<T: Send + 'static>(
    store: &MobStore,
    command: Command,
    response: oneshot::Receiver<Result<T, MobStoreError>>,
) -> Result<T, MobStoreError> {
    let sender = store.sender.clone();
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
    let delivery_id = simulation.next_pending_update;
    simulation.next_pending_update = simulation
        .next_pending_update
        .checked_add(1)
        .expect("simulation update delivery IDs exhausted");
    let previous = simulation.pending_updates.insert(
        delivery_id,
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
    update.delivery_id = Some(delivery_id);
}

fn commit_pending_update(
    simulation: &mut Simulation,
    delivery_id: u64,
) -> Result<(), MobStoreError> {
    let Some(mut pending) = simulation.pending_updates.remove(&delivery_id) else {
        return Err(missing_delivery(delivery_id));
    };
    let result = pending.transaction.as_ref().map_or(Ok(()), |transaction| {
        super::apply_commit_player_attack(simulation, transaction)
    });
    if let Err(error) = result {
        pending.expires_at = Instant::now() + UPDATE_DELIVERY_TIMEOUT;
        simulation.pending_updates.insert(delivery_id, pending);
        return Err(error);
    }
    Ok(())
}

fn rollback_pending_update(
    simulation: &mut Simulation,
    delivery_id: u64,
) -> Result<bool, MobStoreError> {
    let Some(mut pending) = simulation.pending_updates.remove(&delivery_id) else {
        return Ok(false);
    };
    let result = rollback_and_restore_pending_update(simulation, &pending);
    if let Err(error) = result {
        pending.expires_at = Instant::now() + UPDATE_DELIVERY_TIMEOUT;
        simulation.pending_updates.insert(delivery_id, pending);
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
        .map(|(delivery_id, _)| *delivery_id)
        .collect::<Vec<_>>();
    for delivery_id in expired {
        if let Err(error) = rollback_pending_update(simulation, delivery_id) {
            tracing::error!(%error, delivery_id, "failed to expire a simulation update delivery");
        }
    }
}

fn missing_delivery(delivery_id: u64) -> MobStoreError {
    MobStoreError::UpdateDelivery {
        message: format!("simulation update delivery {delivery_id} no longer exists"),
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
    CommitPlayerAttack {
        delivery_id: u64,
        response: Response<()>,
    },
    RollbackPlayerUpdate {
        delivery_id: u64,
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
