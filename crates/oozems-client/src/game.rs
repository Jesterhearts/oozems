use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use oozems_proto::v1::Map;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::Vec2;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::spawn_local;
use web_sys::CanvasRenderingContext2d;
use web_sys::HtmlCanvasElement;
use web_sys::HtmlImageElement;
use web_sys::KeyboardEvent;

use crate::api;
use crate::movement;
use crate::movement::MapTransition;
use crate::movement::MotionState;
use crate::movement::PlayerInput;
use crate::render;
use crate::show_status;

const PLAYER_ID: &str = "local-player";
const SAVE_INTERVAL_MS: f64 = 2_000.0;

pub struct Game {
    pub canvas: HtmlCanvasElement,
    pub context: CanvasRenderingContext2d,
    pub images: HashMap<String, MapImage>,
    pub map: Map,
    pub player: PlayerState,
    pub frame_time_ms: f64,
    input: Rc<RefCell<InputState>>,
    save_in_flight: Rc<Cell<bool>>,
    transition_in_flight: Rc<Cell<bool>>,
    dirty: bool,
    jump_consumed: bool,
    portal_consumed: bool,
    last_frame_ms: f64,
    next_save_ms: f64,
    motion: MotionState,
}

pub struct MapImage {
    pub element: HtmlImageElement,
    pub requested: Cell<bool>,
    pub url: String,
}

#[derive(Clone, Copy, Default)]
struct InputState {
    left: bool,
    right: bool,
    up: bool,
    down: bool,
    jump: bool,
}

pub async fn run() -> Result<(), String> {
    show_status("Loading saved player...", false);
    let player = api::bootstrap(PLAYER_ID)
        .await
        .map_err(|error| error.to_string())?;
    show_status("Loading map...", false);
    let map = api::get_map(player.map_id)
        .await
        .map_err(|error| error.to_string())?;
    let game = build_game(player, map)?;
    let asset_count = game.borrow().images.len();

    show_status(
        &format!("Ready. Streaming {asset_count} map assets as needed."),
        false,
    );
    schedule_frame(game)?;
    Ok(())
}

fn build_game(
    player: PlayerState,
    map: Map,
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

    let input = Rc::new(RefCell::new(InputState::default()));
    install_keyboard_input(&window, input.clone())?;
    let images = begin_asset_downloads(&map)?;

    Ok(Rc::new(RefCell::new(Game {
        canvas,
        context,
        images,
        map,
        player,
        frame_time_ms: 0.0,
        input,
        save_in_flight: Rc::new(Cell::new(false)),
        transition_in_flight: Rc::new(Cell::new(false)),
        dirty: false,
        jump_consumed: false,
        portal_consumed: false,
        last_frame_ms: 0.0,
        next_save_ms: SAVE_INTERVAL_MS,
        motion: MotionState::default(),
    })))
}

fn begin_asset_downloads(map: &Map) -> Result<HashMap<String, MapImage>, String> {
    map.assets
        .iter()
        .map(|asset| {
            let image = HtmlImageElement::new().map_err(js_error)?;
            image.set_decoding("async");
            Ok((
                asset.id.clone(),
                MapImage {
                    element: image,
                    requested: Cell::new(false),
                    url: asset.url.clone(),
                },
            ))
        })
        .collect()
}

fn install_keyboard_input(
    window: &web_sys::Window,
    input: Rc<RefCell<InputState>>,
) -> Result<(), String> {
    let pressed_input = input.clone();
    let keydown = Closure::<dyn FnMut(KeyboardEvent)>::new(move |event: KeyboardEvent| {
        if set_key(&mut pressed_input.borrow_mut(), &event.code(), true) {
            event.prevent_default();
        }
    });
    window
        .add_event_listener_with_callback("keydown", keydown.as_ref().unchecked_ref())
        .map_err(js_error)?;
    keydown.forget();

    let keyup = Closure::<dyn FnMut(KeyboardEvent)>::new(move |event: KeyboardEvent| {
        if set_key(&mut input.borrow_mut(), &event.code(), false) {
            event.prevent_default();
        }
    });
    window
        .add_event_listener_with_callback("keyup", keyup.as_ref().unchecked_ref())
        .map_err(js_error)?;
    keyup.forget();
    Ok(())
}

