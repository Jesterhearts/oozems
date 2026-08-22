#![forbid(unsafe_code)]

mod api;
mod assets;
mod character_create;
mod character_render;
mod game;
mod movement;
mod render;

use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen_futures::spawn_local;

pub(crate) const PLAYER_ID: &str = "local-player";

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    spawn_local(async {
        if let Err(error) = start_client().await {
            show_status(&format!("Could not start: {error}"), true);
        }
    });
}

async fn start_client() -> Result<(), String> {
    show_status("Loading saved player...", false);
    let bootstrap = api::bootstrap(PLAYER_ID)
        .await
        .map_err(|error| error.to_string())?;
    match bootstrap.player {
        Some(player) => {
            let appearance = player
                .appearance
                .ok_or("saved player has no character appearance")?;
            show_status("Loading character...", false);
            let sprites = api::get_character_sprites(appearance)
                .await
                .map_err(|error| error.to_string())?;
            set_visible("character-create", false)?;
            set_visible("game-frame", true)?;
            set_visible("controls", true)?;
            game::run(player, sprites).await
        }
        None => character_create::show(bootstrap.creation_options.unwrap_or_default()),
    }
}

pub(crate) fn show_status(
    message: &str,
    is_error: bool,
) {
    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        return;
    };
    let Some(status) = document.get_element_by_id("status") else {
        return;
    };

    status.set_text_content(Some(message));
    status.set_class_name(if is_error { "status error" } else { "status" });
}

pub(crate) fn set_visible(
    id: &str,
    visible: bool,
) -> Result<(), String> {
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or("browser document is unavailable")?;
    let element = document
        .get_element_by_id(id)
        .ok_or_else(|| format!("{id} element is missing"))?;
    if visible {
        element.remove_attribute("hidden").map_err(js_error)
    } else {
        element.set_attribute("hidden", "").map_err(js_error)
    }
}

pub(crate) fn js_error(error: JsValue) -> String {
    error
        .as_string()
        .unwrap_or_else(|| "browser operation failed".to_owned())
}
