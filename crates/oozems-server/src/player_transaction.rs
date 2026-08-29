use std::sync::Arc;

use oozems_proto::v1::DroppedItem;
use oozems_proto::v1::PlayerState;
use thiserror::Error;

use crate::attacks::BasicAttackCooldowns;
use crate::attacks::BasicAttackReservation;
use crate::database::Database;
use crate::database::DatabaseError;
use crate::effects::ActiveEffects;
use crate::effects::PlayerEffects;
use crate::items::DropStore;
use crate::items::PickedUpItem;
use crate::items::StagedDropGrant;
use crate::mobs::MobStore;
use crate::mobs::MobUpdate;
use crate::movement::CommittedRelocation;
use crate::movement::MovementTracker;
use crate::movement::RelocationPlan;
use crate::player_lock::PlayerGuard;
use crate::recovery::RecoveryActivityRollback;
use crate::recovery::RecoveryTimers;
use crate::recovery::RecoveryToken;
use crate::skills::SkillCooldownReservation;
use crate::skills::SkillCooldowns;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayerPersistence {
    None,
    Full,
}

pub struct PlayerTransaction {
    original_player: PlayerState,
    staged_player: PlayerState,
    persistence: PlayerPersistence,
    durable_saved: Option<PlayerState>,
    effects: Option<EffectChange>,
    drops: Option<DropChange>,
    pickup: Option<PickupRollback>,
    mob: Option<MobChange>,
    skill_cooldown: Option<SkillCooldownChange>,
    basic_attack: Option<BasicAttackChange>,
    recovery: Option<RecoveryChange>,
    relocation: Option<RelocationChange>,
    activity: Option<ActivityChange>,
}

struct EffectChange {
    store: Arc<ActiveEffects>,
    original: PlayerEffects,
    staged: PlayerEffects,
    committed: bool,
}

struct DropChange {
    store: Arc<DropStore>,
    staged: Vec<StagedDropGrant>,
    committed: bool,
}

struct PickupRollback {
    store: Arc<DropStore>,
    map_id: u32,
    item: DroppedItem,
    owner_player_id: Option<String>,
}

struct MobChange {
    store: Arc<MobStore>,
    update: MobUpdate,
    committed: bool,
}

struct SkillCooldownChange {
    store: Arc<SkillCooldowns>,
    reservation: SkillCooldownReservation,
}

struct BasicAttackChange {
    store: Arc<BasicAttackCooldowns>,
    reservation: BasicAttackReservation,
}

struct RecoveryChange {
    store: Arc<RecoveryTimers>,
    reservation: RecoveryToken,
}

struct RelocationChange {
    store: Arc<MovementTracker>,
    plan: RelocationPlan,
    committed: Option<CommittedRelocation>,
}

struct ActivityChange {
    store: Arc<RecoveryTimers>,
    player_id: String,
    now_ms: u64,
    rollback: Option<RecoveryActivityRollback>,
}

pub struct CommittedPlayerTransaction {
    pub player: PlayerState,
    pub effects: Option<PlayerEffects>,
    pub committed_drops: Vec<DroppedItem>,
    pub mob_update: Option<MobUpdate>,
}

#[derive(Debug, Error)]
pub enum PlayerTransactionError {
    #[error("player transaction plan is invalid: {0}")]
    Plan(#[source] PlayerTransactionPlanError),
    #[error("player transaction worker failed")]
    Worker(#[source] tokio::task::JoinError),
    #[error("player transaction failed: {failure}")]
    Failed { failure: Box<TransactionFailure> },
    #[error(
        "player transaction failed and compensation was incomplete; reconciliation is required: \
         {failure}; rollback failures: {rollback_failures:?}"
    )]
    Reconciliation {
        failure: Box<TransactionFailure>,
        rollback_failures: Vec<RollbackFailure>,
    },
}

#[derive(Debug, Error)]
pub enum PlayerTransactionPlanError {
    #[error("staged drops belong to a different drop store")]
    DropStoreMismatch,
    #[error("a relocation plan requires full player persistence")]
    RelocationRequiresFullPersistence,
    #[error("relocation belongs to player {planned:?}, but the staged player is {staged:?}")]
    RelocationPlayerMismatch { planned: String, staged: String },
    #[error("relocation targets map {planned}, but the staged player targets map {staged}")]
    RelocationMapMismatch { planned: u32, staged: u32 },
    #[error("the staged player position does not match the relocation target")]
    RelocationPositionMismatch,
}

