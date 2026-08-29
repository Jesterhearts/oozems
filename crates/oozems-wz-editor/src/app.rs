use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use eframe::egui;
use eframe::egui::Color32;
use oozems_skill_semantics::OverloadedSkillProperty;
use oozems_skill_semantics::SkillArchiveFacts;
use oozems_skill_semantics::SkillPropertyScope;
use oozems_skill_semantics::SkillSemanticCatalog;
use oozems_skill_semantics::SkillValueTransform;
use oozems_skill_semantics::load_optional as load_skill_semantics;
use oozems_skill_semantics::validate_archive;
use oozems_wz::Archive;
use oozems_wz::OpenOptions;
use oozems_wz::PropertyEdit;
use oozems_wz::PropertyKind;
use serde_json::Value;

use crate::document::DefinitionDocument;
use crate::document::DefinitionEntry;
use crate::document::EditableNode;
use crate::scripts::ScriptFile;

mod script_form;

const ACCENT: Color32 = Color32::from_rgb(230, 176, 88);
const ERROR: Color32 = Color32::from_rgb(242, 123, 107);
const MUTED: Color32 = Color32::from_rgb(166, 174, 164);
const MAXIMUM_SEMANTIC_LEVEL_NODES: usize = 10_000;

pub struct EditorPaths {
    pub quest: PathBuf,
    pub quest_output: PathBuf,
    pub skill: PathBuf,
    pub skill_output: PathBuf,
    pub strings: PathBuf,
    pub scripts: PathBuf,
    pub skill_semantics: PathBuf,
}

