use std::cell::RefCell;
use std::rc::Rc;

use oozems_proto::v1::RecoverPlayerResponse;

use super::Game;
use crate::api;

const RECOVERY_INTERVAL_MS: f64 = 10_000.0;

#[derive(Default)]
pub(super) struct RecoveryState {
    next_attempt_ms: Option<f64>,
}

pub(super) fn update(
    state: &mut RecoveryState,
    needs_recovery: bool,
    can_poll: bool,
    in_flight: bool,
    timestamp_ms: f64,
) -> bool {
    if !needs_recovery {
        state.next_attempt_ms = None;
        return false;
    }
    let deadline_ms = state
        .next_attempt_ms
        .get_or_insert(timestamp_ms + RECOVERY_INTERVAL_MS);
    if timestamp_ms < *deadline_ms || !can_poll || in_flight {
        return false;
    }
    state.next_attempt_ms = Some(timestamp_ms + RECOVERY_INTERVAL_MS);
    true
}

pub(super) fn begin(
    game: Rc<RefCell<Game>>,
    permit: super::requests::RequestPermit,
) {
    let player_id = game.borrow().player.id.clone();
    super::requests::spawn_request(
        game,
        permit,
        move || async move {
            api::recover_player(&player_id)
                .await
                .map_err(|error| error.to_string())
        },
        |game, result, request_started_ms| match result {
            Ok(response) => match install(game, response, request_started_ms) {
                Ok(()) => super::requests::RequestStatus::silent(),
                Err(error) => super::requests::RequestStatus::error(format!(
                    "Recovery could not finish: {error}"
                )),
            },
            Err(error) => {
                let retry_at_ms = game.clock.now_ms + RECOVERY_INTERVAL_MS;
                game.requests.recovery.next_attempt_ms = Some(retry_at_ms);
                super::requests::RequestStatus::error(format!("Recovery failed: {error}"))
            }
        },
    );
}

pub(super) fn reset(state: &mut RecoveryState) {
    state.next_attempt_ms = None;
}

fn install(
    game: &mut Game,
    mut response: RecoverPlayerResponse,
    request_started_ms: f64,
) -> Result<(), String> {
    let (player, active_buffs) = super::responses::take_player_and_active_buffs(&mut response)?;
    api::require_data(player.stats.as_ref(), "character stats")
        .map_err(|error| error.to_string())?;
    super::install_full_player_update(game, player);
    super::install_active_buffs(game, active_buffs, request_started_ms);
    let retry_after_ms = response.retry_after_ms.max(1) as f64;
    game.requests.recovery.next_attempt_ms = Some(game.clock.now_ms + retry_after_ms);
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

        assert!(!update(&mut state, true, true, false, 1_000.0));
        assert!(!update(
            &mut state,
            true,
            true,
            false,
            1_000.0 + RECOVERY_INTERVAL_MS - 1.0
        ));
        assert!(update(
            &mut state,
            true,
            true,
            false,
            1_000.0 + RECOVERY_INTERVAL_MS
        ));
        assert!(!update(
            &mut state,
            true,
            true,
            false,
            1_000.0 + RECOVERY_INTERVAL_MS + 1.0
        ));
    }

    #[test]
    fn full_stats_suspend_the_recovery_poll() {
        let mut state = RecoveryState::default();

        assert!(!update(&mut state, true, true, false, 1_000.0));
        assert!(!update(&mut state, false, true, false, 9_000.0));
        assert!(!update(&mut state, true, true, false, 10_000.0));
        assert!(!update(&mut state, true, true, false, 19_999.0));
        assert!(update(&mut state, true, true, false, 20_000.0));
    }

    #[test]
    fn busy_client_does_not_discard_an_overdue_poll() {
        let mut state = RecoveryState::default();

        assert!(!update(&mut state, true, true, false, 1_000.0));
        assert!(!update(&mut state, true, false, false, 11_000.0));
        assert!(update(&mut state, true, true, false, 11_001.0));
    }
}
