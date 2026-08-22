use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use oozems_proto::v1::CharacterAppearance;
use oozems_proto::v1::CharacterCreationOptions;
use oozems_proto::v1::CharacterGender;
use oozems_proto::v1::CharacterSpriteSet;
use oozems_proto::v1::CharacterStyleOption;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::spawn_local;
use web_sys::CanvasRenderingContext2d;
use web_sys::Document;
use web_sys::Event;
use web_sys::HtmlButtonElement;
use web_sys::HtmlCanvasElement;
use web_sys::HtmlInputElement;
use web_sys::HtmlSelectElement;

use crate::PLAYER_ID;
use crate::api;
use crate::assets;
use crate::assets::BrowserAsset;
use crate::character_render;
use crate::character_render::CharacterAnimation;
use crate::character_render::CharacterPlacement;
use crate::game;
use crate::js_error;
use crate::set_visible;
use crate::show_status;

struct Creator {
    active: Cell<bool>,
    button: HtmlButtonElement,
    context: CanvasRenderingContext2d,
    face: HtmlSelectElement,
    gender: HtmlSelectElement,
    hair: HtmlSelectElement,
    images: RefCell<HashMap<String, BrowserAsset>>,
    name: HtmlInputElement,
    options: CharacterCreationOptions,
    preview_generation: Cell<u32>,
    skin: HtmlSelectElement,
    sprites: RefCell<Option<CharacterSpriteSet>>,
}

pub fn show(options: CharacterCreationOptions) -> Result<(), String> {
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or("browser document is unavailable")?;
    validate_options(&options)?;
    let canvas = element::<HtmlCanvasElement>(&document, "character-preview")?;
    let context = canvas
        .get_context("2d")
        .map_err(js_error)?
        .ok_or("2D preview canvas is unavailable")?
        .dyn_into::<CanvasRenderingContext2d>()
        .map_err(|_| "could not create a 2D preview context")?;
    context.set_image_smoothing_enabled(false);

    let creator = Rc::new(Creator {
        active: Cell::new(true),
        button: element(&document, "create-button")?,
        context,
        face: element(&document, "character-face")?,
        gender: element(&document, "character-gender")?,
        hair: element(&document, "character-hair")?,
        images: RefCell::new(HashMap::new()),
        name: element(&document, "character-name")?,
        options,
        preview_generation: Cell::new(0),
        skin: element(&document, "character-skin")?,
        sprites: RefCell::new(None),
    });

    populate_select(&document, &creator.skin, &creator.options.skins)?;
    populate_gendered_styles(&document, &creator)?;
    install_change_handlers(&document, &creator)?;
    install_submit_handler(&document, &creator)?;
    set_visible("character-create", true)?;
    set_visible("game-frame", false)?;
    set_visible("controls", false)?;
    creator.name.focus().map_err(js_error)?;
    request_preview(&creator);
    schedule_preview(creator)?;
    show_status("Choose a name and appearance.", false);
    Ok(())
}

fn validate_options(options: &CharacterCreationOptions) -> Result<(), String> {
    let has_gender = |styles: &[CharacterStyleOption], gender: CharacterGender| {
        styles.iter().any(|style| style.gender == gender as i32)
    };
    if options.skins.is_empty()
        || !has_gender(&options.faces, CharacterGender::Male)
        || !has_gender(&options.faces, CharacterGender::Female)
        || !has_gender(&options.hairs, CharacterGender::Male)
        || !has_gender(&options.hairs, CharacterGender::Female)
    {
        return Err(
            "Character creation is unavailable because Character.wz has no complete styles."
                .to_owned(),
        );
    }
    Ok(())
}