pub struct PreparedEditor {
    quest_archive: Archive,
    quest_entries: Vec<DefinitionEntry>,
    quest_script_references: BTreeMap<String, String>,
    skill_archive: Archive,
    skill_entries: Vec<DefinitionEntry>,
    skill_semantics: SkillSemanticCatalog,
    scripts: ScriptFile,
    paths: EditorPaths,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EditorTab {
    Quests,
    Skills,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum QuestView {
    Definition,
    Script(String),
}

struct AddPropertyDraft {
    name: String,
    kind: PropertyKind,
    error: Option<String>,
}

impl Default for AddPropertyDraft {
    fn default() -> Self {
        Self {
            name: String::new(),
            kind: PropertyKind::Int,
            error: None,
        }
    }
}

pub struct EditorApp {
    tab: EditorTab,
    quest_archive: Archive,
    quest_entries: Vec<DefinitionEntry>,
    quest_filter: String,
    quest_document: Option<DefinitionDocument>,
    quest_script_references: BTreeMap<String, String>,
    quest_staged: BTreeMap<String, Value>,
    quest_saved: BTreeMap<String, Value>,
    quest_view: QuestView,
    skill_archive: Archive,
    skill_entries: Vec<DefinitionEntry>,
    skill_filter: String,
    skill_document: Option<DefinitionDocument>,
    skill_staged: BTreeMap<String, Value>,
    skill_structure: BTreeMap<String, PropertyEdit>,
    skill_saved: BTreeMap<String, Value>,
    skill_structure_saved: BTreeMap<String, PropertyEdit>,
    skill_add_drafts: BTreeMap<String, AddPropertyDraft>,
    skill_semantics: SkillSemanticCatalog,
    scripts: ScriptFile,
    scripts_dirty: bool,
    paths: EditorPaths,
    status: String,
    confirm_close: bool,
    allow_close: bool,
}

impl PreparedEditor {
    pub fn load(paths: EditorPaths) -> Result<Self> {
        let options = OpenOptions::default();
        let quest_archive = oozems_wz::open_archive(&paths.quest, options)
            .with_context(|| format!("failed to open {}", paths.quest.display()))?;
        let quest_entries = crate::document::quest_index(&quest_archive)?;
        let quest_script_references =
            crate::document::quest_script_reference_paths(&quest_archive)?;
        let skill_archive = oozems_wz::open_archive(&paths.skill, options)
            .with_context(|| format!("failed to open {}", paths.skill.display()))?;
        let strings = oozems_wz::open_archive(&paths.strings, options)
            .with_context(|| format!("failed to open {}", paths.strings.display()))?;
        let skill_entries = crate::document::skill_index(&skill_archive, &strings)?;
        let skill_semantics = load_skill_semantics(&paths.skill_semantics)?;
        let skill_facts = skill_archive_facts(&skill_archive, &skill_entries, &skill_semantics)?;
        validate_archive(&skill_semantics, &skill_facts)?;
        let scripts = crate::scripts::load(&paths.scripts)?;
        Ok(Self {
            quest_archive,
            quest_entries,
            quest_script_references,
            skill_archive,
            skill_entries,
            skill_semantics,
            scripts,
            paths,
        })
    }
}

fn skill_archive_facts(
    archive: &Archive,
    entries: &[DefinitionEntry],
    semantics: &SkillSemanticCatalog,
) -> Result<SkillArchiveFacts> {
    let mut facts = SkillArchiveFacts::default();
    let paths = entries
        .iter()
        .map(|entry| {
            facts.add_skill(entry.id);
            (entry.id, entry.path.as_str())
        })
        .collect::<BTreeMap<_, _>>();
    let mut configured = BTreeMap::<u32, BTreeSet<OverloadedSkillProperty>>::new();
    for (skill_id, property) in semantics.configured_properties() {
        configured.entry(skill_id).or_default().insert(property);
    }
    for (skill_id, properties) in configured {
        let Some(path) = paths.get(&skill_id) else {
            continue;
        };
        let level_path = format!("{path}/level");
        let levels = oozems_wz::tree(archive, &level_path, MAXIMUM_SEMANTIC_LEVEL_NODES)
            .with_context(|| format!("failed to inspect semantic mappings for skill {skill_id}"))?;
        facts.set_level_count(skill_id, levels.children.len());
        for level in levels.children {
            for property in &properties {
                let Some(node) = level
                    .children
                    .iter()
                    .find(|child| child.name == property.name())
                else {
                    continue;
                };
                let semantic = semantics
                    .property_semantic(skill_id, SkillPropertyScope::Level, property.name())
                    .expect("configured property has a semantic mapping");
                if let SkillValueTransform::Numeric { offset } = semantic.transform() {
                    validate_numeric_semantic_value(
                        node.value.as_ref(),
                        offset,
                        semantic.normalized_stats(),
                        skill_id,
                        *property,
                    )?;
                }
                facts.add_level_property(skill_id, *property);
            }
        }
    }
    Ok(facts)
}

fn validate_numeric_semantic_value(
    value: Option<&Value>,
    offset: i64,
    normalized_stats: &[oozems_skill_semantics::NormalizedSkillStat],
    skill_id: u32,
    property: OverloadedSkillProperty,
) -> Result<()> {
    let Some(number) = value.and_then(|value| match value {
        Value::Number(value) => value.as_f64(),
        Value::String(value) => value.trim().parse::<f64>().ok(),
        _ => None,
    }) else {
        anyhow::bail!(
            "skill {skill_id} property {} has a nonnumeric mapped value",
            property.name()
        );
    };
    let normalized = number + offset as f64;
    if !number.is_finite() || !normalized.is_finite() {
        anyhow::bail!(
            "skill {skill_id} property {} has a nonnumeric mapped value",
            property.name()
        );
    }
    if normalized_stats
        .iter()
        .any(|stat| !stat.accepts_number(normalized))
    {
        anyhow::bail!(
            "skill {skill_id} property {} normalizes outside its stat range",
            property.name()
        );
    }
    Ok(())
}

fn validate_skill_document_semantics(
    document: &DefinitionDocument,
    semantics: &SkillSemanticCatalog,
) -> Result<()> {
    let configured = semantics
        .configured_properties()
        .filter_map(|(skill_id, property)| (skill_id == document.id).then_some(property))
        .collect::<BTreeSet<_>>();
    if configured.is_empty() {
        return Ok(());
    }
    let mut level_count = 0;
    for group in &document.groups {
        level_count +=
            validate_skill_node_semantics(&group.root, document.id, semantics, &configured)?;
    }
    if level_count == 0 {
        anyhow::bail!("skill {} no longer has direct skill levels", document.id);
    }
    Ok(())
}

fn validate_skill_node_semantics(
    node: &EditableNode,
    skill_id: u32,
    semantics: &SkillSemanticCatalog,
    configured: &BTreeSet<OverloadedSkillProperty>,
) -> Result<usize> {
    if node.removed {
        return Ok(0);
    }
    let mut level_count = 0;
    if is_direct_skill_level(&node.path) {
        level_count = 1;
        for property in configured {
            let Some(property_node) = node
                .children
                .iter()
                .find(|child| child.name == property.name() && !child.removed)
            else {
                anyhow::bail!(
                    "skill {skill_id} level {} no longer has configured direct property {}",
                    node.name,
                    property.name()
                );
            };
            let semantic = semantics
                .property_semantic(skill_id, SkillPropertyScope::Level, property.name())
                .expect("configured property has a semantic mapping");
            if let SkillValueTransform::Numeric { offset } = semantic.transform() {
                validate_numeric_semantic_value(
                    property_node.value.as_ref(),
                    offset,
                    semantic.normalized_stats(),
                    skill_id,
                    *property,
                )?;
            }
        }
    }
    for child in &node.children {
        level_count += validate_skill_node_semantics(child, skill_id, semantics, configured)?;
    }
    Ok(level_count)
}

fn is_direct_skill_level(path: &str) -> bool {
    let mut segments = path.rsplit('/').filter(|segment| !segment.is_empty());
    segments
        .next()
        .is_some_and(|level| level.parse::<u32>().is_ok())
        && segments.next() == Some("level")
}

impl EditorApp {
    pub fn new(
        context: &eframe::CreationContext<'_>,
        prepared: PreparedEditor,
    ) -> Self {
        configure_style(&context.egui_ctx);
        let nimble_feet = prepared
            .skill_entries
            .iter()
            .find(|entry| entry.name.eq_ignore_ascii_case("Nimble Feet"))
            .cloned();
        let skill_document = nimble_feet.as_ref().and_then(|entry| {
            crate::document::load_skill(
                &prepared.skill_archive,
                entry,
                &BTreeMap::new(),
                &BTreeMap::new(),
            )
            .ok()
        });
        Self {
            tab: EditorTab::Skills,
            quest_archive: prepared.quest_archive,
            quest_entries: prepared.quest_entries,
            quest_filter: String::new(),
            quest_document: None,
            quest_script_references: prepared.quest_script_references,
            quest_staged: BTreeMap::new(),
            quest_saved: BTreeMap::new(),
            quest_view: QuestView::Definition,
            skill_archive: prepared.skill_archive,
            skill_entries: prepared.skill_entries,
            skill_filter: String::new(),
            skill_document,
            skill_staged: BTreeMap::new(),
            skill_structure: BTreeMap::new(),
            skill_saved: BTreeMap::new(),
            skill_structure_saved: BTreeMap::new(),
            skill_add_drafts: BTreeMap::new(),
            skill_semantics: prepared.skill_semantics,
            scripts: prepared.scripts,
            scripts_dirty: false,
            paths: prepared.paths,
            status: "Ready. Nimble Feet is selected as the permanent-buff example.".to_owned(),
            confirm_close: false,
            allow_close: false,
        }
    }
}

impl eframe::App for EditorApp {
    fn ui(
        &mut self,
        ui: &mut egui::Ui,
        _frame: &mut eframe::Frame,
    ) {
        if ui.input_mut(|input| {
            input.consume_shortcut(&egui::KeyboardShortcut::new(
                egui::Modifiers::COMMAND,
                egui::Key::S,
            ))
        }) {
            save_active(self);
        }
        egui::Panel::top("wz-editor-toolbar").show(ui, |ui| draw_toolbar(ui, self));
        egui::Panel::bottom("wz-editor-status").show(ui, |ui| draw_status(ui, self));
        egui::Panel::left("wz-editor-definitions")
            .resizable(true)
            .default_size(280.0)
            .size_range(220.0..=420.0)
            .show(ui, |ui| draw_sidebar(ui, self));
        egui::CentralPanel::default().show(ui, |ui| draw_editor(ui, self));
        handle_close_request(ui.ctx(), self);
    }
}

fn configure_style(context: &egui::Context) {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = Color32::from_rgb(30, 31, 27);
    visuals.window_fill = Color32::from_rgb(37, 38, 33);
    visuals.extreme_bg_color = Color32::from_rgb(20, 21, 18);
    visuals.selection.bg_fill = Color32::from_rgb(117, 82, 40);
    visuals.selection.stroke.color = Color32::from_rgb(255, 220, 153);
    visuals.widgets.hovered.bg_fill = Color32::from_rgb(68, 61, 45);
    context.set_visuals(visuals);
}

fn draw_toolbar(
    ui: &mut egui::Ui,
    app: &mut EditorApp,
) {
    ui.horizontal(|ui| {
        ui.colored_label(ACCENT, egui::RichText::new("OOZEMS WZ STUDIO").strong());
        ui.separator();
        ui.selectable_value(&mut app.tab, EditorTab::Quests, "Quest.wz + scripts");
        ui.selectable_value(&mut app.tab, EditorTab::Skills, "Skill.wz");
        ui.separator();
        let save_label = match app.tab {
            EditorTab::Quests => "Save Quest  Ctrl+S",
            EditorTab::Skills => "Save Skill  Ctrl+S",
        };
        if ui.button(save_label).clicked() {
            save_active(app);
        }
        if app.quest_staged != app.quest_saved || app.scripts_dirty {
            ui.colored_label(ACCENT, "Quest changes unsaved");
        }
        if app.skill_staged != app.skill_saved || app.skill_structure != app.skill_structure_saved {
            ui.colored_label(ACCENT, "Skill changes unsaved");
        }
    });
}

fn draw_status(
    ui: &mut egui::Ui,
    app: &EditorApp,
) {
    ui.horizontal(|ui| {
        let color = if app.status.starts_with("Error:") {
            ERROR
        } else {
            MUTED
        };
        ui.colored_label(color, &app.status);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let path = match app.tab {
                EditorTab::Quests => format!(
                    "{} | {}",
                    app.paths.quest_output.display(),
                    app.paths.scripts.display()
                ),
                EditorTab::Skills => app.paths.skill_output.display().to_string(),
            };
            ui.weak(path);
        });
    });
}

