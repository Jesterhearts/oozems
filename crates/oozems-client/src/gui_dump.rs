use std::cell::RefCell;
use std::rc::Rc;
use std::rc::Weak;

use oozems_proto::v1::GameGui;
use oozems_proto::v1::GuiLayout;
use oozems_proto::v1::GuiWindow;
use oozems_proto::v1::npc_interaction;
use wasm_bindgen::JsCast;
use wasm_bindgen::JsValue;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_futures::future_to_promise;
use web_sys::Blob;
use web_sys::CanvasRenderingContext2d;
use web_sys::HtmlAnchorElement;
use web_sys::HtmlCanvasElement;
use web_sys::Url;

use crate::cash_shop_ui;
use crate::game::Game;
use crate::game_gui;
use crate::js_error;

const GUI_ELEMENT_NAMES: &str = "game, status-bar, stat-window, equipment-window, \
                                 inventory-window, key-config-window, skill-window, \
                                 npc-dialog-window, shop-window, cash-shop-window";

type DumpGuiBridge = Closure<dyn Fn(String, f64, f64, f64, f64, String) -> js_sys::Promise>;

thread_local! {
    static ACTIVE_GAME: RefCell<Weak<RefCell<Game>>> = const { RefCell::new(Weak::new()) };
    static DUMP_GUI_BRIDGE: RefCell<Option<DumpGuiBridge>> = const { RefCell::new(None) };
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PixelRect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum InteractionElement {
    #[default]
    None,
    NpcDialog,
    Shop,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct VisibleGui {
    cash_shop: bool,
    equipment: bool,
    interaction: InteractionElement,
    inventory: bool,
    key_config: bool,
    skill: bool,
    stat: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GuiElement {
    Game,
    StatusBar,
    StatWindow,
    EquipmentWindow,
    InventoryWindow,
    KeyConfigWindow,
    SkillWindow,
    NpcDialogWindow,
    ShopWindow,
    CashShopWindow,
}

impl GuiElement {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "game" => Ok(Self::Game),
            "status-bar" => Ok(Self::StatusBar),
            "stat-window" => Ok(Self::StatWindow),
            "equipment-window" => Ok(Self::EquipmentWindow),
            "inventory-window" => Ok(Self::InventoryWindow),
            "key-config-window" => Ok(Self::KeyConfigWindow),
            "skill-window" => Ok(Self::SkillWindow),
            "npc-dialog-window" => Ok(Self::NpcDialogWindow),
            "shop-window" => Ok(Self::ShopWindow),
            "cash-shop-window" => Ok(Self::CashShopWindow),
            _ => Err(format!(
                "unknown GUI element '{value}'; expected one of: {GUI_ELEMENT_NAMES}"
            )),
        }
    }
}

pub(crate) fn install(game: &Rc<RefCell<Game>>) {
    ACTIVE_GAME.with(|active| {
        *active.borrow_mut() = Rc::downgrade(game);
    });
    if let Err(error) = install_browser_bridge() {
        web_sys::console::warn_1(&JsValue::from_str(&format!(
            "Could not install the GUI dump bridge: {error}"
        )));
    }
}

/// Downloads a cropped PNG of a currently rendered game GUI element.
///
/// The crop coordinates are backing-canvas pixels relative to the selected
/// element, not CSS pixels or absolute game-canvas coordinates. The returned
/// promise resolves after the browser encodes the PNG and starts its download.
#[wasm_bindgen(js_name = dumpGui)]
pub fn dump_gui(
    element: String,
    crop_x: f64,
    crop_y: f64,
    crop_width: f64,
    crop_height: f64,
    output_file: String,
) -> js_sys::Promise {
    future_to_promise(async move {
        let crop = parse_crop(crop_x, crop_y, crop_width, crop_height)
            .map_err(|error| JsValue::from_str(&error))?;
        dump_gui_inner(&element, crop, &output_file)
            .await
            .map_err(|error| JsValue::from_str(&error))?;
        Ok(JsValue::UNDEFINED)
    })
}

fn install_browser_bridge() -> Result<(), String> {
    let bridge_is_installed = DUMP_GUI_BRIDGE.with(|bridge| bridge.borrow().is_some());
    if bridge_is_installed {
        return Ok(());
    }

    let bridge = Closure::<dyn Fn(String, f64, f64, f64, f64, String) -> js_sys::Promise>::new(
        |element: String,
         crop_x: f64,
         crop_y: f64,
         crop_width: f64,
         crop_height: f64,
         output_file: String| {
            dump_gui(
                element,
                crop_x,
                crop_y,
                crop_width,
                crop_height,
                output_file,
            )
        },
    );
    let window = web_sys::window().ok_or("browser window is unavailable")?;
    js_sys::Reflect::set(
        window.as_ref(),
        &JsValue::from_str("oozemsDumpGui"),
        bridge.as_ref(),
    )
    .map_err(js_error)?;
    DUMP_GUI_BRIDGE.with(|installed| {
        installed.replace(Some(bridge));
    });
    Ok(())
}

async fn dump_gui_inner(
    element: &str,
    crop: PixelRect,
    output_file: &str,
) -> Result<(), String> {
    validate_output_file(output_file)?;
    let active = ACTIVE_GAME
        .with(|active| active.borrow().upgrade())
        .ok_or("the game GUI is not running")?;
    let (canvas, source) = {
        let game = active
            .try_borrow()
            .map_err(|_| "the game GUI is busy; retry the dump after this frame")?;
        let canvas_width = game.surface.canvas.width();
        let canvas_height = game.surface.canvas.height();
        let visible = visible_gui(&game)?;
        let bounds = resolve_element_bounds(
            &game.ui.gui,
            visible,
            canvas_width,
            canvas_height,
            GuiElement::parse(element)?,
        )?;
        let element_pixels = visible_pixels(bounds, canvas_width, canvas_height)?;
        let source = apply_crop(element_pixels, crop)?;
        (game.surface.canvas.clone(), source)
    };
    download_png(&canvas, source, output_file).await
}

fn parse_crop(
    x: f64,
    y: f64,
    width: f64,
    height: f64,
) -> Result<PixelRect, String> {
    Ok(PixelRect {
        x: parse_crop_value("x", x)?,
        y: parse_crop_value("y", y)?,
        width: parse_crop_value("width", width)?,
        height: parse_crop_value("height", height)?,
    })
}

fn parse_crop_value(
    name: &str,
    value: f64,
) -> Result<u32, String> {
    if !value.is_finite() || value < 0.0 || value.fract() != 0.0 || value > f64::from(u32::MAX) {
        return Err(format!(
            "GUI crop {name} must be a nonnegative 32-bit integer"
        ));
    }
    Ok(value as u32)
}

fn visible_gui(game: &Game) -> Result<VisibleGui, String> {
    let state = game
        .ui
        .gui_state
        .try_borrow()
        .map_err(|_| "the game GUI state is busy; retry the dump after this frame")?;
    let interaction = game
        .ui
        .interaction
        .interaction
        .as_ref()
        .and_then(|interaction| interaction.view.as_ref())
        .map_or(InteractionElement::None, |view| match view {
            npc_interaction::View::Dialog(_) | npc_interaction::View::Taxi(_) => {
                InteractionElement::NpcDialog
            }
            npc_interaction::View::Shop(_) => InteractionElement::Shop,
        });
    Ok(VisibleGui {
        cash_shop: game.ui.cash_shop.open,
        equipment: state.equipment_open,
        interaction,
        inventory: state.inventory_open,
        key_config: state.key_config_open,
        skill: state.skills_open,
        stat: state.stats_open,
    })
}

fn resolve_element_bounds(
    gui: &GameGui,
    visible: VisibleGui,
    canvas_width: u32,
    canvas_height: u32,
    element: GuiElement,
) -> Result<Rect, String> {
    let standard_gui_visible = !visible.cash_shop;
    match element {
        GuiElement::Game => Ok(Rect {
            x: 0.0,
            y: 0.0,
            width: f64::from(canvas_width),
            height: f64::from(canvas_height),
        }),
        GuiElement::StatusBar => {
            require_visible(standard_gui_visible, "status-bar")?;
            status_bar_bounds(gui.status_bar.as_ref(), canvas_width, canvas_height)
        }
        GuiElement::StatWindow => window_element_bounds(
            gui.stat_window.as_ref(),
            standard_gui_visible && visible.stat,
            "stat-window",
        ),
        GuiElement::EquipmentWindow => window_element_bounds(
            gui.equipment_window.as_ref(),
            standard_gui_visible && visible.equipment,
            "equipment-window",
        ),
        GuiElement::InventoryWindow => window_element_bounds(
            gui.inventory_window.as_ref(),
            standard_gui_visible && visible.inventory,
            "inventory-window",
        ),
        GuiElement::KeyConfigWindow => window_element_bounds(
            gui.key_config_window.as_ref(),
            standard_gui_visible && visible.key_config,
            "key-config-window",
        ),
        GuiElement::SkillWindow => window_element_bounds(
            gui.skill_window.as_ref(),
            standard_gui_visible && visible.skill,
            "skill-window",
        ),
        GuiElement::NpcDialogWindow => window_element_bounds(
            gui.npc_dialog_window.as_ref(),
            standard_gui_visible && visible.interaction == InteractionElement::NpcDialog,
            "npc-dialog-window",
        ),
        GuiElement::ShopWindow => window_element_bounds(
            gui.shop_window.as_ref(),
            standard_gui_visible && visible.interaction == InteractionElement::Shop,
            "shop-window",
        ),
        GuiElement::CashShopWindow => {
            require_visible(visible.cash_shop, "cash-shop-window")?;
            cash_shop_bounds(gui.cash_shop_window.as_ref(), canvas_width, canvas_height)
        }
    }
}

fn require_visible(
    visible: bool,
    name: &str,
) -> Result<(), String> {
    if visible {
        Ok(())
    } else {
        Err(format!("GUI element '{name}' is not currently rendered"))
    }
}

fn status_bar_bounds(
    layout: Option<&GuiLayout>,
    canvas_width: u32,
    canvas_height: u32,
) -> Result<Rect, String> {
    let layout = valid_layout(layout, "status-bar")?;
    Ok(Rect {
        x: 0.0,
        y: f64::from(game_gui::status_bar_top(
            canvas_height as f32,
            layout.height,
        )),
        width: f64::from(canvas_width),
        height: f64::from(layout.height),
    })
}

fn window_element_bounds(
    window: Option<&GuiWindow>,
    visible: bool,
    name: &str,
) -> Result<Rect, String> {
    require_visible(visible, name)?;
    let window = window.ok_or_else(|| format!("GUI element '{name}' is unavailable"))?;
    let layout = valid_layout(window.layout.as_ref(), name)?;
    Ok(Rect {
        x: f64::from(window.x),
        y: f64::from(window.y),
        width: f64::from(layout.width),
        height: f64::from(layout.height),
    })
}

fn cash_shop_bounds(
    window: Option<&GuiWindow>,
    canvas_width: u32,
    canvas_height: u32,
) -> Result<Rect, String> {
    let name = "cash-shop-window";
    let window = window.ok_or_else(|| format!("GUI element '{name}' is unavailable"))?;
    let layout = valid_layout(window.layout.as_ref(), name)?;
    let transform =
        cash_shop_ui::screen_transform(window, canvas_width as f32, canvas_height as f32)
            .ok_or_else(|| format!("GUI element '{name}' has invalid screen geometry"))?;
    Ok(Rect {
        x: f64::from(transform.origin_x + window.x * transform.scale),
        y: f64::from(transform.origin_y + window.y * transform.scale),
        width: f64::from(layout.width * transform.scale),
        height: f64::from(layout.height * transform.scale),
    })
}

fn valid_layout<'a>(
    layout: Option<&'a GuiLayout>,
    name: &str,
) -> Result<&'a GuiLayout, String> {
    layout
        .filter(|layout| game_gui::valid_layout(layout))
        .ok_or_else(|| format!("GUI element '{name}' has no valid layout"))
}

