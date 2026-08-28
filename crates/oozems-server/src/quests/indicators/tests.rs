use oozems_proto::v1::Map;
use oozems_proto::v1::Npc;
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

#[test]
fn completion_script_item_condition_marks_its_npc_when_satisfied() {
    let definitions = item_definitions();
    let mut definition = quest(100);
    definition.completion.npc_id = Some(10);
    definition.completion.script = Some("completion_check".to_owned());
    let scripts = script_catalog(
        r#"
            [[scripts]]
            name = "completion_check"

            [[scripts.conditions]]
            type = "item_quantity"
            item_id = 4000000
            quantity = 1
        "#,
        &definition,
        &definitions,
    );
    let mut player = player(Vec::new(), 4);
    player
        .quests
        .push(player_quest(definition.id, QuestStatus::Started));

    assert_eq!(
        indicator_with_scripts(&player, 10, &[&definition], &scripts),
        NpcQuestIndicator::Unspecified
    );

    player
        .inventory
        .as_mut()
        .expect("inventory")
        .stacks
        .push(InventoryItemStack {
            item_id: ITEM_A,
            quantity: 1,
            expires_at_unix_ms: 0,
        });
    assert_eq!(
        indicator_with_scripts(&player, 10, &[&definition], &scripts),
        NpcQuestIndicator::Ready
    );
}

#[test]
fn projection_updates_each_map_npc_and_returns_its_spawn_update() {
    let mut definition = quest(100);
    definition.start.npc_id = Some(10);
    let player = player(Vec::new(), 4);
    let mut map = Map {
        npcs: vec![
            Npc {
                spawn_id: 7,
                npc_id: 10,
                ..Npc::default()
            },
            Npc {
                spawn_id: 8,
                npc_id: 11,
                ..Npc::default()
            },
        ],
        ..Map::default()
    };

    let updates = super::project_npc_quest_indicators(
        &mut map,
        &player,
        &PlayerEffects::default(),
        &[&definition],
        &item_definitions(),
        scripts(),
        environment(1_000),
    );

    assert_eq!(
        updates
            .iter()
            .map(|update| (update.npc_spawn_id, update.indicator))
            .collect::<Vec<_>>(),
        vec![
            (7, NpcQuestIndicator::Available as i32),
            (8, NpcQuestIndicator::Unspecified as i32),
        ]
    );
    assert_eq!(map.npcs[0].quest_indicator, updates[0].indicator);
    assert_eq!(map.npcs[1].quest_indicator, updates[1].indicator);
}

fn indicator(
    player: &PlayerState,
    npc_id: u32,
    definitions: &[&QuestDefinition],
) -> NpcQuestIndicator {
    indicator_with_scripts(player, npc_id, definitions, scripts())
}

fn indicator_with_scripts(
    player: &PlayerState,
    npc_id: u32,
    definitions: &[&QuestDefinition],
    scripts: &QuestScriptCatalog,
) -> NpcQuestIndicator {
    super::npc_quest_indicator(
        player,
        &PlayerEffects::default(),
        npc_id,
        definitions,
        &item_definitions(),
        scripts,
        environment(1_000),
    )
}
