use std::collections::BTreeMap;
use std::collections::BTreeSet;

use anyhow::Context;
use anyhow::Result;
use oozems_wz::Archive;
use oozems_wz::NodeTree;
use oozems_wz::PropertyEdit;
use oozems_wz::PropertyKind;
use serde_json::Value;

const MAXIMUM_INDEX_NODES: usize = 500_000;
const MAXIMUM_DEFINITION_NODES: usize = 50_000;

#[derive(Clone, Debug)]
pub struct DefinitionEntry {
    pub id: u32,
    pub name: String,
    pub path: String,
    pub description: Option<String>,
    pub level_descriptions: BTreeMap<u32, String>,
}

#[derive(Clone, Debug)]
pub struct DefinitionDocument {
    pub id: u32,
    pub name: String,
    pub description: Option<String>,
    pub level_descriptions: BTreeMap<u32, String>,
    pub groups: Vec<DefinitionGroup>,
}

struct SkillText {
    name: String,
    description: Option<String>,
    levels: BTreeMap<u32, String>,
}

#[derive(Clone, Debug)]
pub struct DefinitionGroup {
    pub label: &'static str,
    pub root: EditableNode,
}

#[derive(Clone, Debug)]
pub struct EditableNode {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub original: Option<Value>,
    pub value: Option<Value>,
    pub timed_value: Option<i64>,
    pub children: Vec<EditableNode>,
    pub added: bool,
    pub removed: bool,
}

impl EditableNode {
    pub fn new_property(
        parent_path: &str,
        name: String,
        kind: PropertyKind,
    ) -> Self {
        let (kind_name, value) = match kind {
            PropertyKind::Short => ("short", Some(Value::from(0))),
            PropertyKind::Int => ("int", Some(Value::from(0))),
            PropertyKind::Long => ("long", Some(Value::from(0))),
            PropertyKind::Float => ("float", Some(Value::from(0.0))),
            PropertyKind::Double => ("double", Some(Value::from(0.0))),
            PropertyKind::String => ("string", Some(Value::from(""))),
            PropertyKind::Vector => ("vector", Some(serde_json::json!({ "x": 0, "y": 0 }))),
            PropertyKind::Null => ("null", None),
            PropertyKind::Property => ("property", None),
        };
        Self {
            path: format!("{parent_path}/{name}"),
            name,
            kind: kind_name.to_owned(),
            original: None,
            value,
            timed_value: None,
            children: Vec::new(),
            added: true,
            removed: false,
        }
    }

