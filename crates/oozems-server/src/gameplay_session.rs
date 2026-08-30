use std::collections::HashMap;
use std::sync::Mutex;

use thiserror::Error;

const MAX_ACTIVE_SESSIONS: usize = 4_096;
const TOKEN_BYTES: usize = 32;

pub struct GameplaySessions {
    store: Mutex<GameplaySessionStore>,
    capacity: usize,
}

#[derive(Default)]
struct GameplaySessionStore {
    current: HashMap<String, GameplaySession>,
    access_sequence: u64,
}

struct GameplaySession {
    token: String,
    last_access: u64,
}

#[derive(Debug, Error)]
pub enum GameplaySessionError {
    #[error("the gameplay session store is unavailable")]
    Store,
    #[error("secure gameplay session token generation failed: {0}")]
    Random(String),
}

impl Default for GameplaySessions {
    fn default() -> Self {
        Self {
            store: Mutex::new(GameplaySessionStore::default()),
            capacity: MAX_ACTIVE_SESSIONS,
        }
    }
}

pub fn issue_session(
    sessions: &GameplaySessions,
    player_id: &str,
) -> Result<String, GameplaySessionError> {
    let mut bytes = [0_u8; TOKEN_BYTES];
    getrandom::fill(&mut bytes).map_err(|error| GameplaySessionError::Random(error.to_string()))?;
    let token = hex::encode(bytes);
    let mut store = sessions
        .store
        .lock()
        .map_err(|_| GameplaySessionError::Store)?;
    let access_sequence = next_access_sequence(&mut store);
    if let Some(current) = store.current.get_mut(player_id) {
        current.token.clone_from(&token);
        current.last_access = access_sequence;
        return Ok(token);
    }
    if store.current.len() >= sessions.capacity {
        let least_recently_used = store
            .current
            .iter()
            .min_by_key(|(player_id, session)| (session.last_access, player_id.as_str()))
            .map(|(player_id, _)| player_id.clone())
            .expect("a full gameplay session store contains an entry");
        store.current.remove(&least_recently_used);
    }
    store.current.insert(
        player_id.to_owned(),
        GameplaySession {
            token: token.clone(),
            last_access: access_sequence,
        },
    );
    Ok(token)
}

pub fn is_current_session(
    sessions: &GameplaySessions,
    player_id: &str,
    token: &str,
) -> Result<bool, GameplaySessionError> {
    let mut store = sessions
        .store
        .lock()
        .map_err(|_| GameplaySessionError::Store)?;
    let is_current = store
        .current
        .get(player_id)
        .is_some_and(|current| constant_time_eq(current.token.as_bytes(), token.as_bytes()));
    if is_current {
        let access_sequence = next_access_sequence(&mut store);
        store
            .current
            .get_mut(player_id)
            .expect("the current gameplay session was checked above")
            .last_access = access_sequence;
    }
    Ok(is_current)
}

fn next_access_sequence(store: &mut GameplaySessionStore) -> u64 {
    store.access_sequence = store.access_sequence.saturating_add(1);
    store.access_sequence
}

fn constant_time_eq(
    left: &[u8],
    right: &[u8],
) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sessions_with_capacity(capacity: usize) -> GameplaySessions {
        assert!(capacity > 0);
        GameplaySessions {
            store: Mutex::new(GameplaySessionStore::default()),
            capacity,
        }
    }

    #[test]
    fn issuing_a_new_session_revokes_the_previous_token() {
        let sessions = GameplaySessions::default();
        let first = issue_session(&sessions, "player").expect("first session");
        let second = issue_session(&sessions, "player").expect("second session");

        assert_ne!(first, second);
        assert!(!is_current_session(&sessions, "player", &first).expect("old session check"));
        assert!(is_current_session(&sessions, "player", &second).expect("new session check"));
        assert!(!is_current_session(&sessions, "other", &second).expect("other player check"));
    }

    #[test]
    fn session_capacity_evicts_the_least_recently_used_player() {
        let sessions = sessions_with_capacity(2);
        let first = issue_session(&sessions, "first").expect("first session");
        let second = issue_session(&sessions, "second").expect("second session");
        assert!(is_current_session(&sessions, "first", &first).expect("touch first session"));

        let third = issue_session(&sessions, "third").expect("third session");

        assert!(is_current_session(&sessions, "first", &first).expect("first session check"));
        assert!(!is_current_session(&sessions, "second", &second).expect("second session check"));
        assert!(is_current_session(&sessions, "third", &third).expect("third session check"));
        assert_eq!(
            sessions.store.lock().expect("session store").current.len(),
            2
        );
    }
}
