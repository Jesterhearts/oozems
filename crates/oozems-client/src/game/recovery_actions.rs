use std::cell::Cell;
use std::cell::RefCell;
use std::rc::Rc;

use oozems_proto::v1::RecoverPlayerResponse;
use wasm_bindgen_futures::spawn_local;

use super::Game;
use crate::api;
use crate::show_status;

const RECOVERY_INTERVAL_MS: f64 = 10_000.0;

#[derive(Default)]
pub(super) struct RecoveryState {
    in_flight: Rc<Cell<bool>>,
    next_attempt_ms: Option<f64>,
}

pub(super) fn update(
    state: &mut RecoveryState,
    needs_recovery: bool,
    can_poll: bool,
    timestamp_ms: f64,
) -> bool {
    if !needs_recovery {
        state.next_attempt_ms = None;
        return false;
    }
    let deadline_ms = state
        .next_attempt_ms
        .get_or_insert(timestamp_ms + RECOVERY_INTERVAL_MS);
    if timestamp_ms < *deadline_ms || !can_poll || state.in_flight.get() {
        return false;
    }
    state.next_attempt_ms = Some(timestamp_ms + RECOVERY_INTERVAL_MS);
    true
}

pub(super) fn begin(game: Rc<RefCell<Game>>) {
    let in_flight = game.borrow().recovery_state.in_flight.clone();
    if in_flight.replace(true) {
        return;
    }
    let player_id = game.borrow().player.id.clone();
    spawn_local(async move {
        match api::recover_player(&player_id).await {
            Ok(response) => {
                if let Err(error) = install(&mut game.borrow_mut(), response) {
                    show_status(&format!("Recovery could not finish: {error}"), true);
                }
            }
            Err(error) => {
                let mut game = game.borrow_mut();
                let retry_at_ms = game.frame_time_ms + RECOVERY_INTERVAL_MS;
                game.recovery_state.next_attempt_ms = Some(retry_at_ms);
                show_status(&format!("Recovery failed: {error}"), true);
            }
        }
        in_flight.set(false);
    });
}

pub(super) fn reset(state: &mut RecoveryState) {
    state.next_attempt_ms = None;
}

pub(super) fn is_in_flight(state: &RecoveryState) -> bool {
    state.in_flight.get()
}

fn install(
    game: &mut Game,
    response: RecoverPlayerResponse,
) -> Result<(), String> {
    let stats = response
        .player
        .and_then(|player| player.stats)
        .ok_or("recovery response did not contain character stats")?;
    game.player.stats = Some(stats);
    let retry_after_ms = response.retry_after_ms.max(1) as f64;
    game.recovery_state.next_attempt_ms = Some(game.frame_time_ms + retry_after_ms);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::RECOVERY_INTERVAL_MS;
    use super::RecoveryState;
    use super::update;

    #[test]
    fn recovery_poll_becomes_due_after_ten_seconds() {
        let mut state = RecoveryState::default();

        assert!(!update(&mut state, true, true, 1_000.0));
        assert!(!update(
            &mut state,
            true,
            true,
            1_000.0 + RECOVERY_INTERVAL_MS - 1.0
        ));
        assert!(update(
            &mut state,
            true,
            true,
            1_000.0 + RECOVERY_INTERVAL_MS
        ));
        assert!(!update(
            &mut state,
            true,
            true,
            1_000.0 + RECOVERY_INTERVAL_MS + 1.0
        ));
    }

    #[test]
    fn full_stats_suspend_the_recovery_poll() {
        let mut state = RecoveryState::default();

        assert!(!update(&mut state, true, true, 1_000.0));
        assert!(!update(&mut state, false, true, 9_000.0));
        assert!(!update(&mut state, true, true, 10_000.0));
        assert!(!update(&mut state, true, true, 19_999.0));
        assert!(update(&mut state, true, true, 20_000.0));
    }

    #[test]
    fn busy_client_does_not_discard_an_overdue_poll() {
        let mut state = RecoveryState::default();

        assert!(!update(&mut state, true, true, 1_000.0));
        assert!(!update(&mut state, true, false, 11_000.0));
        assert!(update(&mut state, true, true, 11_001.0));
    }
}
