use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use oozems_proto::v1::CharacterSpriteSet;
use oozems_proto::v1::GameGui;
use oozems_proto::v1::Map;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::Vec2;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::spawn_local;
use web_sys::CanvasRenderingContext2d;
use web_sys::HtmlCanvasElement;
use web_sys::KeyboardEvent;

use crate::api;
use crate::assets;
use crate::assets::BrowserAsset;
use crate::character_render::CharacterAnimation;
use crate::js_error;
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
    pub images: HashMap<String, BrowserAsset>,
    pub map: Map,
    pub player: PlayerState,
    pub frame_time_ms: f64,
    input: Rc<RefCell<InputState>>,
    save_in_flight: Rc<std::cell::Cell<bool>>,
    transition_in_flight: Rc<std::cell::Cell<bool>>,
    dirty: bool,
    jump_consumed: bool,
    portal_consumed: bool,
    last_frame_ms: f64,
    next_save_ms: f64,
    motion: MotionState,
}

#[derive(Clone, Copy, Default)]
struct InputState {
    left: bool,
    right: bool,
    up: bool,
    down: bool,
    jump: bool,
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

    let input = Rc::new(RefCell::new(InputState::default()));
    install_keyboard_input(&window, input.clone())?;
    let images = prepare_game_assets(&map, &character_sprites, &gui)?;

    Ok(Rc::new(RefCell::new(Game {
        canvas,
        character_animation: CharacterAnimation::Idle,
        character_animation_started_ms: 0.0,
        context,
        character_sprites,
        facing_left: false,
        gui,
        images,
        map,
        player,
        frame_time_ms: 0.0,
        input,
        save_in_flight: Rc::new(std::cell::Cell::new(false)),
        transition_in_flight: Rc::new(std::cell::Cell::new(false)),
        dirty: false,
        jump_consumed: false,
        portal_consumed: false,
        last_frame_ms: 0.0,
        next_save_ms: SAVE_INTERVAL_MS,
        motion: MotionState::default(),
    })))
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
    let images = prepare_game_assets(&map, &game.character_sprites, &game.gui)?;
    let position = movement::destination_position(&map, &transition.target_portal_name)
        .unwrap_or_else(|| fallback_position(&map));

    game.player.map_id = map.id;
    game.player.position = Some(position);
    game.map = map;
    game.images = images;
    game.motion = MotionState::default();
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
    let save_in_flight = game.save_in_flight.clone();
    let player = game.player.clone();
    spawn_local(async move {
        if let Err(error) = api::save_player(player).await {
            show_status(&format!("Save failed: {error}"), true);
        }
        save_in_flight.set(false);
    });
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::Ladder;
    use oozems_proto::v1::Map;

    use super::InputState;
    use super::character_animation;
    use super::set_key;
    use super::update_character_animation;
    use crate::character_render::CharacterAnimation;
    use crate::movement::MotionState;
    use crate::movement::PlayerInput;

    #[test]
    fn up_interacts_and_space_jumps() {
        let mut input = InputState::default();

        assert!(set_key(&mut input, "ArrowUp", true));
        assert!(input.up);
        assert!(!input.jump);

        assert!(set_key(&mut input, "Space", true));
        assert!(input.jump);
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