fn draw_sidebar(
    ui: &mut egui::Ui,
    app: &mut EditorApp,
) {
    match app.tab {
        EditorTab::Quests => draw_definition_sidebar(ui, app, false),
        EditorTab::Skills => draw_definition_sidebar(ui, app, true),
    }
}

fn draw_definition_sidebar(
    ui: &mut egui::Ui,
    app: &mut EditorApp,
    skill: bool,
) {
    let (heading, filter, entries, selected) = if skill {
        (
            "Skills",
            &mut app.skill_filter,
            &app.skill_entries,
            app.skill_document.as_ref().map(|document| document.id),
        )
    } else {
        (
            "Quests",
            &mut app.quest_filter,
            &app.quest_entries,
            app.quest_document.as_ref().map(|document| document.id),
        )
    };
    ui.heading(heading);
    ui.add(egui::TextEdit::singleline(filter).hint_text("Search by name or ID"));
    ui.separator();
    let query = filter.trim().to_ascii_lowercase();
    let mut open = None;
    egui::ScrollArea::vertical().show(ui, |ui| {
        for entry in entries.iter().filter(|entry| {
            query.is_empty()
                || entry.id.to_string().contains(&query)
                || entry.name.to_ascii_lowercase().contains(&query)
        }) {
            let label = format!("{}  {}", entry.id, entry.name);
            if ui
                .selectable_label(selected == Some(entry.id), label)
                .clicked()
            {
                open = Some(entry.clone());
            }
        }
    });
    if let Some(entry) = open {
        if skill {
            open_skill(app, &entry);
        } else {
            open_quest(app, &entry);
        }
    }
}

