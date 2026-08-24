use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use oozems_proto::v1::ActiveBuffState;
use oozems_proto::v1::CharacterSpriteSet;
use oozems_proto::v1::GameGui;
use oozems_proto::v1::KeyAction;
use oozems_proto::v1::KeyBinding;
use oozems_proto::v1::Map;
use oozems_proto::v1::MorphDefinition;
use oozems_proto::v1::MovementRules;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::SkillBook;
use oozems_proto::v1::Vec2;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::spawn_local;
use web_sys::CanvasRenderingContext2d;
use web_sys::HtmlCanvasElement;
use web_sys::KeyboardEvent;
use web_sys::MouseEvent;

use crate::api;
use crate::assets;
use crate::assets::BrowserAsset;
use crate::character_render::CharacterAnimation;
use crate::game_gui;
use crate::game_gui::CanvasPoint;
use crate::game_gui::GuiAction;
use crate::game_gui::GuiState;
use crate::game_gui::KeyDrag;
use crate::game_gui::PointerButton;
use crate::interaction_ui;
use crate::interaction_ui::InteractionState;
use crate::js_error;
use crate::keymap;
use crate::keymap::KeyboardState;
use crate::mob_render::MobRenderState;
use crate::movement;
use crate::movement::MapTransition;
use crate::movement::MotionState;
use crate::movement::PlayerInput;
use crate::render;
use crate::show_status;
use crate::skill_effects;
use crate::skill_effects::SkillEffectState;

pub(crate) mod buffs;
mod interaction_actions;
mod item_actions;
mod movement_actions;
mod player_updates;
mod recovery_actions;
mod skill_actions;

use player_updates::PlayerDomains;
use player_updates::PlayerInstallation;
use player_updates::PlayerRevisions;
use player_updates::appearance_assets_are_eligible;
use player_updates::appearance_refresh;
use player_updates::install_player_update;
use player_updates::install_revision;
use player_updates::synchronize_skill_book;
use player_updates::visible_appearance_identity;

const SAVE_INTERVAL_MS: f64 = 2_000.0;
const APPEARANCE_RETRY_LIMIT: u8 = 2;
const APPEARANCE_RETRY_BACKOFF_MS: f64 = 1_000.0;

pub(crate) fn monotonic_time_ms() -> f64 {
    web_sys::window()
        .and_then(|window| window.performance())
        .map_or(0.0, |performance| performance.now())
}

pub struct Game {
    pub canvas: HtmlCanvasElement,
    pub character_animation: CharacterAnimationState,
    pub context: CanvasRenderingContext2d,
    pub character_sprites: CharacterSpriteSet,
    pub facing_left: bool,
    pub gui: GameGui,
    pub gui_state: Rc<RefCell<GuiState>>,
    pub images: HashMap<String, BrowserAsset>,
    pub interaction: InteractionState,
    pub key_bindings: Rc<RefCell<Vec<KeyBinding>>>,
    key_bindings_generation: u64,
    key_bindings_pending: bool,
    pub key_drag: Option<KeyDrag>,
    pub pointer: Option<CanvasPoint>,
    pub(crate) selected_buff: Option<buffs::BuffKey>,
    pub map: Map,
    pub mob_render: MobRenderState,
    pub(crate) npc_animations: render::npc::NpcAnimationPlaybackState,
    pub movement_rules: MovementRules,
    pub motion: MotionState,
    pub player: PlayerState,
    pub skill_book: SkillBook,
    pub(crate) active_buffs: buffs::TrackedBuffs,
    pub(crate) morph_definition: Option<MorphDefinition>,
    pub(crate) skill_effect_state: SkillEffectState,
    pub frame_time_ms: f64,
    pub world_layers: Vec<i32>,
    input: Rc<RefCell<KeyboardState>>,
    save_failed: Rc<Cell<bool>>,
    save_in_flight: Rc<std::cell::Cell<bool>>,
    item_action_in_flight: Rc<Cell<bool>>,
    skill_action_in_flight: Rc<Cell<bool>>,
    transition_in_flight: Rc<std::cell::Cell<bool>>,
    dirty: bool,
    suppress_click: Rc<Cell<bool>>,
    last_frame_ms: f64,
    next_save_ms: f64,
    movement_sync: movement_actions::MovementSyncState,
    player_revisions: PlayerRevisions,
    recovery_state: recovery_actions::RecoveryState,
    appearance_refresh_state: AppearanceRefreshState,
    morph_refresh_state: MorphRefreshState,
    gui_refresh_state: GuiRefreshState,
}

fn install_full_player_update(
    game: &mut Game,
    update: PlayerState,
) -> PlayerInstallation {
    let mut domains = PlayerDomains::FULL;
    domains.key_bindings = !game.key_bindings_pending;
    let installed = install_player_update(
        &mut game.player,
        &mut game.player_revisions,
        update,
        domains,
    );
    if installed.domains.skills {
        synchronize_skill_book(&mut game.skill_book, &game.player);
        if game.key_bindings_pending {
            let updated = keymap::retain_learned_skill_bindings(
                &game.key_bindings.borrow(),
                &game.player.learned_skills,
            );
            if updated != *game.key_bindings.borrow() {
                *game.key_bindings.borrow_mut() = updated.clone();
                game.player.key_bindings = updated;
                game.key_bindings_generation = game.key_bindings_generation.saturating_add(1);
                game.dirty = true;
            }
        }
        let dragged_skill_was_removed = game.key_drag.as_ref().is_some_and(|drag| {
            let keymap::BindingTarget::Skill(skill_id) = drag.target else {
                return false;
            };
            !game
                .player
                .learned_skills
                .iter()
                .any(|skill| skill.skill_id == skill_id && skill.level > 0)
        });
        if dragged_skill_was_removed {
            game.key_drag = None;
        }
    }
    if installed.domains.key_bindings {
        *game.key_bindings.borrow_mut() = game.player.key_bindings.clone();
    }
    queue_appearance_refresh(game, installed);
    installed
}

