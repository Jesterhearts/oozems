use std::collections::VecDeque;

use oozems_proto::v1::GameGui;
use oozems_proto::v1::KeyAction;
use oozems_proto::v1::Map;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::Vec2;

use super::Game;
use super::WorldRuntime;
use super::buffs;
use super::input::apply_canvas_input;
use super::input::interaction_is_busy;
use super::movement_actions;
use super::recovery_actions;
use super::request_dispatch::PendingRequest;
use super::request_dispatch::PendingRequests;
use super::request_dispatch::PendingTransition;
use super::request_dispatch::collect_refresh_requests;
use super::request_dispatch::save_key_bindings_if_due;
use super::request_dispatch::synchronize_morph;
use super::requests;
use crate::audio;
use crate::audio::MapSound;
use crate::cash_shop_ui::CashShopState;
use crate::character_render::CharacterAnimation;
use crate::game_gui;
use crate::game_gui::GuiAction;
use crate::game_gui::GuiState;
use crate::interaction_ui::InteractionState;
use crate::keymap;
use crate::keymap::BindingTarget;
use crate::keymap::FrameInput;
use crate::level_up_effect;
use crate::movement;
use crate::movement::MapTransition;
use crate::movement::MotionState;
use crate::movement::PlayerInput;
use crate::skill_effects;

