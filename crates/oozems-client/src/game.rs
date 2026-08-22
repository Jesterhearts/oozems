use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use oozems_proto::v1::CharacterSpriteSet;
use oozems_proto::v1::GameGui;
use oozems_proto::v1::ItemActionResponse;
use oozems_proto::v1::KeyAction;
use oozems_proto::v1::KeyBinding;
use oozems_proto::v1::Map;
use oozems_proto::v1::PlayerState;
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
use crate::game_gui::GuiAction;
use crate::game_gui::GuiState;
use crate::game_gui::KeyDrag;
use crate::game_gui::PointerButton;
use crate::js_error;
use crate::keymap;
use crate::keymap::KeyboardState;
use crate::movement;
use crate::movement::MapTransition;
use crate::movement::MotionState;
use crate::movement::PlayerInput;
use crate::render;
use crate::show_status;

const SAVE_INTERVAL_MS: f64 = 2_000.0;

pub struct Game {
    pub canvas: HtmlCanvasElement,
    pub character_animation: CharacterAnimation,
    pub character_animation_started_ms: f64,
    pub context: CanvasRenderingContext2d,
    pub character_sprites: CharacterSpriteSet,
    pub facing_left: bool,
    pub gui: GameGui,
    pub gui_state: Rc<RefCell<GuiState>>,
    pub images: HashMap<String, BrowserAsset>,
    pub key_bindings: Rc<RefCell<Vec<KeyBinding>>>,
    pub key_drag: Option<KeyDrag>,
    pub map: Map,
    pub motion: MotionState,
    pub player: PlayerState,
    pub frame_time_ms: f64,
    pub world_layers: Vec<i32>,
    input: Rc<RefCell<KeyboardState>>,
    save_failed: Rc<Cell<bool>>,
    save_in_flight: Rc<std::cell::Cell<bool>>,
    item_action_in_flight: Rc<Cell<bool>>,
    transition_in_flight: Rc<std::cell::Cell<bool>>,
    dirty: bool,
    suppress_click: Rc<Cell<bool>>,
    last_frame_ms: f64,
    next_save_ms: f64,
}

pub async fn run(
    player: PlayerState,
    character_sprites: CharacterSpriteSet,
) -> Result<(), String> {
    show_status("Loading map and GUI...", false);
    let map = api::get_map(player.map_id)
        .await
        .map_err(|error| error.to_string())?;
    let gui_result = api::get_gui().await;
    let gui_warning = gui_result.as_ref().err().map(ToString::to_string);
    let gui = gui_result.unwrap_or_default();
    let game = build_game(player, map, character_sprites, gui)?;
    let asset_count = game.borrow().images.len();

    match gui_warning {
        Some(error) => show_status(
            &format!(
                "Ready. Streaming {asset_count} assets as needed. Using fallback HUD: {error}"
            ),
            true,
        ),
        None => show_status(
            &format!("Ready. Streaming {asset_count} assets as needed."),
            false,
        ),
    }
    schedule_frame(game)?;
    Ok(())
}

fn build_game(
    player: PlayerState,
    map: Map,
    character_sprites: CharacterSpriteSet,
    gui: GameGui,
) -> Result<Rc<RefCell<Game>>, String> {
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
    let images = prepare_game_assets(&map, &character_sprites, &gui)?;
    let motion = player
        .position
        .as_ref()
        .map_or_else(MotionState::default, |position| {
            movement::initial_motion_state(&map, position)
        });
    let world_layers = render::world_layers(&map);

    let game = Rc::new(RefCell::new(Game {
        canvas: canvas.clone(),
        character_animation: CharacterAnimation::Idle,
        character_animation_started_ms: 0.0,
        context,
        character_sprites,
        facing_left: false,
        gui,
        gui_state,
        images,
        key_bindings,
        key_drag: None,
        map,
        motion,
        player,
        frame_time_ms: 0.0,
        world_layers,
        input,
        save_failed: Rc::new(Cell::new(false)),
        save_in_flight: Rc::new(std::cell::Cell::new(false)),
        item_action_in_flight: Rc::new(Cell::new(false)),
        transition_in_flight: Rc::new(std::cell::Cell::new(false)),
        dirty: false,
        suppress_click: Rc::new(Cell::new(false)),
        last_frame_ms: 0.0,
        next_save_ms: SAVE_INTERVAL_MS,
    }));
    install_canvas_input(&canvas, game.clone())?;
    Ok(game)
}

