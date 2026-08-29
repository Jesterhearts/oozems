use eframe::egui;

use super::EditorApp;
use crate::scripts::Action;
use crate::scripts::Condition;
use crate::scripts::QuestState;
use crate::scripts::ScriptProgram;

pub(super) fn draw_linked_script_editor(
    ui: &mut egui::Ui,
    app: &mut EditorApp,
    phase: &str,
    name: &str,
) {
    let Some(index) = app
        .scripts
        .scripts
        .iter()
        .position(|script| script.name == name)
    else {
        ui.vertical_centered(|ui| {
            ui.heading(format!("{phase}: {name}"));
            ui.label("Quest.wz references this script, but quest-scripts.toml does not define it.");
            if ui.button(format!("Create {name}")).clicked() {
                app.scripts.scripts.push(ScriptProgram {
                    name: name.to_owned(),
                    ..ScriptProgram::default()
                });
                app.scripts_dirty = true;
                app.status =
                    format!("Created linked script {name}. Save to update quest-scripts.toml.");
            }
        });
        return;
    };
    egui::ScrollArea::vertical().show(ui, |ui| {
        let program = &mut app.scripts.scripts[index];
        ui.heading(format!("{phase}: {}", program.name));
        ui.label("This script is linked directly from the selected quest's WZ checks.");
        ui.separator();
        app.scripts_dirty |= draw_conditions(ui, &mut program.conditions);
        ui.add_space(12.0);
        app.scripts_dirty |= draw_actions(ui, &mut program.actions);
        ui.add_space(12.0);
        app.scripts_dirty |= draw_pages(ui, "Result dialogue", &mut program.result_pages);
        ui.add_space(12.0);
        app.scripts_dirty |= draw_pages(
            ui,
            "Conditions-not-met dialogue",
            &mut program.incomplete_pages,
        );
    });
}

fn draw_conditions(
    ui: &mut egui::Ui,
    conditions: &mut Vec<Condition>,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.heading("Conditions");
        if ui.button("Add condition").clicked() {
            conditions.push(Condition::default());
            changed = true;
        }
    });
    ui.label("All conditions must be met.");
    let mut remove = None;
    for (index, condition) in conditions.iter_mut().enumerate() {
        ui.group(|ui| {
            ui.horizontal(|ui| {
                let mut kind = condition.kind_index();
                egui::ComboBox::from_id_salt(("condition-kind", index))
                    .selected_text(Condition::KINDS[kind])
                    .show_ui(ui, |ui| {
                        for (candidate, label) in Condition::KINDS.iter().enumerate() {
                            ui.selectable_value(&mut kind, candidate, *label);
                        }
                    });
                if kind != condition.kind_index() {
                    *condition = Condition::from_kind(kind);
                    changed = true;
                }
                changed |= draw_condition_fields(ui, condition);
                if ui.small_button("Remove").clicked() {
                    remove = Some(index);
                }
            });
        });
    }
    if let Some(index) = remove {
        conditions.remove(index);
        changed = true;
    }
    changed
}

fn draw_condition_fields(
    ui: &mut egui::Ui,
    condition: &mut Condition,
) -> bool {
    match condition {
        Condition::MinimumLevel { level } | Condition::MaximumLevel { level } => {
            labeled_u32(ui, "Level", level)
        }
        Condition::JobIds { ids } => draw_id_list(ui, ids),
        Condition::MapId { map_id } => labeled_u32(ui, "Map", map_id),
        Condition::MesosAtLeast { amount } | Condition::MesosAtMost { amount } => {
            labeled_u64(ui, "Amount", amount)
        }
        Condition::ItemQuantity { item_id, quantity } => {
            labeled_u32(ui, "Item", item_id) | labeled_u64(ui, "Quantity", quantity)
        }
        Condition::QuestState { quest_id, state } => {
            labeled_u32(ui, "Quest", quest_id) | draw_quest_state(ui, state)
        }
        Condition::QuestRecordEquals {
            quest_id,
            index,
            value,
        }
        | Condition::QuestRecordAtLeast {
            quest_id,
            index,
            value,
        }
        | Condition::QuestRecordAtMost {
            quest_id,
            index,
            value,
        } => draw_record_fields(ui, quest_id, index, value),
    }
}