fn draw_editor(
    ui: &mut egui::Ui,
    app: &mut EditorApp,
) {
    match app.tab {
        EditorTab::Quests => draw_quest_editor(ui, app),
        EditorTab::Skills => {
            if let Some(document) = app.skill_document.as_mut() {
                draw_definition_document(
                    ui,
                    document,
                    true,
                    &mut app.skill_add_drafts,
                    &app.skill_semantics,
                );
                crate::document::stage_document(document, &mut app.skill_staged);
                crate::document::stage_skill_structure(document, &mut app.skill_structure);
            } else {
                draw_empty(
                    ui,
                    "Select a skill to edit its level and metadata properties.",
                );
            }
        }
    }
}

fn draw_quest_editor(
    ui: &mut egui::Ui,
    app: &mut EditorApp,
) {
    let Some(document) = app.quest_document.as_ref() else {
        draw_empty(
            ui,
            "Select a quest to edit its WZ definition and linked scripts together.",
        );
        return;
    };
    let references = crate::document::script_references(document);
    if matches!(&app.quest_view, QuestView::Script(name) if !references.iter().any(|reference| reference.name == *name))
    {
        app.quest_view = QuestView::Definition;
    }
    ui.horizontal_wrapped(|ui| {
        ui.strong(format!("{}  {}", document.id, document.name));
        ui.separator();
        ui.selectable_value(
            &mut app.quest_view,
            QuestView::Definition,
            "Quest definition",
        );
        for reference in &references {
            let label = format!("{}: {}", reference.phase, reference.name);
            ui.selectable_value(
                &mut app.quest_view,
                QuestView::Script(reference.name.clone()),
                label,
            );
        }
        if references.is_empty() {
            ui.weak("No start or completion script is referenced by this quest.");
        }
    });
    ui.separator();
    match app.quest_view.clone() {
        QuestView::Definition => {
            let document = app.quest_document.as_mut().expect("quest checked above");
            draw_definition_document(
                ui,
                document,
                false,
                &mut app.skill_add_drafts,
                &app.skill_semantics,
            );
            crate::document::stage_document(document, &mut app.quest_staged);
        }
        QuestView::Script(name) => {
            let phase = references
                .iter()
                .find(|reference| reference.name == name)
                .map_or("Quest script", |reference| reference.phase);
            script_form::draw_linked_script_editor(ui, app, phase, &name);
        }
    }
}

fn draw_empty(
    ui: &mut egui::Ui,
    message: &str,
) {
    ui.centered_and_justified(|ui| {
        ui.colored_label(MUTED, message);
    });
}

fn draw_definition_document(
    ui: &mut egui::Ui,
    document: &mut DefinitionDocument,
    skill: bool,
    add_drafts: &mut BTreeMap<String, AddPropertyDraft>,
    semantics: &SkillSemanticCatalog,
) {
    ui.heading(format!("{}  {}", document.id, document.name));
    if skill {
        if let Some(description) = &document.description {
            ui.label(description.replace("\\n", "\n"));
        }
        ui.label(
            "Edit existing properties or add and remove typed properties at any skill container. \
             Selecting Permanent on a Duration stores -1 in WZ.",
        );
    } else {
        ui.label(
            "Values are edited in their existing WZ types. Container and media structure is \
             preserved.",
        );
    }
    ui.separator();
    let skill_id = skill.then_some(document.id);
    let level_descriptions = document.level_descriptions.clone();
    egui::ScrollArea::vertical().show(ui, |ui| {
        for group in &mut document.groups {
            egui::CollapsingHeader::new(group.label)
                .default_open(true)
                .show(ui, |ui| {
                    draw_node(
                        ui,
                        &mut group.root,
                        true,
                        skill_id,
                        &level_descriptions,
                        add_drafts,
                        skill,
                        semantics,
                    )
                });
            ui.add_space(8.0);
        }
    });
}

fn draw_node(
    ui: &mut egui::Ui,
    node: &mut EditableNode,
    root: bool,
    skill_id: Option<u32>,
    level_descriptions: &BTreeMap<u32, String>,
    add_drafts: &mut BTreeMap<String, AddPropertyDraft>,
    structural: bool,
    semantics: &SkillSemanticCatalog,
) -> bool {
    if node.removed {
        ui.horizontal(|ui| {
            ui.colored_label(
                ERROR,
                egui::RichText::new(format!("{} (removed)", node.name)).strikethrough(),
            );
            if ui.small_button("Undo removal").clicked() {
                node.removed = false;
            }
        });
        return false;
    }
    if !node.accepts_properties() {
        return draw_scalar(ui, node, skill_id, structural && !root, semantics);
    }
    let level = node
        .path
        .rsplit_once('/')
        .filter(|(parent, _)| parent.ends_with("/level"))
        .and_then(|(_, level)| level.parse::<u32>().ok());
    let authored_description = level.and_then(|level| level_descriptions.get(&level).cloned());
    let label = if root {
        format!("{} ({})", node.name, node.kind)
    } else if level.is_some() {
        let summary = skill_level_summary(node, skill_id.unwrap_or_default(), semantics);
        if summary.is_empty() {
            format!("Level {}", node.name)
        } else {
            format!("Level {}: {summary}", node.name)
        }
    } else {
        node.name.clone()
    };
    let mut remove = false;
    egui::CollapsingHeader::new(label)
        .id_salt(&node.path)
        .default_open(root || matches!(node.name.as_str(), "level" | "0" | "1"))
        .show(ui, |ui| {
            if let Some(description) = &authored_description {
                ui.weak(format!(
                    "String.wz text: {}",
                    description.replace("\\n", " ")
                ));
            }
            if structural && !root && ui.small_button("Remove property").clicked() {
                remove = true;
                return;
            }
            let mut index = 0;
            while index < node.children.len() {
                let remove_child = draw_node(
                    ui,
                    &mut node.children[index],
                    false,
                    skill_id,
                    level_descriptions,
                    add_drafts,
                    structural,
                    semantics,
                );
                if remove_child && node.children[index].added {
                    let path = node.children[index].path.clone();
                    node.children.remove(index);
                    add_drafts.remove(&path);
                } else {
                    if remove_child {
                        node.children[index].removed = true;
                    }
                    index += 1;
                }
            }
            if structural {
                draw_add_property(ui, node, add_drafts);
            }
        });
    remove
}

