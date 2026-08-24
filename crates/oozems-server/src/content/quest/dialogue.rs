use std::collections::HashMap;

use wz_reader::WzNodeArc;
use wz_reader::WzNodeCast;

use super::QuestContentError;
use super::importer;
use super::invalid;
use super::model::*;
use super::unsupported;
use crate::content::wz;

struct AuditedMissingAnswerOverride {
    quest_id: u32,
    phase: &'static str,
    archive_index: u32,
    source: &'static str,
    choice_ids: &'static [u32],
    correct_choice_id: u32,
}

const AUDITED_MISSING_ANSWER_OVERRIDES: &[AuditedMissingAnswerOverride] =
    &[AuditedMissingAnswerOverride {
        quest_id: 6_034,
        phase: "completion",
        archive_index: 0,
        source: "Would you like to pick up the crumbled piece of paper?\n\n#L0##bYes.#l\n#L1#No.#l",
        choice_ids: &[0, 1],
        correct_choice_id: 0,
    }];
const QUEST_3077_SAY_FINGERPRINT: u64 = 0xde08989b64839107;

pub(super) fn read_dialogue(
    quest_id: u32,
    say: Option<&WzNodeArc>,
    action: &WzNodeArc,
) -> Result<QuestDialogue, QuestContentError> {
    validate_audited_dialogue_source(quest_id, say)?;
    let say_start = say.map(|say| wz::child(say, "0")).transpose()?.flatten();
    let say_completion = say.map(|say| wz::child(say, "1")).transpose()?.flatten();
    let action_start = wz::child(action, "0")?;
    let action_completion = wz::child(action, "1")?;

    let mut start_dialogue = read_start_dialogue(quest_id, say_start.as_ref(), true)?;
    let mut retained_fields = std::mem::take(&mut start_dialogue.retained_fields);
    if !start_dialogue_has_content(&start_dialogue) {
        let fallback = read_start_dialogue(quest_id, action_start.as_ref(), false)?;
        if start_dialogue_has_content(&fallback) {
            start_dialogue = fallback;
        }
    }

    let (mut completion, mut question, completion_retained) =
        read_optional_completion_dialogue(quest_id, say_completion.as_ref())?;
    retained_fields.extend(completion_retained);
    if !completion_dialogue_has_content(&completion, question.as_ref()) {
        let (fallback, fallback_question, _) =
            read_optional_completion_dialogue(quest_id, action_completion.as_ref())?;
        if completion_dialogue_has_content(&fallback, fallback_question.as_ref()) {
            completion = fallback;
            question = fallback_question;
        }
    }
    retained_fields.sort();
    retained_fields.dedup();
    Ok(QuestDialogue {
        offer_pages: start_dialogue.offer_pages,
        accepted_pages: start_dialogue.accepted_pages,
        declined_pages: start_dialogue.declined_pages,
        has_start_decision: start_dialogue.has_decision,
        start_question: start_dialogue.question,
        completion,
        question,
        retained_fields,
    })
}

fn validate_audited_dialogue_source(
    quest_id: u32,
    say: Option<&WzNodeArc>,
) -> Result<(), QuestContentError> {
    if quest_id == 3_077 {
        let say = say.ok_or_else(|| {
            invalid(
                quest_id,
                "audited duplicate dialogue correction has no Say/3077 source",
            )
        })?;
        let actual = importer::audited_node_fingerprint(quest_id, say)?;
        if actual != QUEST_3077_SAY_FINGERPRINT {
            return Err(invalid(
                quest_id,
                format!(
                    "audited Say/3077 fingerprint changed from {QUEST_3077_SAY_FINGERPRINT:#018x} \
                     to {actual:#018x}"
                ),
            ));
        }
    }
    for audited in AUDITED_MISSING_ANSWER_OVERRIDES
        .iter()
        .filter(|audited| audited.quest_id == quest_id)
    {
        let phase_index = match audited.phase {
            "start" => "0",
            "completion" => "1",
            _ => unreachable!("audited dialogue phase is static"),
        };
        let phase = say
            .map(|say| wz::child(say, phase_index))
            .transpose()?
            .flatten()
            .ok_or_else(|| {
                invalid(
                    quest_id,
                    format!(
                        "audited missing-answer override has no Say/{phase_index} source branch"
                    ),
                )
            })?;
        if importer::optional_i64(&phase, "ask", quest_id)? != Some(1) {
            return Err(invalid(
                quest_id,
                format!(
                    "audited missing-answer override for {} dialogue no longer has ask=1",
                    audited.phase
                ),
            ));
        }
        validate_audited_missing_answer_pages(
            quest_id,
            audited.phase,
            &numbered_pages(quest_id, &phase)?,
        )?;
    }
    Ok(())
}

#[derive(Default)]
struct ParsedStartDialogue {
    offer_pages: Vec<String>,
    accepted_pages: Vec<String>,
    declined_pages: Vec<String>,
    has_decision: bool,
    question: Option<QuestQuestionSequence>,
    retained_fields: Vec<String>,
}

