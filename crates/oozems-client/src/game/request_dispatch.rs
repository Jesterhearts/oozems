use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use oozems_proto::v1::CharacterSpriteSet;
use oozems_proto::v1::GameGui;
use oozems_proto::v1::MorphDefinition;
use oozems_proto::v1::MovementSnapshot;
use oozems_proto::v1::PlayerState;

use super::Game;
use super::cash_shop_actions;
use super::install_active_buffs;
use super::install_full_player_update;
use super::interaction_actions;
use super::item_actions;
use super::movement_actions;
use super::player_updates;
use super::player_updates::PlayerInstallation;
use super::player_updates::appearance_assets_are_eligible;
use super::player_updates::appearance_refresh;
use super::player_updates::install_revision;
use super::player_updates::visible_appearance_identity;
use super::recovery_actions;
use super::refresh;
use super::requests;
use super::respawn_actions;
use super::responses;
use super::runtime::PersistenceState;
use super::runtime::defer_transition;
use super::runtime::restart_character_animation;
use super::skill_actions;
use crate::api;
use crate::assets;
use crate::game_gui;
use crate::game_gui::GuiAction;
use crate::interaction_ui::InteractionUiAction;
use crate::movement::MapTransition;
use crate::show_status;

const APPEARANCE_RETRY_LIMIT: u8 = 2;
const APPEARANCE_RETRY_BACKOFF_MS: f64 = 1_000.0;
const MORPH_RETRY_BACKOFF_MS: f64 = 5_000.0;
const GUI_RETRY_BACKOFF_MS: f64 = 5_000.0;
pub(super) const SAVE_INTERVAL_MS: f64 = 2_000.0;

pub(super) struct RequestState {
    pub(super) admission: requests::RequestAdmission,
    pub(super) movement: movement_actions::MovementSyncState,
    pub(super) recovery: recovery_actions::RecoveryState,
    pub(super) deferred_transitions: VecDeque<PendingTransition>,
    pub(super) appearance: AppearanceRefreshState,
    pub(super) morph: MorphRefreshState,
    pub(super) gui: GuiRefreshState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct GuiRefreshRequest {
    pub(super) player_id: String,
    pub(super) item_ids: Vec<u32>,
}

#[derive(Clone, Debug, PartialEq)]
pub(super) struct PendingTransition {
    pub(super) source: MovementSnapshot,
    pub(super) transition: MapTransition,
}

pub(super) enum PendingRequest {
    Appearance(player_updates::AppearanceRefresh),
    Morph(u32),
    Gui(GuiRefreshRequest),
    CashShopOpen,
    CashShopPurchase(u32),
    InteractionOpen(u32),
    InteractionAction(InteractionUiAction),
    Item(GuiAction),
    PickUp,
    Movement,
    BasicAttack,
    Skill(GuiAction),
    Recovery,
    Respawn,
    Save(Box<PendingSave>),
    Transition(PendingTransition),
}

impl PendingRequest {
    fn kind(&self) -> requests::RequestKind {
        match self {
            Self::Appearance(_) => requests::RequestKind::Appearance,
            Self::Morph(_) => requests::RequestKind::Morph,
            Self::Gui(_) => requests::RequestKind::Gui,
            Self::CashShopOpen => requests::RequestKind::CashShopCatalog,
            Self::CashShopPurchase(_) => requests::RequestKind::CashShopPurchase,
            Self::InteractionOpen(_) | Self::InteractionAction(_) => {
                requests::RequestKind::Interaction
            }
            Self::Item(_) | Self::PickUp => requests::RequestKind::Item,
            Self::Movement => requests::RequestKind::Movement,
            Self::BasicAttack | Self::Skill(_) => requests::RequestKind::Skill,
            Self::Recovery => requests::RequestKind::Recovery,
            Self::Respawn => requests::RequestKind::Respawn,
            Self::Save(_) => requests::RequestKind::Save,
            Self::Transition(_) => requests::RequestKind::Transition,
        }
    }
}

#[derive(Default)]
pub(super) struct PendingRequests {
    pub(super) requests: Vec<PendingRequest>,
}

impl PendingRequests {
    pub(super) fn push(
        &mut self,
        request: PendingRequest,
    ) {
        self.requests.push(request);
    }

