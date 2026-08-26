use oozems_proto::v1::Npc;
use oozems_proto::v1::NpcInteractionResponse;
use oozems_proto::v1::NpcTaxiDestination;
use oozems_proto::v1::NpcTaxiView;

use super::invalid;
use crate::api::ApiError;
use crate::api::PlayerMutation;
use crate::api::load_map;
use crate::api::prepare_player_mutation;
use crate::app::AppState;
use crate::player_lock::PlayerGuard;

pub(super) async fn take_taxi(
    state: &AppState,
    guard: &PlayerGuard,
    mutation: PlayerMutation,
    source_map: &oozems_proto::v1::Map,
    npc: &Npc,
    target_map_id: u32,
    now_unix_ms: u64,
) -> Result<NpcInteractionResponse, ApiError> {
    let current = mutation.player.clone();
    let taxi = state
        .interactions
        .taxi(current.map_id, npc.spawn_id)
        .ok_or_else(|| invalid("this NPC does not operate a taxi"))?;
    let destination = taxi
        .destinations
        .iter()
        .find(|destination| destination.map_id == target_map_id)
        .ok_or_else(|| invalid("the selected taxi destination is not available"))?;
    if current.mesos < destination.fare {
        return Err(invalid(
            "the player does not have enough mesos for that taxi",
        ));
    }
    let mut target_map = load_map(state, destination.map_id)
        .await?
        .ok_or_else(|| invalid("the taxi destination map does not exist"))?;
    let position = crate::movement::authorized_destination(&target_map, &destination.portal_name)?;
    target_map.dropped_items = crate::items::map_drops(&state.drops, target_map.id)?;
    let simulation = crate::mobs::map_snapshot(&state.mobs, &target_map).await?;
    target_map.mobs = simulation.mobs;
    target_map.mob_projectiles = simulation.mob_projectiles;
    target_map.simulation_sequence = simulation.sequence;
    let mut player = current.clone();
    player.mesos -= destination.fare;
    player.map_id = target_map.id;
    player.position = Some(position);
    let (decision, rollback) = crate::movement::relocate_player(
        &state.movement,
        &current,
        source_map,
        &target_map,
        &destination.portal_name,
        state.gameplay.movement,
        now_unix_ms,
    )?;
    let (mut transaction, _) = prepare_player_mutation(state, mutation, player, true, true);
    let player_id = crate::player_transaction::staged_player(&transaction)
        .id
        .clone();
    crate::player_transaction::stage_relocation(
        &mut transaction,
        state.movement.clone(),
        player_id,
        rollback,
        now_unix_ms,
    );
    let player =
        crate::player_transaction::commit_player_transaction(&state.database, guard, transaction)
            .await?
            .player;
    Ok(NpcInteractionResponse {
        player: Some(player),
        interaction: None,
        authoritative: Some(decision.authoritative),
        map: Some(target_map),
        npc_animation: None,
        active_buffs: None,
        quest_indicators: Vec::new(),
    })
}

pub(super) fn taxi_view(taxi: &crate::interactions::TaxiDefinition) -> NpcTaxiView {
    NpcTaxiView {
        destinations: taxi
            .destinations
            .iter()
            .map(|destination| NpcTaxiDestination {
                map_id: destination.map_id,
                label: destination.label.clone(),
                fare: destination.fare,
            })
            .collect(),
    }
}
