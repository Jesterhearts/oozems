use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::path::Path;

use thiserror::Error;
use wz_reader::WzNodeArc;

use super::wz::WzContentError;
use super::wz::node_name;
use super::wz::open_archive;
use super::wz::parse;
use super::wz::sorted_children;
use super::wz::wrap_archive_root;

mod dialogue;
mod importer;
pub(super) mod model;
mod restoration;

use model::QuestDefinition;

const QUEST_ARCHIVE: &str = "Quest.wz";

pub(crate) struct QuestContent {
    _base: WzNodeArc,
    definitions: HashMap<u32, QuestDefinition>,
    item_reference_ids: BTreeSet<u32>,
    #[cfg(test)]
    report: QuestLoadReport,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct QuestLoadReport {
    pub compatible_quests: usize,
    pub unsupported_reasons: BTreeMap<String, usize>,
    pub retained_metadata_fields: BTreeMap<String, usize>,
}

#[derive(Debug, Error)]
pub enum QuestContentError {
    #[error(transparent)]
    Wz(#[from] WzContentError),
    #[error("Quest.wz quest {quest_id} is unsupported ({category}): {message}")]
    Unsupported {
        quest_id: u32,
        category: String,
        message: String,
    },
    #[error("Quest.wz quest {quest_id} is invalid: {message}")]
    Invalid { quest_id: u32, message: String },
}

impl QuestContent {
    pub fn open_optional(
        directory: &Path,
        quest_ids: Option<&BTreeSet<u32>>,
        item_source_ids: &BTreeSet<u32>,
        equipment_source_ids: &BTreeSet<u32>,
        consume_effect_ids: &BTreeSet<u32>,
        monster_book_card_ids: &BTreeSet<u32>,
        morph_ids: &BTreeSet<u32>,
        skill_source_ids: &BTreeSet<u32>,
        skill_source_names: &BTreeMap<u32, String>,
    ) -> Result<Option<Self>, QuestContentError> {
        if quest_ids.is_some_and(BTreeSet::is_empty) {
            return Ok(None);
        }
        let path = directory.join(QUEST_ARCHIVE);
        if !path
            .try_exists()
            .map_err(|source| WzContentError::Metadata {
                path: path.clone(),
                source,
            })?
        {
            tracing::warn!(path = %path.display(), "Quest.wz is absent; quests will be unavailable");
            return Ok(None);
        }

        let root = open_archive(&path)?;
        let base = wrap_archive_root(&root)?;
        parse(&root, format!("{} root", path.display()))?;
        let check = importer::required_child(&root, "Check.img", 0)?;
        let act = importer::required_child(&root, "Act.img", 0)?;
        let say = importer::required_child(&root, "Say.img", 0)?;
        let info = importer::required_child(&root, "QuestInfo.img", 0)?;
        for (name, node) in [
            ("Check.img", &check),
            ("Act.img", &act),
            ("Say.img", &say),
            ("QuestInfo.img", &info),
        ] {
            parse(node, format!("{} {name}", path.display()))?;
        }

        let strict = quest_ids.is_some();
        let archive_quest_ids = discover_quest_ids([&check, &act, &say, &info])?;
        let quest_ids = match quest_ids {
            Some(quest_ids) => quest_ids.clone(),
            None => archive_quest_ids.clone(),
        };
        let mut item_reference_ids = BTreeSet::new();
        let mut definitions = HashMap::new();
        let mut report = QuestLoadReport::default();
        for quest_id in quest_ids {
            let Some(references) = handle_quest_load_result(
                importer::item_reference_ids(quest_id, &check, &act),
                strict,
                &mut report,
            )?
            else {
                continue;
            };
            item_reference_ids.extend(references);
            let Some(definition) = handle_quest_load_result(
                importer::load_definition(
                    quest_id,
                    &check,
                    &act,
                    &say,
                    &info,
                    item_source_ids,
                    equipment_source_ids,
                    consume_effect_ids,
                    monster_book_card_ids,
                    morph_ids,
                    skill_source_ids,
                    skill_source_names,
                    &archive_quest_ids,
                ),
                strict,
                &mut report,
            )?
            else {
                continue;
            };
            for field in &definition.info.retained_metadata_fields {
                *report
                    .retained_metadata_fields
                    .entry(field.clone())
                    .or_default() += 1;
            }
            for field in &definition.dialogue.retained_fields {
                *report
                    .retained_metadata_fields
                    .entry(format!("dialogue/{field}"))
                    .or_default() += 1;
            }
            definitions.insert(quest_id, definition);
        }
        // Act/4960 is absent, so the raw reference scan cannot see its audited alias
        // outputs.
        if let Some(quest) = definitions.get(&4_960) {
            for actions in [&quest.start_actions, &quest.completion_actions] {
                item_reference_ids.extend(actions.fixed_items.iter().map(|item| item.item_id));
                item_reference_ids
                    .extend(actions.conditional_items.iter().map(|item| item.item_id));
                item_reference_ids.extend(actions.weighted_items.iter().map(|item| item.item_id));
                item_reference_ids.extend(actions.selectable_items.iter().map(|item| item.item_id));
            }
        }
        report.compatible_quests = definitions.len();
        tracing::info!(
            path = %path.display(),
            compatible_quests = report.compatible_quests,
            unsupported_quests = report.unsupported_reasons.values().sum::<usize>(),
            unsupported_reasons = ?report.unsupported_reasons,
            retained_metadata_fields = ?report.retained_metadata_fields,
            "WZ quest source ready"
        );
        Ok(Some(Self {
            _base: base,
            definitions,
            item_reference_ids,
            #[cfg(test)]
            report,
        }))
    }

