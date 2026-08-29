use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

use oozems_proto::v1::KeyBinding;
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use web_sys::HtmlCanvasElement;
use web_sys::KeyboardEvent;
use web_sys::MouseEvent;

use super::Game;
use super::request_dispatch::PendingRequest;
use super::request_dispatch::PendingRequests;
use super::requests;
use crate::audio;
use crate::audio::AudioState;
use crate::cash_shop_ui;
use crate::cash_shop_ui::CashShopAction;
use crate::game_gui;
use crate::game_gui::CanvasPoint;
use crate::game_gui::GuiAction;
use crate::game_gui::PointerButton;
use crate::interaction_ui;
use crate::js_error;
use crate::keymap;
use crate::keymap::KeyboardState;
use crate::render;
use crate::show_status;

pub(super) struct GameInput {
    pub(super) keyboard: Rc<RefCell<KeyboardState>>,
    canvas_actions: Rc<RefCell<VecDeque<CanvasInputAction>>>,
    suppress_click: bool,
    _handlers: EventHandlers,
}

pub(super) fn install(
    window: &web_sys::Window,
    canvas: &HtmlCanvasElement,
    keyboard: Rc<RefCell<KeyboardState>>,
    bindings: Rc<RefCell<Vec<KeyBinding>>>,
    audio_state: Rc<RefCell<AudioState>>,
) -> Result<GameInput, String> {
    let canvas_actions = Rc::new(RefCell::new(VecDeque::new()));
    let handlers = EventHandlers {
        _keyboard: install_keyboard_input(window, keyboard.clone(), bindings)?,
        _canvas: install_canvas_input(canvas, canvas_actions.clone())?,
        _audio: audio::install_input(window, audio_state)?,
    };
    Ok(GameInput {
        keyboard,
        canvas_actions,
        suppress_click: false,
        _handlers: handlers,
    })
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum CanvasInputAction {
    Down(CanvasPoint),
    Move(CanvasPoint),
    Leave,
    Up(CanvasPoint),
    Pointer(CanvasPoint, PointerButton),
    DoubleClick(CanvasPoint),
}

struct EventHandlers {
    _keyboard: KeyboardEventHandlers,
    _canvas: CanvasEventHandlers,
    _audio: audio::AudioEventHandlers,
}

struct KeyboardEventHandlers {
    window: web_sys::Window,
    keydown: Closure<dyn FnMut(KeyboardEvent)>,
    keyup: Closure<dyn FnMut(KeyboardEvent)>,
}

impl Drop for KeyboardEventHandlers {
    fn drop(&mut self) {
        let _ = self
            .window
            .remove_event_listener_with_callback("keydown", self.keydown.as_ref().unchecked_ref());
        let _ = self
            .window
            .remove_event_listener_with_callback("keyup", self.keyup.as_ref().unchecked_ref());
    }
}

struct CanvasEventHandlers {
    canvas: HtmlCanvasElement,
    mouse_down: Closure<dyn FnMut(MouseEvent)>,
    mouse_move: Closure<dyn FnMut(MouseEvent)>,
    mouse_leave: Closure<dyn FnMut(MouseEvent)>,
    mouse_up: Closure<dyn FnMut(MouseEvent)>,
    click: Closure<dyn FnMut(MouseEvent)>,
    double_click: Closure<dyn FnMut(MouseEvent)>,
    context_menu: Closure<dyn FnMut(MouseEvent)>,
}

impl Drop for CanvasEventHandlers {
    fn drop(&mut self) {
        for (name, callback) in [
            ("mousedown", self.mouse_down.as_ref()),
            ("mousemove", self.mouse_move.as_ref()),
            ("mouseleave", self.mouse_leave.as_ref()),
            ("mouseup", self.mouse_up.as_ref()),
            ("click", self.click.as_ref()),
            ("dblclick", self.double_click.as_ref()),
            ("contextmenu", self.context_menu.as_ref()),
        ] {
            let _ = self
                .canvas
                .remove_event_listener_with_callback(name, callback.unchecked_ref());
        }
    }
}
fn install_keyboard_input(
    window: &web_sys::Window,
    input: Rc<RefCell<KeyboardState>>,
    bindings: Rc<RefCell<Vec<KeyBinding>>>,
) -> Result<KeyboardEventHandlers, String> {
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
    Ok(KeyboardEventHandlers {
        window: window.clone(),
        keydown,
        keyup,
    })
}

fn install_canvas_input(
    canvas: &HtmlCanvasElement,
    actions: Rc<RefCell<VecDeque<CanvasInputAction>>>,
) -> Result<CanvasEventHandlers, String> {
    let down_canvas = canvas.clone();
    let down_actions = actions.clone();
    let mouse_down = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        if event.button() != 0 {
            return;
        }
        let Some(point) = canvas_event_point(&down_canvas, &event) else {
            return;
        };
        down_actions
            .borrow_mut()
            .push_back(CanvasInputAction::Down(point));
        event.prevent_default();
        let _ = down_canvas.focus();
    });
    canvas
        .add_event_listener_with_callback("mousedown", mouse_down.as_ref().unchecked_ref())
        .map_err(js_error)?;
    let move_canvas = canvas.clone();
    let move_actions = actions.clone();
    let mouse_move = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        let Some(point) = canvas_event_point(&move_canvas, &event) else {
            return;
        };
        move_actions
            .borrow_mut()
            .push_back(CanvasInputAction::Move(point));
        event.prevent_default();
    });
    canvas
        .add_event_listener_with_callback("mousemove", mouse_move.as_ref().unchecked_ref())
        .map_err(js_error)?;
    let leave_actions = actions.clone();
    let mouse_leave = Closure::<dyn FnMut(MouseEvent)>::new(move |_event: MouseEvent| {
        leave_actions
            .borrow_mut()
            .push_back(CanvasInputAction::Leave);
    });
    canvas
        .add_event_listener_with_callback("mouseleave", mouse_leave.as_ref().unchecked_ref())
        .map_err(js_error)?;
    let up_canvas = canvas.clone();
    let up_actions = actions.clone();
    let mouse_up = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        if event.button() != 0 {
            return;
        }
        let Some(point) = canvas_event_point(&up_canvas, &event) else {
            return;
        };
        up_actions
            .borrow_mut()
            .push_back(CanvasInputAction::Up(point));
        event.prevent_default();
        let _ = up_canvas.focus();
    });
    canvas
        .add_event_listener_with_callback("mouseup", mouse_up.as_ref().unchecked_ref())
        .map_err(js_error)?;
    let event_canvas = canvas.clone();
    let click_actions = actions.clone();
    let click = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        let Some(point) = canvas_event_point(&event_canvas, &event) else {
            return;
        };
        click_actions
            .borrow_mut()
            .push_back(CanvasInputAction::Pointer(point, PointerButton::Left));
        event.prevent_default();
        let _ = event_canvas.focus();
    });
    canvas
        .add_event_listener_with_callback("click", click.as_ref().unchecked_ref())
        .map_err(js_error)?;
    let double_click_canvas = canvas.clone();
    let double_click_actions = actions.clone();
    let double_click = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        if event.button() != 0 {
            return;
        }
        let Some(point) = canvas_event_point(&double_click_canvas, &event) else {
            return;
        };
        double_click_actions
            .borrow_mut()
            .push_back(CanvasInputAction::DoubleClick(point));
        event.prevent_default();
        let _ = double_click_canvas.focus();
    });
    canvas
        .add_event_listener_with_callback("dblclick", double_click.as_ref().unchecked_ref())
        .map_err(js_error)?;
    let context_canvas = canvas.clone();
    let context_actions = actions;
    let context_menu = Closure::<dyn FnMut(MouseEvent)>::new(move |event: MouseEvent| {
        let Some(point) = canvas_event_point(&context_canvas, &event) else {
            return;
        };
        context_actions
            .borrow_mut()
            .push_back(CanvasInputAction::Pointer(point, PointerButton::Right));
        event.prevent_default();
        let _ = context_canvas.focus();
    });
    canvas
        .add_event_listener_with_callback("contextmenu", context_menu.as_ref().unchecked_ref())
        .map_err(js_error)?;
    Ok(CanvasEventHandlers {
        canvas: canvas.clone(),
        mouse_down,
        mouse_move,
        mouse_leave,
        mouse_up,
        click,
        double_click,
        context_menu,
    })
}

