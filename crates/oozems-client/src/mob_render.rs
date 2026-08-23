use std::collections::HashMap;

use oozems_proto::v1::Mob;
use oozems_proto::v1::Vec2;

#[derive(Default)]
pub struct MobRenderState {
    transitions: HashMap<String, MobTransition>,
}

#[derive(Clone, Copy)]
struct MobTransition {
    from: Vec2,
    to: Vec2,
    started_at_ms: f64,
    duration_ms: f64,
}

pub fn install_snapshot(
    state: &mut MobRenderState,
    mobs: &mut Vec<Mob>,
    next_mobs: Vec<Mob>,
    timestamp_ms: f64,
    duration_ms: u64,
) {
    let transitions = next_mobs
        .iter()
        .filter_map(|next| {
            let to = finite_position(next)?;
            let from = mobs
                .iter()
                .find(|current| current.id == next.id)
                .and_then(|current| position(state, current, timestamp_ms))
                .unwrap_or(to);
            Some((
                next.id.clone(),
                MobTransition {
                    from,
                    to,
                    started_at_ms: timestamp_ms,
                    duration_ms: duration_ms.max(1) as f64,
                },
            ))
        })
        .collect();
    *mobs = next_mobs;
    state.transitions = transitions;
}

pub fn position(
    state: &MobRenderState,
    mob: &Mob,
    timestamp_ms: f64,
) -> Option<Vec2> {
    let fallback = finite_position(mob)?;
    let Some(transition) = state.transitions.get(&mob.id) else {
        return Some(fallback);
    };
    let progress =
        ((timestamp_ms - transition.started_at_ms) / transition.duration_ms).clamp(0.0, 1.0) as f32;
    Some(Vec2 {
        x: transition.from.x + (transition.to.x - transition.from.x) * progress,
        y: transition.from.y + (transition.to.y - transition.from.y) * progress,
    })
}

fn finite_position(mob: &Mob) -> Option<Vec2> {
    mob.position
        .filter(|position| position.x.is_finite() && position.y.is_finite())
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::Mob;
    use oozems_proto::v1::Vec2;

    use super::MobRenderState;
    use super::install_snapshot;
    use super::position;

    #[test]
    fn authoritative_snapshots_are_interpolated_over_the_poll_interval() {
        let mut state = MobRenderState::default();
        let mut mobs = vec![mob("slime", 100.0, 300.0)];

        install_snapshot(
            &mut state,
            &mut mobs,
            vec![mob("slime", 120.0, 280.0)],
            1_000.0,
            200,
        );

        assert_eq!(
            position(&state, &mobs[0], 1_000.0),
            Some(Vec2 { x: 100.0, y: 300.0 })
        );
        assert_eq!(
            position(&state, &mobs[0], 1_100.0),
            Some(Vec2 { x: 110.0, y: 290.0 })
        );
        assert_eq!(
            position(&state, &mobs[0], 1_300.0),
            Some(Vec2 { x: 120.0, y: 280.0 })
        );
    }

    #[test]
    fn a_new_snapshot_continues_from_the_current_rendered_position() {
        let mut state = MobRenderState::default();
        let mut mobs = vec![mob("slime", 100.0, 300.0)];
        install_snapshot(
            &mut state,
            &mut mobs,
            vec![mob("slime", 120.0, 300.0)],
            1_000.0,
            200,
        );

        install_snapshot(
            &mut state,
            &mut mobs,
            vec![mob("slime", 140.0, 300.0)],
            1_100.0,
            200,
        );

        assert_eq!(
            position(&state, &mobs[0], 1_100.0),
            Some(Vec2 { x: 110.0, y: 300.0 })
        );
    }

    fn mob(
        id: &str,
        x: f32,
        y: f32,
    ) -> Mob {
        Mob {
            id: id.to_owned(),
            position: Some(Vec2 { x, y }),
            ..Mob::default()
        }
    }
}
