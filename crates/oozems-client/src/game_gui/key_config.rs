use oozems_proto::v1::GameGui;
use oozems_proto::v1::KeyAction;
use oozems_proto::v1::KeyBinding;
use oozems_proto::v1::SkillBook;

use super::CanvasPoint;
use super::CanvasRect;
use super::GuiState;
use super::WindowKind;
use super::rect_contains;
use super::resolve_window;
use super::skill_at_point;
use super::valid_layout;
use super::valid_sprite;
use crate::keymap::BindingTarget;

#[derive(Clone, Debug, PartialEq)]
pub struct KeyDrag {
    pub target: BindingTarget,
    pub point: CanvasPoint,
}

#[derive(Clone, Debug, PartialEq)]
pub struct KeyIconPlacement {
    pub target: BindingTarget,
    pub asset_id: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

struct BindingIcon {
    asset_id: String,
    width: f32,
    height: f32,
}

pub fn begin_key_drag(
    state: GuiState,
    gui: &GameGui,
    skill_book: &SkillBook,
    bindings: &[KeyBinding],
    viewport_width: f32,
    viewport_height: f32,
    point: CanvasPoint,
) -> Option<KeyDrag> {
    let target = skill_at_point(
        state,
        gui,
        skill_book,
        viewport_width,
        viewport_height,
        point,
    )
    .map(BindingTarget::Skill)
    .or_else(|| {
        palette_action_at(state, gui, viewport_width, viewport_height, point)
            .map(BindingTarget::Action)
    })
    .or_else(|| bound_target_at(state, gui, bindings, viewport_width, viewport_height, point))?;
    Some(KeyDrag { target, point })
}

pub fn move_key_drag(
    drag: &mut KeyDrag,
    point: CanvasPoint,
) {
    drag.point = point;
}

pub fn finish_key_drag(
    state: GuiState,
    gui: &GameGui,
    bindings: &[KeyBinding],
    viewport_width: f32,
    viewport_height: f32,
    drag: &KeyDrag,
    point: CanvasPoint,
) -> Option<Vec<KeyBinding>> {
    let code = key_code_at(state, gui, viewport_width, viewport_height, point)?;
    Some(crate::keymap::assign_target(bindings, code, drag.target))
}

pub fn bound_key_icons(
    state: GuiState,
    gui: &GameGui,
    skill_book: &SkillBook,
    bindings: &[KeyBinding],
    viewport_width: f32,
    viewport_height: f32,
) -> Vec<KeyIconPlacement> {
    let Some(placement) = resolve_window(
        gui,
        state.window_placements,
        WindowKind::KeyConfig,
        viewport_width,
        viewport_height,
    ) else {
        return Vec::new();
    };
    let layout = placement.layout;
    bindings
        .iter()
        .filter_map(|binding| {
            let target = crate::keymap::target_for_code(bindings, &binding.code)?;
            let icon = target_icon(gui, skill_book, target)?;
            let slot = gui.key_slots.iter().find(|slot| {
                slot.code == binding.code && valid_key_slot(slot, layout.width, layout.height)
            })?;
            Some(KeyIconPlacement {
                target,
                asset_id: icon.asset_id,
                x: placement.origin.x + slot.x + (slot.width - icon.width) / 2.0,
                y: placement.origin.y + slot.y + (slot.height - icon.height) / 2.0,
                width: icon.width,
                height: icon.height,
            })
        })
        .collect()
}

pub fn dragged_key_icon(
    gui: &GameGui,
    skill_book: &SkillBook,
    drag: &KeyDrag,
) -> Option<KeyIconPlacement> {
    let icon = target_icon(gui, skill_book, drag.target)?;
    Some(KeyIconPlacement {
        target: drag.target,
        asset_id: icon.asset_id,
        x: drag.point.x - icon.width / 2.0,
        y: drag.point.y - icon.height / 2.0,
        width: icon.width,
        height: icon.height,
    })
}

fn palette_action_at(
    state: GuiState,
    gui: &GameGui,
    viewport_width: f32,
    viewport_height: f32,
    point: CanvasPoint,
) -> Option<KeyAction> {
    let placement = resolve_window(
        gui,
        state.window_placements,
        WindowKind::KeyConfig,
        viewport_width,
        viewport_height,
    )?;
    let layout = placement.layout;
    gui.key_actions.iter().find_map(|definition| {
        let icon = definition.icon.as_ref()?;
        if !valid_sprite(icon, layout.width, layout.height) {
            return None;
        }
        rect_contains(
            CanvasRect {
                x: placement.origin.x + icon.x,
                y: placement.origin.y + icon.y,
                width: icon.width,
                height: icon.height,
            },
            point,
        )
        .then(|| KeyAction::try_from(definition.action).ok())
        .flatten()
    })
}

fn bound_target_at(
    state: GuiState,
    gui: &GameGui,
    bindings: &[KeyBinding],
    viewport_width: f32,
    viewport_height: f32,
    point: CanvasPoint,
) -> Option<BindingTarget> {
    let code = key_code_at(state, gui, viewport_width, viewport_height, point)?;
    crate::keymap::target_for_code(bindings, code)
}

fn target_icon(
    gui: &GameGui,
    skill_book: &SkillBook,
    target: BindingTarget,
) -> Option<BindingIcon> {
    match target {
        BindingTarget::Action(action) => {
            let icon = action_definition(gui, action)?.icon.as_ref()?;
            Some(BindingIcon {
                asset_id: icon.asset_id.clone(),
                width: icon.width,
                height: icon.height,
            })
        }
        BindingTarget::Skill(skill_id) => {
            let definition = skill_book
                .skills
                .iter()
                .filter_map(|skill| skill.definition.as_ref())
                .find(|definition| definition.skill_id == skill_id)?;
            Some(BindingIcon {
                asset_id: definition.icon_asset_id.clone(),
                width: definition.icon_width,
                height: definition.icon_height,
            })
        }
    }
}

fn key_code_at(
    state: GuiState,
    gui: &GameGui,
    viewport_width: f32,
    viewport_height: f32,
    point: CanvasPoint,
) -> Option<&str> {
    let placement = resolve_window(
        gui,
        state.window_placements,
        WindowKind::KeyConfig,
        viewport_width,
        viewport_height,
    )?;
    let layout = placement.layout;
    gui.key_slots.iter().find_map(|slot| {
        if !valid_key_slot(slot, layout.width, layout.height) {
            return None;
        }
        rect_contains(
            CanvasRect {
                x: placement.origin.x + slot.x,
                y: placement.origin.y + slot.y,
                width: slot.width,
                height: slot.height,
            },
            point,
        )
        .then_some(slot.code.as_str())
    })
}

fn action_definition(
    gui: &GameGui,
    action: KeyAction,
) -> Option<&oozems_proto::v1::KeyActionDefinition> {
    let layout = valid_key_config_window(gui)?.layout.as_ref()?;
    gui.key_actions.iter().find(|definition| {
        definition.action == action as i32
            && definition
                .icon
                .as_ref()
                .is_some_and(|icon| valid_sprite(icon, layout.width, layout.height))
    })
}

fn valid_key_config_window(gui: &GameGui) -> Option<&oozems_proto::v1::GuiWindow> {
    let window = gui.key_config_window.as_ref()?;
    window
        .layout
        .as_ref()
        .filter(|layout| valid_layout(layout))?;
    Some(window)
}

fn valid_key_slot(
    slot: &oozems_proto::v1::KeySlot,
    layout_width: f32,
    layout_height: f32,
) -> bool {
    let values = [slot.x, slot.y, slot.width, slot.height];
    !slot.code.is_empty()
        && values.iter().all(|value| value.is_finite())
        && slot.x >= 0.0
        && slot.y >= 0.0
        && slot.width > 0.0
        && slot.height > 0.0
        && slot.x + slot.width <= layout_width
        && slot.y + slot.height <= layout_height
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::GameGui;
    use oozems_proto::v1::GuiLayout;
    use oozems_proto::v1::GuiRegion;
    use oozems_proto::v1::GuiSprite;
    use oozems_proto::v1::GuiSpriteTemplate;
    use oozems_proto::v1::GuiWindow;
    use oozems_proto::v1::KeyAction;
    use oozems_proto::v1::KeyActionDefinition;
    use oozems_proto::v1::KeySlot;
    use oozems_proto::v1::PlayerSkill;
    use oozems_proto::v1::SkillBook;
    use oozems_proto::v1::SkillDefinition;

    use super::begin_key_drag;
    use super::bound_key_icons;
    use super::finish_key_drag;
    use crate::game_gui::CanvasPoint;
    use crate::game_gui::GuiState;
    use crate::game_gui::WindowKind;
    use crate::game_gui::begin_window_drag;
    use crate::game_gui::window::set_window_offset;
    use crate::keymap::BindingTarget;

    #[test]
    fn key_actions_drag_from_the_wz_palette_onto_a_keyboard_slot() {
        let gui = gui_fixture();
        let skill_book = SkillBook::default();
        let point = CanvasPoint { x: 175.0, y: 330.0 };
        let state = GuiState::default();
        let drag = begin_key_drag(state, &gui, &skill_book, &[], 960.0, 600.0, point)
            .expect("pickup palette icon");

        assert_eq!(drag.target, BindingTarget::Action(KeyAction::PickUp));
        let bindings = finish_key_drag(
            state,
            &gui,
            &[],
            960.0,
            600.0,
            &drag,
            CanvasPoint { x: 250.0, y: 197.0 },
        )
        .expect("KeyA target");
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].code, "KeyA");
        assert_eq!(bindings[0].action, KeyAction::PickUp as i32);