fn handle_canvas_pointer(
    game: &mut Game,
    point: CanvasPoint,
    button: PointerButton,
    pending: &mut PendingRequests,
) -> bool {
    if crate::death_ui::is_open(game.ui.death) {
        if crate::death_ui::click_requests_respawn(&game.ui.gui, &mut game.ui.death, point, button)
        {
            pending.push(PendingRequest::Respawn);
        }
        return true;
    }
    if button == PointerButton::Left && std::mem::take(&mut game.input.suppress_click) {
        return true;
    }
    if game.ui.cash_shop.open {
        let action = cash_shop_ui::action_at(
            &game.ui.cash_shop,
            &game.ui.gui,
            game.surface.canvas.width() as f32,
            game.surface.canvas.height() as f32,
            point,
            button,
            game.requests
                .admission
                .is_active(requests::RequestKind::CashShopCatalog),
        );
        match action {
            Some(CashShopAction::Close) => game.ui.cash_shop.close(),
            Some(CashShopAction::Buy { offer_id }) => {
                pending.push(PendingRequest::CashShopPurchase(offer_id));
            }
            Some(CashShopAction::Consume) | None => {}
        }
        return true;
    }
    if interaction_is_busy(game) {
        if button != PointerButton::Left {
            return true;
        }
        let action = interaction_ui::click_action(
            &game.ui.gui,
            &game.ui.interaction,
            game.player.state.inventory.as_ref(),
            point,
        );
        let Some(action) = action else {
            return true;
        };
        if interaction_ui::apply_local_action(&mut game.ui.interaction, action) {
            return true;
        }
        pending.push(PendingRequest::InteractionAction(action));
        return true;
    }
    if button == PointerButton::Left && render::select_active_buff(game, point) {
        return true;
    }
    let action = game_gui::click_action(
        *game.ui.gui_state.borrow(),
        &game.ui.gui,
        game.player.state.inventory.as_ref(),
        Some(&game.player.skill_book),
        game.surface.canvas.width() as f32,
        game.surface.canvas.height() as f32,
        point,
        button,
    );
    let Some(action) = action else {
        return false;
    };
    if game_gui::apply_local_action(&mut game.ui.gui_state.borrow_mut(), action) {
        return true;
    }
    if matches!(action, GuiAction::AllocateAbility { .. })
        && !game_gui::can_allocate_ability(game.player.state.stats.as_ref())
    {
        return true;
    }
    match action {
        GuiAction::OpenCashShop => pending.push(PendingRequest::CashShopOpen),
        GuiAction::AllocateAbility { .. }
        | GuiAction::AllocateSkill { .. }
        | GuiAction::UseSkill { .. } => {
            pending.push(PendingRequest::Skill(action));
        }
        _ => pending.push(PendingRequest::Item(action)),
    }
    true
}