#[derive(Default)]
struct AppearanceRefreshState {
    cached_sprites: HashMap<player_updates::AppearanceIdentity, CharacterSpriteSet>,
    pending: Option<player_updates::AppearanceRefresh>,
    in_flight: Option<player_updates::AppearanceRefresh>,
    retry_identity: Option<player_updates::AppearanceIdentity>,
    retry_count: u8,
    retry_after_ms: f64,
}

#[derive(Default)]
struct MorphRefreshState {
    cached: HashMap<u32, MorphDefinition>,
    pending: Option<u32>,
    in_flight: Option<u32>,
    retry_after_ms: HashMap<u32, f64>,
}

#[derive(Default)]
struct GuiRefreshState {
    in_flight: bool,
    retry_after_ms: f64,
    required: bool,
}

const MORPH_RETRY_BACKOFF_MS: f64 = 5_000.0;
const GUI_RETRY_BACKOFF_MS: f64 = 5_000.0;

fn install_active_buffs(
    game: &mut Game,
    state: buffs::ValidatedState,
    request_started_ms: f64,
) {
    let received_at_ms = monotonic_time_ms();
    buffs::install(
        &mut game.active_buffs,
        state,
        received_at_ms,
        elapsed_since(request_started_ms, received_at_ms),
    );
    synchronize_morph(game);
}

fn validate_active_buffs(state: ActiveBuffState) -> Result<buffs::ValidatedState, String> {
    buffs::validate_state(state)
}

fn elapsed_since(
    started_ms: f64,
    finished_ms: f64,
) -> f64 {
    (finished_ms - started_ms).max(0.0)
}

