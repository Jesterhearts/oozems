use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::Weak;

use thiserror::Error;
use tokio::sync::Mutex as AsyncMutex;
use tokio::sync::OwnedMutexGuard;

#[derive(Default)]
pub struct PlayerLocks {
    locks: Mutex<HashMap<String, Weak<AsyncMutex<()>>>>,
}

#[derive(Debug, Error)]
pub enum PlayerLockError {
    #[error("the player operation lock store is unavailable")]
    Store,
}

pub async fn acquire_player(
    locks: &PlayerLocks,
    player_id: &str,
) -> Result<OwnedMutexGuard<()>, PlayerLockError> {
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
    Ok(lock.lock_owned().await)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::PlayerLocks;
    use super::acquire_player;

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
    async fn different_players_do_not_block_each_other() {
        let locks = PlayerLocks::default();
        let _first = acquire_player(&locks, "first").await.expect("first lock");

        let second = acquire_player(&locks, "second").await;

        assert!(second.is_ok());
    }
}
