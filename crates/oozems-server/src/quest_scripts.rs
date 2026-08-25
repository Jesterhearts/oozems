use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::collections::HashMap;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;

use oozems_proto::v1::ItemDefinition;
use oozems_proto::v1::PlayerState;
use oozems_proto::v1::QuestStatus;
use serde::Deserialize;
use thiserror::Error;

use crate::content::QuestDefinition;
use crate::content::QuestItemDelta;
use crate::content::QuestRecordWrite;
use crate::content::QuestStateAction;
use crate::content::QuestStateActionState;
use crate::items::ItemDefinitionLookup;

const MAXIMUM_PROGRAMS: usize = 1_024;
const MAXIMUM_PROGRAM_OPERATIONS: usize = 64;
const MAXIMUM_PAGES_PER_BRANCH: usize = 16;
const MAXIMUM_PAGE_BYTES: usize = 4_096;
const MAXIMUM_SCRIPT_NAME_BYTES: usize = 256;

#[derive(Clone, Debug, Default)]
pub struct QuestScriptCatalog {
    programs: HashMap<String, QuestScriptProgram>,
    item_reference_ids: BTreeSet<u32>,
    ignored_programs: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuestScriptPhase {
    Start,
    Completion,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuestScriptPlan {
    pub script: String,
    pub item_deltas: Vec<QuestItemDelta>,
    pub mesos: i64,
    pub experience: u64,
    pub fame: i32,
    pub quest_state_actions: Vec<QuestStateAction>,
    pub record_writes: Vec<QuestRecordWrite>,
    pub result_pages: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuestScriptResolution {
    NotReferenced,
    Missing {
        script: String,
    },
    ConditionsNotMet {
        script: String,
        incomplete_pages: Vec<String>,
    },
    Ready(QuestScriptPlan),
}

#[derive(Debug, Error)]
pub enum QuestScriptConfigError {
    #[error("failed to read quest script configuration {path}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse quest script configuration {path}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("quest script configuration {path} is invalid: {message}")]
    Invalid { path: PathBuf, message: String },
}

#[derive(Clone, Debug)]
struct QuestScriptProgram {
    conditions: Vec<QuestScriptCondition>,
    plan: QuestScriptPlan,
    incomplete_pages: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum QuestScriptCondition {
    MinimumLevel {
        level: u32,
    },
    MaximumLevel {
        level: u32,
    },
    JobIds {
        ids: Vec<u32>,
    },
    MapId {
        map_id: u32,
    },
    MesosAtLeast {
        amount: u64,
    },
    MesosAtMost {
        amount: u64,
    },
    ItemQuantity {
        item_id: u32,
        quantity: u64,
    },
    QuestState {
        quest_id: u32,
        state: QuestScriptQuestState,
    },
    QuestRecordEquals {
        quest_id: u32,
        index: u32,
        value: String,
    },
    QuestRecordAtLeast {
        quest_id: u32,
        index: u32,
        value: String,
    },
    QuestRecordAtMost {
        quest_id: u32,
        index: u32,
        value: String,
    },
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum QuestScriptQuestState {
    NotStarted,
    Started,
    Completed,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum QuestScriptAction {
    ItemDelta {
        item_id: u32,
        delta: i64,
    },
    Mesos {
        delta: i64,
    },
    Experience {
        amount: u64,
    },
    Fame {
        delta: i32,
    },
    SetRecord {
        quest_id: u32,
        index: u32,
        value: String,
    },
    SetQuestStatus {
        quest_id: u32,
        state: QuestScriptQuestState,
    },
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct QuestScriptFile {
    scripts: Vec<QuestScriptFileProgram>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct QuestScriptFileProgram {
    name: String,
    #[serde(default)]
    conditions: Vec<QuestScriptCondition>,
    #[serde(default)]
    actions: Vec<QuestScriptAction>,
    #[serde(default)]
    result_pages: Vec<String>,
    #[serde(default)]
    incomplete_pages: Vec<String>,
}

impl QuestScriptCatalog {
    pub fn load<'a>(
        path: &Path,
        quest_definitions: impl IntoIterator<Item = &'a QuestDefinition>,
        archive_script_references: &BTreeSet<String>,
        item_definitions: &(impl ItemDefinitionLookup + ?Sized),
    ) -> Result<Self, QuestScriptConfigError> {
        let source = match fs::read_to_string(path) {
            Ok(source) => source,
            Err(source) if source.kind() == ErrorKind::NotFound => String::new(),
            Err(source) => {
                return Err(QuestScriptConfigError::Read {
                    path: path.to_owned(),
                    source,
                });
            }
        };
        let file = toml::from_str::<QuestScriptFile>(&source).map_err(|source| {
            QuestScriptConfigError::Parse {
                path: path.to_owned(),
                source,
            }
        })?;
        let quest_definitions = quest_definitions.into_iter().collect::<Vec<_>>();
        build_catalog(
            path,
            file,
            &quest_definitions,
            archive_script_references,
            item_definitions,
        )
    }

    pub fn len(&self) -> usize {
        self.programs.len()
    }

    pub fn item_reference_ids(&self) -> &BTreeSet<u32> {
        &self.item_reference_ids
    }

    pub fn ignored_len(&self) -> usize {
        self.ignored_programs
    }
}

pub fn resolve(
    catalog: &QuestScriptCatalog,
    quest: &QuestDefinition,
    phase: QuestScriptPhase,
    player: &PlayerState,
    item_definitions: &[ItemDefinition],
) -> QuestScriptResolution {
    let Some(script) = referenced_script(quest, phase) else {
        return QuestScriptResolution::NotReferenced;
    };
    let Some(program) = catalog.programs.get(script) else {
        return QuestScriptResolution::Missing {
            script: script.to_owned(),
        };
    };
    if !program
        .conditions
        .iter()
        .all(|condition| condition_matches(condition, player, item_definitions))
    {
        return QuestScriptResolution::ConditionsNotMet {
            script: script.to_owned(),
            incomplete_pages: program.incomplete_pages.clone(),
        };
    }
    QuestScriptResolution::Ready(program.plan.clone())
}

fn build_catalog(
    path: &Path,
    file: QuestScriptFile,
    quest_definitions: &[&QuestDefinition],
    archive_script_references: &BTreeSet<String>,
    item_definitions: &(impl ItemDefinitionLookup + ?Sized),
) -> Result<QuestScriptCatalog, QuestScriptConfigError> {
    if file.scripts.len() > MAXIMUM_PROGRAMS {
        return invalid(
            path,
            format!("more than {MAXIMUM_PROGRAMS} script programs are configured"),
        );
    }
    let references = referenced_programs(quest_definitions);
    let known_quest_ids = quest_definitions
        .iter()
        .map(|quest| quest.id)
        .collect::<BTreeSet<_>>();
    let mut item_reference_ids = BTreeSet::new();
    let mut configured_names = BTreeSet::new();
    let mut programs = HashMap::new();
    let mut ignored_programs = 0;
    for program in file.scripts {
        validate_program_shape(path, &program)?;
        if !configured_names.insert(program.name.clone()) {
            return invalid(
                path,
                format!("script name {:?} is duplicated", program.name),
            );
        }
        let Some(uses) = references.get(&program.name) else {
            if archive_script_references.contains(&program.name) {
                ignored_programs += 1;
                continue;
            }
            return invalid(
                path,
                format!("script {:?} is not referenced by Quest.wz", program.name),
            );
        };
        item_reference_ids.extend(program_item_reference_ids(&program));
        validate_conditions(path, &program.name, &program.conditions, item_definitions)?;
        let plan = build_plan(
            path,
            &program.name,
            &program.actions,
            item_definitions,
            &known_quest_ids,
            program.result_pages,
        )?;
        for (quest, phase) in uses {
            validate_merged_actions(path, &program.name, quest, *phase, &plan)?;
        }
        let name = program.name;
        let script_program = QuestScriptProgram {
            conditions: program.conditions,
            plan,
            incomplete_pages: program.incomplete_pages,
        };
        programs.insert(name, script_program);
    }
    Ok(QuestScriptCatalog {
        programs,
        item_reference_ids,
        ignored_programs,
    })
}

#[cfg(test)]
fn collect_item_reference_ids(file: &QuestScriptFile) -> BTreeSet<u32> {
    file.scripts
        .iter()
        .flat_map(program_item_reference_ids)
        .collect()
}

fn program_item_reference_ids(program: &QuestScriptFileProgram) -> impl Iterator<Item = u32> + '_ {
    let condition_ids = program.conditions.iter().filter_map(|condition| {
        if let QuestScriptCondition::ItemQuantity { item_id, .. } = condition {
            Some(*item_id)
        } else {
            None
        }
    });
    let action_ids = program.actions.iter().filter_map(|action| {
        if let QuestScriptAction::ItemDelta { item_id, .. } = action {
            Some(*item_id)
        } else {
            None
        }
    });
    condition_ids.chain(action_ids)
}

fn validate_program_shape(
    path: &Path,
    program: &QuestScriptFileProgram,
) -> Result<(), QuestScriptConfigError> {
    if program.name.trim().is_empty() {
        return invalid(path, "script names cannot be empty");
    }
    if program.name.trim() != program.name {
        return invalid(
            path,
            format!("script name {:?} has surrounding whitespace", program.name),
        );
    }
    if program.name.len() > MAXIMUM_SCRIPT_NAME_BYTES {
        return invalid(
            path,
            format!(
                "script name {:?} exceeds {MAXIMUM_SCRIPT_NAME_BYTES} bytes",
                program.name
            ),
        );
    }
    let operations = program
        .conditions
        .len()
        .checked_add(program.actions.len())
        .and_then(|count| count.checked_add(program.result_pages.len()))
        .and_then(|count| count.checked_add(program.incomplete_pages.len()))
        .unwrap_or(usize::MAX);
    if operations > MAXIMUM_PROGRAM_OPERATIONS {
        return invalid(
            path,
            format!(
                "script {:?} exceeds {MAXIMUM_PROGRAM_OPERATIONS} total conditions, actions, and \
                 pages",
                program.name
            ),
        );
    }
    validate_pages(path, &program.name, "result", &program.result_pages)?;
    validate_pages(path, &program.name, "incomplete", &program.incomplete_pages)
}

fn validate_pages(
    path: &Path,
    script: &str,
    branch: &str,
    pages: &[String],
) -> Result<(), QuestScriptConfigError> {
    if pages.len() > MAXIMUM_PAGES_PER_BRANCH {
        return invalid(
            path,
            format!("script {script:?} has more than {MAXIMUM_PAGES_PER_BRANCH} {branch} pages"),
        );
    }
    if pages.iter().any(|page| page.trim().is_empty()) {
        return invalid(
            path,
            format!("script {script:?} has an empty {branch} page"),
        );
    }
    if pages.iter().any(|page| page.len() > MAXIMUM_PAGE_BYTES) {
        return invalid(
            path,
            format!("script {script:?} has a {branch} page exceeding {MAXIMUM_PAGE_BYTES} bytes"),
        );
    }
    Ok(())
}

fn validate_conditions(
    path: &Path,
    script: &str,
    conditions: &[QuestScriptCondition],
    item_definitions: &(impl ItemDefinitionLookup + ?Sized),
) -> Result<(), QuestScriptConfigError> {
    let mut minimum_level = None;
    let mut maximum_level = None;
    let mut minimum_mesos = None;
    let mut maximum_mesos = None;
    let mut allowed_jobs: Option<BTreeSet<u32>> = None;
    let mut map_id = None;
    let mut quest_states = BTreeMap::new();
    let mut record_limits = BTreeMap::<(u32, u32), RecordLimits>::new();
    for condition in conditions {
        match condition {
            QuestScriptCondition::MinimumLevel { level } => {
                if *level == 0 {
                    return invalid(path, format!("script {script:?} has a zero level limit"));
                }
                minimum_level = Some(minimum_level.map_or(*level, |value: u32| value.max(*level)));
            }
            QuestScriptCondition::MaximumLevel { level } => {
                if *level == 0 {
                    return invalid(path, format!("script {script:?} has a zero level limit"));
                }
                maximum_level = Some(maximum_level.map_or(*level, |value: u32| value.min(*level)));
            }
            QuestScriptCondition::JobIds { ids } => {
                if ids.is_empty() {
                    return invalid(path, format!("script {script:?} has an empty job ID list"));
                }
                let jobs = ids.iter().copied().collect::<BTreeSet<_>>();
                if jobs.len() != ids.len() {
                    return invalid(path, format!("script {script:?} has duplicate job IDs"));
                }
                allowed_jobs = Some(match allowed_jobs {
                    Some(existing) => existing.intersection(&jobs).copied().collect(),
                    None => jobs,
                });
            }
            QuestScriptCondition::MapId {
                map_id: required_map,
            } => {
                if map_id.is_some_and(|existing| existing != *required_map) {
                    return invalid(
                        path,
                        format!("script {script:?} requires different map IDs"),
                    );
                }
                map_id = Some(*required_map);
            }
            QuestScriptCondition::MesosAtLeast { amount } => {
                minimum_mesos =
                    Some(minimum_mesos.map_or(*amount, |value: u64| value.max(*amount)));
            }
            QuestScriptCondition::MesosAtMost { amount } => {
                maximum_mesos =
                    Some(maximum_mesos.map_or(*amount, |value: u64| value.min(*amount)));
            }
            QuestScriptCondition::ItemQuantity { item_id, quantity } => {
                validate_item(path, script, *item_id, item_definitions)?;
                if *quantity == 0 {
                    return invalid(path, format!("script {script:?} has a zero item quantity"));
                }
            }
            QuestScriptCondition::QuestState { quest_id, state } => {
                crate::quest_records::validate_quest_id(*quest_id).map_err(|error| {
                    QuestScriptConfigError::Invalid {
                        path: path.to_owned(),
                        message: format!("script {script:?} has an invalid quest state: {error}"),
                    }
                })?;
                if quest_states
                    .insert(*quest_id, *state)
                    .is_some_and(|existing| existing != *state)
                {
                    return invalid(
                        path,
                        format!(
                            "script {script:?} requires conflicting states for quest {quest_id}"
                        ),
                    );
                }
            }
            QuestScriptCondition::QuestRecordEquals {
                quest_id,
                index,
                value,
            } => {
                validate_record_value(path, script, *quest_id, value)?;
                let limits = record_limits.entry((*quest_id, *index)).or_default();
                if limits
                    .equal
                    .replace(value.clone())
                    .is_some_and(|existing| existing != *value)
                {
                    return invalid(
                        path,
                        format!(
                            "script {script:?} requires conflicting values for quest record \
                             {quest_id}[{index}]"
                        ),
                    );
                }
            }
            QuestScriptCondition::QuestRecordAtLeast {
                quest_id,
                index,
                value,
            } => {
                let value = validate_numeric_record_value(path, script, *quest_id, value)?;
                let limits = record_limits.entry((*quest_id, *index)).or_default();
                limits.minimum = Some(limits.minimum.map_or(value, |current| current.max(value)));
            }
            QuestScriptCondition::QuestRecordAtMost {
                quest_id,
                index,
                value,
            } => {
                let value = validate_numeric_record_value(path, script, *quest_id, value)?;
                let limits = record_limits.entry((*quest_id, *index)).or_default();
                limits.maximum = Some(limits.maximum.map_or(value, |current| current.min(value)));
            }
        }
    }
    if minimum_level
        .zip(maximum_level)
        .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        return invalid(path, format!("script {script:?} has invalid level limits"));
    }
    if minimum_mesos
        .zip(maximum_mesos)
        .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        return invalid(path, format!("script {script:?} has invalid mesos limits"));
    }
    if allowed_jobs.is_some_and(|jobs| jobs.is_empty()) {
        return invalid(
            path,
            format!("script {script:?} has mutually exclusive job ID conditions"),
        );
    }
    for ((quest_id, index), limits) in record_limits {
        if limits
            .minimum
            .zip(limits.maximum)
            .is_some_and(|(minimum, maximum)| minimum > maximum)
        {
            return invalid(
                path,
                format!(
                    "script {script:?} has incompatible numeric predicates for quest record \
                     {quest_id}[{index}]"
                ),
            );
        }
        if let Some(equal) = limits.equal {
            let numeric = crate::quest_records::strict_decimal(&equal);
            if (limits.minimum.is_some() || limits.maximum.is_some())
                && numeric.is_none_or(|value| {
                    limits.minimum.is_some_and(|minimum| value < minimum)
                        || limits.maximum.is_some_and(|maximum| value > maximum)
                })
            {
                return invalid(
                    path,
                    format!(
                        "script {script:?} has incompatible predicates for quest record \
                         {quest_id}[{index}]"
                    ),
                );
            }
        }
    }
    Ok(())
}

#[derive(Default)]
struct RecordLimits {
    equal: Option<String>,
    minimum: Option<u64>,
    maximum: Option<u64>,
}

fn validate_record_value(
    path: &Path,
    script: &str,
    quest_id: u32,
    value: &str,
) -> Result<(), QuestScriptConfigError> {
    crate::quest_records::validate_quest_id(quest_id)
        .and_then(|()| crate::quest_records::validate_value(value))
        .map_err(|error| QuestScriptConfigError::Invalid {
            path: path.to_owned(),
            message: format!("script {script:?} has an invalid quest record value: {error}"),
        })
}

fn validate_numeric_record_value(
    path: &Path,
    script: &str,
    quest_id: u32,
    value: &str,
) -> Result<u64, QuestScriptConfigError> {
    validate_record_value(path, script, quest_id, value)?;
    crate::quest_records::strict_decimal(value).ok_or_else(|| QuestScriptConfigError::Invalid {
        path: path.to_owned(),
        message: format!("script {script:?} quest record predicate must be strictly decimal"),
    })
}

fn build_plan(
    path: &Path,
    script: &str,
    actions: &[QuestScriptAction],
    item_definitions: &(impl ItemDefinitionLookup + ?Sized),
    known_quest_ids: &BTreeSet<u32>,
    result_pages: Vec<String>,
) -> Result<QuestScriptPlan, QuestScriptConfigError> {
    let mut plan = QuestScriptPlan {
        script: script.to_owned(),
        item_deltas: Vec::new(),
        mesos: 0,
        experience: 0,
        fame: 0,
        quest_state_actions: Vec::new(),
        record_writes: Vec::new(),
        result_pages,
    };
    let mut quest_state_targets = BTreeSet::new();
    let mut record_writes = BTreeMap::new();
    for action in actions {
        match action {
            QuestScriptAction::ItemDelta { item_id, delta } => {
                validate_item(path, script, *item_id, item_definitions)?;
                if *delta == 0 {
                    return invalid(path, format!("script {script:?} has a zero item delta"));
                }
                if *delta == i64::MIN {
                    return invalid(
                        path,
                        format!("script {script:?} has an item delta that cannot be applied"),
                    );
                }
                plan.item_deltas.push(QuestItemDelta {
                    item_id: *item_id,
                    count: *delta,
                    expiration: None,
                });
            }
            QuestScriptAction::Mesos { delta } => {
                if *delta == 0 {
                    return invalid(path, format!("script {script:?} has a zero mesos delta"));
                }
                if *delta == i64::MIN {
                    return invalid(
                        path,
                        format!("script {script:?} has a mesos delta that cannot be applied"),
                    );
                }
                plan.mesos = plan.mesos.checked_add(*delta).ok_or_else(|| {
                    QuestScriptConfigError::Invalid {
                        path: path.to_owned(),
                        message: format!("script {script:?} mesos actions overflow"),
                    }
                })?;
            }
            QuestScriptAction::Experience { amount } => {
                if *amount == 0 {
                    return invalid(path, format!("script {script:?} has a zero EXP grant"));
                }
                plan.experience = plan.experience.checked_add(*amount).ok_or_else(|| {
                    QuestScriptConfigError::Invalid {
                        path: path.to_owned(),
                        message: format!("script {script:?} EXP actions overflow"),
                    }
                })?;
            }
            QuestScriptAction::Fame { delta } => {
                if *delta == 0 {
                    return invalid(path, format!("script {script:?} has a zero fame delta"));
                }
                plan.fame = plan.fame.checked_add(*delta).ok_or_else(|| {
                    QuestScriptConfigError::Invalid {
                        path: path.to_owned(),
                        message: format!("script {script:?} fame actions overflow"),
                    }
                })?;
            }
            QuestScriptAction::SetRecord {
                quest_id,
                index,
                value,
            } => {
                validate_record_value(path, script, *quest_id, value)?;
                if record_writes
                    .insert((*quest_id, *index), value.clone())
                    .is_some()
                {
                    return invalid(
                        path,
                        format!(
                            "script {script:?} writes quest record {quest_id}[{index}] more than \
                             once"
                        ),
                    );
                }
            }
            QuestScriptAction::SetQuestStatus { quest_id, state } => {
                if *quest_id == 0 {
                    return invalid(
                        path,
                        format!("script {script:?} has a zero quest status target"),
                    );
                }
                if !known_quest_ids.contains(quest_id) {
                    return invalid(
                        path,
                        format!(
                            "script {script:?} quest status target {quest_id} is not a loaded \
                             quest definition"
                        ),
                    );
                }
                if !quest_state_targets.insert(*quest_id) {
                    return invalid(
                        path,
                        format!("script {script:?} sets quest {quest_id} status more than once"),
                    );
                }
                plan.quest_state_actions.push(QuestStateAction {
                    quest_id: *quest_id,
                    state: match state {
                        QuestScriptQuestState::NotStarted => QuestStateActionState::NotStarted,
                        QuestScriptQuestState::Started => QuestStateActionState::Started,
                        QuestScriptQuestState::Completed => QuestStateActionState::Completed,
                    },
                });
            }
        }
    }
    plan.record_writes = record_writes
        .into_iter()
        .map(|((quest_id, index), value)| QuestRecordWrite {
            quest_id,
            index,
            value,
        })
        .collect();
    validate_item_arithmetic(path, script, &plan.item_deltas, &[])?;
    Ok(plan)
}

fn validate_merged_actions(
    path: &Path,
    script: &str,
    quest: &QuestDefinition,
    phase: QuestScriptPhase,
    plan: &QuestScriptPlan,
) -> Result<(), QuestScriptConfigError> {
    let actions = match phase {
        QuestScriptPhase::Start => &quest.start_actions,
        QuestScriptPhase::Completion => &quest.completion_actions,
    };
    if actions.money.checked_add(plan.mesos).is_none()
        || actions.experience.checked_add(plan.experience).is_none()
        || actions.fame.checked_add(plan.fame).is_none()
    {
        return invalid(
            path,
            format!(
                "script {script:?} actions overflow quest {} {phase:?} actions",
                quest.id
            ),
        );
    }
    validate_item_arithmetic(path, script, &actions.fixed_items, &plan.item_deltas)?;
    if let Some(write) = actions.record_writes.iter().find(|wz| {
        plan.record_writes
            .iter()
            .any(|script| script.quest_id == wz.quest_id && script.index == wz.index)
    }) {
        return invalid(
            path,
            format!(
                "script {script:?} and quest {} both write quest record {}[{}]",
                quest.id, write.quest_id, write.index
            ),
        );
    }
    if plan
        .quest_state_actions
        .iter()
        .any(|action| action.quest_id == quest.id)
    {
        return invalid(
            path,
            format!(
                "script {script:?} cannot set the status of its transitioning quest {}",
                quest.id
            ),
        );
    }
    if let Some(action) = actions.quest_state_actions.iter().find(|wz| {
        plan.quest_state_actions
            .iter()
            .any(|script| script.quest_id == wz.quest_id)
    }) {
        return invalid(
            path,
            format!(
                "script {script:?} and quest {} both set quest {} status",
                quest.id, action.quest_id
            ),
        );
    }
    for weighted in &actions.weighted_items {
        let weighted = [QuestItemDelta {
            item_id: weighted.item_id,
            count: i64::from(weighted.count),
            expiration: weighted.expiration,
        }];
        let mut fixed = actions.fixed_items.clone();
        fixed.extend_from_slice(&weighted);
        validate_item_arithmetic(path, script, &fixed, &plan.item_deltas)?;
    }
    for selectable in &actions.selectable_items {
        let selectable = [QuestItemDelta {
            item_id: selectable.item_id,
            count: i64::from(selectable.count),
            expiration: selectable.expiration,
        }];
        let mut fixed = actions.fixed_items.clone();
        fixed.extend_from_slice(&selectable);
        validate_item_arithmetic(path, script, &fixed, &plan.item_deltas)?;
    }
    Ok(())
}

fn validate_item_arithmetic(
    path: &Path,
    script: &str,
    first: &[QuestItemDelta],
    second: &[QuestItemDelta],
) -> Result<(), QuestScriptConfigError> {
    let mut removals = BTreeMap::<u32, u64>::new();
    let mut grants = BTreeMap::<u32, u64>::new();
    for delta in first.iter().chain(second) {
        let quantities = if delta.count < 0 {
            &mut removals
        } else {
            &mut grants
        };
        let quantity = quantities.entry(delta.item_id).or_default();
        *quantity = quantity
            .checked_add(delta.count.unsigned_abs())
            .filter(|quantity| *quantity <= i64::MAX as u64)
            .ok_or_else(|| QuestScriptConfigError::Invalid {
                path: path.to_owned(),
                message: format!(
                    "script {script:?} item actions for {} cannot be represented",
                    delta.item_id
                ),
            })?;
    }
    Ok(())
}

fn validate_item(
    path: &Path,
    script: &str,
    item_id: u32,
    item_definitions: &(impl ItemDefinitionLookup + ?Sized),
) -> Result<(), QuestScriptConfigError> {
    match item_definitions.item_definition(item_id) {
        Ok(Some(_)) => Ok(()),
        Ok(None) => invalid(
            path,
            format!("script {script:?} item {item_id} is not in the item catalog"),
        ),
        Err(error) => invalid(
            path,
            format!("script {script:?} item {item_id} metadata could not be loaded: {error}"),
        ),
    }
}

fn referenced_programs<'a>(
    quest_definitions: &[&'a QuestDefinition]
) -> HashMap<String, Vec<(&'a QuestDefinition, QuestScriptPhase)>> {
    let mut references = HashMap::<String, Vec<_>>::new();
    for quest in quest_definitions {
        for phase in [QuestScriptPhase::Start, QuestScriptPhase::Completion] {
            if let Some(script) = referenced_script(quest, phase) {
                references
                    .entry(script.to_owned())
                    .or_default()
                    .push((*quest, phase));
            }
        }
    }
    references
}

fn referenced_script(
    quest: &QuestDefinition,
    phase: QuestScriptPhase,
) -> Option<&str> {
    match phase {
        QuestScriptPhase::Start => quest.start.script.as_deref(),
        QuestScriptPhase::Completion => quest.completion.script.as_deref(),
    }
}

fn condition_matches(
    condition: &QuestScriptCondition,
    player: &PlayerState,
    item_definitions: &[ItemDefinition],
) -> bool {
    match condition {
        QuestScriptCondition::MinimumLevel { level } => player.level >= *level,
        QuestScriptCondition::MaximumLevel { level } => player.level <= *level,
        QuestScriptCondition::JobIds { ids } => {
            ids.contains(&player.stats.as_ref().map_or(0, |stats| stats.job_id))
        }
        QuestScriptCondition::MapId { map_id } => player.map_id == *map_id,
        QuestScriptCondition::MesosAtLeast { amount } => player.mesos >= *amount,
        QuestScriptCondition::MesosAtMost { amount } => player.mesos <= *amount,
        QuestScriptCondition::ItemQuantity { item_id, quantity } => {
            player.inventory.as_ref().is_some_and(|inventory| {
                crate::items::count_inventory_item(inventory, item_definitions, *item_id)
                    .is_ok_and(|current| current >= *quantity)
            })
        }
        QuestScriptCondition::QuestState { quest_id, state } => {
            player_quest_state(player, *quest_id) == *state
        }
        QuestScriptCondition::QuestRecordEquals {
            quest_id,
            index,
            value,
        } => crate::quest_records::get(player, *quest_id, *index) == Some(value),
        QuestScriptCondition::QuestRecordAtLeast {
            quest_id,
            index,
            value,
        } => record_numeric_matches(player, *quest_id, *index, value, u64::ge),
        QuestScriptCondition::QuestRecordAtMost {
            quest_id,
            index,
            value,
        } => record_numeric_matches(player, *quest_id, *index, value, u64::le),
    }
}

fn record_numeric_matches(
    player: &PlayerState,
    quest_id: u32,
    index: u32,
    expected: &str,
    compare: impl FnOnce(&u64, &u64) -> bool,
) -> bool {
    crate::quest_records::get(player, quest_id, index)
        .and_then(crate::quest_records::strict_decimal)
        .zip(crate::quest_records::strict_decimal(expected))
        .is_some_and(|(current, expected)| compare(&current, &expected))
}

fn player_quest_state(
    player: &PlayerState,
    quest_id: u32,
) -> QuestScriptQuestState {
    player
        .quests
        .iter()
        .find(|quest| quest.quest_id == quest_id)
        .and_then(|quest| QuestStatus::try_from(quest.status).ok())
        .map_or(QuestScriptQuestState::NotStarted, |status| match status {
            QuestStatus::Started => QuestScriptQuestState::Started,
            QuestStatus::Completed => QuestScriptQuestState::Completed,
            QuestStatus::Unspecified => QuestScriptQuestState::NotStarted,
        })
}

fn invalid<T>(
    path: &Path,
    message: impl Into<String>,
) -> Result<T, QuestScriptConfigError> {
    Err(QuestScriptConfigError::Invalid {
        path: path.to_owned(),
        message: message.into(),
    })
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::BTreeSet;
    use std::fs;

    use oozems_proto::v1::CharacterStats;
    use oozems_proto::v1::InventoryState;
    use oozems_proto::v1::ItemDefinition;
    use oozems_proto::v1::PlayerState;

    use super::QuestScriptAction;
    use super::QuestScriptCatalog;
    use super::QuestScriptFile;
    use super::QuestScriptPhase;
    use super::QuestScriptResolution;
    use super::build_catalog;
    use super::collect_item_reference_ids;
    use super::resolve;
    use crate::content::QuestActions;
    use crate::content::QuestCompletionRequirements;
    use crate::content::QuestDefinition;
    use crate::content::QuestDialogue;
    use crate::content::QuestInfo;
    use crate::content::QuestStartRequirements;
    use crate::content::QuestStateAction;
    use crate::content::QuestStateActionState;
    use crate::items::ItemDefinitionLookup;
    use crate::items::ItemRuleError;

    const ITEM_ID: u32 = 4_000_000;

    struct LazyItemDefinitions {
        definition: ItemDefinition,
        lookups: Cell<usize>,
    }

    impl ItemDefinitionLookup for LazyItemDefinitions {
        fn item_definition(
            &self,
            item_id: u32,
        ) -> Result<Option<&ItemDefinition>, ItemRuleError> {
            self.lookups.set(self.lookups.get() + 1);
            Ok((self.definition.item_id == item_id).then_some(&self.definition))
        }
    }

    #[test]
    fn missing_referenced_script_is_unresolved() {
        let quest = scripted_quest("missing");
        let resolution = resolve(
            &QuestScriptCatalog::default(),
            &quest,
            QuestScriptPhase::Start,
            &player(),
            &item_definitions(),
        );

        assert_eq!(
            resolution,
            QuestScriptResolution::Missing {
                script: "missing".to_owned()
            }
        );
    }

    #[test]
    fn conditions_gate_a_typed_action_plan() {
        let quest = scripted_quest("starter_check");
        let catalog = load(
            r#"
                [[scripts]]
                name = "starter_check"
                result_pages = ["The script result is ready."]
                incomplete_pages = ["Return at level 5."]

                [[scripts.conditions]]
                type = "minimum_level"
                level = 5

                [[scripts.actions]]
                type = "item_delta"
                item_id = 4000000
                delta = 2

                [[scripts.actions]]
                type = "mesos"
                delta = -50

                [[scripts.actions]]
                type = "experience"
                amount = 10

                [[scripts.actions]]
                type = "fame"
                delta = 1
            "#,
            &[&quest],
            &item_definitions(),
        )
        .expect("valid catalog");

        let incomplete = resolve(
            &catalog,
            &quest,
            QuestScriptPhase::Start,
            &player(),
            &item_definitions(),
        );
        assert_eq!(
            incomplete,
            QuestScriptResolution::ConditionsNotMet {
                script: "starter_check".to_owned(),
                incomplete_pages: vec!["Return at level 5.".to_owned()],
            }
        );

        let mut eligible = player();
        eligible.level = 5;
        let QuestScriptResolution::Ready(plan) = resolve(
            &catalog,
            &quest,
            QuestScriptPhase::Start,
            &eligible,
            &item_definitions(),
        ) else {
            panic!("eligible player should produce a plan");
        };
        assert_eq!(plan.item_deltas[0].item_id, ITEM_ID);
        assert_eq!(plan.item_deltas[0].count, 2);
        assert_eq!(plan.mesos, -50);
        assert_eq!(plan.experience, 10);
        assert_eq!(plan.fame, 1);
        assert_eq!(plan.result_pages, vec!["The script result is ready."]);
    }

    #[test]
    fn duplicate_names_and_unknown_items_are_rejected() {
        let quest = scripted_quest("duplicate");
        let duplicate = load(
            r#"
                [[scripts]]
                name = "duplicate"

                [[scripts]]
                name = "duplicate"
            "#,
            &[&quest],
            &item_definitions(),
        )
        .expect_err("duplicate names must fail");
        assert!(duplicate.to_string().contains("duplicated"));

        let unknown = load(
            r#"
                [[scripts]]
                name = "duplicate"

                [[scripts.actions]]
                type = "item_delta"
                item_id = 9999999
                delta = 1
            "#,
            &[&quest],
            &item_definitions(),
        )
        .expect_err("unknown item must fail");
        assert!(unknown.to_string().contains("not in the item catalog"));
    }

    #[test]
    fn record_conditions_read_helper_records_and_actions_are_typed() {
        let quest = scripted_quest("record_script");
        let catalog = load(
            r#"
                [[scripts]]
                name = "record_script"

                [[scripts.conditions]]
                type = "quest_record_equals"
                quest_id = 900
                index = 7
                value = "007"

                [[scripts.conditions]]
                type = "quest_record_at_least"
                quest_id = 901
                index = 0
                value = "10"

                [[scripts.conditions]]
                type = "quest_record_at_most"
                quest_id = 901
                index = 0
                value = "20"

                [[scripts.actions]]
                type = "set_record"
                quest_id = 902
                index = 42
                value = "Result"
            "#,
            &[&quest],
            &item_definitions(),
        )
        .expect("record script catalog");
        let mut eligible = player();
        crate::quest_records::set(&mut eligible, 900, 7, "007".to_owned())
            .expect("exact helper record");
        crate::quest_records::set(&mut eligible, 901, 0, "0015".to_owned())
            .expect("numeric helper record");

        let QuestScriptResolution::Ready(plan) = resolve(
            &catalog,
            &quest,
            QuestScriptPhase::Start,
            &eligible,
            &item_definitions(),
        ) else {
            panic!("matching helper records should resolve the script");
        };
        assert_eq!(plan.record_writes.len(), 1);
        assert_eq!(plan.record_writes[0].quest_id, 902);
        assert_eq!(plan.record_writes[0].index, 42);
        assert_eq!(plan.record_writes[0].value, "Result");

        crate::quest_records::set(&mut eligible, 900, 7, "0070".to_owned())
            .expect("different exact helper record");
        assert!(matches!(
            resolve(
                &catalog,
                &quest,
                QuestScriptPhase::Start,
                &eligible,
                &item_definitions(),
            ),
            QuestScriptResolution::ConditionsNotMet { .. }
        ));
    }

    #[test]
    fn malformed_and_incompatible_record_operations_are_rejected() {
        let quest = scripted_quest("record_script");
        for (source, expected) in [
            (
                r#"
                    [[scripts]]
                    name = "record_script"
                    [[scripts.conditions]]
                    type = "quest_record_at_least"
                    quest_id = 1
                    index = 0
                    value = "+1"
                "#,
                "strictly decimal",
            ),
            (
                r#"
                    [[scripts]]
                    name = "record_script"
                    [[scripts.conditions]]
                    type = "quest_record_equals"
                    quest_id = 0
                    index = 0
                    value = "x"
                "#,
                "nonzero",
            ),
            (
                r#"
                    [[scripts]]
                    name = "record_script"
                    [[scripts.conditions]]
                    type = "quest_record_at_least"
                    quest_id = 1
                    index = 0
                    value = "20"
                    [[scripts.conditions]]
                    type = "quest_record_at_most"
                    quest_id = 1
                    index = 0
                    value = "10"
                "#,
                "incompatible numeric predicates",
            ),
            (
                r#"
                    [[scripts]]
                    name = "record_script"
                    [[scripts.actions]]
                    type = "set_record"
                    quest_id = 1
                    index = 0
                    value = "1234567890123456"
                "#,
                "at most 15 bytes",
            ),
            (
                r#"
                    [[scripts]]
                    name = "record_script"
                    [[scripts.actions]]
                    type = "set_record"
                    quest_id = 1
                    index = 0
                    value = "first"
                    [[scripts.actions]]
                    type = "set_record"
                    quest_id = 1
                    index = 0
                    value = "second"
                "#,
                "more than once",
            ),
        ] {
            let error = load(source, &[&quest], &item_definitions())
                .expect_err("invalid record script must fail");
            assert!(
                error.to_string().contains(expected),
                "{error} should contain {expected:?}"
            );
        }
    }

    #[test]
    fn quest_status_actions_are_typed_and_strictly_validated() {
        let quest = scripted_quest("status_script");
        let mut target = scripted_quest("unused");
        target.id = 200;
        target.start.script = None;
        let catalog = load(
            r#"
                [[scripts]]
                name = "status_script"

                [[scripts.actions]]
                type = "set_quest_status"
                quest_id = 200
                state = "started"
            "#,
            &[&quest, &target],
            &item_definitions(),
        )
        .expect("quest status script");
        let QuestScriptResolution::Ready(plan) = resolve(
            &catalog,
            &quest,
            QuestScriptPhase::Start,
            &player(),
            &item_definitions(),
        ) else {
            panic!("quest status script should resolve");
        };
        assert_eq!(
            plan.quest_state_actions,
            vec![QuestStateAction {
                quest_id: 200,
                state: QuestStateActionState::Started,
            }]
        );

        for (source, expected) in [
            (
                r#"
                    [[scripts]]
                    name = "status_script"
                    [[scripts.actions]]
                    type = "set_quest_status"
                    quest_id = 0
                    state = "completed"
                "#,
                "zero quest status target",
            ),
            (
                r#"
                    [[scripts]]
                    name = "status_script"
                    [[scripts.actions]]
                    type = "set_quest_status"
                    quest_id = 300
                    state = "completed"
                "#,
                "not a loaded quest definition",
            ),
            (
                r#"
                    [[scripts]]
                    name = "status_script"
                    [[scripts.actions]]
                    type = "set_quest_status"
                    quest_id = 100
                    state = "completed"
                "#,
                "transitioning quest",
            ),
            (
                r#"
                    [[scripts]]
                    name = "status_script"
                    [[scripts.actions]]
                    type = "set_quest_status"
                    quest_id = 200
                    state = "started"
                    [[scripts.actions]]
                    type = "set_quest_status"
                    quest_id = 200
                    state = "completed"
                "#,
                "more than once",
            ),
        ] {
            let error = load(source, &[&quest, &target], &item_definitions())
                .expect_err("invalid quest status script must fail");
            assert!(
                error.to_string().contains(expected),
                "{error} should contain {expected:?}"
            );
        }

        let unknown_state = load(
            r#"
                [[scripts]]
                name = "status_script"
                [[scripts.actions]]
                type = "set_quest_status"
                quest_id = 200
                state = "unknown"
            "#,
            &[&quest, &target],
            &item_definitions(),
        )
        .expect_err("unknown quest status must fail");
        assert!(unknown_state.to_string().contains("failed to parse"));

        let mut conflicting = quest.clone();
        conflicting
            .start_actions
            .quest_state_actions
            .push(QuestStateAction {
                quest_id: 200,
                state: QuestStateActionState::Completed,
            });
        let conflict = load(
            r#"
                [[scripts]]
                name = "status_script"
                [[scripts.actions]]
                type = "set_quest_status"
                quest_id = 200
                state = "started"
            "#,
            &[&conflicting, &target],
            &item_definitions(),
        )
        .expect_err("merged duplicate quest status targets must fail");
        assert!(conflict.to_string().contains("both set quest 200 status"));
    }

    #[test]
    fn lazy_lookup_script_items_are_accepted_and_collected_for_projection() {
        let item_id = 4_000_001;
        let definitions = LazyItemDefinitions {
            definition: ItemDefinition {
                item_id,
                stack_max: 100,
                ..ItemDefinition::default()
            },
            lookups: Cell::new(0),
        };
        let quest = scripted_quest("lazy_item");
        let scripts = load(
            &format!(
                r#"
                    [[scripts]]
                    name = "lazy_item"

                    [[scripts.conditions]]
                    type = "item_quantity"
                    item_id = {item_id}
                    quantity = 1

                    [[scripts.actions]]
                    type = "item_delta"
                    item_id = {item_id}
                    delta = 1
                "#
            ),
            &[&quest],
            &definitions,
        )
        .expect("lazy script item should load");

        assert_eq!(scripts.item_reference_ids().len(), 1);
        assert!(scripts.item_reference_ids().contains(&item_id));
        assert_eq!(definitions.lookups.get(), 2);
    }

    #[test]
    fn missing_configuration_file_is_an_empty_catalog() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let catalog = QuestScriptCatalog::load(
            &directory.path().join("missing.toml"),
            [],
            &BTreeSet::new(),
            &[],
        )
        .expect("missing configuration");

        assert_eq!(catalog.len(), 0);
    }

    #[test]
    fn archive_referenced_scripts_for_unloaded_quests_are_ignored() {
        let archive_references = BTreeSet::from(["raw_only".to_owned()]);
        let catalog = load_with_archive_references(
            r#"
                [[scripts]]
                name = "raw_only"

                [[scripts.actions]]
                type = "item_delta"
                item_id = 9999999
                delta = 1
            "#,
            &[],
            &archive_references,
            &[],
        )
        .expect("inactive archive script");

        assert_eq!(catalog.len(), 0);
        assert_eq!(catalog.ignored_len(), 1);
        assert!(catalog.item_reference_ids().is_empty());

        let error = load("[[scripts]]\nname = \"not_in_archive\"", &[], &[])
            .expect_err("unknown script name");
        assert!(error.to_string().contains("not referenced by Quest.wz"));

        let duplicate = load_with_archive_references(
            concat!(
                "[[scripts]]\nname = \"raw_only\"\n",
                "[[scripts]]\nname = \"raw_only\"\n",
            ),
            &[],
            &archive_references,
            &[],
        )
        .expect_err("duplicate inactive script");
        assert!(duplicate.to_string().contains("duplicated"));

        let malformed_references = BTreeSet::from([" raw_only".to_owned()]);
        let malformed = load_with_archive_references(
            "[[scripts]]\nname = \" raw_only\"",
            &[],
            &malformed_references,
            &[],
        )
        .expect_err("malformed inactive script");
        assert!(malformed.to_string().contains("surrounding whitespace"));
    }

    #[test]
    fn loaded_reference_takes_precedence_over_archive_reference() {
        let quest = scripted_quest("active");
        let archive_references = BTreeSet::from(["active".to_owned()]);
        let catalog = load_with_archive_references(
            "[[scripts]]\nname = \"active\"",
            &[&quest],
            &archive_references,
            &[],
        )
        .expect("active archive script");

        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog.ignored_len(), 0);
    }

    #[test]
    fn v83_example_catalog_is_complete_and_valid() {
        let source = include_str!("../../../examples/v83/quest-scripts.toml");
        let file = toml::from_str::<QuestScriptFile>(source).expect("parse v83 example catalog");
        assert_eq!(file.scripts.len(), 678);

        let mut quests = file
            .scripts
            .iter()
            .enumerate()
            .map(|(index, program)| {
                let mut quest = scripted_quest(&program.name);
                quest.id = 1_000_000 + u32::try_from(index).expect("catalog index fits u32");
                quest
            })
            .collect::<Vec<_>>();
        let status_target_ids = file
            .scripts
            .iter()
            .flat_map(|program| &program.actions)
            .filter_map(|action| match action {
                QuestScriptAction::SetQuestStatus { quest_id, .. } => Some(*quest_id),
                _ => None,
            })
            .collect::<std::collections::BTreeSet<_>>();
        quests.extend(status_target_ids.into_iter().map(|quest_id| {
            let mut quest = scripted_quest("unconfigured_target");
            quest.id = quest_id;
            quest.start.script = None;
            quest
        }));
        let item_definitions = collect_item_reference_ids(&file)
            .into_iter()
            .map(|item_id| ItemDefinition {
                item_id,
                ..ItemDefinition::default()
            })
            .collect::<Vec<_>>();
        let quest_references = quests.iter().collect::<Vec<_>>();
        let catalog = build_catalog(
            std::path::Path::new("examples/v83/quest-scripts.toml"),
            file,
            &quest_references,
            &BTreeSet::new(),
            &item_definitions,
        )
        .expect("valid v83 example catalog");

        assert_eq!(catalog.len(), 678);
        assert_eq!(
            catalog
                .programs
                .values()
                .filter(|program| {
                    !program.conditions.is_empty()
                        || !program.plan.item_deltas.is_empty()
                        || program.plan.mesos != 0
                        || program.plan.experience != 0
                        || program.plan.fame != 0
                        || !program.plan.quest_state_actions.is_empty()
                        || !program.plan.record_writes.is_empty()
                        || !program.plan.result_pages.is_empty()
                        || !program.incomplete_pages.is_empty()
                })
                .count(),
            132
        );
        for script in ["q6030e", "q6031e", "q6032e", "q10272e"] {
            let program = catalog.programs.get(script).expect("configured fallback");
            assert!(program.conditions.is_empty());
            assert!(program.plan.item_deltas.is_empty());
            assert_eq!(program.plan.mesos, 0);
        }
        let medal = catalog
            .programs
            .get("q29900e")
            .expect("configured medal completion");
        assert_eq!(medal.plan.item_deltas.len(), 1);
        assert_eq!(medal.plan.item_deltas[0].item_id, 1_142_107);
        assert_eq!(medal.plan.item_deltas[0].count, 1);
    }

    fn load(
        source: &str,
        quests: &[&QuestDefinition],
        item_definitions: &(impl ItemDefinitionLookup + ?Sized),
    ) -> Result<QuestScriptCatalog, super::QuestScriptConfigError> {
        load_with_archive_references(source, quests, &BTreeSet::new(), item_definitions)
    }

    fn load_with_archive_references(
        source: &str,
        quests: &[&QuestDefinition],
        archive_script_references: &BTreeSet<String>,
        item_definitions: &(impl ItemDefinitionLookup + ?Sized),
    ) -> Result<QuestScriptCatalog, super::QuestScriptConfigError> {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join("quest-scripts.toml");
        fs::write(&path, source).expect("write quest scripts");
        QuestScriptCatalog::load(
            &path,
            quests.iter().copied(),
            archive_script_references,
            item_definitions,
        )
    }

    fn scripted_quest(script: &str) -> QuestDefinition {
        QuestDefinition {
            id: 100,
            name: "Scripted quest".to_owned(),
            start: QuestStartRequirements {
                script: Some(script.to_owned()),
                ..QuestStartRequirements::default()
            },
            completion: QuestCompletionRequirements::default(),
            start_actions: QuestActions::default(),
            completion_actions: QuestActions::default(),
            dialogue: QuestDialogue::default(),
            info: QuestInfo::default(),
        }
    }

    fn player() -> PlayerState {
        PlayerState {
            level: 1,
            stats: Some(CharacterStats::default()),
            inventory: Some(InventoryState::default()),
            mesos: 100,
            ..PlayerState::default()
        }
    }

    fn item_definitions() -> Vec<ItemDefinition> {
        vec![ItemDefinition {
            item_id: ITEM_ID,
            stack_max: 100,
            ..ItemDefinition::default()
        }]
    }
}
