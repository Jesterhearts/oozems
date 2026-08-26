use std::cell::Cell;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use oozems_proto::v1::CharacterAppearance;
use oozems_proto::v1::CharacterCreationOptions;
use oozems_proto::v1::CharacterEquipmentOption;
use oozems_proto::v1::CharacterGender;
use oozems_proto::v1::CharacterSpriteSet;
use oozems_proto::v1::CharacterStyleOption;
use oozems_proto::v1::EquipmentSlot;
use oozems_proto::v1::EquippedItem;
use oozems_proto::v1::StartingEquipmentSelection;
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
    bottom: HtmlSelectElement,
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
    shoes: HtmlSelectElement,
    sprites: RefCell<Option<CharacterSpriteSet>>,
    top: HtmlSelectElement,
    weapon: HtmlSelectElement,
}

thread_local! {
    static EVENT_HANDLERS: RefCell<Option<CreatorEventHandlers>> = const { RefCell::new(None) };
}

type EventClosure = Closure<dyn FnMut(Event)>;

struct CreatorEventHandlers {
    gender: HtmlSelectElement,
    gender_change: EventClosure,
    styles: Vec<(HtmlSelectElement, EventClosure)>,
    form: web_sys::Element,
    submit: EventClosure,
}

impl Drop for CreatorEventHandlers {
    fn drop(&mut self) {
        let _ = self.gender.remove_event_listener_with_callback(
            "change",
            self.gender_change.as_ref().unchecked_ref(),
        );
        for (select, change) in &self.styles {
            let _ = select
                .remove_event_listener_with_callback("change", change.as_ref().unchecked_ref());
        }
        let _ = self
            .form
            .remove_event_listener_with_callback("submit", self.submit.as_ref().unchecked_ref());
    }
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
        bottom: element(&document, "character-bottom")?,
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
        shoes: element(&document, "character-shoes")?,
        sprites: RefCell::new(None),
        top: element(&document, "character-top")?,
        weapon: element(&document, "character-weapon")?,
    });

    populate_select(&document, &creator.skin, &creator.options.skins)?;
    populate_equipment_select(
        &document,
        &creator.top,
        &creator.options.equipment,
        EquipmentSlot::Top,
    )?;
    populate_equipment_select(
        &document,
        &creator.bottom,
        &creator.options.equipment,
        EquipmentSlot::Bottom,
    )?;
    populate_equipment_select(
        &document,
        &creator.shoes,
        &creator.options.equipment,
        EquipmentSlot::Shoes,
    )?;
    populate_equipment_select(
        &document,
        &creator.weapon,
        &creator.options.equipment,
        EquipmentSlot::Weapon,
    )?;
    populate_gendered_styles(&document, &creator)?;
    let event_handlers = install_event_handlers(&document, &creator)?;
    EVENT_HANDLERS.with(|current| {
        current.replace(Some(event_handlers));
    });
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
        || [
            EquipmentSlot::Top,
            EquipmentSlot::Bottom,
            EquipmentSlot::Shoes,
            EquipmentSlot::Weapon,
        ]
        .into_iter()
        .any(|slot| {
            !options
                .equipment
                .iter()
                .any(|option| option.slot == slot as i32)
        })
    {
        return Err(
            "Character creation is unavailable because Character.wz has no complete styles."
                .to_owned(),
        );
    }
    Ok(())
}

