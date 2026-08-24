use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use oozems_wz::Archive;
use oozems_wz::NodeTree;
use oozems_wz::OpenOptions;
use serde::Serialize;

use crate::script::QuestPhase;

const MAXIMUM_INDEX_NODES: usize = 500_000;
const MAXIMUM_BRANCH_NODES: usize = 100_000;
const MAXIMUM_STRING_NODES: usize = 300_000;
const MAXIMUM_NOTES_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Serialize)]
pub struct ScriptReference {
    pub phase: QuestPhase,
    pub name: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct QuestSummary {
    pub quest_id: u32,
    pub name: String,
    pub scripts: Vec<ScriptReference>,
}

#[derive(Debug, Serialize)]
pub struct EvidenceBundle {
    pub quest_id: u32,
    pub quest_name: String,
    pub scripts: Vec<ScriptReference>,
    pub quest_wz: QuestArchiveEvidence,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub npc_wz: Vec<NpcMetadataEvidence>,
    #[serde(skip_serializing_if = "StringArchiveEvidence::is_empty")]
    pub string_wz: StringArchiveEvidence,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct QuestArchiveEvidence {
    pub check: NodeTree,
    pub act: NodeTree,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub say: Option<NodeTree>,
    pub info: NodeTree,
}

#[derive(Clone, Debug, Serialize)]
pub struct NpcMetadataEvidence {
    pub npc_id: u32,
    pub info: NodeTree,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct StringArchiveEvidence {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub npcs: Vec<ReferencedText>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<ReferencedText>,
}

impl StringArchiveEvidence {
    fn is_empty(&self) -> bool {
        self.npcs.is_empty() && self.items.is_empty()
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ReferencedText {
    pub id: u32,
    pub text: NodeTree,
}

#[derive(Clone, Debug)]
pub struct EvidencePaths {
    pub quest_wz: PathBuf,
    pub npc_wz: Option<PathBuf>,
    pub string_wz: Option<PathBuf>,
    pub notes: Option<PathBuf>,
}

pub struct EvidenceSource {
    check_index: NodeTree,
    act_index: NodeTree,
    say_index: Option<NodeTree>,
    info_index: NodeTree,
    catalog: Vec<QuestSummary>,
    npc_archive: Option<Archive>,
    string_archive: Option<Archive>,
    notes: Option<String>,
}

#[derive(Default)]
pub struct EvidenceCache {
    npc_images: Option<BTreeSet<String>>,
    npc_metadata: BTreeMap<u32, Option<NpcMetadataEvidence>>,
    string_trees: BTreeMap<String, NodeTree>,
}

pub fn default_associated_archive(
    quest_wz: &Path,
    file_name: &str,
) -> Option<PathBuf> {
    let path = quest_wz.parent()?.join(file_name);
    path.is_file().then_some(path)
}

pub fn catalog(
    quest_wz: &Path,
    options: OpenOptions,
) -> Result<Vec<QuestSummary>> {
    let archive = oozems_wz::open_archive(quest_wz, options)?;
    let check = oozems_wz::tree(&archive, "/Check.img", MAXIMUM_INDEX_NODES)?;
    let info = oozems_wz::tree(&archive, "/QuestInfo.img", MAXIMUM_INDEX_NODES)?;
    build_catalog(&check, &info)
}

pub fn assemble(
    paths: &EvidencePaths,
    selector: &str,
    phase: Option<QuestPhase>,
    options: OpenOptions,
) -> Result<EvidenceBundle> {
    let source = open_source(paths, options)?;
    let mut cache = EvidenceCache::default();
    assemble_from_source(&source, &mut cache, selector, phase)
}

pub fn open_source(
    paths: &EvidencePaths,
    options: OpenOptions,
) -> Result<EvidenceSource> {
    let quest_archive = oozems_wz::open_archive(&paths.quest_wz, options)?;
    let check_index = oozems_wz::tree(&quest_archive, "/Check.img", MAXIMUM_INDEX_NODES)?;
    let act_index = oozems_wz::tree(&quest_archive, "/Act.img", MAXIMUM_INDEX_NODES)?;
    let root = oozems_wz::list(&quest_archive, "/", 0, usize::MAX)?;
    let say_index = root
        .nodes
        .iter()
        .any(|node| node.name == "Say.img")
        .then(|| oozems_wz::tree(&quest_archive, "/Say.img", MAXIMUM_INDEX_NODES))
        .transpose()?;
    let info_index = oozems_wz::tree(&quest_archive, "/QuestInfo.img", MAXIMUM_INDEX_NODES)?;
    let catalog = build_catalog(&check_index, &info_index)?;
    let npc_archive = paths
        .npc_wz
        .as_deref()
        .map(|path| oozems_wz::open_archive(path, options))
        .transpose()?;
    let string_archive = paths
        .string_wz
        .as_deref()
        .map(|path| oozems_wz::open_archive(path, options))
        .transpose()?;
    let notes = paths.notes.as_deref().map(read_notes).transpose()?;
    Ok(EvidenceSource {
        check_index,
        act_index,
        say_index,
        info_index,
        catalog,
        npc_archive,
        string_archive,
        notes,
    })
}

pub fn source_catalog(source: &EvidenceSource) -> &[QuestSummary] {
    &source.catalog
}

pub fn assemble_from_source(
    source: &EvidenceSource,
    cache: &mut EvidenceCache,
    selector: &str,
    phase: Option<QuestPhase>,
) -> Result<EvidenceBundle> {
    let selected = select_quest(&source.catalog, selector)?;
    let scripts = selected
        .scripts
        .iter()
        .filter(|script| phase.is_none_or(|phase| script.phase == phase))
        .cloned()
        .collect::<Vec<_>>();
    if scripts.is_empty() {
        bail!(
            "quest {} has no {} script",
            selected.quest_id,
            phase.map_or("selected".to_owned(), |phase| phase.as_str().to_owned())
        );
    }

    let key = selected.quest_id.to_string();
    let check = required_child(&source.check_index, &key, "Check.img")?.clone();
    let info = required_child(&source.info_index, &key, "QuestInfo.img")?.clone();
    let act = required_child(&source.act_index, &key, "Act.img")?.clone();
    let say = source
        .say_index
        .as_ref()
        .and_then(|index| child(index, &key))
        .cloned();
    let quest_wz = QuestArchiveEvidence {
        check,
        act,
        say,
        info,
    };
    let (npc_ids, item_ids) = referenced_ids(&quest_wz);
    let npc_wz = load_npc_metadata(source.npc_archive.as_ref(), &npc_ids, cache)?;
    let string_wz =
        load_referenced_text(source.string_archive.as_ref(), &npc_ids, &item_ids, cache)?;

    Ok(EvidenceBundle {
        quest_id: selected.quest_id,
        quest_name: selected.name.clone(),
        scripts,
        quest_wz,
        npc_wz,
        string_wz,
        notes: source.notes.clone(),
    })
}

fn build_catalog(
    check: &NodeTree,
    info: &NodeTree,
) -> Result<Vec<QuestSummary>> {
    let mut quests = Vec::new();
    for quest in &check.children {
        let Ok(quest_id) = quest.name.parse::<u32>() else {
            continue;
        };
        let mut scripts = Vec::new();
        if let Some(name) = child(quest, "0")
            .and_then(|phase| child(phase, "startscript"))
            .and_then(string_value)
        {
            scripts.push(ScriptReference {
                phase: QuestPhase::Start,
                name: name.to_owned(),
            });
        }
        if let Some(name) = child(quest, "1")
            .and_then(|phase| child(phase, "endscript"))
            .and_then(string_value)
        {
            scripts.push(ScriptReference {
                phase: QuestPhase::Completion,
                name: name.to_owned(),
            });
        }
        if scripts.is_empty() {
            continue;
        }
        let name = child(info, &quest.name)
            .and_then(|quest| child(quest, "name"))
            .and_then(string_value)
            .unwrap_or("Unnamed quest")
            .to_owned();
        quests.push(QuestSummary {
            quest_id,
            name,
            scripts,
        });
    }
    quests.sort_by_key(|quest| quest.quest_id);
    Ok(quests)
}

fn select_quest<'a>(
    catalog: &'a [QuestSummary],
    selector: &str,
) -> Result<&'a QuestSummary> {
    let selector = selector.trim();
    if selector.is_empty() {
        bail!("quest selector cannot be empty");
    }
    if let Ok(quest_id) = selector.parse::<u32>() {
        return catalog
            .iter()
            .find(|quest| quest.quest_id == quest_id)
            .with_context(|| format!("quest {quest_id} has no associated script"));
    }