fn install_change_handlers(
    document: &Document,
    creator: &Rc<Creator>,
) -> Result<(), String> {
    let gender_creator = creator.clone();
    let gender_document = document.clone();
    let gender_change = Closure::<dyn FnMut(Event)>::new(move |_| {
        if let Err(error) = populate_gendered_styles(&gender_document, &gender_creator) {
            show_status(&format!("Could not update styles: {error}"), true);
            return;
        }
        request_preview(&gender_creator);
    });
    creator
        .gender
        .add_event_listener_with_callback("change", gender_change.as_ref().unchecked_ref())
        .map_err(js_error)?;
    gender_change.forget();

    for select in [&creator.skin, &creator.face, &creator.hair] {
        let change_creator = creator.clone();
        let change = Closure::<dyn FnMut(Event)>::new(move |_| {
            request_preview(&change_creator);
        });
        select
            .add_event_listener_with_callback("change", change.as_ref().unchecked_ref())
            .map_err(js_error)?;
        change.forget();
    }
    Ok(())
}

fn install_submit_handler(
    document: &Document,
    creator: &Rc<Creator>,
) -> Result<(), String> {
    let form = document
        .get_element_by_id("character-form")
        .ok_or("character form is missing")?;
    let submit_creator = creator.clone();
    let submit = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
        event.prevent_default();
        let name = submit_creator.name.value();
        if let Err(error) = validate_name(&name) {
            show_create_error(&error);
            return;
        }
        let appearance = match selected_appearance(&submit_creator) {
            Ok(appearance) => appearance,
            Err(error) => {
                show_create_error(&error);
                return;
            }
        };
        submit_creator.button.set_disabled(true);
        show_create_error("");
        show_status("Creating character...", false);

        let pending_creator = submit_creator.clone();
        spawn_local(async move {
            let result = create_and_start(name, appearance).await;
            if let Err(error) = result {
                pending_creator.button.set_disabled(false);
                show_create_error(&error);
                show_status("Character creation failed.", true);
                return;
            }
            pending_creator.active.set(false);
        });
    });
    form.add_event_listener_with_callback("submit", submit.as_ref().unchecked_ref())
        .map_err(js_error)?;
    submit.forget();
    Ok(())
}

async fn create_and_start(
    name: String,
    appearance: CharacterAppearance,
) -> Result<(), String> {
    let sprites = api::get_character_sprites(appearance)
        .await
        .map_err(|error| error.to_string())?;
    let player = api::create_character(PLAYER_ID, &name, appearance)
        .await
        .map_err(|error| error.to_string())?;
    set_visible("character-create", false)?;
    set_visible("game-frame", true)?;
    set_visible("controls", true)?;
    game::run(player, sprites).await
}

fn request_preview(creator: &Rc<Creator>) {
    let appearance = match selected_appearance(creator) {
        Ok(appearance) => appearance,
        Err(error) => {
            show_create_error(&error);
            return;
        }
    };
    let generation = creator.preview_generation.get().wrapping_add(1);
    creator.preview_generation.set(generation);
    let pending_creator = creator.clone();
    spawn_local(async move {
        match api::get_character_sprites(appearance).await {
            Ok(sprites) if pending_creator.preview_generation.get() == generation => {
                match assets::prepare_assets(sprites.assets.iter()) {
                    Ok(images) => {
                        *pending_creator.images.borrow_mut() = images;
                        *pending_creator.sprites.borrow_mut() = Some(sprites);
                        show_create_error("");
                    }
                    Err(error) => show_create_error(&error),
                }
            }
            Ok(_) => {}
            Err(error) if pending_creator.preview_generation.get() == generation => {
                show_create_error(&format!("Could not load preview: {error}"));
            }
            Err(_) => {}
        }
    });
}

fn schedule_preview(creator: Rc<Creator>) -> Result<(), String> {
    let window = web_sys::window().ok_or("browser window is unavailable")?;
    let callback = Closure::once_into_js(move |timestamp_ms: f64| {
        draw_preview(&creator, timestamp_ms);
        if creator.active.get()
            && let Err(error) = schedule_preview(creator)
        {
            show_create_error(&format!("Preview stopped: {error}"));
        }
    });
    window
        .request_animation_frame(callback.unchecked_ref())
        .map_err(js_error)?;
    Ok(())
}

