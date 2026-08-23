use std::collections::BTreeSet;
use std::collections::HashMap;
use std::path::Path;

use thiserror::Error;
use wz_reader::WzNodeArc;

use super::wz::WzContentError;
use super::wz::child;
use super::wz::int_value;
use super::wz::node_name;
use super::wz::open_archive;
use super::wz::parse;
use super::wz::sorted_children;
use super::wz::string_value;
use super::wz::wrap_archive_root;

const QUEST_ARCHIVE: &str = "Quest.wz";
const MAXIMUM_QUESTION_CHOICES: usize = 4;

pub(crate) struct QuestContent {
    _base: WzNodeArc,
    definitions: HashMap<u32, QuestDefinition>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QuestDefinition {
    pub id: u32,
    pub name: String,
    pub start_npc_id: u32,
    pub completion_npc_id: u32,
    pub allowed_jobs: Vec<u32>,
    pub minimum_level: Option<u32>,
    pub offer_pages: Vec<String>,
    pub accepted_pages: Vec<String>,
    pub declined_pages: Vec<String>,
    pub question: Option<QuestQuestion>,
    pub reward_experience: u64,
    pub next_quest_id: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QuestQuestion {
    pub prompt: String,
    pub choices: Vec<QuestChoice>,
    pub correct_choice_id: u32,
    pub success_pages: Vec<String>,
    pub failure_pages: HashMap<u32, Vec<String>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct QuestChoice {
    pub id: u32,
    pub label: String,
}

#[derive(Debug, Error)]
pub enum QuestContentError {
    #[error(transparent)]
    Wz(#[from] WzContentError),
    #[error("Quest.wz quest {quest_id} is unsupported: {message}")]
    Invalid { quest_id: u32, message: String },
}

impl QuestContent {
    pub fn open_optional(
        directory: &Path,
        quest_ids: Option<&BTreeSet<u32>>,
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
        let check = required_child(&root, "Check.img", 0)?;
        let act = required_child(&root, "Act.img", 0)?;
        let say = required_child(&root, "Say.img", 0)?;
        let info = required_child(&root, "QuestInfo.img", 0)?;
        for (name, node) in [
            ("Check.img", &check),
            ("Act.img", &act),
            ("Say.img", &say),
            ("QuestInfo.img", &info),
        ] {
            parse(node, format!("{} {name}", path.display()))?;
        }

        let strict = quest_ids.is_some();
        let quest_ids = match quest_ids {
            Some(quest_ids) => quest_ids.clone(),
            None => discover_quest_ids(&check)?,
        };
        let mut definitions = HashMap::new();
        let mut unsupported = 0_usize;
        for quest_id in quest_ids {
            match load_definition(quest_id, &check, &act, &say, &info) {
                Ok(definition) => {
                    definitions.insert(quest_id, definition);
                }
                Err(QuestContentError::Invalid { .. }) if !strict => unsupported += 1,
                Err(error) => return Err(error),
            }
        }
        tracing::info!(
            path = %path.display(),
            compatible_quests = definitions.len(),
            unsupported_quests = unsupported,
            "WZ quest source ready"
        );
        Ok(Some(Self {
            _base: base,
            definitions,
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
}

fn discover_quest_ids(checks: &WzNodeArc) -> Result<BTreeSet<u32>, QuestContentError> {
    sorted_children(checks)?
        .into_iter()
        .filter_map(|node| match node_name(&node) {
            Ok(name) => name.parse::<u32>().ok().map(Ok),
            Err(error) => Some(Err(error.into())),
        })
        .collect()
}

fn load_definition(
    quest_id: u32,
    checks: &WzNodeArc,
    actions: &WzNodeArc,
    dialogue: &WzNodeArc,
    info: &WzNodeArc,
) -> Result<QuestDefinition, QuestContentError> {
    let key = quest_id.to_string();
    let check = required_child(checks, &key, quest_id)?;
    let action = required_child(actions, &key, quest_id)?;
    let say = required_child(dialogue, &key, quest_id)?;
    let info = required_child(info, &key, quest_id)?;
    let start_check = required_child(&check, "0", quest_id)?;
    let completion_check = required_child(&check, "1", quest_id)?;
    let start_action = required_child(&action, "0", quest_id)?;
    let completion_action = required_child(&action, "1", quest_id)?;
    validate_children(quest_id, &start_check, &["job", "lvmin", "npc"])?;
    validate_children(quest_id, &completion_check, &["npc"])?;
    validate_children(quest_id, &start_action, &[])?;
    validate_children(quest_id, &completion_action, &["exp", "nextQuest"])?;

    let start_npc_id = required_u32(&start_check, "npc", quest_id)?;
    let completion_npc_id = required_u32(&completion_check, "npc", quest_id)?;
    let allowed_jobs = child(&start_check, "job")?
        .map(|jobs| read_u32_values(quest_id, &jobs))
        .transpose()?
        .unwrap_or_default();
    let minimum_level = optional_u32(&start_check, "lvmin", quest_id)?;
    let reward_experience = optional_u32(&completion_action, "exp", quest_id)?.map_or(0, u64::from);
    let next_quest_id = optional_u32(&completion_action, "nextQuest", quest_id)?;
    let start_dialogue = required_child(&say, "0", quest_id)?;
    let completion_dialogue = required_child(&say, "1", quest_id)?;
    let question = read_question(quest_id, &completion_dialogue)?.ok_or_else(|| {
        invalid(
            quest_id,
            "completion dialogue has no supported answer interaction",
        )
    })?;

    Ok(QuestDefinition {
        id: quest_id,
        name: string_value(&info, "name")?
            .filter(|name| !name.is_empty())
            .ok_or_else(|| invalid(quest_id, "QuestInfo.img has no nonempty name"))?,
        start_npc_id,
        completion_npc_id,
        allowed_jobs,
        minimum_level,
        offer_pages: numbered_strings(quest_id, &start_dialogue)?,
        accepted_pages: branch_strings(quest_id, &start_dialogue, "yes")?,
        declined_pages: branch_strings(quest_id, &start_dialogue, "no")?,
        question: Some(question),
        reward_experience,
        next_quest_id,
    })
}

fn read_question(
    quest_id: u32,
    dialogue: &WzNodeArc,
) -> Result<Option<QuestQuestion>, QuestContentError> {
    let Some(ask) = int_value(dialogue, "ask")? else {
        return Ok(None);
    };
    if ask != 1 {
        return Err(invalid(
            quest_id,
            format!("dialogue ask value {ask} is not supported"),
        ));
    }
    let pages = numbered_strings(quest_id, dialogue)?;
    let first = pages
        .first()
        .ok_or_else(|| invalid(quest_id, "question dialogue has no prompt"))?;
    let (prompt, choices) = parse_choices(first)
        .map_err(|message| invalid(quest_id, format!("question choices are invalid: {message}")))?;
    if choices.len() > MAXIMUM_QUESTION_CHOICES {
        return Err(invalid(
            quest_id,
            format!("questions support at most {MAXIMUM_QUESTION_CHOICES} choices"),
        ));
    }
    let stop = required_child(dialogue, "stop", quest_id)?;
    let answers = required_child(&stop, "0", quest_id)?;
    let answer = required_u32(&answers, "answer", quest_id)?;
    let correct_choice_id = answer
        .checked_sub(1)
        .ok_or_else(|| invalid(quest_id, "question answer uses invalid one-based index 0"))?;
    if !choices.iter().any(|choice| choice.id == correct_choice_id) {
        return Err(invalid(
            quest_id,
            "question answer does not identify a listed choice",
        ));
    }
    let mut failure_pages = HashMap::new();
    for choice in &choices {
        if choice.id == correct_choice_id {
            continue;
        }
        let key = choice.id.to_string();
        let text = string_value(&answers, &key)?.ok_or_else(|| {
            invalid(
                quest_id,
                format!("question choice {} has no failure dialogue", choice.id),
            )
        })?;
        failure_pages.insert(choice.id, vec![text]);
    }
    Ok(Some(QuestQuestion {
        prompt,
        choices,
        correct_choice_id,
        success_pages: pages.into_iter().skip(1).collect(),
        failure_pages,
    }))
}

fn parse_choices(source: &str) -> Result<(String, Vec<QuestChoice>), String> {
    let Some(first_choice) = source.find("#L") else {
        return Err("no #L choice markers were found".to_owned());
    };
    let prompt = source[..first_choice].trim().to_owned();
    let mut remaining = &source[first_choice..];
    let mut choices = Vec::new();
    while let Some(marker) = remaining.find("#L") {
        remaining = &remaining[marker + 2..];
        let id_end = remaining
            .find('#')
            .ok_or_else(|| "choice ID is not terminated".to_owned())?;
        let id = remaining[..id_end]
            .parse::<u32>()
            .map_err(|_| "choice ID is not an unsigned integer".to_owned())?;
        remaining = &remaining[id_end + 1..];
        let label_end = remaining
            .find("#l")
            .ok_or_else(|| format!("choice {id} has no #l terminator"))?;
        let label = strip_inline_formatting(remaining[..label_end].trim());
        if label.is_empty() || choices.iter().any(|choice: &QuestChoice| choice.id == id) {
            return Err(format!("choice {id} is empty or duplicated"));
        }
        choices.push(QuestChoice { id, label });
        remaining = &remaining[label_end + 2..];
    }
    if choices.is_empty() {
        return Err("no choices were parsed".to_owned());
    }
    Ok((prompt, choices))
}

fn strip_inline_formatting(source: &str) -> String {
    ["#b", "#r", "#k", "#n"]
        .into_iter()
        .fold(source.to_owned(), |text, marker| text.replace(marker, ""))
        .trim()
        .to_owned()
}

fn branch_strings(
    quest_id: u32,
    node: &WzNodeArc,
    name: &str,
) -> Result<Vec<String>, QuestContentError> {
    child(node, name)?
        .map(|branch| numbered_strings(quest_id, &branch))
        .transpose()
        .map(Option::unwrap_or_default)
}

fn numbered_strings(
    quest_id: u32,
    node: &WzNodeArc,
) -> Result<Vec<String>, QuestContentError> {
    let mut output = Vec::new();
    for child_node in sorted_children(node)? {
        let name = node_name(&child_node)?;
        if name.parse::<u32>().is_err() {
            continue;
        }
        let value = string_value(node, &name)?
            .ok_or_else(|| invalid(quest_id, format!("dialogue page {name} is not a string")))?;
        output.push(value);
    }
    Ok(output)
}

fn read_u32_values(
    quest_id: u32,
    node: &WzNodeArc,
) -> Result<Vec<u32>, QuestContentError> {
    sorted_children(node)?
        .into_iter()
        .map(|value| {
            let name = node_name(&value)?;
            required_u32(node, &name, quest_id)
        })
        .collect()
}

fn validate_children(
    quest_id: u32,
    node: &WzNodeArc,
    allowed: &[&str],
) -> Result<(), QuestContentError> {
    for child in sorted_children(node)? {
        let name = node_name(&child)?;
        if !allowed.contains(&name.as_str()) {
            return Err(invalid(
                quest_id,
                format!("property {name:?} is not in the supported quest subset"),
            ));
        }
    }
    Ok(())
}

fn required_child(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<WzNodeArc, QuestContentError> {
    child(node, name)?
        .ok_or_else(|| invalid(quest_id, format!("required node {name:?} is missing")))
}

fn required_u32(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<u32, QuestContentError> {
    optional_u32(node, name, quest_id)?
        .ok_or_else(|| invalid(quest_id, format!("required integer {name:?} is missing")))
}

fn optional_u32(
    node: &WzNodeArc,
    name: &str,
    quest_id: u32,
) -> Result<Option<u32>, QuestContentError> {
    int_value(node, name)?
        .map(|value| {
            u32::try_from(value)
                .map_err(|_| invalid(quest_id, format!("integer {name:?} is negative")))
        })
        .transpose()
}

fn invalid(
    quest_id: u32,
    message: impl Into<String>,
) -> QuestContentError {
    QuestContentError::Invalid {
        quest_id,
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::parse_choices;

    #[test]
    fn wz_list_markers_become_typed_choices() {
        let (prompt, choices) =
            parse_choices("Which key opens inventory?\n#L0##b I#l\n#L1# K#l\n#L2# S#k#l")
                .expect("valid WZ choices");

        assert_eq!(prompt, "Which key opens inventory?");
        assert_eq!(choices.len(), 3);
        assert_eq!((choices[0].id, choices[0].label.as_str()), (0, "I"));
        assert_eq!((choices[2].id, choices[2].label.as_str()), (2, "S"));
    }
}
