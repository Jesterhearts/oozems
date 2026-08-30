use std::cell::RefCell;
use std::rc::Rc;

use oozems_proto::v1::RespawnPlayerResponse;

use super::Game;
use crate::api;

pub(super) fn begin(
    game: Rc<RefCell<Game>>,
    permit: super::requests::RequestPermit,
) {
    game.borrow_mut().ui.death.respawn_in_flight = true;
    let player_id = game.borrow().player.id.clone();
    super::requests::spawn_request(
        game,
        permit,
        move || async move { api::respawn_player(&player_id).await },
        |game, result, request_started_ms| match result {
            Ok(response) => match install(game, response, request_started_ms) {
                Ok(()) => super::requests::RequestStatus::silent(),
                Err(error) => {
                    super::request_dispatch::require_bootstrap(game);
                    super::requests::RequestStatus::error(format!(
                        "Respawn succeeded, but the destination could not be loaded: {error}. \
                         Reloading the game to continue."
                    ))
                }
            },
            Err(error) => {
                if error.operation_outcome_unknown() {
                    super::request_dispatch::require_bootstrap(game);
                } else {
                    crate::death_ui::allow_retry(&mut game.ui.death);
                }
                super::requests::RequestStatus::error(format!(
                    "Respawn failed: {error}{}",
                    if error.operation_outcome_unknown() {
                        " Reloading to confirm the current map."
                    } else {
                        ""
                    }
                ))
            }
        },
    );
}

fn install(
    game: &mut Game,
    mut response: RespawnPlayerResponse,
    request_started_ms: f64,
) -> Result<(), String> {
    let (player, active_buffs) = super::responses::take_player_and_active_buffs(&mut response)?;
    let stats = api::require_data(player.stats.as_ref(), "character stats")
        .map_err(|error| error.to_string())?;
    if stats.hp == 0 {
        return Err("respawn response still contains a dead player".to_owned());
    }
    let map =
        api::require_data(response.map.take(), "respawn map").map_err(|error| error.to_string())?;
    let authoritative = api::require_data(response.authoritative.take(), "respawn position")
        .map_err(|error| error.to_string())?;
    let player_id = game.player.id.clone();
    let expected_map_id = map.id;
    let prepared = super::responses::prepare_relocation(
        player,
        active_buffs,
        map,
        authoritative,
        &player_id,
        expected_map_id,
    )?;
    super::movement_actions::install_prepared_relocation(game, prepared, request_started_ms)?;
    crate::death_ui::synchronize(&mut game.ui.death, false, game.clock.now_ms);
    Ok(())
}