fn visible_pixels(
    bounds: Rect,
    canvas_width: u32,
    canvas_height: u32,
) -> Result<PixelRect, String> {
    if !bounds.x.is_finite()
        || !bounds.y.is_finite()
        || !bounds.width.is_finite()
        || !bounds.height.is_finite()
        || bounds.width <= 0.0
        || bounds.height <= 0.0
    {
        return Err("the selected GUI element has invalid bounds".to_owned());
    }

    let left = bounds.x.max(0.0).floor();
    let top = bounds.y.max(0.0).floor();
    let right = (bounds.x + bounds.width)
        .min(f64::from(canvas_width))
        .ceil();
    let bottom = (bounds.y + bounds.height)
        .min(f64::from(canvas_height))
        .ceil();
    if right <= left || bottom <= top {
        return Err("the selected GUI element is outside the game canvas".to_owned());
    }

    Ok(PixelRect {
        x: left as u32,
        y: top as u32,
        width: (right - left) as u32,
        height: (bottom - top) as u32,
    })
}

fn apply_crop(
    element: PixelRect,
    crop: PixelRect,
) -> Result<PixelRect, String> {
    if crop.width == 0 || crop.height == 0 {
        return Err("the GUI crop width and height must be positive".to_owned());
    }
    let crop_right = crop
        .x
        .checked_add(crop.width)
        .ok_or("the GUI crop overflows its element bounds")?;
    let crop_bottom = crop
        .y
        .checked_add(crop.height)
        .ok_or("the GUI crop overflows its element bounds")?;
    if crop_right > element.width || crop_bottom > element.height {
        return Err(format!(
            "GUI crop ({}, {}, {}, {}) exceeds the selected element's {} by {} pixel bounds",
            crop.x, crop.y, crop.width, crop.height, element.width, element.height
        ));
    }
    Ok(PixelRect {
        x: element.x + crop.x,
        y: element.y + crop.y,
        width: crop.width,
        height: crop.height,
    })
}