    pub fn accepts_properties(&self) -> bool {
        matches!(
            self.kind.as_str(),
            "image" | "property" | "canvas" | "convex" | "video"
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScriptReference {
    pub phase: &'static str,
    pub name: String,
}

pub fn quest_index(archive: &Archive) -> Result<Vec<DefinitionEntry>> {
    let root = oozems_wz::tree(archive, "/QuestInfo.img", MAXIMUM_INDEX_NODES)
        .context("failed to index Quest.wz definitions")?;
    let mut entries = root
        .children
        .into_iter()
        .filter_map(|quest| {
            let id = quest.name.parse().ok()?;
            let name = child_string(&quest, "name").unwrap_or_else(|| format!("Quest {id}"));
            Some(DefinitionEntry {
                id,
                name,
                path: format!("/QuestInfo.img/{id}"),
                description: None,
                level_descriptions: BTreeMap::new(),
            })
        })
        .collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.id);
    Ok(entries)
}

pub fn quest_script_reference_paths(archive: &Archive) -> Result<BTreeMap<String, String>> {
    let root = oozems_wz::tree(archive, "/Check.img", MAXIMUM_INDEX_NODES)
        .context("failed to index Quest.wz script references")?;
    let mut references = BTreeMap::new();
    collect_tree_script_references("/Check.img", &root, &mut references);
    Ok(references)
}

fn collect_tree_script_references(
    path: &str,
    node: &NodeTree,
    references: &mut BTreeMap<String, String>,
) {
    if matches!(node.name.as_str(), "startscript" | "endscript")
        && let Some(name) = node.value.as_ref().and_then(Value::as_str)
        && !name.trim().is_empty()
    {
        references.insert(path.to_owned(), name.trim().to_owned());
    }
    for child in &node.children {
        let child_path = format!("{path}/{}", child.name);
        collect_tree_script_references(&child_path, child, references);
    }
}

pub fn skill_index(
    archive: &Archive,
    strings: &Archive,
) -> Result<Vec<DefinitionEntry>> {
    let texts = skill_texts(strings)?;
    let images =
        oozems_wz::list(archive, "/", 0, usize::MAX).context("failed to list Skill.wz images")?;
    let mut entries = Vec::new();
    let mut skill_ids = BTreeSet::new();
    for image in images.nodes {
        let Some(job) = image.name.strip_suffix(".img") else {
            continue;
        };
        if job.parse::<u32>().is_err() {
            continue;
        }
        let skill_root = format!("/{}/skill", image.name);
        let Ok(skills) = oozems_wz::list(archive, &skill_root, 0, usize::MAX) else {
            continue;
        };
        for skill in skills.nodes {
            let Ok(id) = skill.name.parse::<u32>() else {
                continue;
            };
            if !skill_ids.insert(id) {
                anyhow::bail!("skill {id} appears more than once in Skill.wz");
            }
            let text = texts.get(&id);
            entries.push(DefinitionEntry {
                id,
                name: text
                    .map(|text| text.name.clone())
                    .unwrap_or_else(|| format!("Skill {id}")),
                path: format!("{skill_root}/{}", skill.name),
                description: text.and_then(|text| text.description.clone()),
                level_descriptions: text.map(|text| text.levels.clone()).unwrap_or_default(),
            });
        }
    }
    entries.sort_by_key(|entry| entry.id);
    Ok(entries)
}

fn skill_texts(strings: &Archive) -> Result<BTreeMap<u32, SkillText>> {
    let root = oozems_wz::tree(strings, "/Skill.img", MAXIMUM_INDEX_NODES)
        .context("failed to index skill names in String.wz")?;
    Ok(root
        .children
        .into_iter()
        .filter_map(|skill| {
            let id = skill.name.parse().ok()?;
            let levels = skill
                .children
                .iter()
                .filter_map(|child| {
                    let level = child.name.strip_prefix('h')?.parse().ok()?;
                    let text = child.value.as_ref()?.as_str()?.to_owned();
                    Some((level, text))
                })
                .collect();
            Some((
                id,
                SkillText {
                    name: child_string(&skill, "name").unwrap_or_else(|| format!("Skill {id}")),
                    description: child_string(&skill, "desc"),
                    levels,
                },
            ))
        })
        .collect())
}

fn child_string(
    node: &NodeTree,
    name: &str,
) -> Option<String> {
    node.children
        .iter()
        .find(|child| child.name == name)?
        .value
        .as_ref()?
        .as_str()
        .map(str::to_owned)
}

pub fn load_quest(
    archive: &Archive,
    entry: &DefinitionEntry,
    staged: &BTreeMap<String, Value>,
) -> Result<DefinitionDocument> {
    let paths = [
        ("Quest information", format!("/QuestInfo.img/{}", entry.id)),
        (
            "Start and completion checks",
            format!("/Check.img/{}", entry.id),
        ),
        (
            "Start and completion actions",
            format!("/Act.img/{}", entry.id),
        ),
        ("Dialogue", format!("/Say.img/{}", entry.id)),
    ];
    let mut groups = Vec::new();
    for (label, path) in paths {
        let Ok(tree) = oozems_wz::tree(archive, &path, MAXIMUM_DEFINITION_NODES) else {
            continue;
        };
        groups.push(DefinitionGroup {
            label,
            root: editable_tree(&path, tree, staged),
        });
    }
    if groups.is_empty() {
        anyhow::bail!("quest {} has no editable definitions", entry.id);
    }
    Ok(DefinitionDocument {
        id: entry.id,
        name: entry.name.clone(),
        description: None,
        level_descriptions: BTreeMap::new(),
        groups,
    })
}

pub fn load_skill(
    archive: &Archive,
    entry: &DefinitionEntry,
    staged: &BTreeMap<String, Value>,
    structural: &BTreeMap<String, PropertyEdit>,
) -> Result<DefinitionDocument> {
    let tree = oozems_wz::tree(archive, &entry.path, MAXIMUM_DEFINITION_NODES)
        .with_context(|| format!("failed to load skill {}", entry.id))?;
    let mut root = editable_tree(&entry.path, tree, staged);
    apply_structural_edits(&mut root, structural);
    Ok(DefinitionDocument {
        id: entry.id,
        name: entry.name.clone(),
        description: entry.description.clone(),
        level_descriptions: entry.level_descriptions.clone(),
        groups: vec![DefinitionGroup {
            label: "Skill definition",
            root,
        }],
    })
}

fn editable_tree(
    path: &str,
    tree: NodeTree,
    staged: &BTreeMap<String, Value>,
) -> EditableNode {
    let value = staged.get(path).cloned().or_else(|| tree.value.clone());
    let timed_value = (tree.name == "time")
        .then(|| value.as_ref().and_then(Value::as_i64))
        .flatten()
        .filter(|value| *value >= 0);
    EditableNode {
        name: tree.name,
        path: path.to_owned(),
        kind: tree.kind.to_owned(),
        original: tree.value,
        value,
        timed_value,
        children: tree
            .children
            .into_iter()
            .map(|child| {
                let child_path = format!("{path}/{}", child.name);
                editable_tree(&child_path, child, staged)
            })
            .collect(),
        added: false,
        removed: false,
    }
}

fn apply_structural_edits(
    root: &mut EditableNode,
    structural: &BTreeMap<String, PropertyEdit>,
) {
    for edit in structural.values() {
        match edit {
            PropertyEdit::Add { path, kind, value } => {
                let Some((parent_path, name)) = path.rsplit_once('/') else {
                    continue;
                };
                let Some(parent) = find_editable_node_mut(root, parent_path) else {
                    continue;
                };
                if parent.children.iter().all(|child| child.name != name) {
                    let mut node = EditableNode::new_property(parent_path, name.to_owned(), *kind);
                    if node.value.is_some() {
                        node.value = Some(value.clone());
                    }
                    parent.children.push(node);
                }
            }
            PropertyEdit::Remove { path } => {
                if let Some(node) = find_editable_node_mut(root, path) {
                    node.removed = true;
                }
            }
            PropertyEdit::Set { .. } => {}
        }
    }
}

fn find_editable_node_mut<'a>(
    node: &'a mut EditableNode,
    path: &str,
) -> Option<&'a mut EditableNode> {
    if node.path == path {
        return Some(node);
    }
    node.children
        .iter_mut()
        .find_map(|child| find_editable_node_mut(child, path))
}