fn synchronize_morph(game: &mut Game) {
    let desired = game.active_buffs.morph_id;
    if desired.is_none() {
        game.morph_definition = None;
        update_morph_refresh_request(
            &mut game.morph_refresh_state,
            None,
            false,
            game.frame_time_ms,
        );
        return;
    }
    let desired = desired.expect("checked active morph");
    if game
        .morph_definition
        .as_ref()
        .is_some_and(|definition| definition.morph_id == desired)
    {
        update_morph_refresh_request(
            &mut game.morph_refresh_state,
            Some(desired),
            true,
            game.frame_time_ms,
        );
        return;
    }
    if let Some(definition) = game.morph_refresh_state.cached.get(&desired).cloned() {
        game.morph_definition = Some(definition);
        update_morph_refresh_request(
            &mut game.morph_refresh_state,
            Some(desired),
            true,
            game.frame_time_ms,
        );
        return;
    }
    game.morph_definition = None;
    update_morph_refresh_request(
        &mut game.morph_refresh_state,
        Some(desired),
        false,
        game.frame_time_ms,
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
    let retry_ready = state
        .retry_after_ms
        .get(&desired)
        .is_none_or(|deadline_ms| now_ms >= *deadline_ms);
    if state.in_flight != Some(desired) && retry_ready {
        state.pending = Some(desired);
    }
}

fn queue_appearance_refresh(
    game: &mut Game,
    installed: PlayerInstallation,
) {
    if !installed.domains.inventory {
        return;
    }
    let Some(mut refresh) = appearance_refresh(&game.player) else {
        return;
    };
    refresh.revision = game.player_revisions.inventory;
    if let Some(in_flight) = game
        .appearance_refresh_state
        .in_flight
        .as_mut()
        .filter(|in_flight| in_flight.identity == refresh.identity)
    {
        in_flight.revision = in_flight.revision.max(refresh.revision);
    }
    if let Some(pending) = game
        .appearance_refresh_state
        .pending
        .as_mut()
        .filter(|pending| pending.identity == refresh.identity)
    {
        pending.revision = pending.revision.max(refresh.revision);
    }
    if !installed.visible_appearance_changed {
        return;
    }

    game.appearance_refresh_state.pending = None;
    reset_appearance_retry(&mut game.appearance_refresh_state);
    if let Some(cached) = game
        .appearance_refresh_state
        .cached_sprites
        .get(&refresh.identity)
        .cloned()
    {
        if install_revision(
            &mut game.player_revisions.appearance_assets,
            refresh.revision,
        ) {
            game.character_sprites = cached;
            restart_character_animation(&mut game.character_animation, game.frame_time_ms);
        }
        return;
    }

    let same_request_is_in_flight = game
        .appearance_refresh_state
        .in_flight
        .as_ref()
        .is_some_and(|in_flight| in_flight.identity == refresh.identity);
    if !same_request_is_in_flight {
        game.appearance_refresh_state.pending = Some(refresh);
    }
}

fn reset_appearance_retry(state: &mut AppearanceRefreshState) {
    state.retry_identity = None;
    state.retry_count = 0;
    state.retry_after_ms = 0.0;
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
    let completed_retries = if state.retry_identity.as_ref() == Some(&request.identity) {
        state.retry_count
    } else {
        0
    };
    let Some((retry_count, retry_after_ms)) = next_appearance_retry(completed_retries, now_ms)
    else {
        return false;
    };
    state.retry_identity = Some(request.identity.clone());
    state.retry_count = retry_count;
    state.retry_after_ms = retry_after_ms;
    state.pending = Some(request);
    true
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharacterAnimationState {
    pub animation: CharacterAnimation,
    started_ms: f64,
    paused_at_ms: Option<f64>,
    one_shot_until_ms: Option<f64>,
}

pub async fn run(
    player: PlayerState,
    character_sprites: CharacterSpriteSet,
    bootstrap_active_buffs: ActiveBuffState,
    bootstrap_requested_at_ms: f64,
) -> Result<(), String> {
    show_status("Loading map, GUI, and skills...", false);
    let map = api::get_map(player.map_id)
        .await
        .map_err(|error| error.to_string())?;
    let movement_rules = api::get_movement_rules()
        .await
        .map_err(|error| error.to_string())?;
    movement::validate_rules(&movement_rules)?;
    let gui_result = api::get_gui(&player.id, Vec::new()).await;
    let gui_refresh_required = gui_result.is_err();
    let gui_warning = gui_result.as_ref().err().map(ToString::to_string);
    let gui = gui_result.unwrap_or_default();
    let skill_requested_at_ms = monotonic_time_ms();
    let skill_result = api::get_skill_book(&player.id).await;
    let skill_warning = skill_result.as_ref().err().map(ToString::to_string);
    let loaded_skills = skill_result.unwrap_or_else(|_| api::LoadedSkillBook {
        skill_book: SkillBook {
            job_id: player.stats.as_ref().map_or(0, |stats| stats.job_id),
            name: "Skills".to_owned(),
            ..SkillBook::default()
        },
        active_buffs: ActiveBuffState::default(),
    });
    let game = build_game(
        player,
        map,
        movement_rules,
        character_sprites,
        gui,
        loaded_skills.skill_book,
        bootstrap_active_buffs,
        bootstrap_requested_at_ms,
        loaded_skills.active_buffs,
        skill_requested_at_ms,
    )?;
    game.borrow_mut().gui_refresh_state.required = gui_refresh_required;
    let asset_count = game.borrow().images.len();

    let warnings = [
        gui_warning.map(|error| format!("GUI unavailable: {error}")),
        skill_warning.map(|error| format!("skill book unavailable: {error}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    if warnings.is_empty() {
        show_status(
            &format!("Ready. Streaming {asset_count} assets as needed."),
            false,
        );
    } else {
        show_status(
            &format!(
                "Ready. Streaming {asset_count} assets as needed. {}",
                warnings.join("; ")
            ),
            true,
        );
    }
    schedule_frame(game)?;
    Ok(())
}

fn build_game(
    mut player: PlayerState,
    map: Map,
    movement_rules: MovementRules,
    character_sprites: CharacterSpriteSet,
    gui: GameGui,
    skill_book: SkillBook,
    bootstrap_active_buffs: ActiveBuffState,
    bootstrap_requested_at_ms: f64,
    skill_active_buffs: ActiveBuffState,
    skill_requested_at_ms: f64,
) -> Result<Rc<RefCell<Game>>, String> {
    if let Some(position) = player.position {
        player.position = Some(movement::constrain_position(&map, position));
    }
    let window = web_sys::window().ok_or("browser window is unavailable")?;
    let document = window.document().ok_or("browser document is unavailable")?;
    let canvas = document
        .get_element_by_id("game")
        .ok_or("game canvas is missing")?
        .dyn_into::<HtmlCanvasElement>()
        .map_err(|_| "game element is not a canvas")?;
    let context = canvas
        .get_context("2d")
        .map_err(js_error)?
        .ok_or("2D canvas is unavailable")?
        .dyn_into::<CanvasRenderingContext2d>()
        .map_err(|_| "could not create a 2D canvas context")?;
    context.set_image_smoothing_enabled(false);

    let input = Rc::new(RefCell::new(KeyboardState::default()));
    let key_bindings = Rc::new(RefCell::new(player.key_bindings.clone()));
    install_keyboard_input(&window, input.clone(), key_bindings.clone())?;
    let gui_state = Rc::new(RefCell::new(GuiState::default()));
    let images = prepare_game_assets(&map, &character_sprites, &gui, &skill_book)?;
    let motion = player
        .position
        .as_ref()
        .map_or_else(MotionState::default, |position| {
            movement::initial_motion_state(&map, position)
        });
    let world_layers = render::world_layers(&map);
    let simulation_sequence = map.simulation_sequence;
    let player_revisions = PlayerRevisions::new(player.revision);
    let mut appearance_refresh_state = AppearanceRefreshState::default();
    if let Some(identity) = visible_appearance_identity(&player) {
        appearance_refresh_state
            .cached_sprites
            .insert(identity, character_sprites.clone());
    }

    let now_local_ms = monotonic_time_ms();
    let mut active_buffs = buffs::from_state(
        bootstrap_active_buffs,
        now_local_ms,
        elapsed_since(bootstrap_requested_at_ms, now_local_ms),
    )?;
    buffs::install(
        &mut active_buffs,
        buffs::validate_state(skill_active_buffs)?,
        now_local_ms,
        elapsed_since(skill_requested_at_ms, now_local_ms),
    );
    let morph_refresh_state = MorphRefreshState {
        pending: active_buffs.morph_id,
        ..MorphRefreshState::default()
    };
    let game = Rc::new(RefCell::new(Game {
        canvas: canvas.clone(),
        character_animation: new_character_animation_state(CharacterAnimation::Idle, true, 0.0),
        context,
        character_sprites,
        facing_left: false,
        gui,
        gui_state,
        images,
        interaction: InteractionState::default(),
        key_bindings,
        key_bindings_generation: 0,
        key_bindings_pending: false,
        key_drag: None,
        pointer: None,
        selected_buff: None,
        map,
        mob_render: crate::mob_render::new_map_state(simulation_sequence),
        npc_animations: render::npc::NpcAnimationPlaybackState::default(),
        movement_rules,
        motion,
        player,
        skill_book,
        active_buffs,
        morph_definition: None,
        skill_effect_state: SkillEffectState::default(),
        frame_time_ms: 0.0,
        world_layers,
        input,
        save_failed: Rc::new(Cell::new(false)),
        save_in_flight: Rc::new(std::cell::Cell::new(false)),
        item_action_in_flight: Rc::new(Cell::new(false)),
        skill_action_in_flight: Rc::new(Cell::new(false)),
        transition_in_flight: Rc::new(std::cell::Cell::new(false)),
        dirty: false,
        suppress_click: Rc::new(Cell::new(false)),
        last_frame_ms: 0.0,
        next_save_ms: SAVE_INTERVAL_MS,
        movement_sync: movement_actions::MovementSyncState::default(),
        player_revisions,
        recovery_state: recovery_actions::RecoveryState::default(),
        appearance_refresh_state,
        morph_refresh_state,
        gui_refresh_state: GuiRefreshState::default(),
    }));
    install_canvas_input(&canvas, game.clone())?;
    Ok(game)
}

fn prepare_game_assets(
    map: &Map,
    character_sprites: &CharacterSpriteSet,
    gui: &GameGui,
    skill_book: &SkillBook,
) -> Result<HashMap<String, BrowserAsset>, String> {
    assets::prepare_assets(
        map.assets
            .iter()
            .chain(character_sprites.assets.iter())
            .chain(gui.assets.iter())
            .chain(skill_book.assets.iter()),
    )
}

fn install_keyboard_input(
    window: &web_sys::Window,
    input: Rc<RefCell<KeyboardState>>,
    bindings: Rc<RefCell<Vec<KeyBinding>>>,
) -> Result<(), String> {
    let pressed_input = input.clone();
    let pressed_bindings = bindings.clone();
    let keydown = Closure::<dyn FnMut(KeyboardEvent)>::new(move |event: KeyboardEvent| {
        if keymap::set_key(
            &mut pressed_input.borrow_mut(),
            &pressed_bindings.borrow(),
            &event.code(),
            true,
        ) {
            event.prevent_default();
        }
    });
    window
        .add_event_listener_with_callback("keydown", keydown.as_ref().unchecked_ref())
        .map_err(js_error)?;
    keydown.forget();

    let keyup = Closure::<dyn FnMut(KeyboardEvent)>::new(move |event: KeyboardEvent| {
        if keymap::set_key(
            &mut input.borrow_mut(),
            &bindings.borrow(),
            &event.code(),
            false,
        ) {
            event.prevent_default();
        }
    });
    window
        .add_event_listener_with_callback("keyup", keyup.as_ref().unchecked_ref())
        .map_err(js_error)?;
    keyup.forget();
    Ok(())
}

fn install_canvas_input(
    canvas: &HtmlCanvasElement,
    game: Rc<RefCell<Game>>,
) -> Result<(), String> {
    let down_canvas = canvas.clone();
    let down_game = game.clone();
    let mouse_down = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        if event.button() != 0 {
            return;
        }
        let Some(point) = canvas_event_point(&down_canvas, &event) else {
            return;
        };
        let drag = {
            let game = down_game.borrow();
            if !game.gui_state.borrow().key_config_open {
                None
            } else {
                game_gui::begin_key_drag(
                    *game.gui_state.borrow(),
                    &game.gui,
                    &game.skill_book,
                    &game.key_bindings.borrow(),
                    point,
                )
            }
        };
        if let Some(drag) = drag {
            down_game.borrow_mut().key_drag = Some(drag);
            event.prevent_default();
            let _ = down_canvas.focus();
        }
    });
    canvas
        .add_event_listener_with_callback("mousedown", mouse_down.as_ref().unchecked_ref())
        .map_err(js_error)?;
    mouse_down.forget();

    let move_canvas = canvas.clone();
    let move_game = game.clone();
    let mouse_move = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        let Some(point) = canvas_event_point(&move_canvas, &event) else {
            return;
        };
        let mut game = move_game.borrow_mut();
        game.pointer = Some(point);
        if let Some(drag) = game.key_drag.as_mut() {
            game_gui::move_key_drag(drag, point);
            event.prevent_default();
        }
    });
    canvas
        .add_event_listener_with_callback("mousemove", mouse_move.as_ref().unchecked_ref())
        .map_err(js_error)?;
    mouse_move.forget();

    let leave_game = game.clone();
    let mouse_leave = Closure::<dyn FnMut(MouseEvent)>::new(move |_event: MouseEvent| {
        let mut game = leave_game.borrow_mut();
        game.key_drag = None;
        game.pointer = None;
    });
    canvas
        .add_event_listener_with_callback("mouseleave", mouse_leave.as_ref().unchecked_ref())
        .map_err(js_error)?;
    mouse_leave.forget();

    let up_canvas = canvas.clone();
    let up_game = game.clone();
    let mouse_up = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        if event.button() != 0 {
            return;
        }
        let Some(point) = canvas_event_point(&up_canvas, &event) else {
            return;
        };
        let mut game = up_game.borrow_mut();
        let Some(drag) = game.key_drag.take() else {
            return;
        };
        let updated = {
            let bindings = game.key_bindings.borrow();
            game_gui::finish_key_drag(&game.gui, &bindings, &drag, point)
        };
        let changed = updated
            .as_ref()
            .is_some_and(|updated| *updated != *game.key_bindings.borrow());
        if let Some(updated) = updated.filter(|_| changed) {
            *game.key_bindings.borrow_mut() = updated.clone();
            game.player.key_bindings = updated;
            game.key_bindings_generation = game.key_bindings_generation.saturating_add(1);
            game.key_bindings_pending = true;
            game.dirty = true;
        }
        game.suppress_click.set(true);
        event.prevent_default();
        let _ = up_canvas.focus();
    });
    canvas
        .add_event_listener_with_callback("mouseup", mouse_up.as_ref().unchecked_ref())
        .map_err(js_error)?;
    mouse_up.forget();

    let event_canvas = canvas.clone();
    let click_game = game.clone();
    let click = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        if handle_canvas_pointer(&click_game, &event_canvas, &event, PointerButton::Left) {
            event.prevent_default();
            let _ = event_canvas.focus();
        }
    });
    canvas
        .add_event_listener_with_callback("click", click.as_ref().unchecked_ref())
        .map_err(js_error)?;
    click.forget();

    let double_click_canvas = canvas.clone();
    let double_click_game = game.clone();
    let double_click = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        if event.button() != 0 {
            return;
        }
        let Some(point) = canvas_event_point(&double_click_canvas, &event) else {
            return;
        };
        let npc_spawn_id = {
            let game = double_click_game.borrow();
            let gui = *game.gui_state.borrow();
            if game.interaction.is_busy()
                || game.item_action_in_flight.get()
                || game.skill_action_in_flight.get()
                || game.transition_in_flight.get()
                || gui.stats_open
                || gui.equipment_open
                || gui.inventory_open
                || gui.key_config_open
                || gui.skills_open
            {
                None
            } else {
                render::npc_at_point(&game, point)
            }
        };
        if let Some(npc_spawn_id) = npc_spawn_id {
            if double_click_game.borrow().gui.npc_dialog_window.is_none() {
                show_status("NPC interaction requires UI.wz.", true);
                return;
            }
            interaction_actions::begin_open(double_click_game.clone(), npc_spawn_id);
            event.prevent_default();
            let _ = double_click_canvas.focus();
        }
    });
    canvas
        .add_event_listener_with_callback("dblclick", double_click.as_ref().unchecked_ref())
        .map_err(js_error)?;
    double_click.forget();

    let context_canvas = canvas.clone();
    let context_menu = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        if handle_canvas_pointer(&game, &context_canvas, &event, PointerButton::Right) {
            event.prevent_default();
            let _ = context_canvas.focus();
        }
    });
    canvas
        .add_event_listener_with_callback("contextmenu", context_menu.as_ref().unchecked_ref())
        .map_err(js_error)?;
    context_menu.forget();
    Ok(())
}