pub(super) fn interaction_is_busy(game: &Game) -> bool {
    game.ui.interaction.is_open()
        || game
            .requests
            .admission
            .is_active(requests::RequestKind::Interaction)
}

pub(super) fn apply_canvas_input(
    game: &mut Game,
    pending: &mut PendingRequests,
) {
    let actions = game
        .input
        .canvas_actions
        .borrow_mut()
        .drain(..)
        .collect::<Vec<_>>();
    for action in actions {
        if crate::death_ui::is_open(game.ui.death) {
            match action {
                CanvasInputAction::Move(point) => game.ui.pointer = Some(point),
                CanvasInputAction::Leave => game.ui.pointer = None,
                CanvasInputAction::Pointer(point, button) => {
                    let _ = handle_canvas_pointer(game, point, button, pending);
                }
                CanvasInputAction::Down(_)
                | CanvasInputAction::Up(_)
                | CanvasInputAction::DoubleClick(_) => {}
            }
            continue;
        }
        match action {
            CanvasInputAction::Down(point) => {
                let state = *game.ui.gui_state.borrow();
                game.ui.key_drag = None;
                game.ui.window_drag = None;
                if state.key_config_open {
                    game.ui.key_drag = game_gui::begin_key_drag(
                        state,
                        &game.ui.gui,
                        &game.player.skill_book,
                        &game.player.key_bindings.current.borrow(),
                        game.surface.canvas.width() as f32,
                        game.surface.canvas.height() as f32,
                        point,
                    );
                }
                if game.ui.key_drag.is_none() {
                    game.ui.window_drag = game_gui::begin_window_drag(
                        state,
                        &game.ui.gui,
                        game.surface.canvas.width() as f32,
                        game.surface.canvas.height() as f32,
                        point,
                    );
                }
            }
            CanvasInputAction::Move(point) => {
                game.ui.pointer = Some(point);
                if let Some(drag) = game.ui.key_drag.as_mut() {
                    game_gui::move_key_drag(drag, point);
                } else if let Some(drag) = game.ui.window_drag.as_mut() {
                    game_gui::move_window_drag(
                        &mut game.ui.gui_state.borrow_mut().window_placements,
                        &game.ui.gui,
                        drag,
                        game.surface.canvas.width() as f32,
                        game.surface.canvas.height() as f32,
                        point,
                    );
                }
            }
            CanvasInputAction::Leave => {
                game.ui.key_drag = None;
                game.ui.window_drag = None;
                game.ui.pointer = None;
            }
            CanvasInputAction::Up(point) => finish_drag(game, point),
            CanvasInputAction::Pointer(point, button) => {
                let _ = handle_canvas_pointer(game, point, button, pending);
            }
            CanvasInputAction::DoubleClick(point) => {
                handle_canvas_double_click(game, point, pending)
            }
        }
    }
}