fn skill_level_summary(
    level: &EditableNode,
    skill_id: u32,
    semantics: &SkillSemanticCatalog,
) -> String {
    level
        .children
        .iter()
        .filter(|property| !property.removed && property.name != "hs")
        .filter_map(|property| {
            let value = property.value.as_ref()?;
            let label = skill_property_label(
                semantics,
                skill_id,
                SkillPropertyScope::Level,
                &property.name,
            );
            let rendered = match (property.name.as_str(), value) {
                ("time", Value::Number(number)) if number.as_i64() == Some(-1) => {
                    "Permanent".to_owned()
                }
                ("time" | "cooltime", Value::Number(number)) => {
                    format!("{} sec", number.as_i64().unwrap_or_default())
                }
                (_, Value::Number(number)) => number.to_string(),
                (_, Value::String(text)) => text.clone(),
                (_, Value::Object(_)) => value.to_string(),
                (_, Value::Null) => "null".to_owned(),
                _ => return None,
            };
            Some(format!("{label} {rendered}"))
        })
        .collect::<Vec<_>>()
        .join(" | ")
}

fn draw_scalar(
    ui: &mut egui::Ui,
    node: &mut EditableNode,
    skill_id: Option<u32>,
    removable: bool,
    semantics: &SkillSemanticCatalog,
) -> bool {
    let dirty = node.added || node.original != node.value;
    let mut remove = false;
    ui.horizontal(|ui| {
        ui.set_min_height(24.0);
        let property_label = skill_id
            .map(|skill_id| {
                skill_property_label(
                    semantics,
                    skill_id,
                    skill_property_scope(&node.path),
                    &node.name,
                )
            })
            .unwrap_or_else(|| node.name.clone());
        let label = egui::RichText::new(property_label).color(if dirty { ACCENT } else { MUTED });
        ui.add_sized([170.0, 20.0], egui::Label::new(label));
        let Some(value) = node.value.as_mut() else {
            ui.weak(format!("{} (not editable)", node.kind));
            if removable && ui.small_button("Remove").clicked() {
                remove = true;
            }
            return;
        };
        if node.name == "time" && value.as_i64().is_some() {
            draw_duration_value(ui, value, &mut node.timed_value);
        } else {
            draw_json_value(ui, value);
        }
        ui.weak(&node.kind);
        if dirty && !node.added && ui.small_button("Reset").clicked() {
            node.value = node.original.clone();
        }
        if removable && ui.small_button("Remove").clicked() {
            remove = true;
        }
    });
    if node.name == "hs" {
        ui.indent(format!("{}-help", node.path), |ui| {
            ui.small(
                "Level-description selector. For example, h3 selects the h3 text template in \
                 String.wz; it does not change a character stat.",
            );
        });
    }
    remove
}

fn draw_add_property(
    ui: &mut egui::Ui,
    parent: &mut EditableNode,
    drafts: &mut BTreeMap<String, AddPropertyDraft>,
) {
    egui::CollapsingHeader::new("+ Add property")
        .id_salt(format!("{}-add-property", parent.path))
        .show(ui, |ui| {
            let draft = drafts.entry(parent.path.clone()).or_default();
            ui.horizontal(|ui| {
                ui.add_sized(
                    [180.0, 20.0],
                    egui::TextEdit::singleline(&mut draft.name)
                        .hint_text("Property name, e.g. jump"),
                );
                egui::ComboBox::from_id_salt(format!("{}-new-kind", parent.path))
                    .selected_text(draft.kind.label())
                    .show_ui(ui, |ui| {
                        for kind in PropertyKind::ALL {
                            ui.selectable_value(&mut draft.kind, kind, kind.label());
                        }
                    });
                if ui.button("Add").clicked() {
                    let name = draft.name.trim();
                    draft.error = if name.is_empty() {
                        Some("Enter a property name.".to_owned())
                    } else if name.contains('/') || matches!(name, "." | "..") {
                        Some("Property names cannot contain '/' or be '.' or '..'.".to_owned())
                    } else if let Some(existing) =
                        parent.children.iter().find(|child| child.name == name)
                    {
                        if existing.removed {
                            Some(format!(
                                "Property '{name}' is staged for removal. Undo its removal to \
                                 edit it."
                            ))
                        } else {
                            Some(format!("Property '{name}' already exists."))
                        }
                    } else {
                        parent.children.push(EditableNode::new_property(
                            &parent.path,
                            name.to_owned(),
                            draft.kind,
                        ));
                        draft.name.clear();
                        None
                    };
                }
            });
            if let Some(error) = &draft.error {
                ui.colored_label(ERROR, error);
            }
        });
}