fn handle_canvas_pointer(
    game: &Rc<RefCell<Game>>,
    canvas: &HtmlCanvasElement,
    event: &MouseEvent,
    button: PointerButton,
) -> bool {
    if button == PointerButton::Left && game.borrow().suppress_click.replace(false) {
        return true;
    }
    let Some(point) = canvas_event_point(canvas, event) else {
        return false;
    };
    if game.borrow().interaction.is_busy() {
        if button != PointerButton::Left {
            return true;
        }
        let action = {
            let game = game.borrow();
            interaction_ui::click_action(
                &game.gui,
                &game.interaction,
                game.player.inventory.as_ref(),
                point,
            )
        };
        let Some(action) = action else {
            return true;
        };
        if interaction_ui::apply_local_action(&mut game.borrow_mut().interaction, action) {
            return true;
        }
        interaction_actions::begin_action(game.clone(), action);
        return true;
    }
    if button == PointerButton::Left && render::select_active_buff(&mut game.borrow_mut(), point) {
        return true;
    }
    let action = {
        let game = game.borrow();
        game_gui::click_action(
            *game.gui_state.borrow(),
            &game.gui,
            game.player.inventory.as_ref(),
            Some(&game.skill_book),
            canvas.width() as f32,
            canvas.height() as f32,
            point,
            button,
        )
    };
    let Some(action) = action else {
        return false;
    };
    let gui_state = game.borrow().gui_state.clone();
    if game_gui::apply_local_action(&mut gui_state.borrow_mut(), action) {
        return true;
    }
    match action {
        GuiAction::AllocateSkill { .. } | GuiAction::UseSkill { .. } => {
            skill_actions::begin(game.clone(), action);
        }
        _ => item_actions::begin(game.clone(), action),
    }
    true
}

