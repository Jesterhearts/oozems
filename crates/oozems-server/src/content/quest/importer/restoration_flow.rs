use super::*;

#[cfg(test)]
mod tests;

pub(crate) fn validate_lost_item_restoration_flow(
    quest_id: u32,
    completion: &QuestCompletionRequirements,
    start_actions: &QuestActions,
    completion_actions: &QuestActions,
    dialogue: &QuestDialogue,
) -> Result<Vec<QuestRestorableItem>, QuestContentError> {
    if dialogue.completion.lost.is_none() {
        return Ok(Vec::new());
    }
    if let Some(rule) = super::super::restoration::audited_rule(quest_id) {
        return validate_audited_lost_item_restoration(
            quest_id,
            completion,
            start_actions,
            completion_actions,
            dialogue,
            rule,
        );
    }
    let ambiguous_item_id = start_actions
        .conditional_items
        .iter()
        .map(|action| action.item_id)
        .chain(
            start_actions
                .weighted_items
                .iter()
                .map(|action| action.item_id),
        )
        .chain(
            start_actions
                .selectable_items
                .iter()
                .map(|action| action.item_id),
        )
        .next();
    if let Some(item_id) = ambiguous_item_id {
        return Err(unsupported(
            quest_id,
            "lost-item restoration action ambiguity",
            format!(
                "lost dialogue has a conditional, weighted, or selectable start grant for item \
                 {item_id}"
            ),
        ));
    }

    let mut restorable_items = Vec::new();
    let item_ids = start_actions
        .fixed_items
        .iter()
        .filter(|action| action.count > 0)
        .map(|action| action.item_id)
        .collect::<BTreeSet<_>>();
    let referenced_item_ids = dialogue
        .completion
        .lost
        .iter()
        .flat_map(|lost| lost.prompt_pages.iter().chain(&lost.success_pages))
        .flat_map(|page| dialogue_item_references(page))
        .collect::<BTreeSet<_>>();
    let completion_item_ids = completion
        .items
        .iter()
        .filter(|requirement| matches!(requirement.condition, QuestItemCondition::AtLeast(_)))
        .map(|requirement| requirement.item_id)
        .collect::<BTreeSet<_>>();
    if item_ids.is_disjoint(&completion_item_ids)
        && !referenced_item_ids.is_empty()
        && item_ids.is_disjoint(&referenced_item_ids)
    {
        return Err(unsupported(
            quest_id,
            "lost-item restoration action ambiguity",
            format!(
                "fixed start grants {item_ids:?} contradict the items named by the lost dialogue"
            ),
        ));
    }
    for item_id in item_ids {
        let actions = start_actions
            .fixed_items
            .iter()
            .filter(|action| action.item_id == item_id)
            .collect::<Vec<_>>();
        if actions.len() != 1 {
            return Err(unsupported(
                quest_id,
                "lost-item restoration action ambiguity",
                format!("restorable item {item_id} has multiple or mixed-sign fixed start actions"),
            ));
        }
        let action = actions[0];
        if action.count <= 0 {
            continue;
        }
        restorable_items.push(QuestRestorableItem {
            item_id,
            target_count: action.count.unsigned_abs(),
            expiration: action.expiration,
            provenance: QuestRestorationProvenance::InferredStartGrant,
            eligibility: QuestRestorationEligibility {
                owner_state: RequiredQuestState::Started,
                required_quests: &[],
                forbidden_quests: &[],
                absent_skill_ids: &[],
                absent_item_ids: &[],
            },
        });
    }
    if restorable_items.is_empty() {
        return Err(unsupported(
            quest_id,
            "lost-item restoration item mapping",
            "completion lost dialogue has no positive unconditional fixed start grant",
        ));
    }
    Ok(restorable_items)
}

pub(crate) fn dialogue_item_references(page: &str) -> impl Iterator<Item = u32> + '_ {
    page.match_indices("#t").filter_map(|(index, _)| {
        let digits = page[index + 2..]
            .chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>();
        (!digits.is_empty())
            .then(|| digits.parse::<u32>().ok())
            .flatten()
    })
}
