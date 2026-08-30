use oozems_proto::v1::DroppedItem;
use oozems_proto::v1::Vec2;

const DROPPED_ITEM_BOB_HEIGHT: f64 = 4.0;
const DROPPED_ITEM_BOB_PERIOD_MS: f64 = 1_400.0;
const PICKUP_FLIGHT_DURATION_MS: f64 = 420.0;
const PICKUP_FLIGHT_HEIGHT: f32 = 24.0;
const PICKUP_TARGET_HEIGHT: f32 = 28.0;

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PickupAnimation {
    drop_id: String,
    pub(crate) item_id: u32,
    pub(crate) start: Option<Vec2>,
    started_ms: f64,
    reconciled: bool,
}

pub(crate) fn start(
    dropped_items: &mut Vec<DroppedItem>,
    animations: &mut Vec<PickupAnimation>,
    drop_id: &str,
    timestamp_ms: f64,
) {
    let drop = dropped_items
        .iter()
        .position(|drop| drop.id == drop_id)
        .map(|index| dropped_items.remove(index));
    let item_id = drop.as_ref().map_or(0, |drop| drop.item_id);
    let start = drop.and_then(|drop| drop.position).map(|mut position| {
        position.y += idle_offset(timestamp_ms);
        position
    });
    animations.retain(|animation| animation.drop_id != drop_id);
    animations.push(PickupAnimation {
        drop_id: drop_id.to_owned(),
        item_id,
        start,
        started_ms: timestamp_ms,
        reconciled: false,
    });
}

pub(crate) fn reconcile_snapshot(
    mut dropped_items: Vec<DroppedItem>,
    animations: &mut [PickupAnimation],
) -> Vec<DroppedItem> {
    for animation in animations {
        if dropped_items
            .iter()
            .any(|drop| drop.id == animation.drop_id)
        {
            dropped_items.retain(|drop| drop.id != animation.drop_id);
        } else {
            animation.reconciled = true;
        }
    }
    dropped_items
}

pub(crate) fn update(
    animations: &mut Vec<PickupAnimation>,
    timestamp_ms: f64,
) {
    animations.retain(|animation| {
        elapsed_ms(animation, timestamp_ms) < PICKUP_FLIGHT_DURATION_MS || !animation.reconciled
    });
}

pub(crate) fn flight_position(
    animation: &PickupAnimation,
    target: Vec2,
    timestamp_ms: f64,
) -> Option<Vec2> {
    let elapsed_ms = elapsed_ms(animation, timestamp_ms);
    if elapsed_ms >= PICKUP_FLIGHT_DURATION_MS {
        return None;
    }
    let start = animation.start?;
    let progress = (elapsed_ms / PICKUP_FLIGHT_DURATION_MS).clamp(0.0, 1.0) as f32;
    let movement = progress * progress * (3.0 - 2.0 * progress);
    let lift = (std::f32::consts::PI * progress).sin() * PICKUP_FLIGHT_HEIGHT;
    Some(Vec2 {
        x: start.x + (target.x - start.x) * movement,
        y: start.y + (target.y - PICKUP_TARGET_HEIGHT - start.y) * movement - lift,
    })
}

pub(crate) fn idle_offset(timestamp_ms: f64) -> f32 {
    let phase = timestamp_ms.rem_euclid(DROPPED_ITEM_BOB_PERIOD_MS) / DROPPED_ITEM_BOB_PERIOD_MS
        * std::f64::consts::TAU;
    ((phase.cos() - 1.0) * DROPPED_ITEM_BOB_HEIGHT / 2.0) as f32
}