fn canvas_event_point(
    canvas: &HtmlCanvasElement,
    event: &MouseEvent,
) -> Option<game_gui::CanvasPoint> {
    game_gui::canvas_point(
        event.offset_x(),
        event.offset_y(),
        canvas.width(),
        canvas.height(),
        canvas.client_width(),
        canvas.client_height(),
    )
}

fn schedule_frame(game: Rc<RefCell<Game>>) -> Result<(), String> {
    let window = web_sys::window().ok_or("browser window is unavailable")?;
    let callback = Closure::once_into_js(move |timestamp_ms: f64| {
        let update = update(&mut game.borrow_mut(), timestamp_ms);
        {
            let game = game.borrow();
            render::draw(&game);
        }
        begin_appearance_refresh(game.clone());
        begin_morph_refresh(game.clone());
        begin_gui_refresh(game.clone());
        if update.pick_up {
            item_actions::begin_pick_up(game.clone());
        }
        if update.movement_snapshot {
            movement_actions::begin(game.clone());
        }
        if update.basic_attack {
            skill_actions::begin_basic_attack(game.clone());
        }
        if let Some(skill_id) = update.skill_id {
            skill_actions::begin(game.clone(), GuiAction::UseSkill { skill_id });
        }
        if update.recover {
            recovery_actions::begin(game.clone());
        }
        if let Some(player) = update.save {
            begin_save(game.clone(), player);
        }
        if let Some(transition) = update.transition {
            movement_actions::begin_portal(game.clone(), transition);
        }
        if let Err(error) = schedule_frame(game) {
            show_status(&format!("Animation stopped: {error}"), true);
        }
    });
    window
        .request_animation_frame(callback.unchecked_ref())
        .map_err(js_error)?;
    Ok(())
}