pub fn stage_document(
    document: &DefinitionDocument,
    staged: &mut BTreeMap<String, Value>,
) {
    for group in &document.groups {
        stage_node(&group.root, staged);
    }
}

pub fn script_references(document: &DefinitionDocument) -> Vec<ScriptReference> {
    let mut references = Vec::new();
    for group in document
        .groups
        .iter()
        .filter(|group| group.root.path.starts_with("/Check.img/"))
    {
        collect_script_references(&group.root, &mut references);
    }
    let mut names = std::collections::BTreeSet::new();
    references.retain(|reference| names.insert(reference.name.clone()));
    references
}

fn collect_script_references(
    node: &EditableNode,
    references: &mut Vec<ScriptReference>,
) {
    let phase = match node.name.as_str() {
        "startscript" => Some("Start script"),
        "endscript" => Some("Completion script"),
        _ => None,
    };
    if let (Some(phase), Some(name)) = (phase, node.value.as_ref().and_then(Value::as_str))
        && !name.trim().is_empty()
    {
        references.push(ScriptReference {
            phase,
            name: name.trim().to_owned(),
        });
    }
    for child in &node.children {
        collect_script_references(child, references);
    }
}

fn stage_node(
    node: &EditableNode,
    staged: &mut BTreeMap<String, Value>,
) {
    match (&node.original, &node.value) {
        _ if node.added || node.removed => {
            staged.remove(&node.path);
        }
        (Some(original), Some(value)) if original != value => {
            staged.insert(node.path.clone(), value.clone());
        }
        _ => {
            staged.remove(&node.path);
        }
    }
    if node.removed {
        let child_prefix = format!("{}/", node.path);
        staged.retain(|path, _| !path.starts_with(&child_prefix));
        return;
    }
    for child in &node.children {
        stage_node(child, staged);
    }
}

pub fn stage_skill_structure(
    document: &DefinitionDocument,
    staged: &mut BTreeMap<String, PropertyEdit>,
) {
    for group in &document.groups {
        staged.retain(|path, _| !path.starts_with(&format!("{}/", group.root.path)));
        collect_structural_edits(&group.root, staged);
    }
}

fn collect_structural_edits(
    node: &EditableNode,
    staged: &mut BTreeMap<String, PropertyEdit>,
) {
    if node.removed {
        if !node.added {
            staged.insert(
                node.path.clone(),
                PropertyEdit::Remove {
                    path: node.path.clone(),
                },
            );
        }
        return;
    }
    if node.added {
        staged.insert(
            node.path.clone(),
            PropertyEdit::Add {
                path: node.path.clone(),
                kind: property_kind(&node.kind),
                value: node.value.clone().unwrap_or(Value::Null),
            },
        );
    }
    for child in &node.children {
        collect_structural_edits(child, staged);
    }
}