fn set_key(
    input: &mut InputState,
    code: &str,
    pressed: bool,
) -> bool {
    match code {
        "ArrowLeft" | "KeyA" => input.left = pressed,
        "ArrowRight" | "KeyD" => input.right = pressed,
        "ArrowUp" | "KeyW" => input.up = pressed,
        "ArrowDown" | "KeyS" => input.down = pressed,
        "Space" => input.jump = pressed,
        _ => return false,
    }
    true
}

fn schedule_frame(game: Rc<RefCell<Game>>) -> Result<(), String> {
    let window = web_sys::window().ok_or("browser window is unavailable")?;
    let callback = Closure::once_into_js(move |timestamp_ms: f64| {
        let transition = update(&mut game.borrow_mut(), timestamp_ms);
        {
            let game = game.borrow();
            render::draw(&game);
        }
        if let Some(transition) = transition {
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

fn update(
    game: &mut Game,
    timestamp_ms: f64,
) -> Option<MapTransition> {
    let elapsed_seconds = if game.last_frame_ms == 0.0 {
        0.0
    } else {
        ((timestamp_ms - game.last_frame_ms) / 1_000.0).clamp(0.0, 0.05) as f32
    };
    game.last_frame_ms = timestamp_ms;
    game.frame_time_ms = timestamp_ms;

    let transition = if game.transition_in_flight.get() {
        None
    } else {
        update_player(game, elapsed_seconds)
    };
    if transition.is_some() {
        game.transition_in_flight.set(true);
    }
    save_if_due(game, timestamp_ms);
    transition
}

fn update_player(
    game: &mut Game,
    elapsed_seconds: f32,
) -> Option<MapTransition> {
    let position = game.player.position?;
    let input = read_player_input(
        *game.input.borrow(),
        &mut game.jump_consumed,
        &mut game.portal_consumed,
    );
    let output = movement::update_player(&game.map, position, game.motion, input, elapsed_seconds);
    game.dirty |= position != output.position;
    game.player.position = Some(output.position);
    game.motion = output.state;
    output.transition
}

fn read_player_input(
    input: InputState,
    jump_consumed: &mut bool,
    portal_consumed: &mut bool,
) -> PlayerInput {
    let jump_pressed = input.jump && !*jump_consumed;
    let portal_pressed = input.up && !*portal_consumed;
    *jump_consumed = input.jump;
    *portal_consumed = input.up;

    PlayerInput {
        horizontal: f32::from(input.right as u8) - f32::from(input.left as u8),
        vertical: f32::from(input.down as u8) - f32::from(input.up as u8),
        jump_pressed,
        portal_pressed,
    }
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
    let images = begin_asset_downloads(&map)?;
    let position = movement::destination_position(&map, &transition.target_portal_name)
        .unwrap_or_else(|| fallback_position(&map));

    game.player.map_id = map.id;
    game.player.position = Some(position);
    game.map = map;
    game.images = images;
    game.motion = MotionState::default();
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
    let save_in_flight = game.save_in_flight.clone();
    let player = game.player.clone();
    spawn_local(async move {
        if let Err(error) = api::save_player(player).await {
            show_status(&format!("Save failed: {error}"), true);
        }
        save_in_flight.set(false);
    });
}

fn js_error(error: JsValue) -> String {
    error
        .as_string()
        .unwrap_or_else(|| "browser operation failed".to_owned())
}

#[cfg(test)]
mod tests {
    use super::InputState;
    use super::set_key;

    #[test]
    fn up_interacts_and_space_jumps() {
        let mut input = InputState::default();

        assert!(set_key(&mut input, "ArrowUp", true));
        assert!(input.up);
        assert!(!input.jump);

        assert!(set_key(&mut input, "Space", true));
        assert!(input.jump);
    }
}
