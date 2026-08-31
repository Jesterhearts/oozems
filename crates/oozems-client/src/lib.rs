#![forbid(unsafe_code)]

mod animation;
mod api;
mod assets;
mod audio;
mod cash_shop_ui;
mod character_create;
mod character_render;
mod death_ui;
mod game;
mod game_gui;
mod gui_dump;
mod hit_test;
mod interaction_ui;
mod item_pickup;
mod keymap;
mod level_up_effect;
mod mob_render;
mod morph_render;
mod movement;
mod quest_journal;
mod quest_tracker;
mod reactor_render;
mod render;
mod skill_effects;

pub use gui_dump::dump_gui;
use wasm_bindgen::JsValue;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen_futures::spawn_local;

pub(crate) const PLAYER_ID: &str = "local-player";
const AUTOMATIC_RELOAD_COOLDOWN_MS: f64 = 30_000.0;
const AUTOMATIC_RELOAD_STORAGE_KEY: &str = "oozems.last-automatic-bootstrap-reload-ms";

pub(crate) enum AutomaticReload {
    Started,
    Suppressed,
}

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
    let bootstrap_requested_at_ms = game::monotonic_time_ms();
    let mut bootstrap = api::bootstrap(PLAYER_ID)
        .await
        .map_err(|error| error.to_string())?;
    match bootstrap.player.take() {
        Some(player) => {
            let bootstrap_active_buffs =
                api::require_data(bootstrap.active_buffs.take(), "active buffs")
                    .map_err(|error| error.to_string())?;
            let appearance = player
                .appearance
                .ok_or("saved player has no character appearance")?;
            let equipment = player
                .inventory
                .as_ref()
                .ok_or("saved player has no inventory")?
                .equipment
                .as_slice();
            show_status("Loading character...", false);
            let sprites = api::get_character_sprites(appearance, Some(equipment))
                .await
                .map_err(|error| error.to_string())?;
            set_visible("character-create", false)?;
            set_visible("game-frame", true)?;
            set_visible("controls", true)?;
            game::run(
                player,
                sprites,
                bootstrap_active_buffs,
                bootstrap_requested_at_ms,
            )
            .await
        }
        None => {
            let creation_options = api::require_data(
                bootstrap.creation_options.take(),
                "character creation options",
            )
            .map_err(|error| error.to_string())?;
            character_create::show(creation_options)
        }
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

pub(crate) fn reload_client() -> Result<AutomaticReload, String> {
    let window = web_sys::window().ok_or("browser window is unavailable")?;
    let storage = window
        .session_storage()
        .map_err(js_error)?
        .ok_or("browser session storage is unavailable")?;
    let now_ms = js_sys::Date::now();
    let previous_reload_ms = storage
        .get_item(AUTOMATIC_RELOAD_STORAGE_KEY)
        .map_err(js_error)?
        .and_then(|value| value.parse::<f64>().ok());
    if !automatic_reload_allowed(now_ms, previous_reload_ms) {
        return Ok(AutomaticReload::Suppressed);
    }
    storage
        .set_item(AUTOMATIC_RELOAD_STORAGE_KEY, &now_ms.to_string())
        .map_err(js_error)?;
    window.location().reload().map_err(js_error)?;
    Ok(AutomaticReload::Started)
}

fn automatic_reload_allowed(
    now_ms: f64,
    previous_reload_ms: Option<f64>,
) -> bool {
    !previous_reload_ms.is_some_and(|previous_ms| {
        previous_ms.is_finite()
            && now_ms >= previous_ms
            && now_ms - previous_ms < AUTOMATIC_RELOAD_COOLDOWN_MS
    })
}

pub(crate) fn js_error(error: JsValue) -> String {
    error
        .as_string()
        .unwrap_or_else(|| "browser operation failed".to_owned())
}

#[cfg(test)]
mod tests {
    use super::automatic_reload_allowed;

    #[test]
    fn automatic_reload_is_suppressed_during_the_cooldown() {
        assert!(automatic_reload_allowed(100_000.0, None));
        assert!(!automatic_reload_allowed(100_000.0, Some(90_000.0)));
        assert!(automatic_reload_allowed(100_000.0, Some(70_000.0)));
        assert!(automatic_reload_allowed(100_000.0, Some(110_000.0)));
    }
}
