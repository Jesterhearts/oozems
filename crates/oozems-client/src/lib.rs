#![forbid(unsafe_code)]

mod api;
mod game;
mod render;

use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen_futures::spawn_local;

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    spawn_local(async {
        if let Err(error) = game::run().await {
            show_status(&format!("Could not start: {error}"), true);
        }
    });
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