    pub(super) fn has_player_mutation(&self) -> bool {
        self.requests.iter().any(|request| {
            matches!(
                request.kind(),
                requests::RequestKind::Save
                    | requests::RequestKind::Item
                    | requests::RequestKind::Skill
                    | requests::RequestKind::Transition
                    | requests::RequestKind::Recovery
                    | requests::RequestKind::Respawn
                    | requests::RequestKind::CashShopPurchase
                    | requests::RequestKind::Interaction
            )
        })
    }

    pub(super) fn has_kind(
        &self,
        kind: requests::RequestKind,
    ) -> bool {
        self.requests.iter().any(|request| request.kind() == kind)
    }
}
pub(super) type AppearanceRefreshState = refresh::KeyedRefreshState<
    player_updates::AppearanceIdentity,
    player_updates::AppearanceRefresh,
    CharacterSpriteSet,
>;
pub(super) type MorphRefreshState = refresh::KeyedRefreshState<u32, u32, MorphDefinition>;
pub(super) type GuiRefreshState = refresh::KeyedRefreshState<(), GuiRefreshRequest, ()>;

pub(super) fn synchronize_morph(game: &mut Game) {
    let desired = game.player.active_buffs.morph_id;
    if desired.is_none() {
        game.world.morph_definition = None;
        update_morph_refresh_request(&mut game.requests.morph, None, false, game.clock.now_ms);
        return;
    }
    let desired = desired.expect("checked active morph");
    if game
        .world
        .morph_definition
        .as_ref()
        .is_some_and(|definition| definition.morph_id == desired)
    {
        update_morph_refresh_request(
            &mut game.requests.morph,
            Some(desired),
            true,
            game.clock.now_ms,
        );
        return;
    }
    if let Some(definition) = game.requests.morph.cached.get(&desired).cloned() {
        game.world.morph_definition = Some(definition);
        update_morph_refresh_request(
            &mut game.requests.morph,
            Some(desired),
            true,
            game.clock.now_ms,
        );
        return;
    }
    game.world.morph_definition = None;
    update_morph_refresh_request(
        &mut game.requests.morph,
        Some(desired),
        false,
        game.clock.now_ms,
    );
}

fn update_morph_refresh_request(
    state: &mut MorphRefreshState,
    desired: Option<u32>,
    ready: bool,
    now_ms: f64,
) {
    if state.pending != desired {
        state.pending = None;
    }
    let Some(desired) = desired.filter(|_| !ready) else {
        state.pending = None;
        return;
    };
    let retry_ready = state.retry_is_ready(&desired, now_ms);
    if state.in_flight != Some(desired) && retry_ready {
        state.pending = Some(desired);
    }
}

pub(super) fn queue_appearance_refresh(
    game: &mut Game,
    installed: PlayerInstallation,
) {
    if !installed.domains.inventory {
        return;
    }
    let Some(mut refresh) = appearance_refresh(&game.player.state) else {
        return;
    };
    refresh.revision = game.player.revisions.inventory;
    if let Some(in_flight) = game
        .requests
        .appearance
        .in_flight
        .as_mut()
        .filter(|in_flight| in_flight.identity == refresh.identity)
    {
        in_flight.revision = in_flight.revision.max(refresh.revision);
    }
    if let Some(pending) = game
        .requests
        .appearance
        .pending
        .as_mut()
        .filter(|pending| pending.identity == refresh.identity)
    {
        pending.revision = pending.revision.max(refresh.revision);
    }
    if !installed.visible_appearance_changed {
        return;
    }

    game.requests.appearance.pending = None;
    reset_appearance_retry(&mut game.requests.appearance);
    if let Some(cached) = game
        .requests
        .appearance
        .cached
        .get(&refresh.identity)
        .cloned()
    {
        if install_revision(
            &mut game.player.revisions.appearance_assets,
            refresh.revision,
        ) {
            game.world.character_sprites = cached;
            restart_character_animation(&mut game.world.character_animation, game.clock.now_ms);
        }
        return;
    }

    let same_request_is_in_flight = game
        .requests
        .appearance
        .in_flight
        .as_ref()
        .is_some_and(|in_flight| in_flight.identity == refresh.identity);
    if !same_request_is_in_flight {
        game.requests.appearance.pending = Some(refresh);
    }
}

fn reset_appearance_retry(state: &mut AppearanceRefreshState) {
    state.retry_count.clear();
    state.retry_after_ms.clear();
}

fn next_appearance_retry(
    completed_retries: u8,
    now_ms: f64,
) -> Option<(u8, f64)> {
    if completed_retries >= APPEARANCE_RETRY_LIMIT {
        return None;
    }
    let retry_count = completed_retries + 1;
    Some((
        retry_count,
        now_ms + APPEARANCE_RETRY_BACKOFF_MS * f64::from(retry_count),
    ))
}

fn schedule_appearance_retry(
    state: &mut AppearanceRefreshState,
    request: player_updates::AppearanceRefresh,
    now_ms: f64,
) -> bool {
    let completed_retries = state
        .retry_count
        .get(&request.identity)
        .copied()
        .unwrap_or_default();
    let Some((retry_count, retry_after_ms)) = next_appearance_retry(completed_retries, now_ms)
    else {
        return false;
    };
    state
        .retry_count
        .insert(request.identity.clone(), retry_count);
    state.delay_retry(request.identity.clone(), retry_after_ms);
    state.pending = Some(request);
    true
}

pub(super) fn collect_refresh_requests(
    game: &mut Game,
    pending: &mut PendingRequests,
) {
    if game.requests.appearance.in_flight.is_none()
        && let Some(request) = game.requests.appearance.pending.take()
    {
        if game
            .requests
            .appearance
            .retry_is_ready(&request.identity, game.clock.now_ms)
        {
            game.requests.appearance.in_flight = Some(request.clone());
            pending.push(PendingRequest::Appearance(request));
        } else {
            game.requests.appearance.pending = Some(request);
        }
    }
    if game.requests.morph.in_flight.is_none()
        && let Some(morph_id) = game.requests.morph.pending.take()
    {
        game.requests.morph.in_flight = Some(morph_id);
        pending.push(PendingRequest::Morph(morph_id));
    }
    let observed_item_ids = game
        .player
        .state
        .inventory
        .iter()
        .flat_map(|inventory| {
            inventory
                .stacks
                .iter()
                .map(|stack| stack.item_id)
                .chain(inventory.equipment.iter().map(|equipped| equipped.item_id))
        })
        .chain(game.world.map.dropped_items.iter().map(|drop| drop.item_id))
        .collect::<Vec<_>>();
    if let Some(request) = take_gui_refresh_request(
        &mut game.requests.gui,
        &game.ui.gui,
        observed_item_ids,
        &game.player.state.id,
        game.clock.now_ms,
    ) {
        pending.push(PendingRequest::Gui(request));
    }
}

fn take_gui_refresh_request(
    state: &mut GuiRefreshState,
    gui: &GameGui,
    observed_item_ids: Vec<u32>,
    player_id: &str,
    now_ms: f64,
) -> Option<GuiRefreshRequest> {
    if state.in_flight.is_some() || !state.retry_is_ready(&(), now_ms) {
        return None;
    }
    let request = state.pending.take().or_else(|| {
        game_gui::item_definition_refresh_ids(
            gui,
            observed_item_ids,
            !state.cached.contains_key(&()),
        )
        .map(|item_ids| GuiRefreshRequest {
            player_id: player_id.to_owned(),
            item_ids,
        })
    })?;
    state.in_flight = Some(request.clone());
    Some(request)
}

pub(super) fn dispatch_requests(
    game: Rc<RefCell<Game>>,
    pending: PendingRequests,
) {
    for request in pending.requests {
        let permit = game.borrow().requests.admission.admit(request.kind());
        let Some(permit) = permit else {
            request_not_admitted(&mut game.borrow_mut(), request);
            continue;
        };
        match request {
            PendingRequest::Appearance(request) => {
                begin_appearance_refresh(game.clone(), request, permit);
            }
            PendingRequest::Morph(morph_id) => begin_morph_refresh(game.clone(), morph_id, permit),
            PendingRequest::Gui(request) => begin_gui_refresh(game.clone(), request, permit),
            PendingRequest::CashShopOpen => cash_shop_actions::begin_open(game.clone(), permit),
            PendingRequest::CashShopPurchase(offer_id) => {
                cash_shop_actions::begin_purchase(game.clone(), offer_id, permit);
            }
            PendingRequest::InteractionOpen(npc_spawn_id) => {
                interaction_actions::begin_open(game.clone(), npc_spawn_id, permit);
            }
            PendingRequest::InteractionAction(action) => {
                interaction_actions::begin_action(game.clone(), action, permit);
            }
            PendingRequest::Item(action) => item_actions::begin(game.clone(), action, permit),
            PendingRequest::PickUp => item_actions::begin_pick_up(game.clone(), permit),
            PendingRequest::Movement => movement_actions::begin(game.clone(), permit),
            PendingRequest::BasicAttack => skill_actions::begin_basic_attack(game.clone(), permit),
            PendingRequest::Skill(action) => skill_actions::begin(game.clone(), action, permit),
            PendingRequest::Recovery => recovery_actions::begin(game.clone(), permit),
            PendingRequest::Respawn => respawn_actions::begin(game.clone(), permit),
            PendingRequest::Save(pending) => begin_save(game.clone(), *pending, permit),
            PendingRequest::Transition(transition) => {
                let player_id = game.borrow().player.id.clone();
                movement_actions::begin_portal(
                    game.clone(),
                    player_id,
                    transition.source,
                    transition.transition,
                    permit,
                );
            }
        }
    }
}

fn request_not_admitted(
    game: &mut Game,
    request: PendingRequest,
) {
    match request {
        PendingRequest::Appearance(request) => {
            game.requests.appearance.in_flight = None;
            game.requests.appearance.pending = Some(request);
        }
        PendingRequest::Morph(morph_id) => {
            game.requests.morph.in_flight = None;
            game.requests.morph.pending = Some(morph_id);
        }
        PendingRequest::Gui(request) => {
            game.requests.gui.in_flight = None;
            game.requests.gui.pending = Some(request);
        }
        PendingRequest::Save(_) => game.persistence.dirty = true,
        PendingRequest::Transition(transition) => {
            defer_transition(
                &mut game.requests.deferred_transitions,
                transition,
                game.world.map.id,
            );
        }
        PendingRequest::Respawn => {}
        PendingRequest::PickUp
        | PendingRequest::Movement
        | PendingRequest::BasicAttack
        | PendingRequest::Recovery => {}
        request => {
            if let Some(message) = request_not_admitted_message(&request) {
                show_status(message, true);
            }
        }
    }
}

fn request_not_admitted_message(request: &PendingRequest) -> Option<&'static str> {
    match request {
        PendingRequest::CashShopOpen => Some("A Cash Shop request is already in progress."),
        PendingRequest::CashShopPurchase(_)
        | PendingRequest::InteractionOpen(_)
        | PendingRequest::InteractionAction(_)
        | PendingRequest::Item(_)
        | PendingRequest::Skill(_) => Some("Another player update is already in progress."),
        _ => None,
    }
}