#[derive(Debug, Error)]
pub enum TransactionFailure {
    #[error("player transaction plan is invalid: {0}")]
    Plan(#[source] PlayerTransactionPlanError),
    #[error("player persistence failed: {0}")]
    Database(#[source] DatabaseError),
    #[error("active-effect commit failed: {0}")]
    Effects(#[source] crate::effects::EffectStoreError),
    #[error("drop commit failed: {0}")]
    Drops(#[source] crate::items::DropStoreError),
    #[error("movement commit failed: {0}")]
    Movement(#[source] crate::movement::MovementError),
    #[error("recovery activity commit failed: {0}")]
    Recovery(#[source] crate::recovery::RecoveryError),
    #[error("mob attack commit failed: {0}")]
    Mobs(#[source] crate::mobs::MobStoreError),
    #[error("request failed before commit: {0}")]
    Request(String),
}

#[derive(Debug, Error)]
pub enum RollbackFailure {
    #[error("mob rollback failed: {0}")]
    Mobs(#[source] crate::mobs::MobStoreError),
    #[error("recovery activity rollback failed: {0}")]
    Activity(#[source] crate::recovery::RecoveryError),
    #[error("relocation rollback failed: {0}")]
    Relocation(#[source] crate::movement::MovementError),
    #[error("drop rollback failed: {0}")]
    Drops(#[source] crate::items::DropStoreError),
    #[error("pickup rollback failed: {0}")]
    Pickup(#[source] crate::items::DropStoreError),
    #[error("active-effect rollback failed: {0}")]
    Effects(#[source] crate::effects::EffectStoreError),
    #[error("skill cooldown rollback failed: {0}")]
    SkillCooldown(#[source] crate::skills::SkillRuleError),
    #[error("basic attack cooldown rollback failed: {0}")]
    BasicAttack(#[source] crate::attacks::AttackRuleError),
    #[error("recovery reservation rollback failed: {0}")]
    Recovery(#[source] crate::recovery::RecoveryError),
    #[error("durable player rollback failed: {0}")]
    Database(#[source] DatabaseError),
}

pub fn new_player_transaction(
    original_player: PlayerState,
    staged_player: PlayerState,
    persistence: PlayerPersistence,
) -> PlayerTransaction {
    debug_assert_eq!(original_player.id, staged_player.id);
    PlayerTransaction {
        original_player,
        staged_player,
        persistence,
        durable_saved: None,
        effects: None,
        drops: None,
        pickup: None,
        mob: None,
        skill_cooldown: None,
        basic_attack: None,
        recovery: None,
        relocation: None,
        activity: None,
    }
}

pub fn staged_player(transaction: &PlayerTransaction) -> &PlayerState {
    &transaction.staged_player
}

pub fn replace_staged_player(
    transaction: &mut PlayerTransaction,
    player: PlayerState,
    persistence: PlayerPersistence,
) {
    debug_assert_eq!(transaction.original_player.id, player.id);
    transaction.staged_player = player;
    transaction.persistence = persistence;
}

pub fn stage_effects(
    transaction: &mut PlayerTransaction,
    store: Arc<ActiveEffects>,
    original: PlayerEffects,
    staged: PlayerEffects,
) {
    transaction.effects = Some(EffectChange {
        store,
        original,
        staged,
        committed: false,
    });
}

pub fn stage_drops(
    transaction: &mut PlayerTransaction,
    store: Arc<DropStore>,
    staged: impl IntoIterator<Item = StagedDropGrant>,
) -> Result<(), PlayerTransactionError> {
    let change = transaction.drops.get_or_insert_with(|| DropChange {
        store: store.clone(),
        staged: Vec::new(),
        committed: false,
    });
    if !Arc::ptr_eq(&change.store, &store) {
        return Err(PlayerTransactionError::Plan(
            PlayerTransactionPlanError::DropStoreMismatch,
        ));
    }
    change.staged.extend(staged);
    Ok(())
}

pub fn stage_pickup(
    transaction: &mut PlayerTransaction,
    store: Arc<DropStore>,
    map_id: u32,
    picked: &PickedUpItem,
) {
    transaction.pickup = Some(PickupRollback {
        store,
        map_id,
        item: picked.drop.clone(),
        owner_player_id: picked.owner_player_id.clone(),
    });
}

pub fn stage_mob_update(
    transaction: &mut PlayerTransaction,
    store: Arc<MobStore>,
    drop_store: Arc<DropStore>,
    update: MobUpdate,
) -> Result<(), PlayerTransactionError> {
    stage_drops(transaction, drop_store, update.staged_drops.clone())?;
    transaction.mob = Some(MobChange {
        store,
        update,
        committed: false,
    });
    Ok(())
}

pub fn stage_skill_cooldown(
    transaction: &mut PlayerTransaction,
    store: Arc<SkillCooldowns>,
    reservation: Option<SkillCooldownReservation>,
) {
    transaction.skill_cooldown =
        reservation.map(|reservation| SkillCooldownChange { store, reservation });
}

pub fn stage_basic_attack(
    transaction: &mut PlayerTransaction,
    store: Arc<BasicAttackCooldowns>,
    reservation: BasicAttackReservation,
) {
    transaction.basic_attack = Some(BasicAttackChange { store, reservation });
}

pub fn stage_recovery(
    transaction: &mut PlayerTransaction,
    store: Arc<RecoveryTimers>,
    reservation: RecoveryToken,
) {
    transaction.recovery = Some(RecoveryChange { store, reservation });
}

pub fn stage_relocation(
    transaction: &mut PlayerTransaction,
    store: Arc<MovementTracker>,
    plan: RelocationPlan,
) {
    transaction.relocation = Some(RelocationChange {
        store,
        plan,
        committed: None,
    });
}

pub fn stage_activity(
    transaction: &mut PlayerTransaction,
    store: Arc<RecoveryTimers>,
    player_id: String,
    now_ms: u64,
) {
    transaction.activity = Some(ActivityChange {
        store,
        player_id,
        now_ms,
        rollback: None,
    });
}

pub async fn commit_player_transaction(
    database: &Database,
    guard: &PlayerGuard,
    transaction: PlayerTransaction,
) -> Result<CommittedPlayerTransaction, PlayerTransactionError> {
    let database = database.clone();
    let guard = guard.clone();
    // Retain the player lock and finish compensation if the request task is
    // cancelled after one of the stores has committed.
    tokio::spawn(
        async move { commit_player_transaction_inner(&database, &guard, transaction).await },
    )
    .await
    .map_err(PlayerTransactionError::Worker)?
}

async fn commit_player_transaction_inner(
    database: &Database,
    _guard: &PlayerGuard,
    mut transaction: PlayerTransaction,
) -> Result<CommittedPlayerTransaction, PlayerTransactionError> {
    if let Err(error) = validate_transaction_plan(&transaction) {
        let rollback_failures = rollback_player_transaction(database, &mut transaction).await;
        return if rollback_failures.is_empty() {
            Err(PlayerTransactionError::Plan(error))
        } else {
            Err(PlayerTransactionError::Reconciliation {
                failure: Box::new(TransactionFailure::Plan(error)),
                rollback_failures,
            })
        };
    }
    if let Err(failure) = commit_staged_changes(database, &mut transaction).await {
        let rollback_failures = rollback_player_transaction(database, &mut transaction).await;
        return if rollback_failures.is_empty() {
            Err(PlayerTransactionError::Failed {
                failure: Box::new(failure),
            })
        } else {
            Err(PlayerTransactionError::Reconciliation {
                failure: Box::new(failure),
                rollback_failures,
            })
        };
    }

    let player = transaction.staged_player;
    let effects = transaction.effects.map(|change| change.staged);
    let committed_drops = transaction
        .drops
        .map(|change| {
            change
                .staged
                .into_iter()
                .map(|grant| grant.item().clone())
                .collect()
        })
        .unwrap_or_default();
    let mob_update = transaction.mob.map(|change| change.update);
    Ok(CommittedPlayerTransaction {
        player,
        effects,
        committed_drops,
        mob_update,
    })
}

pub async fn abort_player_transaction(
    database: &Database,
    guard: &PlayerGuard,
    transaction: PlayerTransaction,
    cause: impl Into<String>,
) -> Result<(), PlayerTransactionError> {
    let database = database.clone();
    let guard = guard.clone();
    let cause = cause.into();
    // Aborts coordinate several stores and need the same cancellation
    // shielding as commits.
    tokio::spawn(async move {
        abort_player_transaction_inner(&database, &guard, transaction, cause).await
    })
    .await
    .map_err(PlayerTransactionError::Worker)?
}

async fn abort_player_transaction_inner(
    database: &Database,
    _guard: &PlayerGuard,
    mut transaction: PlayerTransaction,
    cause: String,
) -> Result<(), PlayerTransactionError> {
    let failure = TransactionFailure::Request(cause);
    let rollback_failures = rollback_player_transaction(database, &mut transaction).await;
    if rollback_failures.is_empty() {
        Ok(())
    } else {
        Err(PlayerTransactionError::Reconciliation {
            failure: Box::new(failure),
            rollback_failures,
        })
    }
}

fn validate_transaction_plan(
    transaction: &PlayerTransaction
) -> Result<(), PlayerTransactionPlanError> {
    let Some(relocation) = transaction.relocation.as_ref() else {
        return Ok(());
    };
    if transaction.persistence != PlayerPersistence::Full {
        return Err(PlayerTransactionPlanError::RelocationRequiresFullPersistence);
    }
    let planned_player_id = crate::movement::relocation_player_id(&relocation.plan);
    if transaction.staged_player.id != planned_player_id {
        return Err(PlayerTransactionPlanError::RelocationPlayerMismatch {
            planned: planned_player_id.to_owned(),
            staged: transaction.staged_player.id.clone(),
        });
    }
    let planned_map_id = crate::movement::relocation_target_map_id(&relocation.plan);
    if transaction.staged_player.map_id != planned_map_id {
        return Err(PlayerTransactionPlanError::RelocationMapMismatch {
            planned: planned_map_id,
            staged: transaction.staged_player.map_id,
        });
    }
    if transaction.staged_player.position
        != Some(crate::movement::relocation_target_position(
            &relocation.plan,
        ))
    {
        return Err(PlayerTransactionPlanError::RelocationPositionMismatch);
    }
    Ok(())
}

async fn commit_staged_changes(
    database: &Database,
    transaction: &mut PlayerTransaction,
) -> Result<(), TransactionFailure> {
    if transaction.persistence == PlayerPersistence::Full {
        let committed = crate::database::save_player(
            database,
            &transaction.original_player,
            &transaction.staged_player,
        )
        .await
        .map_err(TransactionFailure::Database)?;
        transaction.staged_player = committed.clone();
        transaction.durable_saved = Some(committed);
    }
    if let Some(change) = transaction.effects.as_mut() {
        crate::effects::commit_staged(
            &change.store,
            &transaction.staged_player.id,
            &change.original,
            &change.staged,
        )
        .map_err(TransactionFailure::Effects)?;
        change.committed = true;
    }
    if let Some(change) = transaction.drops.as_mut() {
        crate::items::commit_new_staged_drops(&change.store, &change.staged)
            .map_err(TransactionFailure::Drops)?;
        change.committed = true;
    }
    if let Some(change) = transaction.relocation.as_mut() {
        change.committed = Some(
            crate::movement::commit_relocation(&change.store, &change.plan)
                .map_err(TransactionFailure::Movement)?,
        );
    }
    if let Some(change) = transaction.activity.as_mut() {
        change.rollback = Some(
            crate::recovery::delay_recovery_after_activity(
                &change.store,
                &change.player_id,
                change.now_ms,
            )
            .map_err(TransactionFailure::Recovery)?,
        );
    }
    if let Some(change) = transaction.mob.as_mut() {
        crate::mobs::commit_player_attack(&change.store, &mut change.update)
            .await
            .map_err(TransactionFailure::Mobs)?;
        change.committed = true;
    }
    Ok(())
}

async fn rollback_player_transaction(
    database: &Database,
    transaction: &mut PlayerTransaction,
) -> Vec<RollbackFailure> {
    let mut failures = Vec::new();
    rollback_mob(transaction, &mut failures).await;
    rollback_activity(transaction, &mut failures);
    rollback_relocation(transaction, &mut failures);
    rollback_drops(transaction, &mut failures);
    rollback_pickup(transaction, &mut failures);
    rollback_effects(transaction, &mut failures);
    rollback_reservations(transaction, &mut failures);
    if let Some(committed) = transaction.durable_saved.take() {
        match crate::database::restore_player(database, &committed, &transaction.original_player)
            .await
        {
            Ok(player) => {
                transaction.original_player = player;
            }
            Err(error) => failures.push(RollbackFailure::Database(error)),
        }
    }
    failures
}

async fn rollback_mob(
    transaction: &mut PlayerTransaction,
    failures: &mut Vec<RollbackFailure>,
) {
    let Some(change) = transaction.mob.as_mut() else {
        return;
    };
    if change.committed {
        failures.push(RollbackFailure::Mobs(
            crate::mobs::MobStoreError::PlayerAttackTransaction {
                message: "a committed mob attack reached rollback".to_owned(),
            },
        ));
        return;
    }
    if let Err(error) = crate::mobs::rollback_player_update(&change.store, &mut change.update).await
    {
        failures.push(RollbackFailure::Mobs(error));
    }
}

fn rollback_activity(
    transaction: &mut PlayerTransaction,
    failures: &mut Vec<RollbackFailure>,
) {
    let Some(change) = transaction.activity.as_mut() else {
        return;
    };
    let Some(rollback) = change.rollback.take() else {
        return;
    };
    if let Err(error) = crate::recovery::restore_recovery_activity(&change.store, &rollback) {
        failures.push(RollbackFailure::Activity(error));
    }
}

fn rollback_relocation(
    transaction: &mut PlayerTransaction,
    failures: &mut Vec<RollbackFailure>,
) {
    let Some(change) = transaction.relocation.as_mut() else {
        return;
    };
    let Some(committed) = change.committed.take() else {
        return;
    };
    if let Err(error) = crate::movement::restore_relocation(&change.store, committed) {
        failures.push(RollbackFailure::Relocation(error));
    }
}

fn rollback_drops(
    transaction: &mut PlayerTransaction,
    failures: &mut Vec<RollbackFailure>,
) {
    let Some(change) = transaction.drops.as_mut().filter(|change| change.committed) else {
        return;
    };
    match crate::items::rollback_staged_drops(&change.store, &change.staged) {
        Ok(()) => change.committed = false,
        Err(error) => failures.push(RollbackFailure::Drops(error)),
    }
}

fn rollback_pickup(
    transaction: &mut PlayerTransaction,
    failures: &mut Vec<RollbackFailure>,
) {
    let Some(change) = transaction.pickup.take() else {
        return;
    };
    if let Err(error) = crate::items::restore_picked_up_drop(
        &change.store,
        change.map_id,
        change.item,
        change.owner_player_id,
    ) {
        failures.push(RollbackFailure::Pickup(error));
    }
}

fn rollback_effects(
    transaction: &mut PlayerTransaction,
    failures: &mut Vec<RollbackFailure>,
) {
    let Some(change) = transaction
        .effects
        .as_mut()
        .filter(|change| change.committed)
    else {
        return;
    };
    match crate::effects::rollback_staged(
        &change.store,
        &transaction.staged_player.id,
        &change.staged,
        &change.original,
    ) {
        Ok(()) => change.committed = false,
        Err(error) => failures.push(RollbackFailure::Effects(error)),
    }
}

fn rollback_reservations(
    transaction: &mut PlayerTransaction,
    failures: &mut Vec<RollbackFailure>,
) {
    if let Some(change) = transaction.skill_cooldown.take()
        && let Err(error) =
            crate::skills::release_skill_cooldown(&change.store, &change.reservation)
    {
        failures.push(RollbackFailure::SkillCooldown(error));
    }
    if let Some(change) = transaction.basic_attack.take()
        && let Err(error) = crate::attacks::release_basic_attack(&change.store, &change.reservation)
    {
        failures.push(RollbackFailure::BasicAttack(error));
    }
    if let Some(change) = transaction.recovery.take()
        && let Err(error) = crate::recovery::release_recovery(&change.store, &change.reservation)
    {
        failures.push(RollbackFailure::Recovery(error));
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use oozems_proto::v1::CharacterAppearance;
    use oozems_proto::v1::CharacterGender;
    use oozems_proto::v1::CharacterStats;
    use oozems_proto::v1::InventoryState;
    use oozems_proto::v1::Map;
    use oozems_proto::v1::Platform;
    use oozems_proto::v1::Portal;
    use oozems_proto::v1::Vec2;

    use super::*;
    use crate::database::PlayerId;
    use crate::player_lock::PlayerLocks;
    use crate::player_lock::acquire_player;

    #[test]
    fn drop_staging_rejects_a_different_store_in_release_builds() {
        let first = Arc::new(DropStore::new(Duration::from_secs(60)));
        let second = Arc::new(DropStore::new(Duration::from_secs(60)));
        let player = PlayerState {
            id: "drop-plan".to_owned(),
            map_id: 1,
            position: Some(Vec2 { x: 10.0, y: 20.0 }),
            ..PlayerState::default()
        };
        let grant = crate::items::stage_inventory_drop(
            &first,
            &crate::items::RemovedItem {
                item_id: 2_000_000,
                quantity: 1,
                expires_at_unix_ms: 0,
                map_id: player.map_id,
                position: player.position.expect("player position"),
                player: player.clone(),
            },
        )
        .expect("stage drop grant");
        let mut transaction =
            new_player_transaction(player.clone(), player, PlayerPersistence::None);
        stage_drops(&mut transaction, first, [grant.clone()]).expect("first drop store");

        assert!(matches!(
            stage_drops(&mut transaction, second, [grant]),
            Err(PlayerTransactionError::Plan(
                PlayerTransactionPlanError::DropStoreMismatch
            ))
        ));
    }

    #[tokio::test]
    async fn drop_commit_failure_rolls_back_effects_and_the_durable_player() {
        let directory = tempfile::tempdir().expect("temporary database directory");
        let database = crate::database::open_sqlite(&directory.path().join("players.sqlite"))
            .expect("open database");
        let player_id = PlayerId::parse("commit-rollback").expect("player ID");
        let locks = PlayerLocks::default();
        let guard = acquire_player(&locks, player_id.as_str())
            .await
            .expect("player guard");
        let original = crate::database::create_player(&database, &test_player(player_id.as_str()))
            .await
            .expect("create player");
        let effects = Arc::new(ActiveEffects::default());
        let original_effects =
            crate::effects::snapshot(&effects, player_id.as_str(), 1_000).expect("effect snapshot");
        let mut staged_effects = original_effects.clone();
        crate::effects::apply_skill_effect(
            &mut staged_effects,
            &oozems_proto::v1::SkillUseResult {
                skill_id: 1_000,
                duration_ms: 10_000,
                ..oozems_proto::v1::SkillUseResult::default()
            },
            1_000,
        );
        let drops = Arc::new(DropStore::new(Duration::from_secs(60)));
        let grant = crate::items::stage_inventory_drop(
            &drops,
            &crate::items::RemovedItem {
                item_id: 2_000_000,
                quantity: 1,
                expires_at_unix_ms: 0,
                map_id: original.map_id,
                position: original.position.expect("player position"),
                player: original.clone(),
            },
        )
        .expect("stage drop");
        crate::items::commit_staged_drops(&drops, std::slice::from_ref(&grant))
            .expect("seed conflicting drop");
        let mut updated = original.clone();
        updated.mesos = 500;
        let mut transaction =
            new_player_transaction(original.clone(), updated, PlayerPersistence::Full);
        stage_effects(
            &mut transaction,
            effects.clone(),
            original_effects.clone(),
            staged_effects,
        );
        stage_drops(&mut transaction, drops.clone(), [grant.clone()])
            .expect("stage transaction drop");

        let error = commit_player_transaction(&database, &guard, transaction)
            .await
            .err()
            .expect("the conflicting drop must fail commit");

        assert!(matches!(error, PlayerTransactionError::Failed { .. }));
        assert_eq!(
            crate::effects::snapshot(&effects, player_id.as_str(), 1_001)
                .expect("rolled-back effects"),
            original_effects
        );
        assert_eq!(
            crate::items::map_drops(&drops, original.map_id)
                .expect("preexisting drop")
                .len(),
            1
        );
        let durable = crate::database::load_player(&database, &player_id)
            .await
            .expect("load durable player")
            .expect("durable player");
        assert_eq!(durable.mesos, original.mesos);
        assert!(durable.revision > original.revision);
    }

    #[tokio::test]
    async fn full_persistence_is_compensated_when_a_later_store_commit_fails() {
        let directory = tempfile::tempdir().expect("temporary database directory");
        let database = crate::database::open_sqlite(&directory.path().join("players.sqlite"))
            .expect("open database");
        let player_id = PlayerId::parse("full-rollback").expect("player ID");
        let locks = PlayerLocks::default();
        let guard = acquire_player(&locks, player_id.as_str())
            .await
            .expect("player guard");
        let original = crate::database::create_player(&database, &test_player(player_id.as_str()))
            .await
            .expect("create player");
        let effects = Arc::new(ActiveEffects::default());
        let original_effects =
            crate::effects::snapshot(&effects, player_id.as_str(), 1_000).expect("effect snapshot");
        let mut concurrent_effects = original_effects.clone();
        crate::effects::apply_skill_effect(
            &mut concurrent_effects,
            &oozems_proto::v1::SkillUseResult {
                skill_id: 1_000,
                duration_ms: 10_000,
                ..oozems_proto::v1::SkillUseResult::default()
            },
            1_000,
        );
        crate::effects::commit(&effects, player_id.as_str(), concurrent_effects.clone())
            .expect("install concurrent effects");
        let mut moved = original.clone();
        moved.position = Some(Vec2 { x: 90.0, y: 80.0 });
        moved.mesos = 500;
        let mut transaction =
            new_player_transaction(original.clone(), moved, PlayerPersistence::Full);
        stage_effects(
            &mut transaction,
            effects.clone(),
            original_effects.clone(),
            original_effects,
        );

        let error = commit_player_transaction(&database, &guard, transaction)
            .await
            .err()
            .expect("the stale effect snapshot must fail commit");

        assert!(matches!(
            error,
            PlayerTransactionError::Failed { failure }
                if matches!(*failure, TransactionFailure::Effects(_))
        ));
        let durable = crate::database::load_player(&database, &player_id)
            .await
            .expect("load durable player")
            .expect("durable player");
        assert_eq!(durable.position, None);
        assert_eq!(durable.mesos, original.mesos);
        assert_eq!(durable.revision, original.revision + 2);
        assert_eq!(
            crate::effects::snapshot(&effects, player_id.as_str(), 1_001)
                .expect("concurrent effects remain"),
            concurrent_effects
        );
    }

    #[tokio::test]
    async fn persistence_failure_restores_pickup_and_releases_skill_for_immediate_retry() {
        let directory = tempfile::tempdir().expect("temporary database directory");
        let database = crate::database::open_sqlite(&directory.path().join("players.sqlite"))
            .expect("open database");
        let player_id = PlayerId::parse("transaction-player").expect("player ID");
        let locks = PlayerLocks::default();
        let guard = acquire_player(&locks, player_id.as_str())
            .await
            .expect("player guard");
        let original = crate::database::create_player(&database, &test_player(player_id.as_str()))
            .await
            .expect("create player");
        let drops = Arc::new(DropStore::new(Duration::from_secs(60)));
        let dropped = DroppedItem {
            id: "pickup-rollback".to_owned(),
            item_id: 2_380_000,
            position: Some(Vec2 { x: 10.0, y: 20.0 }),
            despawn_at_unix_ms: u64::MAX,
            quantity: 1,
            expires_at_unix_ms: 0,
        };
        crate::items::restore_drop(&drops, 100, dropped.clone(), None).expect("seed drop");
        let definitions = [oozems_proto::v1::ItemDefinition {
            item_id: dropped.item_id,
            name: "Card".to_owned(),
            stack_max: 100,
            ..oozems_proto::v1::ItemDefinition::default()
        }];
        let picked = crate::items::pick_up_nearest(
            &drops,
            original.clone(),
            Vec2 { x: 10.0, y: 20.0 },
            &definitions,
        )
        .expect("pick up staged item");
        let cooldowns = Arc::new(SkillCooldowns::default());
        let reservation = crate::skills::reserve_skill_cooldown(
            &cooldowns,
            player_id.as_str(),
            1_000,
            10_000,
            5_000,
        )
        .expect("reserve cooldown");
        let invalid = picked.player.clone();
        let mut stale_original = original.clone();
        stale_original.revision = i64::MAX as u64;
        let mut transaction =
            new_player_transaction(stale_original, invalid, PlayerPersistence::Full);
        stage_pickup(&mut transaction, drops.clone(), original.map_id, &picked);
        stage_skill_cooldown(&mut transaction, cooldowns.clone(), reservation);

        let error = commit_player_transaction(&database, &guard, transaction)
            .await
            .err()
            .expect("revision overflow must fail persistence");

        assert!(matches!(error, PlayerTransactionError::Failed { .. }));
        assert_eq!(
            crate::items::map_drops(&drops, original.map_id).expect("restored map drops"),
            vec![dropped]
        );
        crate::skills::reserve_skill_cooldown(&cooldowns, player_id.as_str(), 1_000, 10_001, 5_000)
            .expect("immediate retry after persistence failure");
        let durable = crate::database::load_player(&database, &player_id)
            .await
            .expect("load durable player")
            .expect("durable player");
        assert_eq!(durable.revision, original.revision);
        assert_eq!(durable.mesos, original.mesos);
    }

    #[tokio::test]
    async fn relocation_plan_is_rejected_before_any_store_commit() {
        let directory = tempfile::tempdir().expect("temporary database directory");
        let database = crate::database::open_sqlite(&directory.path().join("players.sqlite"))
            .expect("open database");
        let player = test_player("invalid-relocation-plan");
        let player_id = PlayerId::parse(&player.id).expect("player ID");
        let locks = PlayerLocks::default();
        let guard = acquire_player(&locks, player_id.as_str())
            .await
            .expect("player guard");
        let source_map = test_map(100);
        let mut target_map = test_map(200);
        target_map.portals.push(Portal {
            name: "target".to_owned(),
            x: 700.0,
            y: 20.0,
            ..Portal::default()
        });
        let movement = Arc::new(MovementTracker::default());
        crate::movement::initialize_player(
            &movement,
            &player,
            &source_map,
            movement_config(),
            1_000,
        )
        .expect("initialize movement");
        let (_, plan) = crate::movement::relocate_player(
            &movement,
            &player,
            &source_map,
            &target_map,
            "target",
            movement_config(),
            1_200,
        )
        .expect("plan relocation");
        let staged = crate::movement::project_relocation_player(&plan, player.clone())
            .expect("project relocation");
        let mut transaction =
            new_player_transaction(player.clone(), staged, PlayerPersistence::None);
        stage_relocation(&mut transaction, movement.clone(), plan);

        let error = commit_player_transaction(&database, &guard, transaction)
            .await
            .err()
            .expect("non-persistent relocation must be rejected");

        assert!(matches!(
            error,
            PlayerTransactionError::Plan(
                PlayerTransactionPlanError::RelocationRequiresFullPersistence
            )
        ));
        let unchanged = crate::movement::synchronize_player(&movement, player.clone())
            .expect("unchanged source movement");
        assert_eq!(unchanged.map_id, player.map_id);
        assert_eq!(unchanged.position, player.position);
        assert!(
            crate::database::load_player(&database, &player_id)
                .await
                .expect("query database")
                .is_none()
        );
    }

    fn test_player(player_id: &str) -> PlayerState {
        PlayerState {
            id: player_id.to_owned(),
            name: "Mina".to_owned(),
            level: 1,
            map_id: 100,
            position: Some(Vec2 { x: 10.0, y: 20.0 }),
            appearance: Some(CharacterAppearance {
                gender: CharacterGender::Female as i32,
                skin_id: 2_000,
                face_id: 21_000,
                hair_id: 31_000,
            }),
            stats: Some(CharacterStats {
                hp: 50,
                max_hp: 50,
                mp: 20,
                max_mp: 20,
                experience_required: 100,
                ..CharacterStats::default()
            }),
            inventory: Some(InventoryState {
                capacity: 8,
                ..InventoryState::default()
            }),
            ..PlayerState::default()
        }
    }

    fn test_map(id: u32) -> Map {
        Map {
            id,
            width: 800,
            height: 600,
            platforms: vec![Platform {
                x: 0.0,
                y: 20.0,
                end_x: 800.0,
                end_y: 20.0,
                ..Platform::default()
            }],
            ..Map::default()
        }
    }

    fn movement_config() -> crate::gameplay::MovementConfig {
        crate::gameplay::MovementConfig {
            walk_speed: 125.0,
            climb_speed: 100.0,
            gravity: 2_000.0,
            jump_speed: 500.0,
            speed_cap: 140,
            jump_cap: 123,
            snapshot_interval: Duration::from_millis(100),
            maximum_snapshot_gap: Duration::from_secs(2),
            position_tolerance: 20.0,
            ground_tolerance: 10.0,
            platform_edge_tolerance: 10.0,
            ladder_reach: 20.0,
            ladder_end_reach: 20.0,
            portal_horizontal_reach: 80.0,
            portal_vertical_reach: 80.0,
        }
    }
}