fn validate_output_file(output_file: &str) -> Result<(), String> {
    if output_file.is_empty() {
        return Err("the GUI dump output filename is empty".to_owned());
    }
    if output_file.contains(['/', '\\', '\0']) {
        return Err("the GUI dump output must be a filename, not a path".to_owned());
    }
    if !output_file.to_ascii_lowercase().ends_with(".png") {
        return Err("the GUI dump output filename must end in .png".to_owned());
    }
    let stem = &output_file[..output_file.len() - ".png".len()];
    if stem.is_empty()
        || stem.starts_with('.')
        || stem.ends_with(['.', ' '])
        || output_file
            .chars()
            .any(|character| character.is_control() || ":*?\"<>|".contains(character))
        || reserved_windows_stem(stem)
    {
        return Err("the GUI dump output filename is unsupported by common filesystems".to_owned());
    }
    Ok(())
}

fn reserved_windows_stem(stem: &str) -> bool {
    let device = stem.split('.').next().unwrap_or_default();
    matches!(
        device.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

async fn download_png(
    source_canvas: &HtmlCanvasElement,
    source: PixelRect,
    output_file: &str,
) -> Result<(), String> {
    let document = web_sys::window()
        .and_then(|window| window.document())
        .ok_or("browser document is unavailable")?;
    let output_canvas = document
        .create_element("canvas")
        .map_err(js_error)?
        .dyn_into::<HtmlCanvasElement>()
        .map_err(|_| "could not create the GUI dump canvas")?;
    output_canvas.set_width(source.width);
    output_canvas.set_height(source.height);
    let context = output_canvas
        .get_context("2d")
        .map_err(js_error)?
        .ok_or("2D canvas is unavailable for the GUI dump")?
        .dyn_into::<CanvasRenderingContext2d>()
        .map_err(|_| "could not create a 2D GUI dump context")?;
    context.set_image_smoothing_enabled(false);
    context
        .draw_image_with_html_canvas_element_and_sw_and_sh_and_dx_and_dy_and_dw_and_dh(
            source_canvas,
            f64::from(source.x),
            f64::from(source.y),
            f64::from(source.width),
            f64::from(source.height),
            0.0,
            0.0,
            f64::from(source.width),
            f64::from(source.height),
        )
        .map_err(js_error)?;

    let blob = png_blob(&output_canvas).await?;
    let object_url = Url::create_object_url_with_blob(&blob).map_err(js_error)?;
    let download_result = download_blob(&document, &object_url, output_file);
    if let Err(error) = download_result {
        let _ = Url::revoke_object_url(&object_url);
        return Err(error);
    }
    schedule_url_revoke(object_url)
}

async fn png_blob(canvas: &HtmlCanvasElement) -> Result<Blob, String> {
    let canvas = canvas.clone();
    let promise = js_sys::Promise::new(&mut move |resolve, reject| {
        let reject_missing_blob = reject.clone();
        let callback = Closure::once_into_js(move |blob: Option<Blob>| {
            let result = match blob {
                Some(blob) => resolve.call1(&JsValue::UNDEFINED, blob.as_ref()),
                None => reject_missing_blob.call1(
                    &JsValue::UNDEFINED,
                    &JsValue::from_str("the browser could not encode the GUI dump as PNG"),
                ),
            };
            if let Err(error) = result {
                web_sys::console::error_1(&error);
            }
        });
        let callback_function = callback.unchecked_ref::<js_sys::Function>();
        if let Err(error) = canvas.to_blob_with_type(callback_function, "image/png") {
            let _ = reject.call1(&JsValue::UNDEFINED, &error);
            // once_into_js releases its allocation only when invoked.
            let _ = callback_function.call1(&JsValue::UNDEFINED, &JsValue::NULL);
        }
    });
    JsFuture::from(promise)
        .await
        .map_err(js_error)?
        .dyn_into::<Blob>()
        .map_err(|_| "the browser returned an invalid GUI dump PNG".to_owned())
}

fn download_blob(
    document: &web_sys::Document,
    object_url: &str,
    output_file: &str,
) -> Result<(), String> {
    let download = document
        .create_element("a")
        .map_err(js_error)?
        .dyn_into::<HtmlAnchorElement>()
        .map_err(|_| "could not create the GUI dump download")?;
    download.set_href(object_url);
    download.set_download(output_file);
    download.click();
    Ok(())
}

fn schedule_url_revoke(object_url: String) -> Result<(), String> {
    let window = web_sys::window().ok_or("browser window is unavailable")?;
    let revoke = Closure::once_into_js(move || {
        if let Err(error) = Url::revoke_object_url(&object_url) {
            web_sys::console::warn_1(&error);
        }
    });
    let revoke_function = revoke.unchecked_ref::<js_sys::Function>();
    if let Err(error) =
        window.set_timeout_with_callback_and_timeout_and_arguments_0(revoke_function, 1_000)
    {
        // Invoking the once closure also releases its wasm allocation.
        let _ = revoke_function.call0(&JsValue::UNDEFINED);
        return Err(js_error(error));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::GameGui;
    use oozems_proto::v1::GuiLayout;
    use oozems_proto::v1::GuiSprite;
    use oozems_proto::v1::GuiWindow;

    use super::GuiElement;
    use super::PixelRect;
    use super::Rect;
    use super::VisibleGui;
    use super::apply_crop;
    use super::parse_crop;
    use super::resolve_element_bounds;
    use super::validate_output_file;
    use super::visible_pixels;

    #[test]
    fn semantic_elements_resolve_to_canvas_coordinates() {
        let gui = gui_fixture();
        let visible = VisibleGui {
            inventory: true,
            ..VisibleGui::default()
        };

        assert_eq!(
            resolve_element_bounds(&gui, visible, 960, 600, GuiElement::StatusBar),
            Ok(Rect {
                x: 0.0,
                y: 520.0,
                width: 960.0,
                height: 80.0,
            })
        );
        assert_eq!(
            resolve_element_bounds(&gui, visible, 960, 600, GuiElement::InventoryWindow),
            Ok(Rect {
                x: 205.0,
                y: 80.0,
                width: 175.0,
                height: 289.0,
            })
        );
    }

    #[test]
    fn element_selectors_reject_unknown_names_with_the_available_values() {
        assert_eq!(GuiElement::parse("game"), Ok(GuiElement::Game));
        let error = GuiElement::parse("inventory").expect_err("unknown selector should fail");
        assert!(error.contains("inventory-window"));
        assert!(error.contains("cash-shop-window"));
    }

    #[test]
    fn hidden_elements_cannot_dump_unrelated_canvas_pixels() {
        let error = resolve_element_bounds(
            &gui_fixture(),
            VisibleGui::default(),
            960,
            600,
            GuiElement::InventoryWindow,
        )
        .expect_err("closed inventory should fail");

        assert_eq!(
            error,
            "GUI element 'inventory-window' is not currently rendered"
        );
    }

    #[test]
    fn cash_shop_bounds_include_its_screen_transform() {
        let mut gui = gui_fixture();
        let window = gui.cash_shop_window.as_mut().expect("cash shop window");
        window.x = 10.0;
        window.y = 20.0;
        let visible = VisibleGui {
            cash_shop: true,
            ..VisibleGui::default()
        };

        assert_eq!(
            resolve_element_bounds(&gui, visible, 1_600, 900, GuiElement::CashShopWindow),
            Ok(Rect {
                x: 215.0,
                y: 30.0,
                width: 1_200.0,
                height: 900.0,
            })
        );
        assert!(resolve_element_bounds(&gui, visible, 1_600, 900, GuiElement::StatusBar).is_err());
    }

    #[test]
    fn element_bounds_are_rounded_outward_and_clipped_to_the_canvas() {
        assert_eq!(
            visible_pixels(
                Rect {
                    x: -0.25,
                    y: 10.2,
                    width: 20.5,
                    height: 30.1,
                },
                100,
                100,
            ),
            Ok(PixelRect {
                x: 0,
                y: 10,
                width: 21,
                height: 31,
            })
        );
    }

    #[test]
    fn relative_crop_translates_to_absolute_canvas_pixels() {
        let element = PixelRect {
            x: 205,
            y: 80,
            width: 175,
            height: 289,
        };

        assert_eq!(
            apply_crop(
                element,
                PixelRect {
                    x: 7,
                    y: 50,
                    width: 140,
                    height: 180,
                },
            ),
            Ok(PixelRect {
                x: 212,
                y: 130,
                width: 140,
                height: 180,
            })
        );
        assert!(
            apply_crop(
                element,
                PixelRect {
                    x: 170,
                    y: 0,
                    width: 6,
                    height: 1,
                },
            )
            .is_err()
        );
        assert!(
            apply_crop(
                element,
                PixelRect {
                    x: u32::MAX,
                    y: 0,
                    width: 1,
                    height: 0,
                },
            )
            .expect_err("zero height should fail")
            .contains("positive")
        );
        assert_eq!(
            apply_crop(
                element,
                PixelRect {
                    x: 0,
                    y: 0,
                    width: element.width,
                    height: element.height,
                },
            ),
            Ok(element)
        );
    }

    #[test]
    fn javascript_crop_numbers_must_be_exact_nonnegative_integers() {
        assert_eq!(
            parse_crop(7.0, 50.0, 140.0, 180.0),
            Ok(PixelRect {
                x: 7,
                y: 50,
                width: 140,
                height: 180,
            })
        );
        assert!(parse_crop(-1.0, 0.0, 1.0, 1.0).is_err());
        assert!(parse_crop(0.5, 0.0, 1.0, 1.0).is_err());
        assert!(parse_crop(f64::NAN, 0.0, 1.0, 1.0).is_err());
    }

    #[test]
    fn output_is_restricted_to_a_png_download_filename() {
        assert_eq!(validate_output_file("inventory.PNG"), Ok(()));
        assert!(validate_output_file("inventory.jpg").is_err());
        assert!(validate_output_file("screens/inventory.png").is_err());
        assert!(validate_output_file(".png").is_err());
        assert!(validate_output_file("con.png").is_err());
        assert!(validate_output_file("inventory\n.png").is_err());
    }

    fn gui_fixture() -> GameGui {
        GameGui {
            status_bar: Some(layout(960.0, 80.0)),
            inventory_window: Some(GuiWindow {
                x: 205.0,
                y: 80.0,
                layout: Some(layout(175.0, 289.0)),
            }),
            cash_shop_window: Some(GuiWindow {
                layout: Some(layout(800.0, 600.0)),
                ..GuiWindow::default()
            }),
            ..GameGui::default()
        }
    }

    fn layout(
        width: f32,
        height: f32,
    ) -> GuiLayout {
        GuiLayout {
            width,
            height,
            background: Some(GuiSprite {
                name: "background".to_owned(),
                asset_id: "background".to_owned(),
                width,
                height,
                ..GuiSprite::default()
            }),
            ..GuiLayout::default()
        }
    }
}