fn install_event_handlers(
    document: &Document,
    creator: &Rc<Creator>,
) -> Result<CreatorEventHandlers, String> {
    let form = document
        .get_element_by_id("character-form")
        .ok_or("character form is missing")?;
    let gender_creator = creator.clone();
    let gender_document = document.clone();
    let gender_change = Closure::<dyn FnMut(Event)>::new(move |_| {
        if let Err(error) = populate_gendered_styles(&gender_document, &gender_creator) {
            show_status(&format!("Could not update styles: {error}"), true);
            return;
        }
        request_preview(&gender_creator);
    });
    let styles = [
        &creator.skin,
        &creator.face,
        &creator.hair,
        &creator.top,
        &creator.bottom,
        &creator.shoes,
        &creator.weapon,
    ]
    .into_iter()
    .map(|select| {
        let change_creator = creator.clone();
        let change = Closure::<dyn FnMut(Event)>::new(move |_| {
            request_preview(&change_creator);
        });
        (select.clone(), change)
    })
    .collect::<Vec<_>>();
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
        let equipment = match selected_equipment(&submit_creator) {
            Ok(equipment) => equipment,
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
            let result = create_and_start(name, appearance, equipment).await;
            if let Err(error) = result {
                pending_creator.button.set_disabled(false);
                show_create_error(&error);
                show_status("Character creation failed.", true);
                return;
            }
            pending_creator.active.set(false);
            EVENT_HANDLERS.with(|current| {
                current.take();
            });
        });
    });
    let handlers = CreatorEventHandlers {
        gender: creator.gender.clone(),
        gender_change,
        styles,
        form,
        submit,
    };
    handlers
        .gender
        .add_event_listener_with_callback("change", handlers.gender_change.as_ref().unchecked_ref())
        .map_err(js_error)?;
    for (select, change) in &handlers.styles {
        select
            .add_event_listener_with_callback("change", change.as_ref().unchecked_ref())
            .map_err(js_error)?;
    }
    handlers
        .form
        .add_event_listener_with_callback("submit", handlers.submit.as_ref().unchecked_ref())
        .map_err(js_error)?;
    Ok(handlers)
}

async fn create_and_start(
    name: String,
    appearance: CharacterAppearance,
    equipment: Vec<StartingEquipmentSelection>,
) -> Result<(), String> {
    let equipped = sprite_equipment(&equipment);
    let sprites = api::get_character_sprites(appearance, Some(&equipped))
        .await
        .map_err(|error| error.to_string())?;
    let player = api::create_character(PLAYER_ID, &name, appearance, equipment)
        .await
        .map_err(|error| error.to_string())?;
    set_visible("character-create", false)?;
    set_visible("game-frame", true)?;
    set_visible("controls", true)?;
    game::run(
        player,
        sprites,
        oozems_proto::v1::ActiveBuffState::default(),
        game::monotonic_time_ms(),
    )
    .await
}

fn request_preview(creator: &Rc<Creator>) {
    let appearance = match selected_appearance(creator) {
        Ok(appearance) => appearance,
        Err(error) => {
            show_create_error(&error);
            return;
        }
    };
    let equipment = match selected_equipment(creator) {
        Ok(equipment) => equipment,
        Err(error) => {
            show_create_error(&error);
            return;
        }
    };
    let generation = creator.preview_generation.get().wrapping_add(1);
    creator.preview_generation.set(generation);
    let pending_creator = creator.clone();
    spawn_local(async move {
        let equipped = sprite_equipment(&equipment);
        match api::get_character_sprites(appearance, Some(&equipped)).await {
            Ok(sprites) if pending_creator.preview_generation.get() == generation => {
                match assets::prepare_assets(sprites.assets.iter()) {
                    Ok(images) => {
                        assets::merge_assets(&mut pending_creator.images.borrow_mut(), images);
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

fn populate_equipment_select(
    document: &Document,
    select: &HtmlSelectElement,
    options: &[CharacterEquipmentOption],
    slot: EquipmentSlot,
) -> Result<(), String> {
    select.set_inner_html("");
    let mut first_id = None;
    for option in options.iter().filter(|option| option.slot == slot as i32) {
        let element = document.create_element("option").map_err(js_error)?;
        element
            .set_attribute("value", &option.item_id.to_string())
            .map_err(js_error)?;
        element.set_text_content(Some(&option.label));
        select.append_child(&element).map_err(js_error)?;
        first_id.get_or_insert(option.item_id);
    }
    let first_id = first_id.ok_or_else(|| format!("no starting {slot:?} options are available"))?;
    select.set_value(&first_id.to_string());
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

fn selected_equipment(creator: &Creator) -> Result<Vec<StartingEquipmentSelection>, String> {
    [
        (EquipmentSlot::Top, &creator.top),
        (EquipmentSlot::Bottom, &creator.bottom),
        (EquipmentSlot::Shoes, &creator.shoes),
        (EquipmentSlot::Weapon, &creator.weapon),
    ]
    .into_iter()
    .map(|(slot, select)| {
        Ok(StartingEquipmentSelection {
            slot: slot as i32,
            item_id: selected_id(select, "equipment")?,
        })
    })
    .collect()
}

fn sprite_equipment(selections: &[StartingEquipmentSelection]) -> Vec<EquippedItem> {
    selections
        .iter()
        .map(|selection| EquippedItem {
            slot: selection.slot,
            item_id: selection.item_id,
            expires_at_unix_ms: 0,
        })
        .collect()
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