fn begin_gui_refresh(game: Rc<RefCell<Game>>) {
    let request = {
        let game = game.borrow();
        if game.gui_refresh_state.in_flight
            || game.frame_time_ms < game.gui_refresh_state.retry_after_ms
        {
            return;
        }
        let observed_item_ids = game
            .player
            .inventory
            .iter()
            .flat_map(|inventory| {
                inventory
                    .stacks
                    .iter()
                    .map(|stack| stack.item_id)
                    .chain(inventory.equipment.iter().map(|equipped| equipped.item_id))
            })
            .chain(game.map.dropped_items.iter().map(|drop| drop.item_id));
        let Some(refresh_item_ids) = game_gui::item_definition_refresh_ids(
            &game.gui,
            observed_item_ids,
            game.gui_refresh_state.required,
        ) else {
            return;
        };
        (game.player.id.clone(), refresh_item_ids)
    };
    game.borrow_mut().gui_refresh_state.in_flight = true;
    spawn_local(async move {
        let result = api::get_gui(&request.0, request.1)
            .await
            .map_err(|error| error.to_string())
            .and_then(|gui| assets::prepare_assets(gui.assets.iter()).map(|images| (gui, images)));
        let completed_at_ms = monotonic_time_ms();
        let mut game = game.borrow_mut();
        game.gui_refresh_state.in_flight = false;
        match result {
            Ok((gui, images)) => {
                game.gui = gui;
                assets::merge_assets(&mut game.images, images);
                game.gui_refresh_state.retry_after_ms = 0.0;
                game.gui_refresh_state.required = false;
            }
            Err(error) => {
                game.gui_refresh_state.retry_after_ms = completed_at_ms + GUI_RETRY_BACKOFF_MS;
                show_status(&format!("Item metadata refresh failed: {error}"), true);
            }
        }
    });
}

fn begin_appearance_refresh(game: Rc<RefCell<Game>>) {
    let request = {
        let mut game = game.borrow_mut();
        if game.appearance_refresh_state.in_flight.is_some() {
            return;
        }
        let Some(request) = game.appearance_refresh_state.pending.take() else {
            return;
        };
        let retry_is_ready = game
            .appearance_refresh_state
            .retry_identity
            .as_ref()
            .is_none_or(|identity| identity != &request.identity)
            || game.frame_time_ms >= game.appearance_refresh_state.retry_after_ms;
        if !retry_is_ready {
            game.appearance_refresh_state.pending = Some(request);
            return;
        }
        game.appearance_refresh_state.in_flight = Some(request.clone());
        request
    };
    spawn_local(async move {
        let prepared =
            match api::get_character_sprites(request.appearance, Some(&request.equipment)).await {
                Ok(sprites) => {
                    assets::prepare_assets(sprites.assets.iter()).map(|images| (sprites, images))
                }
                Err(error) => Err(error.to_string()),
            };
        let mut game = game.borrow_mut();
        let Some(active) = game.appearance_refresh_state.in_flight.take() else {
            return;
        };
        if active.identity != request.identity {
            return;
        }
        let current_identity = visible_appearance_identity(&game.player);
        let eligible = appearance_assets_are_eligible(
            current_identity.as_ref(),
            &active,
            game.player_revisions.inventory,
            game.player_revisions.appearance_assets,
        );
        match prepared {
            Ok((sprites, images)) if eligible => {
                if install_revision(
                    &mut game.player_revisions.appearance_assets,
                    active.revision,
                ) {
                    assets::merge_assets(&mut game.images, images);
                    game.appearance_refresh_state
                        .cached_sprites
                        .insert(active.identity, sprites.clone());
                    game.character_sprites = sprites;
                    let frame_time_ms = game.frame_time_ms;
                    restart_character_animation(&mut game.character_animation, frame_time_ms);
                    reset_appearance_retry(&mut game.appearance_refresh_state);
                }
            }
            Err(error) if eligible => {
                let frame_time_ms = game.frame_time_ms;
                let retrying = schedule_appearance_retry(
                    &mut game.appearance_refresh_state,
                    active,
                    frame_time_ms,
                );
                let suffix = if retrying { "; retrying" } else { "" };
                show_status(
                    &format!("Character appearance could not refresh: {error}{suffix}"),
                    true,
                );
            }
            Ok(_) | Err(_) => {}
        }
    });
}