fn read_start_dialogue(
    quest_id: u32,
    start: Option<&WzNodeArc>,
    retain_unknown_fields: bool,
) -> Result<ParsedStartDialogue, QuestContentError> {
    let Some(start) = start else {
        return Ok(ParsedStartDialogue::default());
    };
    let numbered_pages = numbered_pages(quest_id, start)?;
    let start_question = read_question_interaction(quest_id, start, &numbered_pages, "start")?;
    let offer_pages = start_question
        .as_ref()
        .map(|question| question.leading_pages.clone())
        .unwrap_or_else(|| page_strings(&numbered_pages));
    let mut retained_fields = Vec::new();
    if retain_unknown_fields {
        for child in wz::sorted_children(start)? {
            let name = wz::node_name(&child)?;
            if name.parse::<u32>().is_ok() || matches!(name.as_str(), "yes" | "no") {
                continue;
            }
            if name == "ask" || name == "stop" && start_question.is_some() {
                continue;
            }
            retained_fields.push(format!("start/{name}"));
        }
        if let Some(question) = &start_question {
            retain_question_stop_fields(start, "start", question, &mut retained_fields)?;
        }
    }
    Ok(ParsedStartDialogue {
        offer_pages,
        accepted_pages: branch_strings(quest_id, start, "yes")?,
        declined_pages: branch_strings(quest_id, start, "no")?,
        has_decision: wz::child(start, "yes")?.is_some() || wz::child(start, "no")?.is_some(),
        question: start_question,
        retained_fields,
    })
}

fn read_optional_completion_dialogue(
    quest_id: u32,
    completion: Option<&WzNodeArc>,
) -> Result<
    (
        QuestCompletionDialogue,
        Option<QuestQuestionSequence>,
        Vec<String>,
    ),
    QuestContentError,
> {
    match completion {
        Some(completion) => read_completion_dialogue(quest_id, completion),
        None => Ok((QuestCompletionDialogue::default(), None, Vec::new())),
    }
}

fn start_dialogue_has_content(dialogue: &ParsedStartDialogue) -> bool {
    dialogue.question.is_some()
        || pages_have_content(&dialogue.offer_pages)
        || pages_have_content(&dialogue.accepted_pages)
        || pages_have_content(&dialogue.declined_pages)
        || dialogue.has_decision
}

fn completion_dialogue_has_content(
    dialogue: &QuestCompletionDialogue,
    question: Option<&QuestQuestionSequence>,
) -> bool {
    question.is_some()
        || pages_have_content(&dialogue.pages)
        || pages_have_content(&dialogue.success_pages)
        || pages_have_content(&dialogue.declined_pages)
        || pages_have_content(&dialogue.incomplete.item_pages)
        || pages_have_content(&dialogue.incomplete.mob_pages)
        || pages_have_content(&dialogue.incomplete.quest_pages)
        || pages_have_content(&dialogue.incomplete.default_pages)
        || dialogue.lost.is_some()
}

fn pages_have_content(pages: &[String]) -> bool {
    pages.iter().any(|page| !page.trim().is_empty())
}

fn read_completion_dialogue(
    quest_id: u32,
    node: &WzNodeArc,
) -> Result<
    (
        QuestCompletionDialogue,
        Option<QuestQuestionSequence>,
        Vec<String>,
    ),
    QuestContentError,
> {
    let numbered_pages = numbered_pages(quest_id, node)?;
    let question = read_question_interaction(quest_id, node, &numbered_pages, "completion")?;
    let pages = question
        .as_ref()
        .map(|question| question.leading_pages.clone())
        .unwrap_or_else(|| page_strings(&numbered_pages));
    let success_pages = branch_strings(quest_id, node, "yes")?;
    let declined_pages = branch_strings(quest_id, node, "no")?;
    let incomplete = read_incomplete_dialogue(quest_id, node)?;
    let lost = read_lost_item_dialogue(quest_id, node)?;
    let mut retained = Vec::new();
    for child in wz::sorted_children(node)? {
        let name = wz::node_name(&child)?;
        if name.parse::<u32>().is_ok()
            || matches!(name.as_str(), "yes" | "no" | "stop" | "ask" | "lost")
        {
            continue;
        }
        retained.push(format!("completion/{name}"));
    }
    if let Some(question) = &question {
        retain_question_stop_fields(node, "completion", question, &mut retained)?;
    } else if let Some(stop) = wz::child(node, "stop")? {
        for child in wz::sorted_children(&stop)? {
            let name = wz::node_name(&child)?;
            if !matches!(name.as_str(), "item" | "mob" | "quest" | "default") {
                retained.push(format!("completion/stop/{name}"));
            }
        }
    }
    Ok((
        QuestCompletionDialogue {
            pages,
            success_pages,
            declined_pages,
            incomplete,
            lost,
        },
        question,
        retained,
    ))
}

fn read_question_interaction(
    quest_id: u32,
    dialogue: &WzNodeArc,
    pages: &[NumberedPage],
    phase: &str,
) -> Result<Option<QuestQuestionSequence>, QuestContentError> {
    let Some(ask) = importer::optional_i64(dialogue, "ask", quest_id)? else {
        return Ok(None);
    };
    if ask != 1 {
        return Err(unsupported(
            quest_id,
            format!("{phase} dialogue interaction"),
            format!("{phase} dialogue ask value {ask} is not supported"),
        ));
    }
    read_question_sequence(quest_id, dialogue, pages, phase)
}

fn retain_question_stop_fields(
    dialogue: &WzNodeArc,
    phase: &str,
    question: &QuestQuestionSequence,
    retained: &mut Vec<String>,
) -> Result<(), QuestContentError> {
    let Some(stop) = wz::child(dialogue, "stop")? else {
        return Ok(());
    };
    for child in wz::sorted_children(&stop)? {
        let name = wz::node_name(&child)?;
        if question
            .steps
            .iter()
            .any(|step| step.archive_index.to_string() == name)
        {
            for field in wz::sorted_children(&child)? {
                let field_name = wz::node_name(&field)?;
                if field_name != "answer" && field_name.parse::<u32>().is_err() {
                    retained.push(format!("{phase}/stop/{name}/{field_name}"));
                }
            }
        } else if phase != "completion"
            || !matches!(name.as_str(), "item" | "mob" | "quest" | "default")
        {
            retained.push(format!("{phase}/stop/{name}"));
        }
    }
    Ok(())
}