fn property_kind(kind: &str) -> PropertyKind {
    match kind {
        "short" => PropertyKind::Short,
        "long" => PropertyKind::Long,
        "float" => PropertyKind::Float,
        "double" => PropertyKind::Double,
        "string" => PropertyKind::String,
        "vector" => PropertyKind::Vector,
        "null" => PropertyKind::Null,
        "property" => PropertyKind::Property,
        _ => PropertyKind::Int,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_contains_only_changed_scalar_values() {
        let mut staged = BTreeMap::new();
        let document = DefinitionDocument {
            id: 1,
            name: "Test".to_owned(),
            description: None,
            level_descriptions: BTreeMap::new(),
            groups: vec![DefinitionGroup {
                label: "Test",
                root: EditableNode {
                    name: "time".to_owned(),
                    path: "/Skill.img/1/time".to_owned(),
                    kind: "int".to_owned(),
                    original: Some(Value::from(12)),
                    value: Some(Value::from(-1)),
                    timed_value: Some(12),
                    children: Vec::new(),
                    added: false,
                    removed: false,
                },
            }],
        };

        stage_document(&document, &mut staged);

        assert_eq!(staged["/Skill.img/1/time"], Value::from(-1));
    }

    #[test]
    fn script_references_follow_the_current_quest_values() {
        let document = DefinitionDocument {
            id: 1,
            name: "Test".to_owned(),
            description: None,
            level_descriptions: BTreeMap::new(),
            groups: vec![DefinitionGroup {
                label: "Checks",
                root: EditableNode {
                    name: "0".to_owned(),
                    path: "/Check.img/1/0".to_owned(),
                    kind: "property".to_owned(),
                    original: None,
                    value: None,
                    timed_value: None,
                    added: false,
                    removed: false,
                    children: vec![EditableNode {
                        name: "startscript".to_owned(),
                        path: "/Check.img/1/0/startscript".to_owned(),
                        kind: "string".to_owned(),
                        original: Some(Value::from("old")),
                        value: Some(Value::from("q1s")),
                        timed_value: None,
                        children: Vec::new(),
                        added: false,
                        removed: false,
                    }],
                },
            }],
        };

        assert_eq!(
            script_references(&document),
            [ScriptReference {
                phase: "Start script",
                name: "q1s".to_owned(),
            }]
        );
    }

    #[test]
    fn skill_structure_stages_added_and_removed_properties() {
        let mut root = EditableNode {
            name: "1002".to_owned(),
            path: "/000.img/skill/1002".to_owned(),
            kind: "property".to_owned(),
            original: None,
            value: None,
            timed_value: None,
            children: vec![EditableNode {
                name: "hs".to_owned(),
                path: "/000.img/skill/1002/hs".to_owned(),
                kind: "string".to_owned(),
                original: Some(Value::from("h3")),
                value: Some(Value::from("h3")),
                timed_value: None,
                children: Vec::new(),
                added: false,
                removed: true,
            }],
            added: false,
            removed: false,
        };
        root.children.push(EditableNode::new_property(
            &root.path,
            "jump".to_owned(),
            PropertyKind::Int,
        ));
        root.children[1].value = Some(Value::from(12));
        let document = DefinitionDocument {
            id: 1002,
            name: "Nimble Feet".to_owned(),
            description: None,
            level_descriptions: BTreeMap::new(),
            groups: vec![DefinitionGroup {
                label: "Skill definition",
                root,
            }],
        };
        let mut staged = BTreeMap::new();

        stage_skill_structure(&document, &mut staged);

        assert!(matches!(
            staged["/000.img/skill/1002/jump"],
            PropertyEdit::Add {
                kind: PropertyKind::Int,
                ref value,
                ..
            } if value == &Value::from(12)
        ));
        assert!(matches!(
            staged["/000.img/skill/1002/hs"],
            PropertyEdit::Remove { .. }
        ));
    }

    #[test]
    fn removing_a_container_clears_staged_descendant_values() {
        let changed_path = "/000.img/skill/1002/level/1/speed";
        let document = DefinitionDocument {
            id: 1002,
            name: "Nimble Feet".to_owned(),
            description: None,
            level_descriptions: BTreeMap::new(),
            groups: vec![DefinitionGroup {
                label: "Skill definition",
                root: EditableNode {
                    name: "1".to_owned(),
                    path: "/000.img/skill/1002/level/1".to_owned(),
                    kind: "property".to_owned(),
                    original: None,
                    value: None,
                    timed_value: None,
                    children: vec![EditableNode {
                        name: "speed".to_owned(),
                        path: changed_path.to_owned(),
                        kind: "int".to_owned(),
                        original: Some(Value::from(10)),
                        value: Some(Value::from(20)),
                        timed_value: None,
                        children: Vec::new(),
                        added: false,
                        removed: false,
                    }],
                    added: false,
                    removed: true,
                },
            }],
        };
        let mut staged = BTreeMap::from([(changed_path.to_owned(), Value::from(20))]);

        stage_document(&document, &mut staged);

        assert!(staged.is_empty());
    }
}