fn begin_gui_refresh(
    game: Rc<RefCell<Game>>,
    request: GuiRefreshRequest,
    permit: requests::RequestPermit,
) {
    requests::spawn_request(
        game,
        permit,
        move || async move {
            let gui = api::get_gui(&request.player_id, request.item_ids)
                .await
                .map_err(|error| error.to_string())?;
            let images = assets::prepare_assets(gui.assets.iter())?;
            Ok((gui, images))
        },
        |game, result, _| {
            game.requests.gui.in_flight = None;
            match result {
                Ok((gui, images)) => {
                    game.ui.gui = gui;
                    assets::merge_assets(&mut game.surface.images, images);
                    game.requests.gui.cached.insert((), ());
                    game.requests.gui.clear_retry(&());
                    requests::RequestStatus::silent()
                }
                Err(error) => {
                    game.requests
                        .gui
                        .delay_retry((), game.clock.now_ms + GUI_RETRY_BACKOFF_MS);
                    requests::RequestStatus::error(format!("Item metadata refresh failed: {error}"))
                }
            }
        },
    );
}

fn begin_appearance_refresh(
    game: Rc<RefCell<Game>>,
    request: player_updates::AppearanceRefresh,
    permit: requests::RequestPermit,
) {
    let requested_identity = request.identity.clone();
    requests::spawn_request(
        game,
        permit,
        move || async move {
            let sprites = api::get_character_sprites(request.appearance, Some(&request.equipment))
                .await
                .map_err(|error| error.to_string())?;
            let images = assets::prepare_assets(sprites.assets.iter())?;
            Ok((sprites, images))
        },
        move |game, result, _| {
            let Some(active) = game.requests.appearance.in_flight.take() else {
                return requests::RequestStatus::silent();
            };
            if active.identity != requested_identity {
                return requests::RequestStatus::silent();
            }
            let current_identity = visible_appearance_identity(&game.player.state);
            let eligible = appearance_assets_are_eligible(
                current_identity.as_ref(),
                &active,
                game.player.revisions.inventory,
                game.player.revisions.appearance_assets,
            );
            match result {
                Ok((sprites, images)) if eligible => {
                    if install_revision(
                        &mut game.player.revisions.appearance_assets,
                        active.revision,
                    ) {
                        assets::merge_assets(&mut game.surface.images, images);
                        game.requests
                            .appearance
                            .cached
                            .insert(active.identity, sprites.clone());
                        game.world.character_sprites = sprites;
                        restart_character_animation(
                            &mut game.world.character_animation,
                            game.clock.now_ms,
                        );
                        reset_appearance_retry(&mut game.requests.appearance);
                    }
                    requests::RequestStatus::silent()
                }
                Err(error) if eligible => {
                    let retrying = schedule_appearance_retry(
                        &mut game.requests.appearance,
                        active,
                        game.clock.now_ms,
                    );
                    let suffix = if retrying { "; retrying" } else { "" };
                    requests::RequestStatus::error(format!(
                        "Character appearance could not refresh: {error}{suffix}"
                    ))
                }
                Ok(_) | Err(_) => requests::RequestStatus::silent(),
            }
        },
    );
}

