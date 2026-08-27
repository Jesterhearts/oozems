use oozems_proto::v1::Mob;
use oozems_proto::v1::MobProjectile;
use shipyard::IntoIter;
use shipyard::UniqueViewMut;
use shipyard::View;

use super::MobMapState;
use super::MobStoreError;
use super::MobUpdate;
use super::components::MobCombat;
use super::components::MobIdentity;
use super::components::MobMotion;
use super::components::PendingEvents;
use super::components::Position;
use super::components::Projectile;

pub(super) fn snapshot(
    state: &MobMapState,
    player_id: Option<&str>,
) -> Result<MobUpdate, MobStoreError> {
    let mobs = state.world.run(
        |positions: View<Position>,
         identities: View<MobIdentity>,
         motions: View<MobMotion>,
         combats: View<MobCombat>| {
            (&positions, &identities, &motions, &combats)
                .iter()
                .map(|(position, identity, motion, combat)| Mob {
                    id: identity.public_id.clone(),
                    definition_id: identity.definition_id,
                    position: Some(position.vector()),
                    flip_x: motion.flip_x,
                    layer: position.layer,
                    current_hp: combat.current_hp,
                    spawn_id: identity.spawn_id,
                    movement_mode: motion.mode as i32,
                })
                .collect()
        },
    );
    let mob_projectiles =
        state
            .world
            .run(|positions: View<Position>, projectiles: View<Projectile>| {
                (&positions, &projectiles)
                    .iter()
                    .filter(|(_, projectile)| !projectile.impacted)
                    .map(|(position, projectile)| MobProjectile {
                        id: projectile.public_id.clone(),
                        source_mob_id: projectile.source_mob_id.clone(),
                        target_player_id: projectile.target_player_id.clone(),
                        position: Some(position.vector()),
                        layer: position.layer,
                    })
                    .collect()
            });
    let combat_events = player_id.map_or_else(Vec::new, |player_id| {
        state.world.run(|mut events: UniqueViewMut<PendingEvents>| {
            events.by_player.remove(player_id).unwrap_or_default()
        })
    });
    let mob_deaths = player_id.map_or_else(Vec::new, |player_id| {
        state.world.run(|mut events: UniqueViewMut<PendingEvents>| {
            events
                .mob_deaths_by_player
                .remove(player_id)
                .unwrap_or_default()
        })
    });
    let staged_drops = player_id.map_or_else(Vec::new, |player_id| {
        state.world.run(|mut events: UniqueViewMut<PendingEvents>| {
            events
                .staged_drops_by_player
                .remove(player_id)
                .unwrap_or_default()
        })
    });
    Ok(MobUpdate {
        mobs,
        mob_projectiles,
        combat_events,
        mob_deaths,
        staged_drops,
        reactors: crate::reactors::snapshot(&state.reactors),
        sequence: state.snapshot_sequence,
        player_attack_transaction: None,
        delivery_id: None,
    })
}