fn elapsed_ms(
    animation: &PickupAnimation,
    timestamp_ms: f64,
) -> f64 {
    (timestamp_ms - animation.started_ms).max(0.0)
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::DroppedItem;
    use oozems_proto::v1::Vec2;

    use super::PICKUP_FLIGHT_DURATION_MS;
    use super::PickupAnimation;
    use super::flight_position;
    use super::idle_offset;
    use super::reconcile_snapshot;
    use super::start;
    use super::update;

    #[test]
    fn pickup_moves_the_confirmed_drop_into_animation_state() {
        let mut dropped_items = vec![
            DroppedItem {
                id: "other".to_owned(),
                item_id: 1,
                position: Some(Vec2 { x: 10.0, y: 20.0 }),
                ..DroppedItem::default()
            },
            DroppedItem {
                id: "picked".to_owned(),
                item_id: 2,
                position: Some(Vec2 { x: 30.0, y: 40.0 }),
                ..DroppedItem::default()
            },
        ];
        let mut animations = Vec::new();

        start(&mut dropped_items, &mut animations, "picked", 0.0);

        assert_eq!(dropped_items.len(), 1);
        assert_eq!(dropped_items[0].id, "other");
        assert_eq!(
            animations,
            vec![PickupAnimation {
                drop_id: "picked".to_owned(),
                item_id: 2,
                start: Some(Vec2 { x: 30.0, y: 40.0 }),
                started_ms: 0.0,
                reconciled: false,
            }]
        );
    }

    #[test]
    fn pickup_flight_starts_at_the_items_current_bob_height() {
        let mut dropped_items = vec![DroppedItem {
            id: "picked".to_owned(),
            item_id: 2,
            position: Some(Vec2 { x: 30.0, y: 40.0 }),
            ..DroppedItem::default()
        }];
        let mut animations = Vec::new();

        start(&mut dropped_items, &mut animations, "picked", 700.0);

        assert_eq!(animations[0].start, Some(Vec2 { x: 30.0, y: 36.0 }));
    }

    #[test]
    fn pickup_flies_up_then_down_to_the_character() {
        let animation = PickupAnimation {
            drop_id: "picked".to_owned(),
            item_id: 2,
            start: Some(Vec2 { x: 0.0, y: 100.0 }),
            started_ms: 100.0,
            reconciled: false,
        };
        let target = Vec2 { x: 100.0, y: 100.0 };

        assert_eq!(flight_position(&animation, target, 100.0), animation.start);
        assert_eq!(
            flight_position(&animation, target, 100.0 + PICKUP_FLIGHT_DURATION_MS / 2.0),
            Some(Vec2 { x: 50.0, y: 62.0 })
        );
        assert_eq!(
            flight_position(&animation, target, 100.0 + PICKUP_FLIGHT_DURATION_MS),
            None
        );
    }

    #[test]
    fn stale_snapshots_cannot_restore_a_confirmed_pickup() {
        let mut animations = vec![PickupAnimation {
            drop_id: "picked".to_owned(),
            item_id: 2,
            start: Some(Vec2::default()),
            started_ms: 100.0,
            reconciled: false,
        }];
        let stale_snapshot = vec![DroppedItem {
            id: "picked".to_owned(),
            ..DroppedItem::default()
        }];

        assert!(reconcile_snapshot(stale_snapshot, &mut animations).is_empty());
        assert!(!animations[0].reconciled);

        assert!(reconcile_snapshot(Vec::new(), &mut animations).is_empty());
        assert!(animations[0].reconciled);
    }

    #[test]
    fn pickup_state_expires_after_the_flight_and_snapshot_reconciliation() {
        let animation = PickupAnimation {
            drop_id: "picked".to_owned(),
            item_id: 2,
            start: Some(Vec2::default()),
            started_ms: 100.0,
            reconciled: false,
        };
        let mut animations = vec![animation.clone()];

        update(&mut animations, 100.0 + PICKUP_FLIGHT_DURATION_MS);
        assert_eq!(animations, vec![animation]);

        animations[0].reconciled = true;
        update(&mut animations, 100.0 + PICKUP_FLIGHT_DURATION_MS);
        assert!(animations.is_empty());
    }

    #[test]
    fn idle_items_float_up_and_return_to_their_resting_height() {
        assert_eq!(idle_offset(0.0), 0.0);
        assert_eq!(idle_offset(350.0), -2.0);
        assert_eq!(idle_offset(700.0), -4.0);
        assert_eq!(idle_offset(1_050.0), -2.0);
        assert_eq!(idle_offset(1_400.0), 0.0);
    }
}
