use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use anyhow::Context;
use anyhow::Result;
use anyhow::bail;
use serde::Deserialize;
use serde::Serialize;

const MAXIMUM_PROGRAM_OPERATIONS: usize = 64;
const MAXIMUM_PROGRAMS: usize = 1_024;
const MAXIMUM_PAGES_PER_BRANCH: usize = 16;
const MAXIMUM_PAGE_BYTES: usize = 4_096;
const MAXIMUM_SCRIPT_NAME_BYTES: usize = 256;
const MAXIMUM_RECORD_VALUE_BYTES: usize = 15;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ScriptFile {
    pub scripts: Vec<ScriptProgram>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ScriptProgram {
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditions: Vec<Condition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<Action>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub result_pages: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub incomplete_pages: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Condition {
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
        state: QuestState,
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

impl Default for Condition {
    fn default() -> Self {
        Self::MinimumLevel { level: 1 }
    }
}

impl Condition {
    pub const KINDS: [&'static str; 11] = [
        "Minimum level",
        "Maximum level",
        "Job IDs",
        "Map ID",
        "Mesos at least",
        "Mesos at most",
        "Item quantity",
        "Quest state",
        "Quest record equals",
        "Quest record at least",
        "Quest record at most",
    ];

    pub fn kind_index(&self) -> usize {
        match self {
            Self::MinimumLevel { .. } => 0,
            Self::MaximumLevel { .. } => 1,
            Self::JobIds { .. } => 2,
            Self::MapId { .. } => 3,
            Self::MesosAtLeast { .. } => 4,
            Self::MesosAtMost { .. } => 5,
            Self::ItemQuantity { .. } => 6,
            Self::QuestState { .. } => 7,
            Self::QuestRecordEquals { .. } => 8,
            Self::QuestRecordAtLeast { .. } => 9,
            Self::QuestRecordAtMost { .. } => 10,
        }
    }

    pub fn from_kind(index: usize) -> Self {
        match index {
            0 => Self::MinimumLevel { level: 1 },
            1 => Self::MaximumLevel { level: 1 },
            2 => Self::JobIds { ids: vec![0] },
            3 => Self::MapId { map_id: 0 },
            4 => Self::MesosAtLeast { amount: 1 },
            5 => Self::MesosAtMost { amount: 1 },
            6 => Self::ItemQuantity {
                item_id: 0,
                quantity: 1,
            },
            7 => Self::QuestState {
                quest_id: 1,
                state: QuestState::NotStarted,
            },
            8 => Self::QuestRecordEquals {
                quest_id: 1,
                index: 0,
                value: String::new(),
            },
            9 => Self::QuestRecordAtLeast {
                quest_id: 1,
                index: 0,
                value: "0".to_owned(),
            },
            _ => Self::QuestRecordAtMost {
                quest_id: 1,
                index: 0,
                value: "0".to_owned(),
            },
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Action {
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
        state: QuestState,
    },
}

impl Default for Action {
    fn default() -> Self {
        Self::Experience { amount: 1 }
    }
}

impl Action {
    pub const KINDS: [&'static str; 6] = [
        "Item delta",
        "Mesos",
        "Experience",
        "Fame",
        "Set record",
        "Set quest status",
    ];

    pub fn kind_index(&self) -> usize {
        match self {
            Self::ItemDelta { .. } => 0,
            Self::Mesos { .. } => 1,
            Self::Experience { .. } => 2,
            Self::Fame { .. } => 3,
            Self::SetRecord { .. } => 4,
            Self::SetQuestStatus { .. } => 5,
        }
    }

    pub fn from_kind(index: usize) -> Self {
        match index {
            0 => Self::ItemDelta {
                item_id: 0,
                delta: 1,
            },
            1 => Self::Mesos { delta: 1 },
            2 => Self::Experience { amount: 1 },
            3 => Self::Fame { delta: 1 },
            4 => Self::SetRecord {
                quest_id: 1,
                index: 0,
                value: String::new(),
            },
            _ => Self::SetQuestStatus {
                quest_id: 1,
                state: QuestState::NotStarted,
            },
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuestState {
    NotStarted,
    Started,
    Completed,
}

impl QuestState {
    pub const ALL: [Self; 3] = [Self::NotStarted, Self::Started, Self::Completed];

    pub fn label(self) -> &'static str {
        match self {
            Self::NotStarted => "Not started",
            Self::Started => "Started",
            Self::Completed => "Completed",
        }
    }
}

pub fn load(path: &Path) -> Result<ScriptFile> {
    let source =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    toml::from_str(&source).with_context(|| format!("failed to parse {}", path.display()))
}

pub fn encode(file: &ScriptFile) -> Result<String> {
    validate(file)?;
    let mut source = toml::to_string_pretty(file).context("failed to encode quest scripts")?;
    source.push('\n');
    let reparsed: ScriptFile =
        toml::from_str(&source).context("encoded quest scripts failed validation")?;
    validate(&reparsed)?;
    Ok(source)
}

pub fn validate(file: &ScriptFile) -> Result<()> {
    if file.scripts.len() > MAXIMUM_PROGRAMS {
        bail!("quest scripts exceed the {MAXIMUM_PROGRAMS} program limit");
    }
    let mut names = BTreeSet::new();
    for program in &file.scripts {
        if program.name.trim().is_empty() {
            bail!("quest script names must not be empty");
        }
        if program.name.trim() != program.name {
            bail!("quest script {:?} has surrounding whitespace", program.name);
        }
        if program.name.len() > MAXIMUM_SCRIPT_NAME_BYTES {
            bail!(
                "quest script {:?} exceeds {MAXIMUM_SCRIPT_NAME_BYTES} bytes",
                program.name
            );
        }
        if !names.insert(program.name.as_str()) {
            bail!("quest script {:?} is defined more than once", program.name);
        }
        validate_program(program)?;
    }
    Ok(())
}

fn validate_program(program: &ScriptProgram) -> Result<()> {
    let operations = program
        .conditions
        .len()
        .checked_add(program.actions.len())
        .and_then(|count| count.checked_add(program.result_pages.len()))
        .and_then(|count| count.checked_add(program.incomplete_pages.len()))
        .unwrap_or(usize::MAX);
    if operations > MAXIMUM_PROGRAM_OPERATIONS {
        bail!(
            "quest script {:?} exceeds {MAXIMUM_PROGRAM_OPERATIONS} total operations and pages",
            program.name
        );
    }
    validate_pages(&program.name, "result", &program.result_pages)?;
    validate_pages(&program.name, "incomplete", &program.incomplete_pages)?;
    for condition in &program.conditions {
        validate_condition(&program.name, condition)?;
    }
    validate_condition_compatibility(&program.name, &program.conditions)?;
    for action in &program.actions {
        validate_action(&program.name, action)?;
    }
    validate_action_compatibility(&program.name, &program.actions)?;
    Ok(())
}

fn validate_condition_compatibility(
    script: &str,
    conditions: &[Condition],
) -> Result<()> {
    let mut minimum_level = None;
    let mut maximum_level = None;
    let mut minimum_mesos = None;
    let mut maximum_mesos = None;
    let mut map_id = None;
    let mut quest_states = BTreeMap::new();
    for condition in conditions {
        match condition {
            Condition::MinimumLevel { level } => {
                minimum_level =
                    Some(minimum_level.map_or(*level, |current: u32| current.max(*level)));
            }
            Condition::MaximumLevel { level } => {
                maximum_level =
                    Some(maximum_level.map_or(*level, |current: u32| current.min(*level)));
            }
            Condition::MesosAtLeast { amount } => {
                minimum_mesos =
                    Some(minimum_mesos.map_or(*amount, |current: u64| current.max(*amount)));
            }
            Condition::MesosAtMost { amount } => {
                maximum_mesos =
                    Some(maximum_mesos.map_or(*amount, |current: u64| current.min(*amount)));
            }
            Condition::MapId { map_id: required } => {
                if map_id.is_some_and(|current| current != *required) {
                    bail!("quest script {script:?} requires different map IDs");
                }
                map_id = Some(*required);
            }
            Condition::QuestState { quest_id, state }
                if quest_states
                    .insert(*quest_id, *state)
                    .is_some_and(|current| current != *state) =>
            {
                bail!("quest script {script:?} requires conflicting states for quest {quest_id}");
            }
            Condition::QuestState { .. } => {}
            _ => {}
        }
    }
    if minimum_level
        .zip(maximum_level)
        .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        bail!("quest script {script:?} has incompatible level limits");
    }
    if minimum_mesos
        .zip(maximum_mesos)
        .is_some_and(|(minimum, maximum)| minimum > maximum)
    {
        bail!("quest script {script:?} has incompatible mesos limits");
    }
    Ok(())
}

fn validate_action_compatibility(
    script: &str,
    actions: &[Action],
) -> Result<()> {
    let mut mesos = 0_i64;
    let mut experience = 0_u64;
    let mut fame = 0_i32;
    let mut records = BTreeSet::new();
    let mut quest_states = BTreeSet::new();
    for action in actions {
        match action {
            Action::Mesos { delta } => {
                mesos = mesos
                    .checked_add(*delta)
                    .with_context(|| format!("quest script {script:?} mesos actions overflow"))?;
            }
            Action::Experience { amount } => {
                experience = experience.checked_add(*amount).with_context(|| {
                    format!("quest script {script:?} experience actions overflow")
                })?;
            }
            Action::Fame { delta } => {
                fame = fame
                    .checked_add(*delta)
                    .with_context(|| format!("quest script {script:?} fame actions overflow"))?;
            }
            Action::SetRecord {
                quest_id, index, ..
            } => {
                if !records.insert((*quest_id, *index)) {
                    bail!(
                        "quest script {script:?} writes quest record {quest_id}[{index}] more \
                         than once"
                    );
                }
            }
            Action::SetQuestStatus { quest_id, .. } => {
                if !quest_states.insert(*quest_id) {
                    bail!("quest script {script:?} sets quest {quest_id} status more than once");
                }
            }
            Action::ItemDelta { .. } => {}
        }
    }
    Ok(())
}

fn validate_pages(
    script: &str,
    branch: &str,
    pages: &[String],
) -> Result<()> {
    if pages.len() > MAXIMUM_PAGES_PER_BRANCH {
        bail!("quest script {script:?} has too many {branch} pages");
    }
    if pages.iter().any(|page| page.trim().is_empty()) {
        bail!("quest script {script:?} has an empty {branch} page");
    }
    if pages.iter().any(|page| page.len() > MAXIMUM_PAGE_BYTES) {
        bail!("quest script {script:?} has a {branch} page exceeding {MAXIMUM_PAGE_BYTES} bytes");
    }
    Ok(())
}

fn validate_condition(
    script: &str,
    condition: &Condition,
) -> Result<()> {
    match condition {
        Condition::MinimumLevel { level } | Condition::MaximumLevel { level } if *level == 0 => {
            bail!("quest script {script:?} has a zero level limit")
        }
        Condition::JobIds { ids } => {
            if ids.is_empty() {
                bail!("quest script {script:?} has an empty job ID list");
            }
            if ids.iter().copied().collect::<BTreeSet<_>>().len() != ids.len() {
                bail!("quest script {script:?} has duplicate job IDs");
            }
        }
        Condition::ItemQuantity { item_id, quantity } if *item_id == 0 || *quantity == 0 => {
            bail!("quest script {script:?} has an invalid item quantity condition");
        }
        Condition::QuestState { quest_id, .. } => validate_quest_id(script, *quest_id)?,
        Condition::QuestRecordEquals {
            quest_id, value, ..
        } => validate_record(script, *quest_id, value, false)?,
        Condition::QuestRecordAtLeast {
            quest_id, value, ..
        }
        | Condition::QuestRecordAtMost {
            quest_id, value, ..
        } => validate_record(script, *quest_id, value, true)?,
        _ => {}
    }
    Ok(())
}

fn validate_action(
    script: &str,
    action: &Action,
) -> Result<()> {
    match action {
        Action::ItemDelta { item_id, delta }
            if *item_id == 0 || *delta == 0 || *delta == i64::MIN =>
        {
            bail!("quest script {script:?} has an invalid item delta")
        }
        Action::Mesos { delta } if *delta == 0 || *delta == i64::MIN => {
            bail!("quest script {script:?} has an invalid mesos delta")
        }
        Action::Experience { amount } if *amount == 0 => {
            bail!("quest script {script:?} has a zero experience grant")
        }
        Action::Fame { delta } if *delta == 0 => {
            bail!("quest script {script:?} has a zero fame delta")
        }
        Action::SetRecord {
            quest_id, value, ..
        } => validate_record(script, *quest_id, value, false)?,
        Action::SetQuestStatus { quest_id, .. } => validate_quest_id(script, *quest_id)?,
        _ => {}
    }
    Ok(())
}

fn validate_quest_id(
    script: &str,
    quest_id: u32,
) -> Result<()> {
    if quest_id == 0 {
        bail!("quest script {script:?} has a zero quest ID");
    }
    Ok(())
}

fn validate_record(
    script: &str,
    quest_id: u32,
    value: &str,
    decimal: bool,
) -> Result<()> {
    validate_quest_id(script, quest_id)?;
    if !value.is_ascii() || value.len() > MAXIMUM_RECORD_VALUE_BYTES {
        bail!(
            "quest script {script:?} has a record value that is not ASCII or exceeds \
             {MAXIMUM_RECORD_VALUE_BYTES} bytes"
        );
    }
    if decimal && (value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit())) {
        bail!("quest script {script:?} has a record predicate that is not decimal");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_scripts_round_trip_through_toml() {
        let file = ScriptFile {
            scripts: vec![ScriptProgram {
                name: "q1021s".to_owned(),
                actions: vec![Action::ItemDelta {
                    item_id: 2_010_007,
                    delta: 7,
                }],
                ..ScriptProgram::default()
            }],
        };

        let source = encode(&file).expect("encode scripts");
        let decoded: ScriptFile = toml::from_str(&source).expect("decode scripts");

        assert_eq!(decoded.scripts[0].name, "q1021s");
        assert!(matches!(
            decoded.scripts[0].actions[0],
            Action::ItemDelta {
                item_id: 2_010_007,
                delta: 7
            }
        ));
    }

    #[test]
    fn invalid_program_shapes_are_rejected_before_save() {
        let file = ScriptFile {
            scripts: vec![ScriptProgram {
                name: "q1s".to_owned(),
                conditions: vec![Condition::ItemQuantity {
                    item_id: 1,
                    quantity: 0,
                }],
                ..ScriptProgram::default()
            }],
        };

        assert!(encode(&file).is_err());
    }
}