    pub fn get(
        &self,
        quest_id: u32,
    ) -> Option<&QuestDefinition> {
        self.definitions.get(&quest_id)
    }

    pub fn definitions(&self) -> impl Iterator<Item = &QuestDefinition> {
        self.definitions.values()
    }

    pub fn item_reference_ids(&self) -> &BTreeSet<u32> {
        &self.item_reference_ids
    }

    #[cfg(test)]
    pub fn report(&self) -> &QuestLoadReport {
        &self.report
    }
}

impl QuestContentError {
    fn reason_category(&self) -> &str {
        match self {
            Self::Unsupported { category, .. } => category,
            Self::Invalid { .. } => "invalid quest data",
            Self::Wz(_) => "WZ read failure",
        }
    }
}

fn handle_quest_load_result<T>(
    result: Result<T, QuestContentError>,
    strict: bool,
    report: &mut QuestLoadReport,
) -> Result<Option<T>, QuestContentError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error @ QuestContentError::Unsupported { .. })
        | Err(error @ QuestContentError::Invalid { .. })
            if !strict =>
        {
            *report
                .unsupported_reasons
                .entry(error.reason_category().to_owned())
                .or_default() += 1;
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn discover_quest_ids<'a>(
    roots: impl IntoIterator<Item = &'a WzNodeArc>
) -> Result<BTreeSet<u32>, QuestContentError> {
    let mut quest_ids = BTreeSet::new();
    for root in roots {
        for node in sorted_children(root)? {
            let name = node_name(&node)?;
            if let Ok(quest_id) = name.parse::<u32>() {
                quest_ids.insert(quest_id);
            }
        }
    }
    Ok(quest_ids)
}

pub(super) fn invalid(
    quest_id: u32,
    message: impl Into<String>,
) -> QuestContentError {
    QuestContentError::Invalid {
        quest_id,
        message: message.into(),
    }
}

pub(super) fn unsupported(
    quest_id: u32,
    category: impl Into<String>,
    message: impl Into<String>,
) -> QuestContentError {
    QuestContentError::Unsupported {
        quest_id,
        category: category.into(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::QuestContentError;
    use super::QuestLoadReport;
    use super::handle_quest_load_result;
    use super::invalid;

    #[test]
    fn reference_scan_errors_are_best_effort_only_without_an_allowlist() {
        let mut report = QuestLoadReport::default();
        let skipped = handle_quest_load_result::<BTreeSet<u32>>(
            Err(invalid(100, "malformed item reference")),
            false,
            &mut report,
        )
        .expect("non-strict loading should skip one malformed quest");

        assert!(skipped.is_none());
        assert_eq!(
            report.unsupported_reasons.get("invalid quest data"),
            Some(&1)
        );

        let error = handle_quest_load_result::<BTreeSet<u32>>(
            Err(invalid(100, "malformed item reference")),
            true,
            &mut QuestLoadReport::default(),
        )
        .expect_err("strict loading should return the reference scan error");
        assert!(matches!(
            error,
            QuestContentError::Invalid { quest_id: 100, .. }
        ));
    }
}