fn begin_morph_refresh(game: Rc<RefCell<Game>>) {
    let morph_id = {
        let mut game = game.borrow_mut();
        if game.morph_refresh_state.in_flight.is_some() {
            return;
        }
        let Some(morph_id) = game.morph_refresh_state.pending.take() else {
            return;
        };
        game.morph_refresh_state.in_flight = Some(morph_id);
        morph_id
    };
    spawn_local(async move {
        let prepared = match api::get_morph(morph_id).await {
            Ok(definition) if definition.morph_id == morph_id => {
                assets::prepare_assets(definition.assets.iter()).map(|images| (definition, images))
            }
            Ok(_) => Err("server returned a different morph definition".to_owned()),
            Err(error) => Err(error.to_string()),
        };
        let mut game = game.borrow_mut();
        if game.morph_refresh_state.in_flight.take() != Some(morph_id) {
            return;
        }
        match prepared {
            Ok((definition, images)) => {
                assets::merge_assets(&mut game.images, images);
                game.morph_refresh_state
                    .cached
                    .insert(morph_id, definition.clone());
                game.morph_refresh_state.retry_after_ms.remove(&morph_id);
                if game.active_buffs.morph_id == Some(morph_id) {
                    game.morph_definition = Some(definition);
                }
            }
            Err(error) if game.active_buffs.morph_id == Some(morph_id) => {
                let retry_at = game.frame_time_ms + MORPH_RETRY_BACKOFF_MS;
                game.morph_refresh_state
                    .retry_after_ms
                    .insert(morph_id, retry_at);
                show_status(&format!("Morph could not load: {error}"), true);
            }
            Err(_) => {}
        }
        synchronize_morph(&mut game);
    });
}

struct FrameUpdate {
    transition: Option<MapTransition>,
    basic_attack: bool,
    pick_up: bool,
    movement_snapshot: bool,
    recover: bool,
    save: Option<PendingSave>,
    skill_id: Option<u32>,
}

struct PendingSave {
    player: PlayerState,
    key_bindings_generation: u64,
}

fn update(
    game: &mut Game,
    timestamp_ms: f64,
) -> FrameUpdate {
    let elapsed_seconds = if game.last_frame_ms == 0.0 {
        0.0
    } else {
        ((timestamp_ms - game.last_frame_ms) / 1_000.0).clamp(0.0, 0.05) as f32
    };
    game.last_frame_ms = timestamp_ms;
    game.frame_time_ms = timestamp_ms;
    skill_effects::update(&mut game.skill_effect_state, &game.images, timestamp_ms);
    if game.save_failed.replace(false) {
        game.dirty = true;
    }
    let now_ms = js_sys::Date::now().max(0.0) as u64;
    game.map.dropped_items.retain(|drop| {
        drop.despawn_at_unix_ms > now_ms
            && (drop.expires_at_unix_ms == 0 || drop.expires_at_unix_ms > now_ms)
    });

    let mut input = {
        let bindings = game.key_bindings.borrow();
        keymap::drain_frame_input(&mut game.input.borrow_mut(), &bindings)
    };
    let key_config_open = game.gui_state.borrow().key_config_open;
    if key_config_open {
        input.player = PlayerInput::default();
        input.skills.clear();
        input
            .actions
            .retain(|action| *action == KeyAction::OpenKeyConfig);
    }
    let interaction_busy = game.interaction.is_busy();
    if interaction_busy {
        input.player = PlayerInput::default();
        input.skills.clear();
        input.actions.clear();
    }
    let pick_up = apply_key_actions(&mut game.gui_state.borrow_mut(), &game.gui, &input.actions);
    buffs::apply(&mut game.active_buffs, &mut input.player, timestamp_ms);
    synchronize_morph(game);

    if game.active_buffs.attacks_disabled {
        input
            .actions
            .retain(|action| *action != KeyAction::BasicAttack);
        input.skills.clear();
    }
    let basic_attack_requested = input.actions.contains(&KeyAction::BasicAttack);

    let transition = if game.transition_in_flight.get() {
        None
    } else {
        let selected = update_player(game, elapsed_seconds, input.player);
        (!game.skill_action_in_flight.get())
            .then_some(selected)
            .flatten()
    };
    if transition.is_some() {
        game.transition_in_flight.set(true);
    }
    let (basic_attack, skill_id) = select_combat_requests(
        game.active_buffs.attacks_disabled,
        transition.is_some(),
        game.transition_in_flight.get(),
        basic_attack_requested,
        input.skills.into_iter().next(),
    );
    let movement_observation =
        movement_actions::observation(game.map.id, game.player.position, game.motion);
    let movement_snapshot = movement_actions::update(
        &mut game.movement_sync,
        game.movement_rules.snapshot_interval_ms,
        transition.is_none() && !game.transition_in_flight.get(),
        timestamp_ms,
        movement_observation,
    );
    let needs_recovery = game
        .player
        .stats
        .as_ref()
        .is_some_and(|stats| stats.hp < stats.max_hp || stats.mp < stats.max_mp);
    let can_poll_recovery = game.character_animation.animation == CharacterAnimation::Idle
        && !pick_up
        && !basic_attack
        && skill_id.is_none()
        && transition.is_none()
        && !game.item_action_in_flight.get()
        && !game.save_in_flight.get()
        && !game.skill_action_in_flight.get()
        && !game.transition_in_flight.get()
        && !interaction_busy;
    let recover = recovery_actions::update(
        &mut game.recovery_state,
        needs_recovery,
        can_poll_recovery,
        timestamp_ms,
    );
    let save = save_if_due(game, timestamp_ms);
    FrameUpdate {
        transition,
        basic_attack,
        pick_up,
        movement_snapshot,
        recover,
        save,
        skill_id,
    }
}

