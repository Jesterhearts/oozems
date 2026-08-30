use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Weak;

use thiserror::Error;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::OwnedMutexGuard;

type StoredLocks = Mutex<HashMap<String, Weak<AsyncMutex<()>>>>;

#[derive(Default)]
pub struct PlayerLocks {
    locks: Arc<StoredLocks>,
    reconciliations: Arc<crate::player_reconciliation::PlayerReconciliations>,
}

#[derive(Clone)]
pub struct PlayerGuard {
    lease: Arc<PlayerGuardLease>,
}

struct PlayerGuardLease {
    player_id: String,
    guard: Option<OwnedMutexGuard<()>>,
    locks: Weak<StoredLocks>,
    reconciliations: Arc<crate::player_reconciliation::PlayerReconciliations>,
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PlayerLockError {
    #[error("the player operation lock store is unavailable")]
    Store,
    #[error("the player guard belongs to a different lock store")]
    ForeignStore,
    #[error("the player guard holds {held_player_id:?}, not {requested_player_id:?}")]
    WrongPlayer {
        held_player_id: String,
        requested_player_id: String,
    },
    #[error(
        "player reconciliation operation {operation_id} must be completed before another \
         operation can run"
    )]
    ReconciliationRequired { operation_id: u64 },
}

pub async fn acquire_player(
    locks: &PlayerLocks,
    player_id: &str,
) -> Result<PlayerGuard, PlayerLockError> {
    let lock = {
        let mut stored = locks.locks.lock().map_err(|_| PlayerLockError::Store)?;
        match stored.get(player_id).and_then(Weak::upgrade) {
            Some(lock) => lock,
            None => {
                let lock = Arc::new(AsyncMutex::new(()));
                stored.insert(player_id.to_owned(), Arc::downgrade(&lock));
                lock
            }
        }
    };
    let guard = lock.lock_owned().await;
    let guard = PlayerGuard {
        lease: Arc::new(PlayerGuardLease {
            player_id: player_id.to_owned(),
            guard: Some(guard),
            locks: Arc::downgrade(&locks.locks),
            reconciliations: locks.reconciliations.clone(),
        }),
    };
    if let Some(record) =
        crate::player_reconciliation::reconciliation_required(&locks.reconciliations, player_id)
    {
        return Err(PlayerLockError::ReconciliationRequired {
            operation_id: record.operation_id,
        });
    }
    Ok(guard)
}

pub(crate) fn validate_player_guard(
    locks: &PlayerLocks,
    guard: &PlayerGuard,
    player_id: &str,
) -> Result<(), PlayerLockError> {
    let guard_locks = guard
        .lease
        .locks
        .upgrade()
        .ok_or(PlayerLockError::ForeignStore)?;
    if !Arc::ptr_eq(&guard_locks, &locks.locks) {
        return Err(PlayerLockError::ForeignStore);
    }
    if !guard.holds_player(player_id) {
        return Err(PlayerLockError::WrongPlayer {
            held_player_id: guard.lease.player_id.clone(),
            requested_player_id: player_id.to_owned(),
        });
    }
    Ok(())
}

impl PlayerGuard {
    pub(crate) fn holds_player(
        &self,
        player_id: &str,
    ) -> bool {
        self.lease.player_id == player_id
    }

    pub(crate) fn mark_reconciliation_required(
        &self,
        failure: String,
        rollback_failures: Vec<String>,
    ) -> crate::player_reconciliation::ReconciliationRecord {
        crate::player_reconciliation::mark_reconciliation_required(
            &self.lease.reconciliations,
            &self.lease.player_id,
            failure,
            rollback_failures,
        )
    }
}

impl Drop for PlayerGuardLease {
    fn drop(&mut self) {
        drop(self.guard.take());
        let Some(locks) = self.locks.upgrade() else {
            return;
        };
        let Ok(mut stored) = locks.lock() else {
            return;
        };
        if stored
            .get(&self.player_id)
            .is_some_and(|lock| lock.strong_count() == 0)
        {
            stored.remove(&self.player_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::PlayerLocks;
    use super::acquire_player;
    use super::validate_player_guard;

    #[tokio::test]
    async fn operations_for_one_player_are_serialized() {
        let locks = Arc::new(PlayerLocks::default());
        let first = acquire_player(&locks, "player").await.expect("first lock");
        let waiting_locks = locks.clone();
        let waiting = tokio::spawn(async move {
            acquire_player(&waiting_locks, "player")
                .await
                .expect("second lock")
        });

        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        drop(first);
        waiting.await.expect("waiting task");
    }

    #[tokio::test]
    async fn cloned_guards_hold_the_player_lock_until_the_last_clone_drops() {
        let locks = Arc::new(PlayerLocks::default());
        let first = acquire_player(&locks, "player").await.expect("first lock");
        let retained = first.clone();
        let waiting_locks = locks.clone();
        let waiting = tokio::spawn(async move {
            acquire_player(&waiting_locks, "player")
                .await
                .expect("second lock")
        });

        drop(first);
        tokio::task::yield_now().await;
        assert!(!waiting.is_finished());
        drop(retained);
        waiting.await.expect("waiting task");
    }

    #[tokio::test]
    async fn different_players_do_not_block_each_other() {
        let locks = PlayerLocks::default();
        let _first = acquire_player(&locks, "first").await.expect("first lock");

        let second = acquire_player(&locks, "second").await;

        assert!(second.is_ok());
    }

    #[tokio::test]
    async fn quarantined_player_is_rejected_after_lock_acquisition() {
        let locks = Arc::new(PlayerLocks::default());
        let first = acquire_player(&locks, "first").await.expect("first lock");
        first.mark_reconciliation_required(
            "commit failed".to_owned(),
            vec!["rollback failed".to_owned()],
        );
        let waiting_locks = locks.clone();
        let waiting = tokio::spawn(async move { acquire_player(&waiting_locks, "first").await });

        assert!(!waiting.is_finished());
        drop(first);

        assert!(matches!(
            waiting.await.expect("waiting task"),
            Err(super::PlayerLockError::ReconciliationRequired { operation_id: 1 })
        ));
        assert!(acquire_player(&locks, "second").await.is_ok());
    }

    #[tokio::test]
    async fn unused_player_locks_are_removed() {
        let locks = PlayerLocks::default();
        let guard = acquire_player(&locks, "player").await.expect("player lock");
        assert_eq!(locks.locks.lock().expect("stored locks").len(), 1);

        drop(guard);

        assert!(locks.locks.lock().expect("stored locks").is_empty());
    }

    #[tokio::test]
    async fn guard_validation_rejects_a_different_lock_store() {
        let source = PlayerLocks::default();
        let other = PlayerLocks::default();
        let guard = acquire_player(&source, "player")
            .await
            .expect("player lock");

        assert_eq!(
            validate_player_guard(&other, &guard, "player"),
            Err(super::PlayerLockError::ForeignStore)
        );
    }

    #[tokio::test]
    async fn guard_validation_rejects_a_different_player() {
        let locks = PlayerLocks::default();
        let guard = acquire_player(&locks, "first").await.expect("player lock");

        assert_eq!(
            validate_player_guard(&locks, &guard, "second"),
            Err(super::PlayerLockError::WrongPlayer {
                held_player_id: "first".to_owned(),
                requested_player_id: "second".to_owned(),
            })
        );
    }
}