fn skill_property_label(
    semantics: &SkillSemanticCatalog,
    skill_id: u32,
    scope: SkillPropertyScope,
    name: &str,
) -> String {
    semantics
        .property_semantic(skill_id, scope, name)
        .map_or_else(
            || name.to_owned(),
            |semantic| format!("{} ({name})", semantic.label()),
        )
}

fn skill_property_scope(path: &str) -> SkillPropertyScope {
    let segments = path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.len() >= 3
        && segments[segments.len() - 3] == "level"
        && segments[segments.len() - 2].parse::<u32>().is_ok()
    {
        SkillPropertyScope::Level
    } else {
        SkillPropertyScope::Common
    }
}

fn draw_duration_value(
    ui: &mut egui::Ui,
    value: &mut Value,
    timed_value: &mut Option<i64>,
) {
    let current = value.as_i64().unwrap_or_default();
    let mut permanent = current == -1;
    if ui.checkbox(&mut permanent, "Permanent").changed() {
        if permanent {
            if current >= 0 {
                *timed_value = Some(current);
            }
            *value = Value::from(-1);
        } else {
            *value = Value::from(timed_value.unwrap_or(1).max(0));
        }
    }
    if !permanent {
        let mut seconds = value.as_i64().unwrap_or_default().max(0);
        if ui
            .add(
                egui::DragValue::new(&mut seconds)
                    .range(0..=i32::MAX)
                    .suffix(" sec"),
            )
            .changed()
        {
            *timed_value = Some(seconds);
            *value = Value::from(seconds);
        }
    }
}

fn draw_json_value(
    ui: &mut egui::Ui,
    value: &mut Value,
) {
    match value {
        Value::Number(number) if number.is_i64() => {
            let mut current = number.as_i64().unwrap_or_default();
            if ui.add(egui::DragValue::new(&mut current)).changed() {
                *value = Value::from(current);
            }
        }
        Value::Number(number) => {
            let mut current = number.as_f64().unwrap_or_default();
            if ui.add(egui::DragValue::new(&mut current)).changed()
                && let Some(number) = serde_json::Number::from_f64(current)
            {
                *value = Value::Number(number);
            }
        }
        Value::String(text) => {
            let multiline = text.contains('\n') || text.len() > 100;
            if multiline {
                ui.add_sized(
                    [ui.available_width().max(220.0), 60.0],
                    egui::TextEdit::multiline(text),
                );
            } else {
                ui.add_sized(
                    [ui.available_width().max(220.0), 20.0],
                    egui::TextEdit::singleline(text),
                );
            }
        }
        Value::Object(object) if object.contains_key("x") && object.contains_key("y") => {
            let mut x = object.get("x").and_then(Value::as_i64).unwrap_or_default();
            let mut y = object.get("y").and_then(Value::as_i64).unwrap_or_default();
            ui.label("x");
            let x_changed = ui.add(egui::DragValue::new(&mut x)).changed();
            ui.label("y");
            let y_changed = ui.add(egui::DragValue::new(&mut y)).changed();
            if x_changed || y_changed {
                object.insert("x".to_owned(), Value::from(x));
                object.insert("y".to_owned(), Value::from(y));
            }
        }
        Value::Null => {
            ui.weak("null");
        }
        _ => {
            ui.weak(value.to_string());
        }
    }
}

fn open_quest(
    app: &mut EditorApp,
    entry: &DefinitionEntry,
) {
    if let Some(document) = &app.quest_document {
        crate::document::stage_document(document, &mut app.quest_staged);
    }
    match crate::document::load_quest(&app.quest_archive, entry, &app.quest_staged) {
        Ok(document) => {
            app.quest_document = Some(document);
            app.quest_view = QuestView::Definition;
            app.status = format!("Loaded quest {}.", entry.id);
        }
        Err(error) => app.status = format!("Error: {error:#}"),
    }
}

fn open_skill(
    app: &mut EditorApp,
    entry: &DefinitionEntry,
) {
    if let Some(document) = &app.skill_document {
        if let Err(error) = validate_skill_document_semantics(document, &app.skill_semantics) {
            app.status = format!("Error: {error:#}");
            return;
        }
        crate::document::stage_document(document, &mut app.skill_staged);
        crate::document::stage_skill_structure(document, &mut app.skill_structure);
    }
    match crate::document::load_skill(
        &app.skill_archive,
        entry,
        &app.skill_staged,
        &app.skill_structure,
    ) {
        Ok(document) => {
            app.skill_document = Some(document);
            app.status = format!("Loaded skill {}.", entry.id);
        }
        Err(error) => app.status = format!("Error: {error:#}"),
    }
}

fn save_active(app: &mut EditorApp) {
    let tab = app.tab;
    let result = match app.tab {
        EditorTab::Quests => save_quest(app),
        EditorTab::Skills => save_skill(app),
    };
    app.status = match result {
        Ok(status) => {
            match tab {
                EditorTab::Quests => app.quest_saved = app.quest_staged.clone(),
                EditorTab::Skills => {
                    app.skill_saved = app.skill_staged.clone();
                    app.skill_structure_saved = app.skill_structure.clone();
                }
            }
            status
        }
        Err(error) => format!("Error: {error:#}"),
    };
}