fn prepare_game_assets(
    map: &Map,
    character_sprites: &CharacterSpriteSet,
    gui: &GameGui,
) -> Result<HashMap<String, BrowserAsset>, String> {
    assets::prepare_assets(
        map.assets
            .iter()
            .chain(character_sprites.assets.iter())
            .chain(gui.assets.iter()),
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
                game_gui::begin_key_drag(&game.gui, &game.key_bindings.borrow(), point)
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
        let Some(drag) = game.key_drag.as_mut() else {
            return;
        };
        game_gui::move_key_drag(drag, point);
        event.prevent_default();
    });
    canvas
        .add_event_listener_with_callback("mousemove", mouse_move.as_ref().unchecked_ref())
        .map_err(js_error)?;
    mouse_move.forget();

    let leave_game = game.clone();
    let mouse_leave = Closure::<dyn FnMut(MouseEvent)>::new(move |_event: MouseEvent| {
        leave_game.borrow_mut().key_drag = None;
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
    let action = {
        let game = game.borrow();
        game_gui::click_action(
            *game.gui_state.borrow(),
            &game.gui,
            game.player.inventory.as_ref(),
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
    begin_item_action(game.clone(), action);
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

fn begin_item_action(
    game: Rc<RefCell<Game>>,
    action: GuiAction,
) {
    let in_flight = game.borrow().item_action_in_flight.clone();
    if in_flight.replace(true) {
        show_status("An item action is already in progress.", true);
        return;
    }
    let player_id = game.borrow().player.id.clone();
    spawn_local(async move {
        let result = request_item_action(&player_id, action).await;
        match result {
            Ok(response) => match prepare_item_action_update(&game, action, response).await {
                Ok(update) => {
                    let warning = update.warning.clone();
                    install_item_action_update(&mut game.borrow_mut(), update);
                    match warning {
                        Some(warning) => show_status(&warning, true),
                        None => show_status(item_action_message(action), false),
                    }
                }
                Err(error) => show_status(&format!("Item action could not finish: {error}"), true),
            },
            Err(error) => show_status(&format!("Item action failed: {error}"), true),
        }
        in_flight.set(false);
    });
}

fn begin_pick_up(game: Rc<RefCell<Game>>) {
    let in_flight = game.borrow().item_action_in_flight.clone();
    if in_flight.replace(true) {
        return;
    }
    let request = {
        let game = game.borrow();
        game.player
            .position
            .map(|position| (game.player.id.clone(), game.map.id, position))
    };
    let Some((player_id, map_id, position)) = request else {
        in_flight.set(false);
        show_status("The character does not have a valid pickup position.", true);
        return;
    };
    spawn_local(async move {
        match api::pick_up_item(&player_id, map_id, position).await {
            Ok(response) => {
                let result = response
                    .player
                    .ok_or("server pickup response did not contain a player")
                    .and_then(|player| {
                        (!response.picked_up_drop_id.is_empty())
                            .then_some((player, response.picked_up_drop_id))
                            .ok_or("server pickup response did not identify the dropped item")
                    });
                match result {
                    Ok((player, drop_id)) => {
                        let mut game = game.borrow_mut();
                        game.player.inventory = player.inventory;
                        game.map.dropped_items.retain(|drop| drop.id != drop_id);
                        show_status("Item picked up.", false);
                    }
                    Err(error) => show_status(&format!("Pickup could not finish: {error}"), true),
                }
            }
            Err(error) => show_status(&format!("Pickup failed: {error}"), true),
        }
        in_flight.set(false);
    });
}

async fn request_item_action(
    player_id: &str,
    action: GuiAction,
) -> Result<ItemActionResponse, api::ClientError> {
    match action {
        GuiAction::Equip { inventory_index } => api::equip_item(player_id, inventory_index).await,
        GuiAction::Unequip { slot } => api::unequip_item(player_id, slot).await,
        GuiAction::Drop { inventory_index } => api::drop_item(player_id, inventory_index).await,
        GuiAction::ToggleStats
        | GuiAction::ToggleEquipment
        | GuiAction::ToggleInventory
        | GuiAction::ToggleKeyConfig
        | GuiAction::CloseStats
        | GuiAction::CloseEquipment
        | GuiAction::CloseInventory
        | GuiAction::CloseKeyConfig => unreachable!("local GUI action reached the server"),
    }
}

struct ItemActionUpdate {
    player: PlayerState,
    dropped_item: Option<oozems_proto::v1::DroppedItem>,
    sprites: Option<CharacterSpriteSet>,
    images: Option<HashMap<String, BrowserAsset>>,
    warning: Option<String>,
}

async fn prepare_item_action_update(
    game: &Rc<RefCell<Game>>,
    action: GuiAction,
    response: ItemActionResponse,
) -> Result<ItemActionUpdate, String> {
    let player = response
        .player
        .ok_or("server item response did not contain a player")?;
    let mut sprites = None;
    let mut images = None;
    let mut warning = None;
    if matches!(action, GuiAction::Equip { .. } | GuiAction::Unequip { .. }) {
        let appearance = player
            .appearance
            .ok_or("server item response did not contain an appearance")?;
        let equipment = player
            .inventory
            .as_ref()
            .map(|inventory| inventory.equipment.as_slice())
            .unwrap_or_default();
        match api::get_character_sprites(appearance, Some(equipment)).await {
            Ok(next_sprites) => {
                let prepared = {
                    let game = game.borrow();
                    prepare_game_assets(&game.map, &next_sprites, &game.gui)
                };
                match prepared {
                    Ok(next_images) => {
                        sprites = Some(next_sprites);
                        images = Some(next_images);
                    }
                    Err(error) => {
                        warning = Some(format!(
                            "Item change was saved, but character assets could not refresh: \
                             {error}"
                        ));
                    }
                }
            }
            Err(error) => {
                warning = Some(format!(
                    "Item change was saved, but character sprites could not refresh: {error}"
                ));
            }
        }
    }
    Ok(ItemActionUpdate {
        player,
        dropped_item: response.dropped_item,
        sprites,
        images,
        warning,
    })
}

fn install_item_action_update(
    game: &mut Game,
    update: ItemActionUpdate,
) {
    game.player.inventory = update.player.inventory;
    if let (Some(sprites), Some(images)) = (update.sprites, update.images) {
        game.character_sprites = sprites;
        game.images = images;
        game.character_animation_started_ms = game.frame_time_ms;
    }
    if let Some(drop) = update.dropped_item
        && update.player.map_id == game.map.id
        && drop.despawn_at_unix_ms > js_sys::Date::now().max(0.0) as u64
    {
        game.map.dropped_items.push(drop);
    }
}

fn item_action_message(action: GuiAction) -> &'static str {
    match action {
        GuiAction::Equip { .. } => "Item equipped.",
        GuiAction::Unequip { .. } => "Item moved to inventory.",
        GuiAction::Drop { .. } => "Item dropped.",
        GuiAction::ToggleStats
        | GuiAction::ToggleEquipment
        | GuiAction::ToggleInventory
        | GuiAction::ToggleKeyConfig
        | GuiAction::CloseStats
        | GuiAction::CloseEquipment
        | GuiAction::CloseInventory
        | GuiAction::CloseKeyConfig => "GUI updated.",
    }
}

fn schedule_frame(game: Rc<RefCell<Game>>) -> Result<(), String> {
    let window = web_sys::window().ok_or("browser window is unavailable")?;
    let callback = Closure::once_into_js(move |timestamp_ms: f64| {
        let update = update(&mut game.borrow_mut(), timestamp_ms);
        {
            let game = game.borrow();
            render::draw(&game);
        }
        if update.pick_up {
            begin_pick_up(game.clone());
        }
        if let Some(transition) = update.transition {
            begin_map_transition(game.clone(), transition);
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

struct FrameUpdate {
    transition: Option<MapTransition>,
    pick_up: bool,
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
    if game.save_failed.replace(false) {
        game.dirty = true;
    }
    let now_ms = js_sys::Date::now().max(0.0) as u64;
    game.map
        .dropped_items
        .retain(|drop| drop.despawn_at_unix_ms > now_ms);

    let mut input = {
        let bindings = game.key_bindings.borrow();
        keymap::drain_frame_input(&mut game.input.borrow_mut(), &bindings)
    };
    let key_config_open = game.gui_state.borrow().key_config_open;
    if key_config_open {
        input.player = PlayerInput::default();
        input
            .actions
            .retain(|action| *action == KeyAction::OpenKeyConfig);
    }
    let pick_up = apply_key_actions(&mut game.gui_state.borrow_mut(), &game.gui, &input.actions);

    let transition = if game.transition_in_flight.get() {
        None
    } else {
        update_player(game, elapsed_seconds, input.player)
    };
    if transition.is_some() {
        game.transition_in_flight.set(true);
    }
    save_if_due(game, timestamp_ms);
    FrameUpdate {
        transition,
        pick_up,
    }
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
            KeyAction::PickUp => {
                pick_up = true;
                continue;
            }
            KeyAction::OpenCharacter => GuiAction::ToggleStats,
            KeyAction::OpenEquipment => GuiAction::ToggleEquipment,
            KeyAction::OpenInventory => GuiAction::ToggleInventory,
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
    if input.horizontal != 0.0 {
        game.facing_left = input.horizontal < 0.0;
    }
    let output = movement::update_player(&game.map, position, game.motion, input, elapsed_seconds);
    let animation = character_animation(&game.map, output.state, input);
    update_character_animation(
        &mut game.character_animation,
        &mut game.character_animation_started_ms,
        animation,
        game.frame_time_ms,
    );
    game.dirty |= position != output.position;
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

fn update_character_animation(
    current: &mut CharacterAnimation,
    started_ms: &mut f64,
    next: CharacterAnimation,
    timestamp_ms: f64,
) {
    if *current == next {
        return;
    }
    *current = next;
    *started_ms = timestamp_ms;
}

fn begin_map_transition(
    game: Rc<RefCell<Game>>,
    transition: MapTransition,
) {
    let transition_in_flight = game.borrow().transition_in_flight.clone();
    spawn_local(async move {
        match api::get_map(transition.target_map_id).await {
            Ok(map) => {
                let name = map.name.clone();
                let result = install_map(&mut game.borrow_mut(), map, &transition);
                match result {
                    Ok(()) => show_status(&format!("Entered {name}."), false),
                    Err(error) => show_status(&format!("Could not enter map: {error}"), true),
                }
            }
            Err(error) => show_status(&format!("Could not enter map: {error}"), true),
        }
        transition_in_flight.set(false);
    });
}

fn install_map(
    game: &mut Game,
    map: Map,
    transition: &MapTransition,
) -> Result<(), String> {
    let images = prepare_game_assets(&map, &game.character_sprites, &game.gui)?;
    let position = movement::destination_position(&map, &transition.target_portal_name)
        .unwrap_or_else(|| fallback_position(&map));
    let motion = movement::initial_motion_state(&map, &position);
    let world_layers = render::world_layers(&map);

    game.player.map_id = map.id;
    game.player.position = Some(position);
    game.map = map;
    game.images = images;
    game.motion = motion;
    game.world_layers = world_layers;
    game.character_animation = CharacterAnimation::Idle;
    game.character_animation_started_ms = game.frame_time_ms;
    game.dirty = true;
    Ok(())
}

fn fallback_position(map: &Map) -> Vec2 {
    Vec2 {
        x: map.width as f32 / 2.0,
        y: (map.height as f32 / 2.0).min(map.height as f32),
    }
}

fn save_if_due(
    game: &mut Game,
    timestamp_ms: f64,
) {
    if !game.dirty || timestamp_ms < game.next_save_ms || game.save_in_flight.get() {
        return;
    }

    game.dirty = false;
    game.next_save_ms = timestamp_ms + SAVE_INTERVAL_MS;
    game.save_in_flight.set(true);
    let save_failed = game.save_failed.clone();
    let save_in_flight = game.save_in_flight.clone();
    let player = game.player.clone();
    spawn_local(async move {
        if let Err(error) = api::save_player(player).await {
            save_failed.set(true);
            show_status(&format!("Save failed: {error}"), true);
        }
        save_in_flight.set(false);
    });
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::Ladder;
    use oozems_proto::v1::Map;

    use super::character_animation;
    use super::update_character_animation;
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
        let mut animation = CharacterAnimation::Idle;
        let mut started_ms = 20.0;

        update_character_animation(
            &mut animation,
            &mut started_ms,
            CharacterAnimation::Idle,
            100.0,
        );
        assert_eq!(started_ms, 20.0);

        update_character_animation(
            &mut animation,
            &mut started_ms,
            CharacterAnimation::Walk,
            125.0,
        );
        assert_eq!(animation, CharacterAnimation::Walk);
        assert_eq!(started_ms, 125.0);
    }
}