fn select_combat_requests(
    attacks_disabled: bool,
    transition_selected: bool,
    transition_active: bool,
    basic_attack_requested: bool,
    skill_id: Option<u32>,
) -> (bool, Option<u32>) {
    let combat_allowed = !attacks_disabled && !transition_selected && !transition_active;
    (
        combat_allowed && basic_attack_requested,
        combat_allowed.then_some(skill_id).flatten(),
    )
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

fn update_player(
    game: &mut Game,
    elapsed_seconds: f32,
    input: PlayerInput,
) -> Option<MapTransition> {
    let position = game.player.position?;
    let previous_motion = game.motion;
    if input.horizontal != 0.0 {
        game.facing_left = input.horizontal < 0.0;
    }
    let output = movement::update_player(
        &game.map,
        &game.movement_rules,
        position,
        game.motion,
        input,
        elapsed_seconds,
    );
    if output.dropped_through {
        let map_id = game.map.id;
        movement_actions::record_drop_through(
            &mut game.movement_sync,
            map_id,
            position,
            previous_motion,
        );
    }
    let animation = character_animation(&game.map, output.state, input);
    let animation_plays = character_animation_plays(animation, position, output.position);
    update_character_animation(
        &mut game.character_animation,
        animation,
        animation_plays,
        game.frame_time_ms,
    );
    game.player.position = Some(output.position);
    game.motion = output.state;
    output.transition
}

fn character_animation(
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

fn new_character_animation_state(
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

fn update_character_animation(
    state: &mut CharacterAnimationState,
    next: CharacterAnimation,
    plays: bool,
    timestamp_ms: f64,
) {
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

fn start_character_attack_animation(game: &mut Game) {
    let duration_ms = crate::character_render::animation_duration_ms(
        &game.character_sprites,
        CharacterAnimation::Attack,
    );
    game.character_animation =
        new_character_animation_state(CharacterAnimation::Attack, true, game.frame_time_ms);
    game.character_animation.one_shot_until_ms =
        Some(game.frame_time_ms + duration_ms.max(1) as f64);
}

fn restart_character_animation(
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

fn save_if_due(
    game: &mut Game,
    timestamp_ms: f64,
) -> Option<PendingSave> {
    if !game.dirty
        || timestamp_ms < game.next_save_ms
        || game.save_in_flight.get()
        || recovery_actions::is_in_flight(&game.recovery_state)
    {
        return None;
    }

    game.dirty = false;
    game.next_save_ms = timestamp_ms + SAVE_INTERVAL_MS;
    game.save_in_flight.set(true);
    Some(PendingSave {
        player: game.player.clone(),
        key_bindings_generation: game.key_bindings_generation,
    })
}

fn begin_save(
    game: Rc<RefCell<Game>>,
    pending: PendingSave,
) {
    let (save_failed, save_in_flight) = {
        let game = game.borrow();
        (game.save_failed.clone(), game.save_in_flight.clone())
    };
    spawn_local(async move {
        let request_started_ms = monotonic_time_ms();
        match api::save_player(pending.player).await {
            Ok(mut response) => {
                let update =
                    api::require_data(response.player.take(), "player").and_then(|player| {
                        api::require_data(response.active_buffs.take(), "active buffs").and_then(
                            |active_buffs| {
                                validate_active_buffs(active_buffs)
                                    .map(|active_buffs| (player, active_buffs))
                                    .map_err(api::ClientError::InvalidResponse)
                            },
                        )
                    });
                match update {
                    Ok((player, active_buffs)) => {
                        let mut game = game.borrow_mut();
                        if game.key_bindings_generation == pending.key_bindings_generation {
                            game.key_bindings_pending = false;
                        }
                        install_full_player_update(&mut game, player);
                        install_active_buffs(&mut game, active_buffs, request_started_ms);
                    }
                    Err(error) => {
                        save_failed.set(true);
                        show_status(&format!("Save failed: {error}"), true);
                    }
                }
            }
            Err(error) => {
                save_failed.set(true);
                show_status(&format!("Save failed: {error}"), true);
            }
        }
        save_in_flight.set(false);
    });
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::Ladder;
    use oozems_proto::v1::Map;
    use oozems_proto::v1::Vec2;

    use super::MorphRefreshState;
    use super::character_animation;
    use super::character_animation_elapsed_ms;
    use super::character_animation_plays;
    use super::new_character_animation_state;
    use super::next_appearance_retry;
    use super::restart_character_animation;
    use super::select_combat_requests;
    use super::update_character_animation;
    use super::update_morph_refresh_request;
    use crate::character_render::CharacterAnimation;
    use crate::movement::MotionState;
    use crate::movement::PlayerInput;

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
            select_combat_requests(true, false, false, true, Some(1)),
            (false, None)
        );
        assert_eq!(
            select_combat_requests(false, true, false, true, Some(1)),
            (false, None)
        );
        assert_eq!(
            select_combat_requests(false, false, true, true, Some(1)),
            (false, None)
        );
        assert_eq!(
            select_combat_requests(false, false, false, true, Some(1)),
            (true, Some(1))
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