fn begin_morph_refresh(
    game: Rc<RefCell<Game>>,
    morph_id: u32,
    permit: requests::RequestPermit,
) {
    requests::spawn_request(
        game,
        permit,
        move || async move {
            let definition = api::get_morph(morph_id)
                .await
                .map_err(|error| error.to_string())?;
            if definition.morph_id != morph_id {
                return Err("server returned a different morph definition".to_owned());
            }
            let images = assets::prepare_assets(definition.assets.iter())?;
            Ok((definition, images))
        },
        move |game, result, _| {
            if game.requests.morph.in_flight.take() != Some(morph_id) {
                return requests::RequestStatus::silent();
            }
            let status = match result {
                Ok((definition, images)) => {
                    assets::merge_assets(&mut game.surface.images, images);
                    game.requests
                        .morph
                        .cached
                        .insert(morph_id, definition.clone());
                    game.requests.morph.clear_retry(&morph_id);
                    if game.player.active_buffs.morph_id == Some(morph_id) {
                        game.world.morph_definition = Some(definition);
                    }
                    requests::RequestStatus::silent()
                }
                Err(error) if game.player.active_buffs.morph_id == Some(morph_id) => {
                    game.requests
                        .morph
                        .delay_retry(morph_id, game.clock.now_ms + MORPH_RETRY_BACKOFF_MS);
                    requests::RequestStatus::error(format!("Morph could not load: {error}"))
                }
                Err(_) => requests::RequestStatus::silent(),
            };
            synchronize_morph(game);
            status
        },
    );
}