fn save_skill(app: &mut EditorApp) -> Result<String> {
    if let Some(document) = &app.skill_document {
        validate_skill_document_semantics(document, &app.skill_semantics)?;
        crate::document::stage_document(document, &mut app.skill_staged);
        crate::document::stage_skill_structure(document, &mut app.skill_structure);
    }
    let edits = staged_skill_edits(app);
    if app.skill_staged == app.skill_saved && app.skill_structure == app.skill_structure_saved {
        return Err(anyhow::anyhow!("there are no unsaved skill changes"));
    }
    if edits.is_empty() {
        crate::save::write_archive_source(&app.paths.skill, &app.paths.skill_output)?;
        return Ok(format!(
            "Restored {} to the unedited Skill.wz content.",
            app.paths.skill_output.display()
        ));
    }
    let count = crate::save::write_archive_property_edits(
        &app.paths.skill,
        &app.paths.skill_output,
        &edits,
        OpenOptions::default(),
    )?;
    Ok(format!(
        "Saved {count} skill changes to {}.",
        app.paths.skill_output.display()
    ))
}

fn has_unsaved_changes(app: &EditorApp) -> bool {
    app.quest_staged != app.quest_saved
        || app.skill_staged != app.skill_saved
        || app.skill_structure != app.skill_structure_saved
        || app.scripts_dirty
}