fn handle_canvas_double_click(
    game: &Game,
    point: CanvasPoint,
    pending: &mut PendingRequests,
) {
    if crate::death_ui::is_open(game.ui.death)
        || game.ui.cash_shop.open
        || interaction_is_busy(game)
    {
        return;
    }
    let action = game_gui::double_click_action(
        *game.ui.gui_state.borrow(),
        &game.ui.gui,
        game.player.state.inventory.as_ref(),
        game.surface.canvas.width() as f32,
        game.surface.canvas.height() as f32,
        point,
    );
    if let Some(action) = action {
        pending.push(PendingRequest::Item(action));
        return;
    }
    queue_npc_interaction(game, point, pending);
}

pub(super) fn clear_suppressed_click(input: &mut GameInput) {
    input.suppress_click = false;
}

fn finish_drag(
    game: &mut Game,
    point: CanvasPoint,
) {
    if let Some(drag) = game.ui.key_drag.take() {
        finish_key_drag(game, drag, point);
        return;
    }
    let Some(drag) = game.ui.window_drag.take() else {
        return;
    };
    if game_gui::finish_window_drag(drag) {
        game.input.suppress_click = true;
    }
}

fn finish_key_drag(
    game: &mut Game,
    drag: game_gui::KeyDrag,
    point: CanvasPoint,
) {
    let updated = game_gui::finish_key_drag(
        *game.ui.gui_state.borrow(),
        &game.ui.gui,
        &game.player.key_bindings.current.borrow(),
        game.surface.canvas.width() as f32,
        game.surface.canvas.height() as f32,
        &drag,
        point,
    );
    let changed = updated
        .as_ref()
        .is_some_and(|updated| *updated != *game.player.key_bindings.current.borrow());
    if let Some(updated) = updated.filter(|_| changed) {
        *game.player.key_bindings.current.borrow_mut() = updated.clone();
        game.player.state.key_bindings = updated;
        game.player.key_bindings.generation = game.player.key_bindings.generation.saturating_add(1);
        game.player.key_bindings.pending = true;
        game.key_binding_save.dirty = true;
    }
    game.input.suppress_click = true;
}

fn queue_npc_interaction(
    game: &Game,
    point: CanvasPoint,
    pending: &mut PendingRequests,
) {
    if super::runtime::player_is_dead(&game.player.state) {
        return;
    }
    let gui = *game.ui.gui_state.borrow();
    if interaction_is_busy(game)
        || game.requests.admission.player_mutation_is_active()
        || pending.has_player_mutation()
        || game.ui.cash_shop.open
        || gui.stats_open
        || gui.equipment_open
        || gui.inventory_open
        || gui.key_config_open
        || gui.skills_open
    {
        return;
    }
    let Some(npc_spawn_id) = render::npc_at_point(game, point) else {
        return;
    };
    if game.ui.gui.npc_dialog_window.is_none() {
        show_status("NPC interaction requires UI.wz.", true);
        return;
    }
    pending.push(PendingRequest::InteractionOpen(npc_spawn_id));
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
