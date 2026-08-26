use oozems_proto::v1::Map;
use oozems_proto::v1::NpcQuestIndicator;
use oozems_proto::v1::NpcQuestIndicatorUpdate;

use super::*;

#[cfg(test)]
mod tests;

pub fn npc_quest_indicator(
    player: &PlayerState,
    effects: &PlayerEffects,
    npc_id: u32,
    quest_definitions: &[&QuestDefinition],
    item_definitions: &[ItemDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> NpcQuestIndicator {
    let ready = quest_definitions.iter().copied().any(|quest| {
        quest.completion.npc_id == Some(npc_id)
            && completion_readiness(
                player,
                effects,
                quest,
                quest_definitions,
                item_definitions,
                scripts,
                environment,
            )
            .ready
    });
    if ready {
        return NpcQuestIndicator::Ready;
    }

    if quest_definitions.iter().copied().any(|quest| {
        quest.start.npc_id == Some(npc_id)
            && is_available(
                player,
                effects,
                quest,
                item_definitions,
                scripts,
                environment,
            )
    }) {
        NpcQuestIndicator::Available
    } else {
        NpcQuestIndicator::Unspecified
    }
}

pub fn project_npc_quest_indicators(
    map: &mut Map,
    player: &PlayerState,
    effects: &PlayerEffects,
    quest_definitions: &[&QuestDefinition],
    item_definitions: &[ItemDefinition],
    scripts: &QuestScriptCatalog,
    environment: QuestEnvironment,
) -> Vec<NpcQuestIndicatorUpdate> {
    map.npcs
        .iter_mut()
        .map(|npc| {
            let indicator = npc_quest_indicator(
                player,
                effects,
                npc.npc_id,
                quest_definitions,
                item_definitions,
                scripts,
                environment,
            );
            npc.quest_indicator = indicator as i32;
            NpcQuestIndicatorUpdate {
                npc_spawn_id: npc.spawn_id,
                indicator: indicator as i32,
            }
        })
        .collect()
}