    let exact_script = catalog
        .iter()
        .filter(|quest| quest.scripts.iter().any(|script| script.name == selector))
        .collect::<Vec<_>>();
    if exact_script.len() == 1 {
        return Ok(exact_script[0]);
    }
    if exact_script.len() > 1 {
        return ambiguous_selector(selector, &exact_script);
    }

    let selector_lower = selector.to_lowercase();
    let exact_name = catalog
        .iter()
        .filter(|quest| quest.name.to_lowercase() == selector_lower)
        .collect::<Vec<_>>();
    if exact_name.len() == 1 {
        return Ok(exact_name[0]);
    }
    let partial_name = catalog
        .iter()
        .filter(|quest| quest.name.to_lowercase().contains(&selector_lower))
        .collect::<Vec<_>>();
    match partial_name.as_slice() {
        [quest] => Ok(*quest),
        [] => bail!("no scripted quest matches selector {selector:?}"),
        quests => ambiguous_selector(selector, quests),
    }
}

fn ambiguous_selector<'a>(
    selector: &str,
    quests: &[&'a QuestSummary],
) -> Result<&'a QuestSummary> {
    let matches = quests
        .iter()
        .take(10)
        .map(|quest| format!("{} ({})", quest.quest_id, quest.name))
        .collect::<Vec<_>>()
        .join(", ");
    bail!("quest selector {selector:?} is ambiguous; matches: {matches}")
}

fn referenced_ids(evidence: &QuestArchiveEvidence) -> (BTreeSet<u32>, BTreeSet<u32>) {
    let mut npc_ids = BTreeSet::new();
    let mut item_ids = BTreeSet::new();
    for tree in [
        Some(&evidence.check),
        Some(&evidence.act),
        evidence.say.as_ref(),
        Some(&evidence.info),
    ]
    .into_iter()
    .flatten()
    {
        collect_named_ids(tree, false, &mut npc_ids, &mut item_ids);
    }
    (npc_ids, item_ids)
}

fn collect_named_ids(
    node: &NodeTree,
    inside_item: bool,
    npc_ids: &mut BTreeSet<u32>,
    item_ids: &mut BTreeSet<u32>,
) {
    let inside_item = inside_item || node.name == "item";
    if node.name == "npc"
        && let Some(value) = unsigned_value(node)
    {
        npc_ids.insert(value);
    }
    if (inside_item && node.name == "id"
        || matches!(node.name.as_str(), "buff" | "exceptbuff" | "buffItemID"))
        && let Some(value) = unsigned_value(node)
    {
        item_ids.insert(value);
    }
    if let Some(text) = string_value(node) {
        collect_marker_ids(text, 'p', npc_ids);
        for marker in ['i', 't', 'c'] {
            collect_marker_ids(text, marker, item_ids);
        }
    }
    for child in &node.children {
        collect_named_ids(child, inside_item, npc_ids, item_ids);
    }
}

fn collect_marker_ids(
    text: &str,
    marker: char,
    output: &mut BTreeSet<u32>,
) {
    let prefix = format!("#{marker}");
    let mut remaining = text;
    while let Some(position) = remaining.find(&prefix) {
        remaining = &remaining[position + prefix.len()..];
        let digits = remaining.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0
            && let Ok(value) = remaining[..digits].parse::<u32>()
        {
            output.insert(value);
        }
        remaining = &remaining[digits..];
    }
}

fn load_npc_metadata(
    archive: Option<&Archive>,
    npc_ids: &BTreeSet<u32>,
    cache: &mut EvidenceCache,
) -> Result<Vec<NpcMetadataEvidence>> {
    let Some(archive) = archive else {
        return Ok(Vec::new());
    };
    if cache.npc_images.is_none() {
        cache.npc_images = Some(
            oozems_wz::list(archive, "/", 0, usize::MAX)?
                .nodes
                .into_iter()
                .map(|node| node.name)
                .collect(),
        );
    }
    let images = cache.npc_images.as_ref().expect("NPC image cache set");
    let mut output = Vec::new();
    for npc_id in npc_ids {
        if let Some(metadata) = cache.npc_metadata.get(npc_id) {
            output.extend(metadata.clone());
            continue;
        }
        let image = format!("{npc_id:07}.img");
        if !images.contains(&image) {
            cache.npc_metadata.insert(*npc_id, None);
            continue;
        }
        let children = oozems_wz::list(archive, &format!("/{image}"), 0, usize::MAX)?;
        if !children.nodes.iter().any(|node| node.name == "info") {
            cache.npc_metadata.insert(*npc_id, None);
            continue;
        }
        let metadata = NpcMetadataEvidence {
            npc_id: *npc_id,
            info: oozems_wz::tree(archive, &format!("/{image}/info"), MAXIMUM_BRANCH_NODES)?,
        };
        output.push(metadata.clone());
        cache.npc_metadata.insert(*npc_id, Some(metadata));
    }
    Ok(output)
}

fn load_referenced_text(
    archive: Option<&Archive>,
    npc_ids: &BTreeSet<u32>,
    item_ids: &BTreeSet<u32>,
    cache: &mut EvidenceCache,
) -> Result<StringArchiveEvidence> {
    let Some(archive) = archive else {
        return Ok(StringArchiveEvidence::default());
    };
    let npcs = if npc_ids.is_empty() {
        Vec::new()
    } else {
        let tree = cached_string_tree(archive, "Npc.img", cache)?;
        retain_referenced_text(tree, npc_ids)
    };

    let mut items = BTreeMap::new();
    for image in referenced_item_string_images(item_ids) {
        let tree = cached_string_tree(archive, image, cache)?;
        collect_referenced_text(tree, item_ids, &mut items);
    }
    Ok(StringArchiveEvidence {
        npcs,
        items: items
            .into_iter()
            .map(|(id, text)| ReferencedText { id, text })
            .collect(),
    })
}

fn cached_string_tree<'a>(
    archive: &Archive,
    image: &str,
    cache: &'a mut EvidenceCache,
) -> Result<&'a NodeTree> {
    if !cache.string_trees.contains_key(image) {
        let tree = oozems_wz::tree(archive, &format!("/{image}"), MAXIMUM_STRING_NODES)?;
        cache.string_trees.insert(image.to_owned(), tree);
    }
    Ok(cache
        .string_trees
        .get(image)
        .expect("string tree inserted into cache"))
}