fn read_lost_item_dialogue(
    quest_id: u32,
    node: &WzNodeArc,
) -> Result<Option<QuestLostItemDialogue>, QuestContentError> {
    let Some(lost) = wz::child(node, "lost")? else {
        return Ok(None);
    };
    if !dialogue_branch_has_content(&lost)? {
        return Ok(None);
    }
    for child in wz::sorted_children(&lost)? {
        let name = wz::node_name(&child)?;
        if name.parse::<u32>().is_err() && name != "yes" {
            return Err(unsupported(
                quest_id,
                "lost-item restoration branch structure",
                format!("completion lost branch field {name:?} is not safely representable"),
            ));
        }
    }
    let prompt_pages = numbered_strings(quest_id, &lost)?;
    let success_pages = branch_strings(quest_id, &lost, "yes")?;
    if prompt_pages.is_empty() {
        let completion_success_pages = branch_strings(quest_id, node, "yes")?;
        return Err(unsupported(
            quest_id,
            "lost-item restoration branch structure",
            format!(
                "completion lost dialogue has {} prompt pages, {} nested yes-result pages, and {} \
                 completion yes-result pages",
                prompt_pages.len(),
                success_pages.len(),
                completion_success_pages.len()
            ),
        ));
    }
    if let Some(yes) = wz::child(&lost, "yes")? {
        for child in wz::sorted_children(&yes)? {
            let name = wz::node_name(&child)?;
            if name.parse::<u32>().is_err() {
                return Err(unsupported(
                    quest_id,
                    "lost-item restoration branch structure",
                    format!(
                        "completion lost yes branch field {name:?} is not safely representable"
                    ),
                ));
            }
        }
    }
    Ok(Some(QuestLostItemDialogue {
        prompt_pages,
        success_pages,
        items: Vec::new(),
    }))
}

fn dialogue_branch_has_content(node: &WzNodeArc) -> Result<bool, QuestContentError> {
    for child in wz::sorted_children(node)? {
        if dialogue_branch_has_content(&child)? {
            return Ok(true);
        }
    }

    let read = node.read().map_err(|_| wz::WzContentError::Lock {
        context: "quest dialogue branch",
    })?;
    if read.is_null() || read.is_sub_property() {
        return Ok(false);
    }
    drop(read);
    Ok(importer::scalar_string(node)?.is_none_or(|text| !text.trim().is_empty()))
}

