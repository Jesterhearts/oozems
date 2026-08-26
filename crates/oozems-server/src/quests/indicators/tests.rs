use oozems_proto::v1::NpcQuestIndicator;

use crate::quests::test_support::*;

#[test]
fn available_quest_marks_its_start_npc() {
    let mut definition = quest(100);
    definition.start.npc_id = Some(10);
    let player = player(Vec::new(), 4);

    assert_eq!(
        indicator(&player, 10, &[&definition]),
        NpcQuestIndicator::Available
    );
    assert_eq!(
        indicator(&player, 11, &[&definition]),
        NpcQuestIndicator::Unspecified
    );
}

#[test]
fn ready_completion_at_an_npc_takes_priority_over_an_available_quest() {
    let mut available = quest(100);
    available.start.npc_id = Some(10);
    let mut ready = quest(101);
    ready.completion.npc_id = Some(10);
    let mut player = player(Vec::new(), 4);
    player
        .quests
        .push(player_quest(ready.id, QuestStatus::Started));

    assert_eq!(
        indicator(&player, 10, &[&available, &ready]),
        NpcQuestIndicator::Ready
    );
}

#[test]
fn incomplete_active_quest_does_not_mark_its_completion_npc_ready() {
    let mut definition = quest(100);
    definition.completion.npc_id = Some(10);
    definition.completion.items = vec![item_requirement(ITEM_A, 1)];
    let mut player = player(Vec::new(), 4);
    player
        .quests
        .push(player_quest(definition.id, QuestStatus::Started));

    assert_eq!(
        indicator(&player, 10, &[&definition]),
        NpcQuestIndicator::Unspecified
    );
}

fn indicator(
    player: &PlayerState,
    npc_id: u32,
    definitions: &[&QuestDefinition],
) -> NpcQuestIndicator {
    super::npc_quest_indicator(
        player,
        &PlayerEffects::default(),
        npc_id,
        definitions,
        &item_definitions(),
        scripts(),
        environment(1_000),
    )
}