fn referenced_item_string_images(item_ids: &BTreeSet<u32>) -> BTreeSet<&'static str> {
    let mut images = BTreeSet::new();
    for item_id in item_ids {
        match item_id / 1_000_000 {
            1 => {
                images.insert("Eqp.img");
            }
            2 => {
                images.insert("Consume.img");
            }
            3 => {
                images.insert("Ins.img");
            }
            4 => {
                images.insert("Etc.img");
            }
            5 => {
                images.insert("Cash.img");
                images.insert("Pet.img");
            }
            _ => {}
        }
    }
    images
}

fn retain_referenced_text(
    root: &NodeTree,
    ids: &BTreeSet<u32>,
) -> Vec<ReferencedText> {
    root.children
        .iter()
        .filter_map(|node| {
            let id = node.name.parse::<u32>().ok()?;
            ids.contains(&id).then(|| ReferencedText {
                id,
                text: node.clone(),
            })
        })
        .collect()
}

fn collect_referenced_text(
    node: &NodeTree,
    ids: &BTreeSet<u32>,
    output: &mut BTreeMap<u32, NodeTree>,
) {
    if let Ok(id) = node.name.parse::<u32>()
        && ids.contains(&id)
        && child(node, "name").and_then(string_value).is_some()
    {
        output.entry(id).or_insert_with(|| node.clone());
        return;
    }
    for child in &node.children {
        collect_referenced_text(child, ids, output);
    }
}