        let icons = bound_key_icons(state, &gui, &skill_book, &bindings, 960.0, 600.0);
        assert_eq!(icons.len(), 1);
        assert_eq!((icons[0].x, icons[0].y), (246.0, 193.0));
    }

    #[test]
    fn learned_skills_drag_from_native_rows() {
        let gui = gui_fixture();
        let book = skill_book_fixture();
        let state = GuiState {
            key_config_open: true,
            skills_open: true,
            ..GuiState::default()
        };

        let drag = begin_key_drag(
            state,
            &gui,
            &book,
            &[],
            960.0,
            600.0,
            CanvasPoint { x: 40.0, y: 180.0 },
        )
        .expect("learned skill drag");
        assert_eq!(drag.target, BindingTarget::Skill(1_000));
        let bindings = finish_key_drag(
            state,
            &gui,
            &[],
            960.0,
            600.0,
            &drag,
            CanvasPoint { x: 250.0, y: 197.0 },
        )
        .expect("KeyA target");
        assert_eq!(bindings[0].skill_id, 1_000);
        assert_eq!(bindings[0].action, KeyAction::Unspecified as i32);
    }

    #[test]
    fn key_icons_and_slots_follow_the_moved_key_config_window() {
        let gui = gui_fixture();
        let skill_book = SkillBook::default();
        let mut state = GuiState {
            key_config_open: true,
            ..GuiState::default()
        };
        set_window_offset(
            &mut state.window_placements,
            WindowKind::KeyConfig,
            CanvasPoint { x: 40.0, y: 25.0 },
        );
        let palette_point = CanvasPoint { x: 215.0, y: 355.0 };

        let drag = begin_key_drag(state, &gui, &skill_book, &[], 960.0, 600.0, palette_point)
            .expect("moved pickup palette icon");
        let bindings = finish_key_drag(
            state,
            &gui,
            &[],
            960.0,
            600.0,
            &drag,
            CanvasPoint { x: 290.0, y: 222.0 },
        )
        .expect("moved KeyA target");
        let icons = bound_key_icons(state, &gui, &skill_book, &bindings, 960.0, 600.0);

        assert_eq!(icons.len(), 1);
        assert_eq!((icons[0].x, icons[0].y), (286.0, 218.0));
        assert!(
            begin_key_drag(
                state,
                &gui,
                &skill_book,
                &bindings,
                960.0,
                600.0,
                CanvasPoint { x: 290.0, y: 222.0 },
            )
            .is_some()
        );
        assert!(
            begin_window_drag(
                state,
                &gui,
                960.0,
                600.0,
                CanvasPoint { x: 290.0, y: 222.0 },
            )
            .is_none()
        );
        assert!(
            begin_key_drag(
                state,
                &gui,
                &skill_book,
                &bindings,
                960.0,
                600.0,
                CanvasPoint { x: 210.0, y: 90.0 },
            )
            .is_none()
        );
        assert!(
            begin_window_drag(state, &gui, 960.0, 600.0, CanvasPoint { x: 210.0, y: 90.0 },)
                .is_some()
        );
    }

    fn gui_fixture() -> GameGui {
        GameGui {
            skill_window: Some(GuiWindow {
                x: 20.0,
                y: 80.0,
                layout: Some(GuiLayout {
                    width: 175.0,
                    height: 289.0,
                    background: Some(sprite("skill-background", 0.0, 0.0, 175.0, 289.0)),
                    sprite_templates: vec![template("skill-row", 141.0, 35.0)],
                    regions: vec![region("skill-list", 17.0, 94.0, 141.0, 140.0)],
                    ..GuiLayout::default()
                }),
            }),
            key_config_window: Some(GuiWindow {
                x: 165.0,
                y: 60.0,
                layout: Some(GuiLayout {
                    width: 629.0,
                    height: 373.0,
                    background: Some(sprite("key-config-background", 0.0, 0.0, 629.0, 373.0)),
                    sprites: vec![sprite("key-action-50", 7.0, 267.0, 32.0, 32.0)],
                    ..GuiLayout::default()
                }),
            }),
            key_actions: vec![KeyActionDefinition {
                action: KeyAction::PickUp as i32,
                label: "Pick up".to_owned(),
                icon: Some(sprite("key-action-50", 7.0, 267.0, 32.0, 32.0)),
            }],
            key_slots: vec![KeySlot {
                code: "KeyA".to_owned(),
                x: 81.0,
                y: 133.0,
                width: 32.0,
                height: 32.0,
            }],
            ..GameGui::default()
        }
    }

    fn sprite(
        name: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> GuiSprite {
        GuiSprite {
            name: name.to_owned(),
            asset_id: format!("asset-{name}"),
            x,
            y,
            width,
            height,
            ..GuiSprite::default()
        }
    }

    fn region(
        name: &str,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> GuiRegion {
        GuiRegion {
            name: name.to_owned(),
            x,
            y,
            width,
            height,
        }
    }

    fn template(
        name: &str,
        width: f32,
        height: f32,
    ) -> GuiSpriteTemplate {
        GuiSpriteTemplate {
            name: name.to_owned(),
            asset_id: format!("asset-{name}"),
            width,
            height,
            ..GuiSpriteTemplate::default()
        }
    }

    fn skill_book_fixture() -> SkillBook {
        SkillBook {
            skills: vec![PlayerSkill {
                definition: Some(SkillDefinition {
                    skill_id: 1_000,
                    max_level: 3,
                    icon_asset_id: "skill-icon".to_owned(),
                    icon_width: 32.0,
                    icon_height: 32.0,
                    ..SkillDefinition::default()
                }),
                level: 1,
                master_level: 0,
            }],
            ..SkillBook::default()
        }
    }
}