pub(super) struct PendingSave {
    player: PlayerState,
    key_bindings_generation: u64,
}

pub(super) fn save_if_due(
    state: &mut PersistenceState,
    admission: &requests::RequestAdmission,
    player: &PlayerState,
    key_bindings_generation: u64,
    can_submit: bool,
    timestamp_ms: f64,
) -> Option<PendingSave> {
    if !state.dirty
        || timestamp_ms < state.next_save_ms
        || !can_submit
        || admission.player_mutation_is_active()
    {
        return None;
    }

    state.dirty = false;
    state.next_save_ms = timestamp_ms + SAVE_INTERVAL_MS;
    Some(PendingSave {
        player: player.clone(),
        key_bindings_generation,
    })
}

fn begin_save(
    game: Rc<RefCell<Game>>,
    pending: PendingSave,
    permit: requests::RequestPermit,
) {
    requests::spawn_request(
        game,
        permit,
        move || async move {
            let mut response = api::save_player(pending.player)
                .await
                .map_err(|error| error.to_string())?;
            let update = responses::take_player_and_active_buffs(&mut response)?;
            Ok((update, pending.key_bindings_generation))
        },
        |game, result, request_started_ms| match result {
            Ok(((player, active_buffs), key_bindings_generation)) => {
                if game.player.key_bindings.generation == key_bindings_generation {
                    game.player.key_bindings.pending = false;
                }
                install_full_player_update(game, player);
                install_active_buffs(game, active_buffs, request_started_ms);
                requests::RequestStatus::silent()
            }
            Err(error) => {
                game.persistence.dirty = true;
                requests::RequestStatus::error(format!("Save failed: {error}"))
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::GameGui;

    use super::GuiRefreshRequest;
    use super::GuiRefreshState;
    use super::MorphRefreshState;
    use super::next_appearance_retry;
    use super::request_not_admitted_message;
    use super::take_gui_refresh_request;
    use super::update_morph_refresh_request;
    use crate::game_gui::GuiAction;

    #[test]
    fn cash_shop_conflicts_distinguish_catalog_and_player_updates() {
        assert_eq!(
            request_not_admitted_message(&super::PendingRequest::CashShopOpen),
            Some("A Cash Shop request is already in progress.")
        );
        assert_eq!(
            request_not_admitted_message(&super::PendingRequest::CashShopPurchase(4)),
            Some("Another player update is already in progress.")
        );
        assert_eq!(
            request_not_admitted_message(&super::PendingRequest::Item(GuiAction::OpenCashShop)),
            Some("Another player update is already in progress.")
        );
    }

    #[test]
    fn missing_initial_gui_requests_observed_items_immediately() {
        let mut state = GuiRefreshState::default();

        assert_eq!(
            take_gui_refresh_request(
                &mut state,
                &GameGui::default(),
                vec![4, 0, 2, 4],
                "player",
                0.0,
            ),
            Some(GuiRefreshRequest {
                player_id: "player".to_owned(),
                item_ids: vec![2, 4],
            })
        );
    }

    #[test]
    fn failed_gui_refresh_retries_at_five_second_deadline() {
        let mut state = GuiRefreshState::default();
        state.delay_retry((), 5_000.0);

        assert_eq!(
            take_gui_refresh_request(&mut state, &GameGui::default(), vec![2], "player", 4_999.0,),
            None
        );
        assert_eq!(
            take_gui_refresh_request(&mut state, &GameGui::default(), vec![2], "player", 5_000.0,),
            Some(GuiRefreshRequest {
                player_id: "player".to_owned(),
                item_ids: vec![2],
            })
        );
    }

    #[test]
    fn valid_gui_only_refreshes_when_observed_item_metadata_is_missing() {
        let mut state = GuiRefreshState::default();
        state.cached.insert((), ());

        assert_eq!(
            take_gui_refresh_request(&mut state, &GameGui::default(), Vec::new(), "player", 0.0,),
            None
        );
        assert_eq!(
            take_gui_refresh_request(&mut state, &GameGui::default(), vec![8], "player", 0.0,),
            Some(GuiRefreshRequest {
                player_id: "player".to_owned(),
                item_ids: vec![8],
            })
        );
    }

    #[test]
    fn morph_a_b_a_race_clears_the_stale_b_request() {
        let mut state = MorphRefreshState {
            in_flight: Some(4),
            ..MorphRefreshState::default()
        };

        update_morph_refresh_request(&mut state, Some(40), false, 100.0);
        assert_eq!(state.pending, Some(40));
        update_morph_refresh_request(&mut state, Some(4), false, 101.0);

        assert_eq!(state.pending, None);
        assert_eq!(state.in_flight, Some(4));
    }

    #[test]
    fn failed_morph_requests_wait_for_the_retry_deadline() {
        let mut state = MorphRefreshState::default();
        state.retry_after_ms.insert(4, 5_000.0);

        update_morph_refresh_request(&mut state, Some(4), false, 4_999.0);
        assert_eq!(state.pending, None);
        update_morph_refresh_request(&mut state, Some(4), false, 5_000.0);
        assert_eq!(state.pending, Some(4));
    }

    #[test]
    fn appearance_retries_are_delayed_and_bounded() {
        assert_eq!(next_appearance_retry(0, 100.0), Some((1, 1_100.0)));
        assert_eq!(next_appearance_retry(1, 1_100.0), Some((2, 3_100.0)));
        assert_eq!(next_appearance_retry(2, 3_100.0), None);
    }
}