fn handle_close_request(
    context: &egui::Context,
    app: &mut EditorApp,
) {
    if app.allow_close {
        return;
    }
    if context.input(|input| input.viewport().close_requested()) && has_unsaved_changes(app) {
        context.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        app.confirm_close = true;
    }
    if !app.confirm_close {
        return;
    }
    egui::Window::new("Unsaved changes")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(context, |ui| {
            ui.label("Quest, skill, or script changes have not been saved.");
            ui.label("Use the Save button on each changed tab before closing.");
            ui.horizontal(|ui| {
                if ui.button("Keep editing").clicked() {
                    app.confirm_close = false;
                }
                if ui.button("Close without saving").clicked() {
                    app.allow_close = true;
                    app.confirm_close = false;
                    context.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        });
}

fn staged_skill_edits(app: &EditorApp) -> Vec<PropertyEdit> {
    let mut edits = app
        .skill_structure
        .values()
        .filter(|edit| matches!(edit, PropertyEdit::Add { .. }))
        .cloned()
        .collect::<Vec<_>>();
    edits.extend(
        app.skill_staged
            .iter()
            .map(|(path, value)| PropertyEdit::Set {
                path: path.clone(),
                value: value.clone(),
            }),
    );
    edits.extend(
        app.skill_structure
            .values()
            .filter(|edit| matches!(edit, PropertyEdit::Remove { .. }))
            .cloned(),
    );
    edits
}

fn save_quest(app: &mut EditorApp) -> Result<String> {
    if let Some(document) = &app.quest_document {
        crate::document::stage_document(document, &mut app.quest_staged);
    }
    reconcile_script_references(app);
    let wz_changed = app.quest_staged != app.quest_saved;
    if !wz_changed && !app.scripts_dirty {
        anyhow::bail!("there are no unsaved quest or script changes");
    }
    let script_source = app
        .scripts_dirty
        .then(|| crate::scripts::encode(&app.scripts))
        .transpose()?;
    let mut saved = Vec::new();
    if wz_changed {
        if app.quest_staged.is_empty() {
            crate::save::write_archive_source(&app.paths.quest, &app.paths.quest_output)?;
            saved.push(format!(
                "unedited Quest.wz content to {}",
                app.paths.quest_output.display()
            ));
        } else {
            let count = crate::save::write_archive_edits(
                &app.paths.quest,
                &app.paths.quest_output,
                &app.quest_staged,
                OpenOptions::default(),
            )?;
            saved.push(format!(
                "{count} WZ changes to {}",
                app.paths.quest_output.display()
            ));
        }
    }
    if let Some(source) = script_source {
        crate::save::write_text_atomic(&app.paths.scripts, &source)?;
        app.scripts_dirty = false;
        saved.push(format!("linked scripts to {}", app.paths.scripts.display()));
    }
    Ok(format!("Saved {}.", saved.join(" and ")))
}

fn reconcile_script_references(app: &mut EditorApp) {
    for (path, original) in &app.quest_script_references {
        let current = app
            .quest_staged
            .get(path)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or(original);
        let previous = app
            .quest_saved
            .get(path)
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or(original);
        if current == previous
            || current.is_empty()
            || app
                .scripts
                .scripts
                .iter()
                .any(|script| script.name == current)
        {
            continue;
        }
        let source = app
            .scripts
            .scripts
            .iter()
            .find(|script| script.name == previous)
            .or_else(|| {
                app.scripts
                    .scripts
                    .iter()
                    .find(|script| script.name == *original)
            })
            .cloned();
        let mut program = source.unwrap_or_default();
        program.name = current.to_owned();
        app.scripts.scripts.push(program);
        app.scripts_dirty = true;
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use oozems_skill_semantics::SkillPropertyScope;
    use oozems_skill_semantics::SkillSemanticCatalog;
    use oozems_wz::PropertyKind;
    use serde_json::Value;

    use super::EditableNode;
    use super::skill_level_summary;
    use super::skill_property_label;
    use super::skill_property_scope;
    use super::validate_skill_document_semantics;
    use crate::document::DefinitionDocument;
    use crate::document::DefinitionGroup;

    #[test]
    fn skill_property_labels_keep_raw_nodes_and_explain_affected_stats() {
        let semantics = test_semantics();
        assert_eq!(
            skill_property_label(&semantics, 40, SkillPropertyScope::Level, "speed"),
            "Speed (speed)"
        );
        assert_eq!(
            skill_property_label(&semantics, 40, SkillPropertyScope::Level, "hs"),
            "Level description selector (hs)"
        );
        assert_eq!(
            skill_property_label(&semantics, 40, SkillPropertyScope::Level, "x"),
            "Accuracy (x)"
        );
        assert_eq!(
            skill_property_label(&semantics, 40, SkillPropertyScope::Level, "y"),
            "Avoidability (y)"
        );
        assert_eq!(
            skill_property_label(&semantics, 40, SkillPropertyScope::Common, "x"),
            "x"
        );
        assert_eq!(
            skill_property_label(&semantics, 40, SkillPropertyScope::Level, "unknown"),
            "unknown"
        );
    }

    #[test]
    fn overloaded_labels_apply_only_to_direct_level_properties() {
        assert_eq!(
            skill_property_scope("/400.img/skill/4000000/level/1/x"),
            SkillPropertyScope::Level
        );
        assert_eq!(
            skill_property_scope("/400.img/skill/4000000/common/x"),
            SkillPropertyScope::Common
        );
        assert_eq!(
            skill_property_scope("/400.img/skill/4000000/level/1/effect/x"),
            SkillPropertyScope::Common
        );
    }

    #[test]
    fn edited_numeric_mappings_reject_formulas_and_removal() {
        let semantics = test_semantics();
        let mut document = mapped_document(Value::from(7));
        validate_skill_document_semantics(&document, &semantics).expect("numeric mapping");

        document.groups[0].root.children[0].children[0].children[0].value =
            Some(Value::from("10+2*x"));
        assert!(validate_skill_document_semantics(&document, &semantics).is_err());

        document.groups[0].root.children[0].children[0].children[0].removed = true;
        assert!(validate_skill_document_semantics(&document, &semantics).is_err());

        let document = mapped_document(Value::from(i64::MAX));
        assert!(validate_skill_document_semantics(&document, &semantics).is_err());
    }

    #[test]
    fn live_skill_summary_includes_added_effects_and_hides_hs_metadata() {
        let mut level = EditableNode::new_property(
            "/000.img/skill/0001002/level",
            "1".to_owned(),
            PropertyKind::Property,
        );
        let mut jump =
            EditableNode::new_property(&level.path, "jump".to_owned(), PropertyKind::Int);
        jump.value = Some(Value::from(10));
        let mut duration =
            EditableNode::new_property(&level.path, "time".to_owned(), PropertyKind::Int);
        duration.value = Some(Value::from(-1));
        let mut hs = EditableNode::new_property(&level.path, "hs".to_owned(), PropertyKind::String);
        hs.value = Some(Value::from("h1"));
        level.children = vec![jump, duration, hs];

        assert_eq!(
            skill_level_summary(&level, 1_002, &SkillSemanticCatalog::default()),
            "Jump (jump) 10 | Duration (time) Permanent"
        );
    }

    fn test_semantics() -> SkillSemanticCatalog {
        oozems_skill_semantics::parse(
            r#"
schema_version = 1

[[level_properties]]
skill_ids = [40]
property = "x"
label = "Accuracy"
normalized_stats = ["accuracy"]
transform = { type = "numeric" }

[[level_properties]]
skill_ids = [40]
property = "y"
label = "Avoidability"
normalized_stats = ["avoidability"]
transform = { type = "numeric" }
"#,
        )
        .expect("test semantic mappings")
    }

    fn mapped_document(value: Value) -> DefinitionDocument {
        let mut root =
            EditableNode::new_property("/400.img/skill", "40".to_owned(), PropertyKind::Property);
        let mut levels =
            EditableNode::new_property(&root.path, "level".to_owned(), PropertyKind::Property);
        let mut level =
            EditableNode::new_property(&levels.path, "1".to_owned(), PropertyKind::Property);
        let mut x = EditableNode::new_property(&level.path, "x".to_owned(), PropertyKind::Int);
        x.value = Some(value);
        level.children.push(x);
        let mut y = EditableNode::new_property(&level.path, "y".to_owned(), PropertyKind::Int);
        y.value = Some(Value::from(3));
        level.children.push(y);
        levels.children.push(level);
        let mut level =
            EditableNode::new_property(&levels.path, "2".to_owned(), PropertyKind::Property);
        let mut x = EditableNode::new_property(&level.path, "x".to_owned(), PropertyKind::Int);
        x.value = Some(Value::from(8));
        level.children.push(x);
        let mut y = EditableNode::new_property(&level.path, "y".to_owned(), PropertyKind::Int);
        y.value = Some(Value::from(4));
        level.children.push(y);
        levels.children.push(level);
        root.children.push(levels);
        DefinitionDocument {
            id: 40,
            name: "Mapped skill".to_owned(),
            description: None,
            level_descriptions: BTreeMap::new(),
            groups: vec![DefinitionGroup {
                label: "Skill definition",
                root,
            }],
        }
    }
}