fn read_incomplete_dialogue(
    quest_id: u32,
    node: &WzNodeArc,
) -> Result<QuestIncompleteDialogue, QuestContentError> {
    let Some(stop) = wz::child(node, "stop")? else {
        return Ok(QuestIncompleteDialogue::default());
    };
    Ok(QuestIncompleteDialogue {
        item_pages: branch_strings(quest_id, &stop, "item")?,
        mob_pages: branch_strings(quest_id, &stop, "mob")?,
        quest_pages: branch_strings(quest_id, &stop, "quest")?,
        default_pages: branch_strings(quest_id, &stop, "default")?,
    })
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct NumberedPage {
    index: u32,
    text: String,
}

fn read_question_sequence(
    quest_id: u32,
    dialogue: &WzNodeArc,
    pages: &[NumberedPage],
    phase: &str,
) -> Result<Option<QuestQuestionSequence>, QuestContentError> {
    validate_audited_missing_answer_pages(quest_id, phase, pages)?;
    let normalized_pages = normalize_audited_duplicate_choice_page(quest_id, phase, pages)?;
    let pages = normalized_pages.as_slice();
    let question_pages = pages
        .iter()
        .filter(|page| page.text.contains("#L"))
        .collect::<Vec<_>>();
    if question_pages.is_empty() {
        return Ok(None);
    }

    let stop = importer::required_child(dialogue, "stop", quest_id)?;
    let mut steps = Vec::with_capacity(question_pages.len());
    for page in question_pages {
        let (prompt, choices) = parse_choices(&page.text).map_err(|message| {
            invalid(
                quest_id,
                format!(
                    "question choices on dialogue page {} are invalid: {message}",
                    page.index
                ),
            )
        })?;
        let answers = importer::required_child(&stop, &page.index.to_string(), quest_id)?;
        let answer = importer::optional_u32(&answers, "answer", quest_id)?;
        let audited_correct_choice_id =
            audited_missing_answer_override(quest_id, phase, page, &choices, answer, &answers)?;
        let correct_choice_id = if let Some(correct_choice_id) = audited_correct_choice_id {
            correct_choice_id
        } else {
            match answer {
                Some(answer) => {
                    let answer_index = answer.checked_sub(1).ok_or_else(|| {
                        invalid(
                            quest_id,
                            format!(
                                "question on dialogue page {} uses invalid one-based answer 0",
                                page.index
                            ),
                        )
                    })?;
                    choices
                        .get(usize::try_from(answer_index).unwrap_or(usize::MAX))
                        .map(|choice| choice.id)
                        .ok_or_else(|| {
                            invalid(
                                quest_id,
                                format!(
                                    "question answer on dialogue page {} does not identify a \
                                     listed choice",
                                    page.index
                                ),
                            )
                        })?
                }
                None if choices.len() == 1 => choices[0].id,
                None => {
                    return Err(invalid(
                        quest_id,
                        format!(
                            "question on dialogue page {} has no one-based answer",
                            page.index
                        ),
                    ));
                }
            }
        };
        let mut failure_pages = HashMap::new();
        for (choice_position, choice) in choices.iter().enumerate() {
            if choice.id == correct_choice_id {
                continue;
            }
            let key = choice_position.to_string();
            let text = wz::child(&answers, &key)?
                .map(|node| importer::scalar_string(&node))
                .transpose()?
                .flatten();
            let Some(text) = text else {
                if audited_correct_choice_id.is_some() {
                    continue;
                }
                return Err(invalid(
                    quest_id,
                    format!(
                        "question choice {} on dialogue page {} has no failure dialogue",
                        choice.id, page.index
                    ),
                ));
            };
            failure_pages.insert(choice.id, vec![text]);
        }
        steps.push(QuestQuestionStep {
            archive_index: page.index,
            prompt,
            choices,
            correct_choice_id,
            continuation_pages: Vec::new(),
            failure_pages,
        });
    }

    let first_index = steps
        .first()
        .expect("question pages are not empty")
        .archive_index;
    let leading_pages = pages
        .iter()
        .filter(|page| page.index < first_index)
        .map(|page| page.text.clone())
        .collect();
    for index in 0..steps.len().saturating_sub(1) {
        let start = steps[index].archive_index;
        let end = steps[index + 1].archive_index;
        steps[index].continuation_pages = pages
            .iter()
            .filter(|page| page.index > start && page.index < end)
            .map(|page| page.text.clone())
            .collect();
    }
    let last_index = steps
        .last()
        .expect("question pages are not empty")
        .archive_index;
    let trailing_pages = pages
        .iter()
        .filter(|page| page.index > last_index)
        .map(|page| page.text.clone())
        .collect();
    Ok(Some(QuestQuestionSequence {
        leading_pages,
        steps,
        trailing_pages,
    }))
}

fn normalize_audited_duplicate_choice_page(
    quest_id: u32,
    phase: &str,
    pages: &[NumberedPage],
) -> Result<Vec<NumberedPage>, QuestContentError> {
    let mut normalized = pages.to_vec();
    if quest_id != 3_077 || phase != "start" {
        return Ok(normalized);
    }
    let page = normalized
        .iter_mut()
        .find(|page| page.index == 1)
        .ok_or_else(|| {
            invalid(
                quest_id,
                "audited Say/3077/0 duplicate correction has no page 1",
            )
        })?;
    let midpoint = page.text.len() / 2;
    if page.text.len() % 2 != 0 || !page.text.is_char_boundary(midpoint) {
        return Err(invalid(
            quest_id,
            "audited Say/3077/0/1 is not two byte-aligned identical halves",
        ));
    }
    let (first, second) = page.text.split_at(midpoint);
    if first != second || first.matches("#L0#").count() != 1 || first.matches("#L").count() != 1 {
        return Err(invalid(
            quest_id,
            "audited Say/3077/0/1 no longer has two identical dialogue-plus-#L0# halves",
        ));
    }
    page.text.truncate(midpoint);
    Ok(normalized)
}

fn validate_audited_missing_answer_pages(
    quest_id: u32,
    phase: &str,
    pages: &[NumberedPage],
) -> Result<(), QuestContentError> {
    for audited in AUDITED_MISSING_ANSWER_OVERRIDES
        .iter()
        .filter(|audited| audited.quest_id == quest_id && audited.phase == phase)
    {
        let source_matches = pages.iter().any(|page| {
            page.index == audited.archive_index && page.text.as_str() == audited.source
        });
        if !source_matches {
            let actual = pages
                .iter()
                .find(|page| page.index == audited.archive_index)
                .map(|page| page.text.as_str());
            return Err(invalid(
                quest_id,
                format!(
                    "audited missing-answer override for {phase} dialogue page {} no longer \
                     matches its source page: {actual:?}",
                    audited.archive_index,
                ),
            ));
        }
    }
    Ok(())
}

fn audited_missing_answer_override(
    quest_id: u32,
    phase: &str,
    page: &NumberedPage,
    choices: &[QuestChoice],
    answer: Option<u32>,
    answers: &WzNodeArc,
) -> Result<Option<u32>, QuestContentError> {
    let Some(audited) = AUDITED_MISSING_ANSWER_OVERRIDES.iter().find(|audited| {
        audited.quest_id == quest_id
            && audited.phase == phase
            && audited.archive_index == page.index
    }) else {
        return Ok(None);
    };
    let choice_ids = choices.iter().map(|choice| choice.id).collect::<Vec<_>>();
    if page.text != audited.source
        || choice_ids != audited.choice_ids
        || answer.is_some()
        || !wz::sorted_children(answers)?.is_empty()
    {
        return Err(invalid(
            quest_id,
            format!(
                "audited missing-answer override for {phase} dialogue page {} no longer matches \
                 its answer shape",
                page.index
            ),
        ));
    }
    Ok(Some(audited.correct_choice_id))
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
        let terminator = remaining.find("#l");
        let next_choice = remaining.find("#L");
        let (label_end, consumed) = match (terminator, next_choice) {
            (Some(terminator), Some(next_choice)) if next_choice < terminator => (next_choice, 0),
            (Some(terminator), _) => (terminator, 2),
            (None, Some(next_choice)) => (next_choice, 0),
            (None, None) => (remaining.len(), 0),
        };
        let label = strip_inline_formatting(remaining[..label_end].trim());
        if label.is_empty() || choices.iter().any(|choice: &QuestChoice| choice.id == id) {
            return Err(format!("choice {id} is empty or duplicated"));
        }
        choices.push(QuestChoice { id, label });
        remaining = &remaining[label_end + consumed..];
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
    wz::child(node, name)?
        .map(|branch| numbered_strings(quest_id, &branch))
        .transpose()
        .map(Option::unwrap_or_default)
}

fn numbered_strings(
    quest_id: u32,
    node: &WzNodeArc,
) -> Result<Vec<String>, QuestContentError> {
    numbered_pages(quest_id, node).map(|pages| page_strings(&pages))
}

fn page_strings(pages: &[NumberedPage]) -> Vec<String> {
    pages.iter().map(|page| page.text.clone()).collect()
}

fn numbered_pages(
    quest_id: u32,
    node: &WzNodeArc,
) -> Result<Vec<NumberedPage>, QuestContentError> {
    let mut output = Vec::new();
    for child in wz::sorted_children(node)? {
        let name = wz::node_name(&child)?;
        let Ok(index) = name.parse::<u32>() else {
            continue;
        };
        let value = importer::scalar_string(&child)?
            .ok_or_else(|| invalid(quest_id, format!("dialogue page {name} is not a string")))?;
        output.push(NumberedPage { index, text: value });
    }
    output.sort_unstable_by_key(|page| page.index);
    Ok(output)
}

#[cfg(test)]
mod tests {
    use wz_reader::WzNode;
    use wz_reader::WzNodeArc;
    use wz_reader::WzObjectType;
    use wz_reader::property::WzString;
    use wz_reader::property::WzSubProperty;
    use wz_reader::property::WzValue;

    use super::NumberedPage;
    use super::normalize_audited_duplicate_choice_page;
    use super::parse_choices;
    use super::read_completion_dialogue;
    use super::read_dialogue;
    use crate::content::quest::QuestContentError;

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

    #[test]
    fn choice_labels_can_end_at_the_next_choice_or_page_end() {
        let (_, choices) = parse_choices("Pick one: #L0#First#L1#Second")
            .expect("choices with omitted WZ terminators");

        assert_eq!(
            choices
                .iter()
                .map(|choice| (choice.id, choice.label.as_str()))
                .collect::<Vec<_>>(),
            vec![(0, "First"), (1, "Second")]
        );
    }

    #[test]
    fn start_ask_interaction_becomes_a_typed_question() {
        let say = property("quest");
        let start = property("0");
        add_integer(&start, "ask", 1);
        add_string(
            &start,
            "0",
            "Which key opens inventory?\n#L0# K#l\n#L1##b I#l",
        );
        add_string(&start, "1", "Think carefully.");
        let yes = property("yes");
        add_string(&yes, "0", "Let us begin.");
        add_child(&start, &yes);
        let no = property("no");
        add_string(&no, "0", "Come back later.");
        add_child(&start, &no);
        let stop = property("stop");
        let answers = property("0");
        add_integer(&answers, "answer", 2);
        add_string(&answers, "0", "That is not the inventory key.");
        add_child(&stop, &answers);
        add_child(&start, &stop);
        add_child(&say, &start);

        let dialogue =
            read_dialogue(100, Some(&say), &property("action")).expect("typed start question");
        let question = dialogue.start_question.expect("start question");
        let step = &question.steps[0];

        assert_eq!(step.prompt, "Which key opens inventory?");
        assert_eq!(
            step.choices
                .iter()
                .map(|choice| (choice.id, choice.label.as_str()))
                .collect::<Vec<_>>(),
            vec![(0, "K"), (1, "I")]
        );
        assert_eq!(step.correct_choice_id, 1);
        assert_eq!(question.trailing_pages, vec!["Think carefully."]);
        assert_eq!(
            step.failure_pages.get(&0),
            Some(&vec!["That is not the inventory key.".to_owned()])
        );
        assert_eq!(dialogue.accepted_pages, vec!["Let us begin."]);
        assert_eq!(dialogue.declined_pages, vec!["Come back later."]);
        assert!(dialogue.has_start_decision);
        assert!(dialogue.retained_fields.is_empty());
    }

    #[test]
    fn start_questions_apply_existing_ask_and_failure_validation() {
        let unsupported_say = property("quest");
        let unsupported_start = property("0");
        add_integer(&unsupported_start, "ask", 2);
        add_child(&unsupported_say, &unsupported_start);

        let error = read_dialogue(100, Some(&unsupported_say), &property("action"))
            .expect_err("ask values other than one must remain unsupported");
        assert!(matches!(
            error,
            QuestContentError::Unsupported { category, .. }
                if category == "start dialogue interaction"
        ));

        let incomplete_say = property("quest");
        let incomplete_start = property("0");
        add_integer(&incomplete_start, "ask", 1);
        add_string(&incomplete_start, "0", "Pick one. #L0#Wrong#l #L1#Right#l");
        let stop = property("stop");
        let answers = property("0");
        add_integer(&answers, "answer", 2);
        add_child(&stop, &answers);
        add_child(&incomplete_start, &stop);
        add_child(&incomplete_say, &incomplete_start);

        let error = read_dialogue(100, Some(&incomplete_say), &property("action"))
            .expect_err("every incorrect answer needs authored failure dialogue");
        assert!(matches!(error, QuestContentError::Invalid { .. }));
    }

    #[test]
    fn say_dialogue_wins_and_act_fallback_only_fills_an_absent_phase() {
        let say = property("quest");
        let say_start = property("0");
        add_string(&say_start, "0", "Translated Say offer");
        add_child(&say, &say_start);

        let action = property("quest");
        let action_start = property("0");
        add_string(&action_start, "0", "Conflicting Act offer");
        let action_yes = property("yes");
        add_string(&action_yes, "0", "Act acceptance");
        add_child(&action_start, &action_yes);
        add_child(&action, &action_start);
        let action_completion = property("1");
        add_string(&action_completion, "0", "Act completion fallback");
        add_child(&action, &action_completion);

        let dialogue = read_dialogue(100, Some(&say), &action).expect("phase-level fallback");

        assert_eq!(dialogue.offer_pages, vec!["Translated Say offer"]);
        assert!(dialogue.accepted_pages.is_empty());
        assert_eq!(dialogue.completion.pages, vec!["Act completion fallback"]);
    }

    #[test]
    fn representable_act_question_is_used_as_a_typed_fallback() {
        let action = property("quest");
        let start = property("0");
        add_integer(&start, "ask", 1);
        add_string(&start, "0", "Pick one. #L0#Wrong#l #L1#Right#l");
        add_string(&start, "1", "Correct.");
        let stop = property("stop");
        let answers = property("0");
        add_integer(&answers, "answer", 2);
        add_string(&answers, "0", "Try again.");
        add_child(&stop, &answers);
        add_child(&start, &stop);
        add_child(&action, &start);

        let dialogue = read_dialogue(100, None, &action).expect("Act question fallback");
        let question = dialogue.start_question.expect("typed fallback question");
        let step = &question.steps[0];

        assert_eq!(step.prompt, "Pick one.");
        assert_eq!(step.correct_choice_id, 1);
        assert_eq!(question.trailing_pages, vec!["Correct."]);
        assert_eq!(
            step.failure_pages.get(&0),
            Some(&vec!["Try again.".to_owned()])
        );
    }

    #[test]
    fn ask_without_choice_pages_is_ordinary_dialogue() {
        let unsupported_action = property("quest");
        let unsupported_start = property("0");
        add_integer(&unsupported_start, "ask", 2);
        let unsupported_stop = property("stop");
        let unsupported_answers = property("0");
        add_integer(&unsupported_answers, "answer", 1);
        add_child(&unsupported_stop, &unsupported_answers);
        add_child(&unsupported_start, &unsupported_stop);
        add_child(&unsupported_action, &unsupported_start);
        let error = read_dialogue(100, None, &unsupported_action)
            .expect_err("fallback ask values other than one must remain unsupported");
        assert!(matches!(
            error,
            QuestContentError::Unsupported { category, .. }
                if category == "start dialogue interaction"
        ));

        let action = property("quest");
        let start = property("0");
        add_integer(&start, "ask", 1);
        add_string(&start, "0", "This has no typed choices.");
        let stop = property("stop");
        let answers = property("0");
        add_integer(&answers, "answer", 1);
        add_child(&stop, &answers);
        add_child(&start, &stop);
        add_child(&action, &start);

        let dialogue = read_dialogue(100, None, &action).expect("ordinary ask dialogue");

        assert!(dialogue.start_question.is_none());
        assert_eq!(dialogue.offer_pages, vec!["This has no typed choices."]);
    }

    #[test]
    fn sequential_questions_preserve_archive_page_order_and_step_answers() {
        let say = property("quest");
        let start = property("0");
        add_integer(&start, "ask", 1);
        add_string(&start, "0", "Introduction");
        add_string(&start, "1", "First? #L7#No#l #L9#Yes#l");
        add_string(&start, "2", "Between first and second");
        add_string(&start, "3", "Continue #L42#Only#l");
        add_string(&start, "4", "Between second and third");
        add_string(&start, "5", "Last? #L3#Right#l #L8#Wrong#l");
        add_string(&start, "6", "Trailing");
        let stop = property("stop");
        let first = property("1");
        add_integer(&first, "answer", 2);
        add_string(&first, "0", "First failure");
        add_child(&stop, &first);
        add_child(&stop, &property("3"));
        let third = property("5");
        add_integer(&third, "answer", 1);
        add_string(&third, "1", "Third failure");
        add_child(&stop, &third);
        add_child(&start, &stop);
        add_child(&say, &start);

        let dialogue = read_dialogue(100, Some(&say), &property("action"))
            .expect("three-step question sequence");
        let question = dialogue.start_question.expect("question sequence");

        assert_eq!(question.leading_pages, vec!["Introduction"]);
        assert_eq!(question.steps.len(), 3);
        assert_eq!(question.steps[0].archive_index, 1);
        assert_eq!(question.steps[0].correct_choice_id, 9);
        assert_eq!(
            question.steps[0].continuation_pages,
            vec!["Between first and second"]
        );
        assert_eq!(question.steps[1].archive_index, 3);
        assert_eq!(question.steps[1].correct_choice_id, 42);
        assert_eq!(
            question.steps[1].continuation_pages,
            vec!["Between second and third"]
        );
        assert_eq!(question.steps[2].archive_index, 5);
        assert_eq!(question.steps[2].correct_choice_id, 3);
        assert_eq!(question.trailing_pages, vec!["Trailing"]);
    }

    #[test]
    fn quest_2015_style_sparse_choice_ids_use_positional_stop_keys() {
        let say = property("quest");
        let start = property("0");
        add_integer(&start, "ask", 1);
        add_string(&start, "0", "Choose. #L0#First#l #L1#Second#l #L3#Third#l");
        let stop = property("stop");
        let answers = property("0");
        add_integer(&answers, "answer", 1);
        add_string(&answers, "1", "Second failure");
        add_string(&answers, "2", "Third failure");
        add_child(&stop, &answers);
        add_child(&start, &stop);
        add_child(&say, &start);

        let dialogue = read_dialogue(2_015, Some(&say), &property("action"))
            .expect("quest 2015-style sparse choices");
        let step = &dialogue.start_question.expect("start question").steps[0];

        assert_eq!(step.correct_choice_id, 0);
        assert_eq!(
            step.failure_pages.get(&1),
            Some(&vec!["Second failure".to_owned()])
        );
        assert_eq!(
            step.failure_pages.get(&3),
            Some(&vec!["Third failure".to_owned()])
        );
        assert!(!step.failure_pages.contains_key(&0));
    }

    #[test]
    fn question_choice_count_is_not_limited_by_the_four_row_viewport() {
        let say = property("quest");
        let start = property("0");
        add_integer(&start, "ask", 1);
        add_string(&start, "0", "Pick. #L0#A#l #L1#B#l #L2#C#l #L3#D#l #L4#E#l");
        let stop = property("stop");
        let answers = property("0");
        add_integer(&answers, "answer", 5);
        for id in 0..4 {
            add_string(&answers, &id.to_string(), "Wrong");
        }
        add_child(&stop, &answers);
        add_child(&start, &stop);
        add_child(&say, &start);

        let dialogue =
            read_dialogue(100, Some(&say), &property("action")).expect("five-choice question");

        assert_eq!(
            dialogue.start_question.expect("question").steps[0]
                .choices
                .len(),
            5
        );
    }

    #[test]
    fn audited_quest_3077_duplicate_page_is_normalized_before_choice_parsing() {
        let half = "A vision appears. #L0# Continue";
        let pages = vec![NumberedPage {
            index: 1,
            text: format!("{half}{half}"),
        }];

        let normalized = normalize_audited_duplicate_choice_page(3_077, "start", &pages)
            .expect("exact duplicated page shape");

        assert_eq!(normalized[0].text, half);
        let (_, choices) = parse_choices(&normalized[0].text).expect("one normalized choice");
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].id, 0);
        assert!(parse_choices(&pages[0].text).is_err());
        assert!(
            parse_choices(
                &normalize_audited_duplicate_choice_page(3_078, "start", &pages)
                    .expect("other quests are unchanged")[0]
                    .text
            )
            .is_err(),
            "duplicate choice IDs must remain invalid outside the audited quest page",
        );
    }

    #[test]
    fn audited_quest_3077_duplicate_page_fails_closed_on_synthetic_drift() {
        for text in [
            "A vision appears. #L0# ContinueA changed vision appears. #L0# Continue",
            "A vision appears. #L0# ContinueA vision appears. #L1# Continue",
            "A vision appears. #L0# Continue",
        ] {
            assert!(matches!(
                normalize_audited_duplicate_choice_page(
                    3_077,
                    "start",
                    &[NumberedPage {
                        index: 1,
                        text: text.to_owned(),
                    }],
                ),
                Err(QuestContentError::Invalid {
                    quest_id: 3_077,
                    ..
                })
            ));
        }
        assert!(matches!(
            normalize_audited_duplicate_choice_page(3_077, "start", &[]),
            Err(QuestContentError::Invalid {
                quest_id: 3_077,
                ..
            })
        ));
    }

    #[test]
    fn audited_quest_6034_missing_answer_selects_yes() {
        let completion = quest_6034_completion();

        let (dialogue, question, retained) =
            read_completion_dialogue(6_034, &completion).expect("audited quest 6034 question");
        let question = question.expect("quest 6034 completion question");
        let step = &question.steps[0];

        assert_eq!(
            step.choices
                .iter()
                .map(|choice| (choice.id, choice.label.as_str()))
                .collect::<Vec<_>>(),
            vec![(0, "Yes."), (1, "No.")]
        );
        assert_eq!(step.correct_choice_id, 0);
        assert!(step.failure_pages.is_empty());
        assert_eq!(
            question.trailing_pages,
            vec!["The writing on the paper is illegible."]
        );
        assert_eq!(dialogue.pages, Vec::<String>::new());
        assert!(retained.is_empty());
    }

    #[test]
    fn audited_quest_6034_missing_answer_override_fails_closed_on_drift() {
        assert!(matches!(
            read_dialogue(6_034, None, &property("action")),
            Err(QuestContentError::Invalid { .. })
        ));

        let changed_source = quest_6034_completion_with_source(
            "Would you like to pick up the crumbled piece of paper? #L0#No #L1#Yes",
        );
        assert!(matches!(
            read_completion_dialogue(6_034, &changed_source),
            Err(QuestContentError::Invalid { .. })
        ));

        let added_answer = quest_6034_completion();
        let answers = added_answer
            .read()
            .expect("completion")
            .at("stop")
            .expect("stop")
            .read()
            .expect("stop")
            .at("0")
            .expect("answers");
        add_integer(&answers, "answer", 1);
        assert!(matches!(
            read_completion_dialogue(6_034, &added_answer),
            Err(QuestContentError::Invalid { .. })
        ));

        let added_response = quest_6034_completion();
        let answers = added_response
            .read()
            .expect("completion")
            .at("stop")
            .expect("stop")
            .read()
            .expect("stop")
            .at("0")
            .expect("answers");
        add_string(&answers, "1", "No response");
        assert!(matches!(
            read_completion_dialogue(6_034, &added_response),
            Err(QuestContentError::Invalid { .. })
        ));

        assert!(matches!(
            read_completion_dialogue(6_035, &quest_6034_completion()),
            Err(QuestContentError::Invalid { .. })
        ));
    }

    #[test]
    fn lost_prompt_and_yes_result_pages_are_typed() {
        let completion = property("1");
        let lost = property("lost");
        let page = WzNode::from_str(
            "0",
            WzString::from_str("I lost the item.", [0; 4]),
            Some(&lost),
        )
        .into_lock();
        add_child(&lost, &page);
        let yes = property("yes");
        let result = WzNode::from_str(
            "0",
            WzString::from_str("Here is a replacement.", [0; 4]),
            Some(&yes),
        )
        .into_lock();
        add_child(&yes, &result);
        add_child(&lost, &yes);
        add_child(&completion, &lost);

        let (dialogue, _, retained) =
            read_completion_dialogue(100, &completion).expect("typed lost interaction");
        let lost = dialogue.lost.expect("lost interaction");

        assert_eq!(lost.prompt_pages, vec!["I lost the item."]);
        assert_eq!(lost.success_pages, vec!["Here is a replacement."]);
        assert!(lost.items.is_empty());
        assert!(retained.is_empty());
    }

    #[test]
    fn lost_prompt_with_empty_yes_closes_after_restoration() {
        let completion = property("1");
        let lost = property("lost");
        add_string(&lost, "0", "Here is another item.");
        add_child(&lost, &property("yes"));
        add_child(&completion, &lost);

        let (dialogue, _, retained) =
            read_completion_dialogue(100, &completion).expect("prompt-only lost interaction");
        let lost = dialogue.lost.expect("lost interaction");

        assert_eq!(lost.prompt_pages, vec!["Here is another item."]);
        assert!(lost.success_pages.is_empty());
        assert!(retained.is_empty());
    }

    #[test]
    fn lost_prompt_without_yes_uses_an_explicit_restore_choice() {
        let completion = property("1");
        let lost = property("lost");
        add_string(&lost, "0", "Here is another item.");
        add_child(&completion, &lost);

        let (dialogue, _, retained) =
            read_completion_dialogue(100, &completion).expect("prompt-only lost interaction");
        let lost = dialogue.lost.expect("lost interaction");

        assert_eq!(lost.prompt_pages, vec!["Here is another item."]);
        assert!(lost.success_pages.is_empty());
        assert!(retained.is_empty());
    }

    #[test]
    fn ambiguous_lost_branch_structure_is_explicitly_unsupported() {
        let completion = property("1");
        let lost = property("lost");
        let unexpected = property("no");
        let page = WzNode::from_str(
            "0",
            WzString::from_str("No branch.", [0; 4]),
            Some(&unexpected),
        )
        .into_lock();
        add_child(&unexpected, &page);
        add_child(&lost, &unexpected);
        add_child(&completion, &lost);

        let error = read_completion_dialogue(100, &completion)
            .expect_err("an unknown lost branch must remain unsupported");

        assert!(matches!(
            error,
            QuestContentError::Unsupported { category, .. }
                if category == "lost-item restoration branch structure"
        ));
    }

    #[test]
    fn behaviorless_lost_placeholders_are_ignored() {
        let placeholders = [
            WzObjectType::Value(WzValue::Null),
            WzObjectType::Property(WzSubProperty::Property),
            WzString::from_str("", [0; 4]).into(),
        ];

        for placeholder in placeholders {
            let completion = property("1");
            let lost = WzNode::from_str("lost", placeholder, Some(&completion)).into_lock();
            add_child(&completion, &lost);

            let (_, _, retained) = read_completion_dialogue(100, &completion)
                .expect("a behaviorless lost placeholder is safe to ignore");
            assert!(retained.is_empty());
        }
    }

    fn quest_6034_completion() -> WzNodeArc {
        quest_6034_completion_with_source(
            "Would you like to pick up the crumbled piece of paper?\n\n#L0##bYes.#l\n#L1#No.#l",
        )
    }

    fn quest_6034_completion_with_source(source: &str) -> WzNodeArc {
        let completion = property("1");
        add_integer(&completion, "ask", 1);
        add_string(&completion, "0", source);
        add_string(&completion, "1", "The writing on the paper is illegible.");
        let stop = property("stop");
        add_child(&stop, &property("0"));
        add_child(&completion, &stop);
        completion
    }

    fn property(name: &str) -> WzNodeArc {
        WzNode::from_str(name, WzObjectType::Property(WzSubProperty::Property), None).into_lock()
    }

    fn add_child(
        parent: &WzNodeArc,
        child: &WzNodeArc,
    ) {
        parent.write().expect("test WZ parent").add(child);
    }

    fn add_integer(
        parent: &WzNodeArc,
        name: &str,
        value: i32,
    ) {
        let child = WzNode::from_str(name, value, Some(parent)).into_lock();
        add_child(parent, &child);
    }

    fn add_string(
        parent: &WzNodeArc,
        name: &str,
        value: &str,
    ) {
        let child =
            WzNode::from_str(name, WzString::from_str(value, [0; 4]), Some(parent)).into_lock();
        add_child(parent, &child);
    }
}