fn draw_actions(
    ui: &mut egui::Ui,
    actions: &mut Vec<Action>,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.heading("Actions");
        if ui.button("Add action").clicked() {
            actions.push(Action::default());
            changed = true;
        }
    });
    ui.label("Actions run in the order shown.");
    let mut remove = None;
    for (index, action) in actions.iter_mut().enumerate() {
        ui.group(|ui| {
            ui.horizontal(|ui| {
                let mut kind = action.kind_index();
                egui::ComboBox::from_id_salt(("action-kind", index))
                    .selected_text(Action::KINDS[kind])
                    .show_ui(ui, |ui| {
                        for (candidate, label) in Action::KINDS.iter().enumerate() {
                            ui.selectable_value(&mut kind, candidate, *label);
                        }
                    });
                if kind != action.kind_index() {
                    *action = Action::from_kind(kind);
                    changed = true;
                }
                changed |= draw_action_fields(ui, action);
                if ui.small_button("Remove").clicked() {
                    remove = Some(index);
                }
            });
        });
    }
    if let Some(index) = remove {
        actions.remove(index);
        changed = true;
    }
    changed
}

fn draw_action_fields(
    ui: &mut egui::Ui,
    action: &mut Action,
) -> bool {
    match action {
        Action::ItemDelta { item_id, delta } => {
            labeled_u32(ui, "Item", item_id) | labeled_i64(ui, "Delta", delta)
        }
        Action::Mesos { delta } => labeled_i64(ui, "Delta", delta),
        Action::Experience { amount } => labeled_u64(ui, "Amount", amount),
        Action::Fame { delta } => {
            ui.label("Delta");
            ui.add(egui::DragValue::new(delta)).changed()
        }
        Action::SetRecord {
            quest_id,
            index,
            value,
        } => draw_record_fields(ui, quest_id, index, value),
        Action::SetQuestStatus { quest_id, state } => {
            labeled_u32(ui, "Quest", quest_id) | draw_quest_state(ui, state)
        }
    }
}

fn draw_pages(
    ui: &mut egui::Ui,
    heading: &str,
    pages: &mut Vec<String>,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.heading(heading);
        if ui.button("Add page").clicked() {
            pages.push(String::new());
            changed = true;
        }
    });
    let mut remove = None;
    for (index, page) in pages.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            ui.label(format!("{}", index + 1));
            changed |= ui
                .add_sized(
                    [ui.available_width() - 70.0, 55.0],
                    egui::TextEdit::multiline(page),
                )
                .changed();
            if ui.small_button("Remove").clicked() {
                remove = Some(index);
            }
        });
    }
    if let Some(index) = remove {
        pages.remove(index);
        changed = true;
    }
    changed
}

fn draw_id_list(
    ui: &mut egui::Ui,
    ids: &mut Vec<u32>,
) -> bool {
    ui.label("IDs");
    let mut changed = false;
    let mut remove = None;
    ui.horizontal_wrapped(|ui| {
        for (index, id) in ids.iter_mut().enumerate() {
            changed |= ui.add(egui::DragValue::new(id)).changed();
            if ui.small_button("x").clicked() {
                remove = Some(index);
            }
        }
        if ui.small_button("Add ID").clicked() {
            ids.push(0);
            changed = true;
        }
    });
    if let Some(index) = remove {
        ids.remove(index);
        changed = true;
    }
    changed
}

fn draw_record_fields(
    ui: &mut egui::Ui,
    quest_id: &mut u32,
    index: &mut u32,
    value: &mut String,
) -> bool {
    labeled_u32(ui, "Quest", quest_id) | labeled_u32(ui, "Index", index) | {
        ui.label("Value");
        ui.text_edit_singleline(value).changed()
    }
}

fn draw_quest_state(
    ui: &mut egui::Ui,
    state: &mut QuestState,
) -> bool {
    let previous = *state;
    egui::ComboBox::from_id_salt(ui.next_auto_id())
        .selected_text(state.label())
        .show_ui(ui, |ui| {
            for candidate in QuestState::ALL {
                ui.selectable_value(state, candidate, candidate.label());
            }
        });
    previous != *state
}

fn labeled_u32(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut u32,
) -> bool {
    ui.label(label);
    ui.add(egui::DragValue::new(value)).changed()
}

fn labeled_u64(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut u64,
) -> bool {
    ui.label(label);
    ui.add(egui::DragValue::new(value)).changed()
}

fn labeled_i64(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut i64,
) -> bool {
    ui.label(label);
    ui.add(egui::DragValue::new(value)).changed()
}
