use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use oozems_proto::v1::Map;
use oozems_proto::v1::PlayerState;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::spawn_local;
use web_sys::CanvasRenderingContext2d;
use web_sys::HtmlCanvasElement;
use web_sys::HtmlImageElement;
use web_sys::KeyboardEvent;

use crate::api;
use crate::render;
use crate::show_status;

const PLAYER_ID: &str = "local-player";
const MOVE_SPEED: f32 = 220.0;
const GRAVITY: f32 = 1_150.0;
const JUMP_SPEED: f32 = 480.0;
const SAVE_INTERVAL_MS: f64 = 2_000.0;

pub struct Game {
    pub canvas: HtmlCanvasElement,
    pub context: CanvasRenderingContext2d,
    pub images: HashMap<String, MapImage>,
    pub map: Map,
    pub player: PlayerState,
    input: Rc<RefCell<InputState>>,
    save_in_flight: Rc<Cell<bool>>,
    dirty: bool,
    jump_consumed: bool,
    last_frame_ms: f64,
    next_save_ms: f64,
    on_ground: bool,
    velocity_y: f32,
}

pub struct MapImage {
    pub element: HtmlImageElement,
    pub requested: Cell<bool>,
    pub url: String,
}

#[derive(Default)]
struct InputState {
    left: bool,
    right: bool,
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
        input,
        save_in_flight: Rc::new(Cell::new(false)),
        dirty: false,
        jump_consumed: false,
        last_frame_ms: 0.0,
        next_save_ms: SAVE_INTERVAL_MS,
        on_ground: false,
        velocity_y: 0.0,
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
        "ArrowUp" | "KeyW" | "Space" => input.jump = pressed,
        _ => return false,
    }
    true
}

fn schedule_frame(game: Rc<RefCell<Game>>) -> Result<(), String> {
    let window = web_sys::window().ok_or("browser window is unavailable")?;
    let callback = Closure::once_into_js(move |timestamp_ms: f64| {
        update(&mut game.borrow_mut(), timestamp_ms);
        render::draw(&game.borrow());
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
) {
    let elapsed_seconds = if game.last_frame_ms == 0.0 {
        0.0
    } else {
        ((timestamp_ms - game.last_frame_ms) / 1_000.0).clamp(0.0, 0.05) as f32
    };
    game.last_frame_ms = timestamp_ms;

    update_player(game, elapsed_seconds);
    save_if_due(game, timestamp_ms);
}

fn update_player(
    game: &mut Game,
    elapsed_seconds: f32,
) {
    let input = game.input.borrow();
    let direction = f32::from(input.right as u8) - f32::from(input.left as u8);
    let jump_requested = input.jump && !game.jump_consumed && game.on_ground;
    if !input.jump {
        game.jump_consumed = false;
    }
    drop(input);

    if jump_requested {
        game.velocity_y = -JUMP_SPEED;
        game.on_ground = false;
        game.jump_consumed = true;
    }

    let Some(position) = game.player.position.as_mut() else {
        return;
    };
    let old_x = position.x;
    let old_y = position.y;
    position.x = (position.x + direction * MOVE_SPEED * elapsed_seconds)
        .clamp(18.0, game.map.width as f32 - 18.0);

    game.velocity_y += GRAVITY * elapsed_seconds;
    let proposed_y = position.y + game.velocity_y * elapsed_seconds;
    let landing_y = find_landing_platform(&game.map, old_x, position.x, position.y, proposed_y);
    if let Some(landing_y) = landing_y {
        position.y = landing_y;
        game.velocity_y = 0.0;
        game.on_ground = true;
    } else {
        position.y = proposed_y.min(game.map.height as f32);
        game.on_ground = false;
    }

    game.dirty |= old_x != position.x || old_y != position.y;
}

fn find_landing_platform(
    map: &Map,
    old_x: f32,
    new_x: f32,
    old_y: f32,
    new_y: f32,
) -> Option<f32> {
    if new_y < old_y {
        return None;
    }

    map.platforms
        .iter()
        .filter_map(|platform| {
            let minimum_x = platform.x.min(platform.end_x);
            let maximum_x = platform.x.max(platform.end_x);
            if new_x < minimum_x - 16.0 || new_x > maximum_x + 16.0 {
                return None;
            }
            let old_surface = platform_y(platform, old_x.clamp(minimum_x, maximum_x))?;
            let new_surface = platform_y(platform, new_x.clamp(minimum_x, maximum_x))?;
            if old_y <= old_surface + 1.0 && new_y >= new_surface {
                Some(new_surface)
            } else {
                None
            }
        })
        .min_by(f32::total_cmp)
}

fn platform_y(
    platform: &oozems_proto::v1::Platform,
    x: f32,
) -> Option<f32> {
    let delta_x = platform.end_x - platform.x;
    if delta_x.abs() < f32::EPSILON {
        return None;
    }
    let progress = (x - platform.x) / delta_x;
    Some(platform.y + progress * (platform.end_y - platform.y))
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
    use oozems_proto::v1::Map;
    use oozems_proto::v1::Platform;

    use super::find_landing_platform;

    #[test]
    fn player_lands_on_a_sloped_foothold() {
        let map = Map {
            platforms: vec![Platform {
                x: 100.0,
                y: 300.0,
                width: 100.0,
                end_x: 200.0,
                end_y: 250.0,
                ..Platform::default()
            }],
            ..Map::default()
        };

        assert_eq!(
            find_landing_platform(&map, 140.0, 150.0, 280.0, 290.0),
            Some(275.0)
        );
    }
}