pub(super) struct KeyBindingSaveState {
    pub(super) dirty: bool,
    pub(super) next_save_ms: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharacterAnimationState {
    pub animation: CharacterAnimation,
    started_ms: f64,
    paused_at_ms: Option<f64>,
    one_shot_until_ms: Option<f64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MovementObservationMode {
    Active,
    Stationary,
    Paused,
}

impl MovementObservationMode {
    fn enabled(self) -> bool {
        self != Self::Paused
    }
}

pub(super) fn update(
    game: &mut Game,
    timestamp_ms: f64,
) -> PendingRequests {
    let elapsed_seconds = if game.clock.last_frame_ms == 0.0 {
        0.0
    } else {
        ((timestamp_ms - game.clock.last_frame_ms) / 1_000.0).clamp(0.0, 0.05) as f32
    };
    game.clock.last_frame_ms = timestamp_ms;
    game.clock.now_ms = timestamp_ms;
    match crate::api::take_recovery_requirement() {
        Some(crate::api::ClientRecovery::Bootstrap) => {
            super::request_dispatch::require_bootstrap(game);
        }
        Some(crate::api::ClientRecovery::ServerRestart) => {
            super::request_dispatch::require_server_restart(game);
        }
        None => {}
    }
    if std::mem::take(&mut game.requests.bootstrap_reload_pending) {
        match crate::reload_client() {
            Ok(crate::AutomaticReload::Started) => {}
            Ok(crate::AutomaticReload::Suppressed) => crate::show_status(
                "Automatic recovery was paused to prevent a reload loop. Check the connection and \
                 close other game tabs, then reload this page.",
                true,
            ),
            Err(error) => crate::show_status(
                &format!("Reload required, but automatic reload failed: {error}"),
                true,
            ),
        }
    }
    crate::item_pickup::update(&mut game.world.pickup_animations, timestamp_ms);
    level_up_effect::update(
        &mut game.ui.level_up,
        &game.ui.gui.level_up_frames,
        &game.surface.images,
        timestamp_ms,
    );
    {
        let mut audio_state = game.audio.borrow_mut();
        audio::update(&mut audio_state, timestamp_ms);
        skill_effects::update(
            &mut game.world.skill_effect_state,
            &game.surface.images,
            &mut audio_state,
            timestamp_ms,
        );
    }
    let now_ms = js_sys::Date::now().max(0.0) as u64;
    game.world
        .map
        .dropped_items
        .retain(|drop| crate::item_pickup::is_active(drop, now_ms));

    let dead = player_is_dead(&game.player.state);
    let death_started = dead && !crate::death_ui::is_open(game.ui.death);
    crate::death_ui::synchronize(&mut game.ui.death, dead, timestamp_ms);
    if death_started {
        super::play_map_sound(game, MapSound::Tombstone);
        game.world.active_setup_item_id = None;
        let mut gui = game.ui.gui_state.borrow_mut();
        gui.stats_open = false;
        gui.equipment_open = false;
        gui.inventory_open = false;
        gui.key_config_open = false;
        gui.skills_open = false;
        drop(gui);
        game.ui.cash_shop.close();
        game.ui.interaction.close();
        game.ui.key_drag = None;
        game.ui.window_drag = None;
        super::input::clear_suppressed_click(&mut game.input);
    }
    let mut pending = PendingRequests::default();
    if let Some(action) = game.requests.deferred_taxi.take() {
        pending.push(PendingRequest::InteractionAction(action));
    }
    apply_canvas_input(game, &mut pending);
    super::synchronize_mouse_cursor(game);
    if crate::death_ui::should_dispatch_respawn(game.ui.death)
        && !pending.has_kind(requests::RequestKind::Respawn)
    {
        pending.push(PendingRequest::Respawn);
    }

    let (mut input, escape_target) = {
        let bindings = game.player.key_bindings.current.borrow();
        (
            keymap::drain_frame_input(&mut game.input.keyboard.borrow_mut(), &bindings),
            keymap::target_for_code(&bindings, "Escape"),
        )
    };
    if apply_escape(
        &mut input,
        escape_target,
        &mut game.ui.cash_shop,
        &mut game.ui.interaction,
        &mut game.ui.gui_state.borrow_mut(),
    ) {
        game.ui.key_drag = None;
        game.ui.window_drag = None;
        super::input::clear_suppressed_click(&mut game.input);
    }
    let key_config_open = game.ui.gui_state.borrow().key_config_open;
    if key_config_open {
        input.player = PlayerInput::default();
        input.skills.clear();
        input
            .actions
            .retain(|action| *action == KeyAction::OpenKeyConfig);
    }
    let interaction_busy = interaction_is_busy(game)
        || pending.has_kind(requests::RequestKind::Interaction)
        || pending.has_kind(requests::RequestKind::Taxi);
    let cash_shop_open =
        game.ui.cash_shop.open || pending.has_kind(requests::RequestKind::CashShopCatalog);
    if interaction_busy || cash_shop_open {
        input.player = PlayerInput::default();
        input.skills.clear();
        input.actions.clear();
    }
    if dead {
        input.player = PlayerInput::default();
        input.skills.clear();
        input.actions.clear();
    }
    let pick_up = apply_key_actions(
        &mut game.ui.gui_state.borrow_mut(),
        &game.ui.gui,
        &input.actions,
    );
    buffs::apply(
        &mut game.player.active_buffs,
        &mut input.player,
        timestamp_ms,
    );
    synchronize_morph(game);

    if game.player.active_buffs.attacks_disabled {
        input
            .actions
            .retain(|action| *action != KeyAction::BasicAttack);
        input.skills.clear();
    }
    let basic_attack_requested = input.actions.contains(&KeyAction::BasicAttack);
    if pick_up || basic_attack_requested || !input.skills.is_empty() {
        game.world.active_setup_item_id = None;
    }

    let transition_active = game.requests.requires_bootstrap
        || game
            .requests
            .admission
            .is_active(requests::RequestKind::Transition);
    let movement = if transition_active || dead {
        PlayerMovement::default()
    } else {
        update_player(
            &mut game.player.state,
            &mut game.world,
            &mut game.requests.movement,
            game.clock.now_ms,
            elapsed_seconds,
            input.player,
        )
    };
    if movement.jumped {
        super::play_map_sound(game, MapSound::Jump);
    }
    let selected_transition = movement.transition;
    if dead {
        update_character_animation(
            &mut game.world.character_animation,
            CharacterAnimation::Death,
            true,
            timestamp_ms,
        );
    }
    let selected_transition = selected_transition.and_then(|transition| {
        let observation = movement_actions::observation(
            game.world.map.id,
            game.player.state.position,
            game.world.motion,
        );
        movement_actions::capture_movement_snapshot(&mut game.requests.movement, observation)
            .map(|source| PendingTransition { source, transition })
    });
    if dead {
        game.requests.deferred_transitions.clear();
    }
    let transition = select_transition_request(
        &mut game.requests.deferred_transitions,
        selected_transition,
        game.world.map.id,
        dead || transition_active
            || pending.has_player_mutation()
            || game.requests.admission.player_mutation_is_active(),
    );
    let transition_pending = transition.is_some() || !game.requests.deferred_transitions.is_empty();
    let movement_observation_mode = movement_observation_mode(
        dead,
        transition_pending || game.ui.death.respawn_requested,
        transition_active,
    );
    let (basic_attack, skill_id) = select_combat_requests(
        game.player.active_buffs.attacks_disabled || dead,
        transition_pending,
        transition_active,
        character_attack_is_active(game.world.character_animation, timestamp_ms),
        basic_attack_requested,
        input.skills.into_iter().next(),
    );
    let movement_observation = movement_actions::observation(
        game.world.map.id,
        game.player.state.position,
        game.world.motion,
    );
    let movement_snapshot = movement_actions::update(
        &mut game.requests.movement,
        game.world.movement_rules.snapshot_interval_ms,
        movement_observation_mode.enabled(),
        game.requests
            .admission
            .is_active(requests::RequestKind::Movement),
        timestamp_ms,
        movement_observation,
    );
    let periodic_recovery = game.player.active_buffs.has_periodic_hp_recovery();
    let (needs_recovery, needs_periodic_recovery) =
        game.player
            .state
            .stats
            .as_ref()
            .map_or((false, false), |stats| {
                (
                    !dead && (stats.hp < stats.max_hp || stats.mp < stats.max_mp),
                    !dead && periodic_recovery && stats.hp < stats.max_hp,
                )
            });
    let can_poll_recovery = matches!(
        game.world.character_animation.animation,
        CharacterAnimation::Idle | CharacterAnimation::Sit
    ) && !pick_up
        && !basic_attack
        && skill_id.is_none()
        && transition.is_none()
        && !pending.has_player_mutation()
        && !game.requests.admission.player_mutation_is_active()
        && !interaction_busy
        && !cash_shop_open;
    let recover = recovery_actions::update(
        &mut game.requests.recovery,
        needs_recovery,
        can_poll_recovery,
        game.requests
            .admission
            .is_active(requests::RequestKind::Recovery),
        timestamp_ms,
        if needs_periodic_recovery {
            recovery_actions::PERIODIC_RECOVERY_INTERVAL_MS
        } else {
            10_000.0
        },
    );
    let save = save_key_bindings_if_due(
        &mut game.key_binding_save,
        &game.requests.admission,
        &game.player.state.id,
        &game.player.state.key_bindings,
        game.player.key_bindings.generation,
        !pending.has_player_mutation()
            && !pick_up
            && !basic_attack
            && skill_id.is_none()
            && !transition_pending
            && !recover,
        timestamp_ms,
    );

    let mut requests = PendingRequests::default();
    collect_refresh_requests(game, &mut requests);
    requests.requests.extend(pending.requests);
    if pick_up {
        requests.push(PendingRequest::PickUp);
    }
    if movement_snapshot {
        requests.push(PendingRequest::Movement);
    }
    if basic_attack {
        requests.push(PendingRequest::BasicAttack);
    }
    if let Some(skill_id) = skill_id {
        requests.push(PendingRequest::Skill(GuiAction::UseSkill { skill_id }));
    }
    if recover {
        requests.push(PendingRequest::Recovery);
    }
    if let Some(save) = save {
        requests.push(PendingRequest::KeyBindingSave(Box::new(save)));
    }
    if let Some(transition) = transition {
        requests.push(PendingRequest::Transition(transition));
    }
    requests
}

fn apply_escape(
    input: &mut FrameInput,
    escape_target: Option<BindingTarget>,
    cash_shop: &mut CashShopState,
    interaction: &mut InteractionState,
    gui: &mut GuiState,
) -> bool {
    if !input.escape_pressed || !close_topmost_ui(cash_shop, interaction, gui) {
        return false;
    }
    match escape_target {
        Some(BindingTarget::Action(action)) => {
            input.actions.retain(|candidate| *candidate != action);
            if action == KeyAction::Jump {
                input.player.jump_pressed = false;
            }
        }
        Some(BindingTarget::Skill(skill_id)) => {
            input.skills.retain(|candidate| *candidate != skill_id);
        }
        None => {}
    }
    true
}

fn close_topmost_ui(
    cash_shop: &mut CashShopState,
    interaction: &mut InteractionState,
    gui: &mut GuiState,
) -> bool {
    if cash_shop.open {
        cash_shop.close();
        return true;
    }
    if interaction.is_open() {
        interaction.close();
        return true;
    }
    game_gui::close_topmost_window(gui)
}

fn movement_observation_mode(
    dead: bool,
    transition_pending: bool,
    transition_active: bool,
) -> MovementObservationMode {
    if transition_pending || transition_active {
        MovementObservationMode::Paused
    } else if dead {
        MovementObservationMode::Stationary
    } else {
        MovementObservationMode::Active
    }
}

fn select_combat_requests(
    attacks_disabled: bool,
    transition_selected: bool,
    transition_active: bool,
    character_attack_active: bool,
    basic_attack_requested: bool,
    skill_id: Option<u32>,
) -> (bool, Option<u32>) {
    let combat_allowed = !attacks_disabled && !transition_selected && !transition_active;
    (
        combat_allowed && !character_attack_active && basic_attack_requested,
        combat_allowed.then_some(skill_id).flatten(),
    )
}

pub(super) fn defer_transition(
    deferred: &mut VecDeque<PendingTransition>,
    request: PendingTransition,
    current_map_id: u32,
) {
    let already_deferred = deferred.iter().any(|pending| {
        pending.source.map_id == request.source.map_id && pending.transition == request.transition
    });
    if request.source.map_id == current_map_id && !already_deferred {
        deferred.push_back(request);
    }
}

fn select_transition_request(
    deferred: &mut VecDeque<PendingTransition>,
    selected: Option<PendingTransition>,
    current_map_id: u32,
    blocked: bool,
) -> Option<PendingTransition> {
    deferred.retain(|request| request.source.map_id == current_map_id);
    if let Some(transition) = selected {
        defer_transition(deferred, transition, current_map_id);
    }
    (!blocked).then(|| deferred.pop_front()).flatten()
}

fn apply_key_actions(
    state: &mut GuiState,
    gui: &GameGui,
    actions: &[KeyAction],
) -> bool {
    let mut pick_up = false;
    for action in actions {
        let gui_action = match action {
            KeyAction::Jump => continue,
            KeyAction::BasicAttack => continue,
            KeyAction::PickUp => {
                pick_up = true;
                continue;
            }
            KeyAction::OpenCharacter => GuiAction::ToggleStats,
            KeyAction::OpenEquipment => GuiAction::ToggleEquipment,
            KeyAction::OpenInventory => GuiAction::ToggleInventory,
            KeyAction::OpenSkills if gui.skill_window.is_some() => GuiAction::ToggleSkills,
            KeyAction::OpenSkills => continue,
            KeyAction::OpenKeyConfig if gui.key_config_window.is_some() => {
                GuiAction::ToggleKeyConfig
            }
            KeyAction::OpenKeyConfig => continue,
            KeyAction::Unspecified => continue,
        };
        let _ = game_gui::apply_local_action(state, gui_action);
    }
    pick_up
}

#[derive(Default)]
struct PlayerMovement {
    transition: Option<MapTransition>,
    jumped: bool,
}

fn update_player(
    player: &mut PlayerState,
    world: &mut WorldRuntime,
    movement_sync: &mut movement_actions::MovementSyncState,
    now_ms: f64,
    elapsed_seconds: f32,
    input: PlayerInput,
) -> PlayerMovement {
    let Some(position) = player.position else {
        return PlayerMovement::default();
    };
    let previous_motion = world.motion;
    if input.horizontal != 0.0 {
        world.facing_left = input.horizontal < 0.0;
    }
    let output = movement::update_player(
        &world.map,
        &world.movement_rules,
        position,
        world.motion,
        input,
        elapsed_seconds,
    );
    if output.dropped_through {
        movement_actions::record_drop_through(
            movement_sync,
            world.map.id,
            position,
            previous_motion,
        );
    }
    let setup_remains_active =
        setup_item_remains_active(world.active_setup_item_id, output.state, input);
    if !setup_remains_active {
        world.active_setup_item_id = None;
    }
    let animation = if world.active_setup_item_id.is_some() {
        CharacterAnimation::Sit
    } else {
        character_animation(&world.map, output.state, input)
    };
    let animation_plays = character_animation_plays(animation, position, output.position);
    update_character_animation(
        &mut world.character_animation,
        animation,
        animation_plays,
        now_ms,
    );
    player.position = Some(output.position);
    world.motion = output.state;
    PlayerMovement {
        jumped: jump_started(previous_motion, input, &output),
        transition: output.transition,
    }
}

fn jump_started(
    previous: MotionState,
    input: PlayerInput,
    output: &movement::MotionOutput,
) -> bool {
    input.jump_pressed
        && (previous.on_ground || previous.climbing.is_some())
        && output.state.velocity_y < 0.0
        && !output.dropped_through
        && output.transition.is_none()
}

fn setup_item_remains_active(
    item_id: Option<u32>,
    motion: MotionState,
    input: PlayerInput,
) -> bool {
    item_id.is_some()
        && motion.on_ground
        && motion.climbing.is_none()
        && input.horizontal == 0.0
        && input.vertical == 0.0
        && !input.jump_pressed
        && !input.portal_pressed
}

pub(super) fn character_animation(
    map: &Map,
    state: MotionState,
    input: PlayerInput,
) -> CharacterAnimation {
    if let Some(index) = state.climbing {
        match map.ladders.get(index) {
            Some(feature) if feature.is_ladder => CharacterAnimation::Ladder,
            Some(_) => CharacterAnimation::Rope,
            None => CharacterAnimation::Idle,
        }
    } else if !state.on_ground {
        CharacterAnimation::Jump
    } else if input.horizontal != 0.0 {
        CharacterAnimation::Walk
    } else {
        CharacterAnimation::Idle
    }
}

pub(super) fn player_is_dead(player: &PlayerState) -> bool {
    player.stats.as_ref().is_some_and(|stats| stats.hp == 0)
}

pub(super) fn new_character_animation_state(
    animation: CharacterAnimation,
    plays: bool,
    timestamp_ms: f64,
) -> CharacterAnimationState {
    CharacterAnimationState {
        animation,
        started_ms: timestamp_ms,
        paused_at_ms: (!plays).then_some(timestamp_ms),
        one_shot_until_ms: None,
    }
}

fn character_animation_plays(
    animation: CharacterAnimation,
    previous_position: Vec2,
    next_position: Vec2,
) -> bool {
    !matches!(
        animation,
        CharacterAnimation::Ladder | CharacterAnimation::Rope
    ) || previous_position.y != next_position.y
}

pub(super) fn update_character_animation(
    state: &mut CharacterAnimationState,
    next: CharacterAnimation,
    plays: bool,
    timestamp_ms: f64,
) {
    if next == CharacterAnimation::Death && state.animation != CharacterAnimation::Death {
        *state = new_character_animation_state(next, true, timestamp_ms);
        return;
    }
    if state
        .one_shot_until_ms
        .is_some_and(|deadline_ms| timestamp_ms < deadline_ms)
    {
        return;
    }
    state.one_shot_until_ms = None;
    if state.animation != next {
        *state = new_character_animation_state(next, plays, timestamp_ms);
        return;
    }
    match (state.paused_at_ms, plays) {
        (Some(paused_at_ms), true) => {
            state.started_ms += (timestamp_ms - paused_at_ms).max(0.0);
            state.paused_at_ms = None;
        }
        (None, false) => state.paused_at_ms = Some(timestamp_ms),
        _ => {}
    }
}

pub(super) fn start_character_attack_animation(
    world: &mut WorldRuntime,
    now_ms: f64,
) {
    world.active_setup_item_id = None;
    let duration_ms = crate::character_render::animation_duration_ms(
        &world.character_sprites,
        CharacterAnimation::Attack,
    );
    world.character_animation =
        new_character_animation_state(CharacterAnimation::Attack, true, now_ms);
    world.character_animation.one_shot_until_ms = Some(now_ms + duration_ms.max(1) as f64);
}

fn character_attack_is_active(
    state: CharacterAnimationState,
    timestamp_ms: f64,
) -> bool {
    state.animation == CharacterAnimation::Attack
        && state
            .one_shot_until_ms
            .is_some_and(|deadline_ms| timestamp_ms < deadline_ms)
}

pub(super) fn restart_character_animation(
    state: &mut CharacterAnimationState,
    timestamp_ms: f64,
) {
    if state
        .one_shot_until_ms
        .is_some_and(|deadline_ms| timestamp_ms < deadline_ms)
    {
        return;
    }
    state.one_shot_until_ms = None;
    state.started_ms = timestamp_ms;
    state.paused_at_ms = state.paused_at_ms.map(|_| timestamp_ms);
}

pub(crate) fn character_animation_elapsed_ms(
    state: CharacterAnimationState,
    timestamp_ms: f64,
) -> f64 {
    (state.paused_at_ms.unwrap_or(timestamp_ms) - state.started_ms).max(0.0)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use oozems_proto::v1::CharacterStats;
    use oozems_proto::v1::GameGui;
    use oozems_proto::v1::GuiWindow;
    use oozems_proto::v1::KeyAction;
    use oozems_proto::v1::KeyBinding;
    use oozems_proto::v1::Ladder;
    use oozems_proto::v1::Map;
    use oozems_proto::v1::MovementContact;
    use oozems_proto::v1::MovementMode;
    use oozems_proto::v1::MovementSnapshot;
    use oozems_proto::v1::NpcInteraction;
    use oozems_proto::v1::PlayerState;
    use oozems_proto::v1::Vec2;

    use super::MovementObservationMode;
    use super::apply_escape;
    use super::apply_key_actions;
    use super::character_animation;
    use super::character_animation_elapsed_ms;
    use super::character_animation_plays;
    use super::character_attack_is_active;
    use super::close_topmost_ui;
    use super::jump_started;
    use super::movement_observation_mode;
    use super::new_character_animation_state;
    use super::player_is_dead;
    use super::restart_character_animation;
    use super::select_combat_requests;
    use super::select_transition_request;
    use super::setup_item_remains_active;
    use super::update_character_animation;

    #[test]
    fn zero_health_is_the_authoritative_death_state() {
        let mut player = PlayerState {
            stats: Some(CharacterStats {
                hp: 1,
                ..CharacterStats::default()
            }),
            ..PlayerState::default()
        };
        assert!(!player_is_dead(&player));

        player.stats.as_mut().expect("stats").hp = 0;
        assert!(player_is_dead(&player));
    }

    #[test]
    fn setup_items_remain_active_only_while_stationary_and_grounded() {
        let grounded = MotionState {
            on_ground: true,
            ..MotionState::default()
        };
        assert!(setup_item_remains_active(
            Some(3_010_072),
            grounded,
            PlayerInput::default()
        ));
        assert!(!setup_item_remains_active(
            Some(3_010_072),
            grounded,
            PlayerInput {
                horizontal: 1.0,
                ..PlayerInput::default()
            }
        ));
        assert!(!setup_item_remains_active(
            Some(3_010_072),
            MotionState::default(),
            PlayerInput::default()
        ));
        assert!(!setup_item_remains_active(
            None,
            grounded,
            PlayerInput::default()
        ));
    }

    #[test]
    fn jump_sound_requires_a_supported_jump_transition() {
        let jump = PlayerInput {
            jump_pressed: true,
            ..PlayerInput::default()
        };
        let ascending = MotionOutput {
            position: Vec2::default(),
            state: MotionState {
                velocity_y: -100.0,
                ..MotionState::default()
            },
            transition: None,
            dropped_through: false,
        };

        assert!(jump_started(
            MotionState {
                on_ground: true,
                ..MotionState::default()
            },
            jump,
            &ascending,
        ));
        assert!(jump_started(
            MotionState {
                climbing: Some(0),
                ..MotionState::default()
            },
            jump,
            &ascending,
        ));
        assert!(!jump_started(MotionState::default(), jump, &ascending));

        let dropped = MotionOutput {
            dropped_through: true,
            ..ascending
        };
        assert!(!jump_started(
            MotionState {
                on_ground: true,
                ..MotionState::default()
            },
            jump,
            &dropped,
        ));
    }

    #[test]
    fn dead_players_keep_stationary_observations_until_a_transition_starts() {
        assert_eq!(
            movement_observation_mode(true, false, false),
            MovementObservationMode::Stationary
        );
        assert!(movement_observation_mode(true, false, false).enabled());
        assert_eq!(
            movement_observation_mode(true, true, false),
            MovementObservationMode::Paused
        );
        assert_eq!(
            movement_observation_mode(true, false, true),
            MovementObservationMode::Paused
        );
    }
    use crate::cash_shop_ui::CashShopState;
    use crate::character_render::CharacterAnimation;
    use crate::game::request_dispatch::PendingTransition;
    use crate::game_gui::GuiState;
    use crate::interaction_ui::InteractionState;
    use crate::keymap;
    use crate::keymap::BindingTarget;
    use crate::keymap::FrameInput;
    use crate::movement::MapTransition;
    use crate::movement::MotionOutput;
    use crate::movement::MotionState;
    use crate::movement::PlayerInput;

    fn transition(target_map_id: u32) -> MapTransition {
        MapTransition {
            target_map_id,
            target_portal_name: "spawn".to_owned(),
        }
    }

    fn pending_transition(
        source_x: f32,
        sequence: u64,
        target_map_id: u32,
    ) -> PendingTransition {
        PendingTransition {
            source: MovementSnapshot {
                sequence,
                map_id: 10,
                position: Some(Vec2 {
                    x: source_x,
                    y: 200.0,
                }),
                mode: MovementMode::Airborne as i32,
                support_contact: Some(MovementContact {
                    position: Some(Vec2 { x: 90.0, y: 210.0 }),
                    mode: MovementMode::Grounded as i32,
                }),
                drop_through: true,
            },
            transition: transition(target_map_id),
        }
    }

    #[test]
    fn blocked_transition_is_dispatched_when_player_mutation_lane_is_free() {
        let mut deferred = VecDeque::new();
        let request = pending_transition(100.0, 7, 20);

        assert_eq!(
            select_transition_request(&mut deferred, Some(request.clone()), 10, true),
            None
        );
        assert_eq!(deferred.len(), 1);
        assert_eq!(
            select_transition_request(&mut deferred, None, 10, false),
            Some(request)
        );
        assert!(deferred.is_empty());
    }

    #[test]
    fn deferred_transition_from_a_previous_map_is_not_dispatched() {
        let mut deferred = VecDeque::from([pending_transition(100.0, 7, 20)]);

        assert_eq!(
            select_transition_request(&mut deferred, None, 30, false),
            None
        );
        assert!(deferred.is_empty());
    }

    #[test]
    fn deferred_transition_retains_the_snapshot_from_portal_selection() {
        let mut deferred = VecDeque::new();
        let selected = pending_transition(100.0, 7, 20);

        assert_eq!(
            select_transition_request(&mut deferred, Some(selected.clone()), 10, true),
            None
        );
        let later_selection = pending_transition(500.0, 8, 20);
        assert_eq!(
            select_transition_request(&mut deferred, Some(later_selection.clone()), 10, true),
            None
        );

        let dispatched =
            select_transition_request(&mut deferred, None, 10, false).expect("deferred transition");
        assert_eq!(dispatched.source, selected.source);
        assert_ne!(dispatched.source, later_selection.source);
        assert!(deferred.is_empty());
    }

    #[test]
    fn configured_key_config_hotkey_toggles_open_and_closed() {
        let bindings = vec![KeyBinding {
            code: "KeyK".to_owned(),
            action: KeyAction::OpenKeyConfig as i32,
            skill_id: 0,
        }];
        let gui = GameGui {
            key_config_window: Some(GuiWindow::default()),
            ..GameGui::default()
        };
        let mut keyboard = keymap::KeyboardState::default();
        let mut state = GuiState::default();

        assert!(keymap::set_key(&mut keyboard, &bindings, "KeyK", true));
        let input = keymap::drain_frame_input(&mut keyboard, &bindings);
        assert!(!apply_key_actions(&mut state, &gui, &input.actions));
        assert!(state.key_config_open);

        assert!(keymap::set_key(&mut keyboard, &bindings, "KeyK", false));
        assert!(keymap::set_key(&mut keyboard, &bindings, "KeyK", true));
        let input = keymap::drain_frame_input(&mut keyboard, &bindings);
        assert!(!apply_key_actions(&mut state, &gui, &input.actions));
        assert!(!state.key_config_open);
    }

    #[test]
    fn modal_ui_closes_before_standard_windows() {
        let mut cash_shop = CashShopState::default();
        cash_shop.begin_open();
        let mut interaction = InteractionState::default();
        interaction.install(Some(NpcInteraction::default()));
        let mut gui = GuiState {
            stats_open: true,
            inventory_open: true,
            skills_open: true,
            ..GuiState::default()
        };

        assert!(close_topmost_ui(&mut cash_shop, &mut interaction, &mut gui));
        assert!(!cash_shop.open);
        assert!(interaction.is_open());
        assert!(gui.skills_open);

        assert!(close_topmost_ui(&mut cash_shop, &mut interaction, &mut gui));
        assert!(!interaction.is_open());
        assert!(gui.skills_open);

        assert!(close_topmost_ui(&mut cash_shop, &mut interaction, &mut gui));
        assert!(!gui.skills_open);
        assert!(gui.inventory_open);
    }

    #[test]
    fn closing_ui_suppresses_only_the_escape_binding() {
        let mut input = FrameInput {
            player: PlayerInput {
                jump_pressed: true,
                portal_pressed: true,
                ..PlayerInput::default()
            },
            actions: vec![KeyAction::Jump, KeyAction::PickUp],
            skills: vec![1_000],
            escape_pressed: true,
        };
        let mut cash_shop = CashShopState::default();
        let mut interaction = InteractionState::default();
        let mut gui = GuiState {
            stats_open: true,
            ..GuiState::default()
        };

        assert!(apply_escape(
            &mut input,
            Some(BindingTarget::Action(KeyAction::Jump)),
            &mut cash_shop,
            &mut interaction,
            &mut gui,
        ));
        assert!(!gui.stats_open);
        assert!(!input.player.jump_pressed);
        assert!(input.player.portal_pressed);
        assert_eq!(input.actions, vec![KeyAction::PickUp]);
        assert_eq!(input.skills, vec![1_000]);
    }

    #[test]
    fn escape_binding_is_preserved_when_no_ui_closes() {
        let mut input = FrameInput {
            player: PlayerInput {
                jump_pressed: true,
                ..PlayerInput::default()
            },
            actions: vec![KeyAction::Jump],
            skills: Vec::new(),
            escape_pressed: true,
        };
        let mut cash_shop = CashShopState::default();
        let mut interaction = InteractionState::default();
        let mut gui = GuiState::default();

        assert!(!apply_escape(
            &mut input,
            Some(BindingTarget::Action(KeyAction::Jump)),
            &mut cash_shop,
            &mut interaction,
            &mut gui,
        ));
        assert!(input.player.jump_pressed);
        assert_eq!(input.actions, vec![KeyAction::Jump]);
    }

    #[test]
    fn closing_ui_suppresses_an_escape_bound_skill() {
        let mut input = FrameInput {
            player: PlayerInput::default(),
            actions: vec![KeyAction::PickUp],
            skills: vec![1_000, 2_000],
            escape_pressed: true,
        };
        let mut cash_shop = CashShopState::default();
        let mut interaction = InteractionState::default();
        let mut gui = GuiState {
            inventory_open: true,
            ..GuiState::default()
        };

        assert!(apply_escape(
            &mut input,
            Some(BindingTarget::Skill(1_000)),
            &mut cash_shop,
            &mut interaction,
            &mut gui,
        ));
        assert!(!gui.inventory_open);
        assert_eq!(input.actions, vec![KeyAction::PickUp]);
        assert_eq!(input.skills, vec![2_000]);
    }

    #[test]
    fn unbound_escape_closes_ui_without_suppressing_other_input() {
        let mut input = FrameInput {
            player: PlayerInput {
                jump_pressed: true,
                ..PlayerInput::default()
            },
            actions: vec![KeyAction::Jump],
            skills: vec![1_000],
            escape_pressed: true,
        };
        let mut cash_shop = CashShopState::default();
        let mut interaction = InteractionState::default();
        let mut gui = GuiState {
            stats_open: true,
            ..GuiState::default()
        };

        assert!(apply_escape(
            &mut input,
            None,
            &mut cash_shop,
            &mut interaction,
            &mut gui,
        ));
        assert!(!gui.stats_open);
        assert!(input.player.jump_pressed);
        assert_eq!(input.actions, vec![KeyAction::Jump]);
        assert_eq!(input.skills, vec![1_000]);
    }

    #[test]
    fn movement_state_selects_the_character_animation() {
        let map = Map {
            ladders: vec![
                Ladder {
                    is_ladder: true,
                    ..Ladder::default()
                },
                Ladder {
                    is_ladder: false,
                    ..Ladder::default()
                },
            ],
            ..Map::default()
        };
        let grounded = MotionState {
            on_ground: true,
            ..MotionState::default()
        };
        let walking = PlayerInput {
            horizontal: -1.0,
            ..PlayerInput::default()
        };

        assert_eq!(
            character_animation(&map, grounded, PlayerInput::default()),
            CharacterAnimation::Idle
        );
        assert_eq!(
            character_animation(&map, grounded, walking),
            CharacterAnimation::Walk
        );
        assert_eq!(
            character_animation(&map, MotionState::default(), walking),
            CharacterAnimation::Jump
        );
        assert_eq!(
            character_animation(
                &map,
                MotionState {
                    climbing: Some(0),
                    ..MotionState::default()
                },
                walking,
            ),
            CharacterAnimation::Ladder
        );
        assert_eq!(
            character_animation(
                &map,
                MotionState {
                    climbing: Some(1),
                    ..MotionState::default()
                },
                walking,
            ),
            CharacterAnimation::Rope
        );
    }

    #[test]
    fn changing_action_restarts_animation_time() {
        let mut animation = new_character_animation_state(CharacterAnimation::Idle, true, 20.0);

        update_character_animation(&mut animation, CharacterAnimation::Idle, true, 100.0);
        assert_eq!(character_animation_elapsed_ms(animation, 100.0), 80.0);
        update_character_animation(&mut animation, CharacterAnimation::Walk, true, 125.0);
        assert_eq!(animation.animation, CharacterAnimation::Walk);
        assert_eq!(character_animation_elapsed_ms(animation, 125.0), 0.0);
    }

    #[test]
    fn one_shot_attack_finishes_before_movement_animation_resumes() {
        let mut animation = new_character_animation_state(CharacterAnimation::Attack, true, 100.0);
        animation.one_shot_until_ms = Some(400.0);

        update_character_animation(&mut animation, CharacterAnimation::Walk, true, 399.0);
        assert_eq!(animation.animation, CharacterAnimation::Attack);
        update_character_animation(&mut animation, CharacterAnimation::Walk, true, 400.0);
        assert_eq!(animation.animation, CharacterAnimation::Walk);
        assert_eq!(character_animation_elapsed_ms(animation, 400.0), 0.0);
    }

    #[test]
    fn active_attack_animation_blocks_another_basic_attack() {
        let mut animation = new_character_animation_state(CharacterAnimation::Attack, true, 100.0);
        animation.one_shot_until_ms = Some(400.0);

        assert!(character_attack_is_active(animation, 399.0));
        assert!(!character_attack_is_active(animation, 400.0));
        assert_eq!(
            select_combat_requests(false, false, false, true, true, None),
            (false, None)
        );
        assert_eq!(
            select_combat_requests(false, false, false, false, true, None),
            (true, None)
        );
    }

    #[test]
    fn death_interrupts_an_active_attack_animation() {
        let mut animation = new_character_animation_state(CharacterAnimation::Attack, true, 100.0);
        animation.one_shot_until_ms = Some(400.0);

        update_character_animation(&mut animation, CharacterAnimation::Death, true, 200.0);

        assert_eq!(animation.animation, CharacterAnimation::Death);
        assert_eq!(character_animation_elapsed_ms(animation, 200.0), 0.0);
        assert_eq!(animation.one_shot_until_ms, None);
    }

    #[test]
    fn sprite_refresh_preserves_an_active_one_shot_animation() {
        let mut animation = new_character_animation_state(CharacterAnimation::Attack, true, 100.0);
        animation.one_shot_until_ms = Some(400.0);

        restart_character_animation(&mut animation, 200.0);

        assert_eq!(animation.animation, CharacterAnimation::Attack);
        assert_eq!(character_animation_elapsed_ms(animation, 200.0), 100.0);
        assert_eq!(animation.one_shot_until_ms, Some(400.0));
    }

    #[test]
    fn stationary_climbing_pauses_and_resumes_animation_time() {
        let mut animation = new_character_animation_state(CharacterAnimation::Ladder, true, 100.0);

        update_character_animation(&mut animation, CharacterAnimation::Ladder, false, 300.0);
        assert_eq!(character_animation_elapsed_ms(animation, 900.0), 200.0);
        update_character_animation(&mut animation, CharacterAnimation::Ladder, true, 1_000.0);
        assert_eq!(character_animation_elapsed_ms(animation, 1_000.0), 200.0);
        assert_eq!(character_animation_elapsed_ms(animation, 1_100.0), 300.0);
    }

    #[test]
    fn climbing_animation_runs_only_during_vertical_displacement() {
        let position = Vec2 { x: 200.0, y: 300.0 };
        let climbed = Vec2 { x: 200.0, y: 290.0 };

        assert!(!character_animation_plays(
            CharacterAnimation::Ladder,
            position,
            position,
        ));
        assert!(!character_animation_plays(
            CharacterAnimation::Rope,
            position,
            position,
        ));
        assert!(character_animation_plays(
            CharacterAnimation::Ladder,
            position,
            climbed,
        ));
        assert!(character_animation_plays(
            CharacterAnimation::Idle,
            position,
            position,
        ));
    }

    #[test]
    fn authoritative_disable_and_transitions_suppress_combat_requests() {
        assert_eq!(
            select_combat_requests(true, false, false, false, true, Some(1)),
            (false, None)
        );
        assert_eq!(
            select_combat_requests(false, true, false, false, true, Some(1)),
            (false, None)
        );
        assert_eq!(
            select_combat_requests(false, false, true, false, true, Some(1)),
            (false, None)
        );
        assert_eq!(
            select_combat_requests(false, false, false, false, true, Some(1)),
            (true, Some(1))
        );
    }
}
