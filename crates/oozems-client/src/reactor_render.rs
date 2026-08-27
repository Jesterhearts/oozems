use std::collections::HashMap;

use oozems_proto::v1::Reactor;
use oozems_proto::v1::ReactorDefinition;
use oozems_proto::v1::ReactorFrame;

#[derive(Default)]
pub(crate) struct ReactorRenderState {
    transitions: HashMap<u32, ReactorTransition>,
}

#[derive(Clone, Copy)]
struct ReactorTransition {
    from_state: u32,
    to_state: u32,
    started_at_ms: f64,
}

pub(crate) fn install_snapshot(
    state: &mut ReactorRenderState,
    reactors: &mut Vec<Reactor>,
    next_reactors: Vec<Reactor>,
    timestamp_ms: f64,
) {
    state.transitions.retain(|spawn_id, transition| {
        next_reactors
            .iter()
            .any(|reactor| reactor.spawn_id == *spawn_id && reactor.state == transition.to_state)
    });
    for next in &next_reactors {
        let Some(current) = reactors
            .iter()
            .find(|current| current.spawn_id == next.spawn_id)
        else {
            continue;
        };
        if current.active && current.state != next.state {
            state.transitions.insert(
                next.spawn_id,
                ReactorTransition {
                    from_state: current.state,
                    to_state: next.state,
                    started_at_ms: timestamp_ms,
                },
            );
        } else if !current.active && next.active {
            state.transitions.remove(&next.spawn_id);
        }
    }
    *reactors = next_reactors;
}

pub(crate) fn frame_selection<'a>(
    state: &ReactorRenderState,
    reactor: &Reactor,
    definition: &'a ReactorDefinition,
    timestamp_ms: f64,
) -> Option<(&'a [ReactorFrame], usize)> {
    if let Some(transition) = state.transitions.get(&reactor.spawn_id)
        && transition.to_state == reactor.state
        && let Some(from_state) = definition
            .states
            .iter()
            .find(|state| state.state == transition.from_state)
    {
        let elapsed_ms = (timestamp_ms - transition.started_at_ms).max(0.0);
        if let Some(index) = crate::animation::frame_index(
            from_state.hit_frames.iter().map(|frame| frame.delay_ms),
            elapsed_ms,
            crate::animation::Playback::Once,
        ) {
            return Some((&from_state.hit_frames, index));
        }
    }
    if !reactor.active {
        return None;
    }
    let current = definition
        .states
        .iter()
        .find(|state| state.state == reactor.state)?;
    let index = crate::animation::frame_index(
        current.frames.iter().map(|frame| frame.delay_ms),
        timestamp_ms,
        crate::animation::Playback::Loop,
    )?;
    Some((&current.frames, index))
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::Reactor;
    use oozems_proto::v1::ReactorDefinition;
    use oozems_proto::v1::ReactorFrame;
    use oozems_proto::v1::ReactorStateDefinition;

    use super::ReactorRenderState;
    use super::frame_selection;
    use super::install_snapshot;

    #[test]
    fn a_state_change_plays_hit_frames_before_the_next_state() {
        let definition = definition();
        let mut state = ReactorRenderState::default();
        let mut reactors = vec![reactor(0, true)];

        install_snapshot(&mut state, &mut reactors, vec![reactor(1, true)], 1_000.0);

        let (frames, index) =
            frame_selection(&state, &reactors[0], &definition, 1_150.0).expect("hit frame");
        assert_eq!(frames[index].asset_id, "hit-1");
        let (frames, index) =
            frame_selection(&state, &reactors[0], &definition, 1_300.0).expect("state frame");
        assert_eq!(frames[index].asset_id, "state-1");
    }

    #[test]
    fn destruction_finishes_the_hit_animation_then_hides_the_reactor() {
        let definition = definition();
        let mut state = ReactorRenderState::default();
        let mut reactors = vec![reactor(0, true)];

        install_snapshot(&mut state, &mut reactors, vec![reactor(1, false)], 1_000.0);

        assert!(frame_selection(&state, &reactors[0], &definition, 1_100.0).is_some());
        assert!(frame_selection(&state, &reactors[0], &definition, 1_300.0).is_none());
    }

    fn reactor(
        state: u32,
        active: bool,
    ) -> Reactor {
        Reactor {
            spawn_id: 7,
            state,
            active,
            ..Reactor::default()
        }
    }

    fn definition() -> ReactorDefinition {
        ReactorDefinition {
            states: vec![
                ReactorStateDefinition {
                    state: 0,
                    frames: vec![frame("state-0", 100)],
                    hit_frames: vec![frame("hit-0", 150), frame("hit-1", 150)],
                    next_state: Some(1),
                },
                ReactorStateDefinition {
                    state: 1,
                    frames: vec![frame("state-1", 100)],
                    ..ReactorStateDefinition::default()
                },
            ],
            ..ReactorDefinition::default()
        }
    }

    fn frame(
        asset_id: &str,
        delay_ms: u32,
    ) -> ReactorFrame {
        ReactorFrame {
            asset_id: asset_id.to_owned(),
            delay_ms,
            ..ReactorFrame::default()
        }
    }
}
