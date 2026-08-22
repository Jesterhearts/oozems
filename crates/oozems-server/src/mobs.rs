use std::collections::HashMap;
use std::sync::Mutex;

use oozems_proto::v1::Map;
use oozems_proto::v1::Mob;
use oozems_proto::v1::MobDefinition;
use oozems_proto::v1::MobSpawnPoint;
use thiserror::Error;

#[derive(Default)]
pub struct MobStore {
    maps: Mutex<HashMap<u32, Vec<Mob>>>,
}

#[derive(Debug, Error)]
pub enum MobStoreError {
    #[error("the mob store lock was poisoned")]
    Lock,
}

pub fn map_mobs(
    store: &MobStore,
    map: &Map,
) -> Result<Vec<Mob>, MobStoreError> {
    let mut maps = store.maps.lock().map_err(|_| MobStoreError::Lock)?;
    Ok(maps
        .entry(map.id)
        .or_insert_with(|| spawn_mobs(map))
        .clone())
}

pub fn spawn_mobs(map: &Map) -> Vec<Mob> {
    let definitions = map
        .mob_definitions
        .iter()
        .map(|definition| (definition.id, definition))
        .collect::<HashMap<_, _>>();
    map.mob_spawn_points
        .iter()
        .filter_map(|spawn| spawn_mob(map.id, spawn, definitions.get(&spawn.mob_id).copied()))
        .collect()
}

fn spawn_mob(
    map_id: u32,
    spawn: &MobSpawnPoint,
    definition: Option<&MobDefinition>,
) -> Option<Mob> {
    let definition = definition.filter(|definition| {
        definition
            .animations
            .iter()
            .any(|animation| !animation.frames.is_empty())
    })?;
    let position = spawn
        .position
        .filter(|position| position.x.is_finite() && position.y.is_finite())?;
    Some(Mob {
        id: format!("{map_id}:{}:0", spawn.spawn_id),
        definition_id: definition.id,
        position: Some(position),
        flip_x: spawn.flip_x,
        layer: spawn.layer,
        current_hp: definition.max_hp.max(1),
        spawn_id: spawn.spawn_id,
    })
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::Map;
    use oozems_proto::v1::MobAnimation;
    use oozems_proto::v1::MobDefinition;
    use oozems_proto::v1::MobFrame;
    use oozems_proto::v1::MobSpawnPoint;
    use oozems_proto::v1::Vec2;

    use super::MobStore;
    use super::map_mobs;
    use super::spawn_mobs;

    #[test]
    fn spawn_points_become_live_mobs_with_full_health() {
        let map = map();

        let mobs = spawn_mobs(&map);

        assert_eq!(mobs.len(), 1);
        assert_eq!(mobs[0].id, "100010000:3:0");
        assert_eq!(mobs[0].definition_id, 100_101);
        assert_eq!(mobs[0].current_hp, 15);
        assert_eq!(mobs[0].position, Some(Vec2 { x: 50.0, y: 200.0 }));
    }

    #[test]
    fn store_initializes_a_map_once() {
        let store = MobStore::default();
        let map = map();

        let first = map_mobs(&store, &map).expect("first lookup");
        let second = map_mobs(&store, &map).expect("second lookup");

        assert_eq!(first, second);
    }

    #[test]
    fn definitions_without_visuals_do_not_spawn() {
        let mut map = map();
        map.mob_definitions[0].animations.clear();

        assert!(spawn_mobs(&map).is_empty());
    }

    fn map() -> Map {
        Map {
            id: 100_010_000,
            mob_spawn_points: vec![MobSpawnPoint {
                spawn_id: 3,
                mob_id: 100_101,
                position: Some(Vec2 { x: 50.0, y: 200.0 }),
                layer: 2,
                ..MobSpawnPoint::default()
            }],
            mob_definitions: vec![MobDefinition {
                id: 100_101,
                max_hp: 15,
                animations: vec![MobAnimation {
                    name: "stand".to_owned(),
                    frames: vec![MobFrame {
                        asset_id: "slime".to_owned(),
                        ..MobFrame::default()
                    }],
                }],
                ..MobDefinition::default()
            }],
            ..Map::default()
        }
    }
}
