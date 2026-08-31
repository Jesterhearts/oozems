use oozems_proto::v1::AbilityStat;
use oozems_proto::v1::GameGui;
use oozems_proto::v1::InventoryState;
use oozems_proto::v1::SkillBook;

use super::CanvasPoint;
use super::GuiAction;
use super::GuiState;
use super::InventoryTab;
use super::PointerButton;
use super::WindowKind;
use super::equipped_item_at;
use super::frontmost_window_at_point;
use super::inventory_item_at;
use super::inventory_tab_at;
use super::rect_contains;
use super::skill_action_at;
use super::window_close_rect;
use super::window_region_rect;

pub(super) fn window_action(
    state: GuiState,
    gui: &GameGui,
    inventory: Option<&InventoryState>,
    skill_book: Option<&SkillBook>,
    viewport_width: f32,
    viewport_height: f32,
    point: CanvasPoint,
    button: PointerButton,
) -> Option<GuiAction> {
    let frontmost = frontmost_window_at_point(state, gui, viewport_width, viewport_height, point);
    if state.key_config_open {
        if frontmost == Some(WindowKind::KeyConfig)
            && button == PointerButton::Left
            && window_close_rect(
                state,
                gui,
                WindowKind::KeyConfig,
                viewport_width,
                viewport_height,
                "key-config-close",
            )
            .is_some_and(|rect| rect_contains(rect, point))
        {
            return Some(GuiAction::CloseKeyConfig);
        }
        return None;
    }
    if state.inventory_open && frontmost == Some(WindowKind::Inventory) {
        if window_close_rect(
            state,
            gui,
            WindowKind::Inventory,
            viewport_width,
            viewport_height,
            "inventory-close",
        )
        .is_some_and(|rect| rect_contains(rect, point))
        {
            return Some(GuiAction::CloseInventory);
        }
        if button == PointerButton::Left
            && let Some(tab) = inventory_tab_at(state, gui, viewport_width, viewport_height, point)
        {
            return Some(GuiAction::SelectInventoryTab { tab });
        }
        if let Some(hit) = inventory_item_at(
            state,
            gui,
            inventory?,
            viewport_width,
            viewport_height,
            point,
        ) {
            return match button {
                PointerButton::Left
                    if state.inventory_tab == InventoryTab::Equipment && hit.can_equip =>
                {
                    Some(GuiAction::Equip {
                        inventory_index: hit.inventory_index,
                    })
                }
                PointerButton::Left => None,
                PointerButton::Right => Some(GuiAction::Drop {
                    inventory_index: hit.inventory_index,
                }),
            };
        }
    }
    if button == PointerButton::Left
        && state.equipment_open
        && frontmost == Some(WindowKind::Equipment)
    {
        if window_close_rect(
            state,
            gui,
            WindowKind::Equipment,
            viewport_width,
            viewport_height,
            "equipment-close",
        )
        .is_some_and(|rect| rect_contains(rect, point))
        {
            return Some(GuiAction::CloseEquipment);
        }
        if let Some(slot) = equipped_item_at(
            state,
            gui,
            inventory?,
            viewport_width,
            viewport_height,
            point,
        ) {
            return Some(GuiAction::Unequip { slot });
        }
    }
    if button == PointerButton::Left
        && state.skills_open
        && frontmost == Some(WindowKind::Skills)
        && window_close_rect(
            state,
            gui,
            WindowKind::Skills,
            viewport_width,
            viewport_height,
            "skill-close",
        )
        .is_some_and(|rect| rect_contains(rect, point))
    {
        return Some(GuiAction::CloseSkills);
    }
    if button == PointerButton::Left && state.skills_open && frontmost == Some(WindowKind::Skills) {
        if let Some(skill_book) = skill_book
            && let Some(action) = skill_action_at(
                state,
                gui,
                skill_book,
                viewport_width,
                viewport_height,
                point,
            )
        {
            return Some(action);
        }
        for (name, action) in [
            ("skill-page-previous", GuiAction::PreviousSkillPage),
            ("skill-page-next", GuiAction::NextSkillPage),
        ] {
            if window_region_rect(
                state,
                gui,
                WindowKind::Skills,
                viewport_width,
                viewport_height,
                name,
            )
            .is_some_and(|rect| rect_contains(rect, point))
            {
                return Some(action);
            }
        }
    }
    if button == PointerButton::Left
        && state.stats_open
        && frontmost == Some(WindowKind::Stats)
        && window_close_rect(
            state,
            gui,
            WindowKind::Stats,
            viewport_width,
            viewport_height,
            "stat-close",
        )
        .is_some_and(|rect| rect_contains(rect, point))
    {
        return Some(GuiAction::CloseStats);
    }
    if button == PointerButton::Left && state.stats_open && frontmost == Some(WindowKind::Stats) {
        for (name, stat) in [
            ("stat-hp-up", AbilityStat::MaxHp),
            ("stat-mp-up", AbilityStat::MaxMp),
            ("stat-strength-up", AbilityStat::Strength),
            ("stat-dexterity-up", AbilityStat::Dexterity),
            ("stat-intelligence-up", AbilityStat::Intelligence),
            ("stat-luck-up", AbilityStat::Luck),
        ] {
            if window_region_rect(
                state,
                gui,
                WindowKind::Stats,
                viewport_width,
                viewport_height,
                name,
            )
            .is_some_and(|rect| rect_contains(rect, point))
            {
                return Some(GuiAction::AllocateAbility { stat });
            }
        }
    }
    None
}