fn draw_preview(
    creator: &Creator,
    timestamp_ms: f64,
) {
    creator.context.set_fill_style_str("#b8e1d0");
    creator.context.fill_rect(0.0, 0.0, 180.0, 220.0);
    creator.context.set_fill_style_str("rgba(29, 45, 43, 0.2)");
    creator.context.begin_path();
    let _ = creator
        .context
        .ellipse(90.0, 194.0, 48.0, 9.0, 0.0, 0.0, std::f64::consts::TAU);
    creator.context.fill();

    let sprites = creator.sprites.borrow();
    let Some(sprites) = sprites.as_ref() else {
        return;
    };
    character_render::draw_character(
        &creator.context,
        &creator.images.borrow(),
        sprites,
        CharacterAnimation::Idle,
        timestamp_ms,
        CharacterPlacement {
            anchor_x: 90.0,
            anchor_y: 190.0,
            scale: 2.5,
            facing_left: false,
        },
    );
}

fn populate_gendered_styles(
    document: &Document,
    creator: &Creator,
) -> Result<(), String> {
    let gender = selected_gender(&creator.gender)?;
    let faces = creator
        .options
        .faces
        .iter()
        .filter(|style| style.gender == gender as i32)
        .cloned()
        .collect::<Vec<_>>();
    let hairs = creator
        .options
        .hairs
        .iter()
        .filter(|style| style.gender == gender as i32)
        .cloned()
        .collect::<Vec<_>>();
    populate_select(document, &creator.face, &faces)?;
    populate_select(document, &creator.hair, &hairs)
}

fn populate_select(
    document: &Document,
    select: &HtmlSelectElement,
    options: &[CharacterStyleOption],
) -> Result<(), String> {
    select.set_inner_html("");
    for option in options {
        let element = document.create_element("option").map_err(js_error)?;
        element
            .set_attribute("value", &option.id.to_string())
            .map_err(js_error)?;
        element.set_text_content(Some(&option.label));
        select.append_child(&element).map_err(js_error)?;
    }
    if let Some(first) = options.first() {
        select.set_value(&first.id.to_string());
    }
    Ok(())
}

fn selected_appearance(creator: &Creator) -> Result<CharacterAppearance, String> {
    Ok(CharacterAppearance {
        gender: selected_gender(&creator.gender)? as i32,
        skin_id: selected_id(&creator.skin, "skin")?,
        face_id: selected_id(&creator.face, "face")?,
        hair_id: selected_id(&creator.hair, "hair")?,
    })
}

fn selected_gender(select: &HtmlSelectElement) -> Result<CharacterGender, String> {
    select
        .value()
        .parse::<i32>()
        .ok()
        .and_then(|value| CharacterGender::try_from(value).ok())
        .filter(|gender| matches!(gender, CharacterGender::Male | CharacterGender::Female))
        .ok_or_else(|| "Select a valid gender.".to_owned())
}

fn selected_id(
    select: &HtmlSelectElement,
    label: &str,
) -> Result<u32, String> {
    select
        .value()
        .parse()
        .map_err(|_| format!("Select a valid {label}."))
}

fn validate_name(name: &str) -> Result<(), String> {
    let valid = (3..=12).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_');
    valid
        .then_some(())
        .ok_or_else(|| "Use 3 to 12 letters, digits, or underscores for the name.".to_owned())
}

fn show_create_error(message: &str) {
    let Some(element) = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("create-error"))
    else {
        return;
    };
    element.set_text_content(Some(message));
}

fn element<T>(
    document: &Document,
    id: &str,
) -> Result<T, String>
where
    T: JsCast,
{
    document
        .get_element_by_id(id)
        .ok_or_else(|| format!("{id} element is missing"))?
        .dyn_into::<T>()
        .map_err(|_| format!("{id} has the wrong element type"))
}

#[cfg(test)]
mod tests {
    use super::validate_name;

    #[test]
    fn character_names_have_one_shared_boundary_rule() {
        assert!(validate_name("Mina_7").is_ok());
        assert!(validate_name("ab").is_err());
        assert!(validate_name("Maple Hero").is_err());
        assert!(validate_name("abcdefghijklz").is_err());
    }
}