fn read_notes(path: &Path) -> Result<String> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("failed to inspect notes {}", path.display()))?;
    if metadata.len() > MAXIMUM_NOTES_BYTES {
        bail!("supplemental notes exceed the 1 MiB input limit");
    }
    let notes = fs::read_to_string(path)
        .with_context(|| format!("failed to read notes {}", path.display()))?;
    if notes.trim().is_empty() {
        bail!("supplemental notes cannot be empty");
    }
    Ok(notes)
}

fn required_child<'a>(
    node: &'a NodeTree,
    name: &str,
    image: &str,
) -> Result<&'a NodeTree> {
    child(node, name).with_context(|| format!("{image} has no quest {name}"))
}

fn child<'a>(
    node: &'a NodeTree,
    name: &str,
) -> Option<&'a NodeTree> {
    node.children.iter().find(|child| child.name == name)
}

fn string_value(node: &NodeTree) -> Option<&str> {
    node.value.as_ref()?.as_str()
}

fn unsigned_value(node: &NodeTree) -> Option<u32> {
    if let Some(value) = node.value.as_ref()?.as_u64() {
        return u32::try_from(value).ok();
    }
    string_value(node)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn catalog_discovers_both_script_phases_and_names() {
        let check = branch(
            "Check.img",
            vec![branch(
                "100",
                vec![
                    branch("0", vec![scalar("startscript", json!("q100s"))]),
                    branch("1", vec![scalar("endscript", json!("q100e"))]),
                ],
            )],
        );
        let info = branch(
            "QuestInfo.img",
            vec![branch(
                "100",
                vec![scalar("name", json!("A Scripted Quest"))],
            )],
        );

        let quests = build_catalog(&check, &info).expect("catalog");

        assert_eq!(quests.len(), 1);
        assert_eq!(quests[0].quest_id, 100);
        assert_eq!(quests[0].name, "A Scripted Quest");
        assert_eq!(quests[0].scripts.len(), 2);
        assert_eq!(quests[0].scripts[1].phase, QuestPhase::Completion);
        assert_eq!(quests[0].scripts[1].name, "q100e");
    }

    #[test]
    fn selector_accepts_id_script_or_unique_name_text() {
        let catalog = vec![QuestSummary {
            quest_id: 100,
            name: "A Scripted Quest".to_owned(),
            scripts: vec![ScriptReference {
                phase: QuestPhase::Start,
                name: "q100s".to_owned(),
            }],
        }];

        for selector in ["100", "q100s", "scripted"] {
            assert_eq!(
                select_quest(&catalog, selector)
                    .expect("selected quest")
                    .quest_id,
                100
            );
        }
    }

    #[test]
    fn references_include_structured_and_inline_ids() {
        let evidence = QuestArchiveEvidence {
            check: branch(
                "100",
                vec![
                    scalar("npc", json!(9000000)),
                    branch(
                        "item",
                        vec![branch("0", vec![scalar("id", json!(4_000_001))])],
                    ),
                ],
            ),
            act: branch("100", Vec::new()),
            say: Some(branch(
                "100",
                vec![scalar("0", json!("Ask #p9000001# for #t4000002#."))],
            )),
            info: branch("100", Vec::new()),
        };

        let (npcs, items) = referenced_ids(&evidence);

        assert_eq!(npcs, BTreeSet::from([9_000_000, 9_000_001]));
        assert_eq!(items, BTreeSet::from([4_000_001, 4_000_002]));
    }

    fn branch(
        name: &str,
        children: Vec<NodeTree>,
    ) -> NodeTree {
        NodeTree {
            name: name.to_owned(),
            kind: "property",
            value: None,
            details: None,
            children,
        }
    }

    fn scalar(
        name: &str,
        value: serde_json::Value,
    ) -> NodeTree {
        NodeTree {
            name: name.to_owned(),
            kind: "scalar",
            value: Some(value),
            details: None,
            children: Vec::new(),
        }
    }
}
