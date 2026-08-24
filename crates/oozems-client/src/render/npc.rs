use std::collections::HashMap;

use oozems_proto::v1::Map;
use oozems_proto::v1::Npc;
use oozems_proto::v1::NpcAnimationEvent;
use oozems_proto::v1::NpcFrame;

use crate::assets;
use crate::game::Game;
use crate::game_gui::CanvasPoint;

#[derive(Default)]
pub(crate) struct NpcAnimationPlaybackState {
    latest_revision: u64,
    playbacks: HashMap<u32, NpcAnimationPlayback>,
}

#[derive(Clone, Debug, PartialEq)]
struct NpcAnimationPlayback {
    action_name: String,
    player_revision: u64,
    started_ms: f64,
}

pub(crate) fn install_event(
    state: &mut NpcAnimationPlaybackState,
    map: &Map,
    event: NpcAnimationEvent,
    response_player_revision: u64,
    current_player_revision: u64,
    started_ms: f64,
) -> Result<bool, String> {
    if event.map_id != map.id {
        return Err(format!(
            "ignored NPC animation for map {}; current map is {}",
            event.map_id, map.id
        ));
    }
    let npc = map
        .npcs
        .iter()
        .find(|npc| npc.spawn_id == event.npc_spawn_id)
        .ok_or_else(|| {
            format!(
                "ignored NPC animation for missing spawn {}",
                event.npc_spawn_id
            )
        })?;
    if event.npc_id != npc.npc_id {
        return Err(format!(
            "ignored NPC animation for NPC {}; spawn {} contains NPC {}",
            event.npc_id, event.npc_spawn_id, npc.npc_id
        ));
    }
    if event.player_revision != response_player_revision {
        return Err(format!(
            "ignored NPC animation revision {}; response player revision is {}",
            event.player_revision, response_player_revision
        ));
    }
    if event.player_revision != current_player_revision {
        return if event.player_revision < current_player_revision {
            Ok(false)
        } else {
            Err(format!(
                "ignored NPC animation revision {}; current player revision is {}",
                event.player_revision, current_player_revision
            ))
        };
    }
    if event.player_revision <= state.latest_revision {
        return Ok(false);
    }
    let animation = npc
        .animations
        .iter()
        .find(|animation| animation.name == event.action_name)
        .ok_or_else(|| {
            format!(
                "NPC {} has no animation named {:?}; using its standing animation",
                npc.npc_id, event.action_name
            )
        })?;
    if animation_duration_ms(&animation.frames) == 0 {
        return Err(format!(
            "NPC {} animation {:?} has no duration; using its standing animation",
            npc.npc_id, event.action_name
        ));
    }

    state.latest_revision = event.player_revision;
    state.playbacks.insert(
        event.npc_spawn_id,
        NpcAnimationPlayback {
            action_name: event.action_name,
            player_revision: event.player_revision,
            started_ms,
        },
    );
    Ok(true)
}

pub(crate) fn clear(state: &mut NpcAnimationPlaybackState) {
    *state = NpcAnimationPlaybackState::default();
}

pub(super) fn draw(
    game: &Game,
    camera_x: f64,
    camera_y: f64,
    layer: i32,
) {
    for npc in &game.map.npcs {
        if npc.layer != layer {
            continue;
        }
        let Some(position) = &npc.position else {
            continue;
        };
        let Some(frame) = ready_frame(game, npc) else {
            continue;
        };
        if !super::sprite_is_visible(
            game,
            frame_x(position.x, frame, npc.flip_x),
            position.y - frame.origin_y,
            frame.width,
            frame.height,
            camera_x,
            camera_y,
        ) {
            continue;
        }
        super::draw_sprite(
            game,
            &frame.asset_id,
            frame_x(position.x, frame, npc.flip_x),
            position.y - frame.origin_y,
            frame.width,
            frame.height,
            npc.flip_x,
            camera_x,
            camera_y,
        );
    }
}

pub(super) fn at_point(
    game: &Game,
    point: CanvasPoint,
    camera_x: f64,
    camera_y: f64,
) -> Option<u32> {
    game.world_layers.iter().rev().find_map(|layer| {
        game.map.npcs.iter().rev().find_map(|npc| {
            if npc.layer != *layer {
                return None;
            }
            let position = npc.position.as_ref()?;
            let frame = ready_frame(game, npc)?;
            let left = f64::from(frame_x(position.x, frame, npc.flip_x)) - camera_x;
            let top = f64::from(position.y - frame.origin_y) - camera_y;
            point_in_frame(
                point,
                left,
                top,
                f64::from(frame.width),
                f64::from(frame.height),
            )
            .then_some(npc.spawn_id)
        })
    })
}

fn ready_frame<'a>(
    game: &'a Game,
    npc: &'a Npc,
) -> Option<&'a NpcFrame> {
    let standing_frames = standing_frames(npc)?;
    let sequence = frame_sequence(npc, &game.npc_animations, game.frame_time_ms);
    let preferred_index = if sequence.one_shot {
        one_shot_frame_index(sequence.frames, sequence.elapsed_ms)
    } else {
        animation_frame_index(sequence.frames, sequence.elapsed_ms)
    }?;
    if let Some(index) = assets::ready_or_fallback_index(
        &game.images,
        sequence.frames.iter().map(|frame| frame.asset_id.as_str()),
        preferred_index,
    ) {
        return sequence.frames.get(index);
    }
    if sequence.one_shot {
        let default_index = animation_frame_index(standing_frames, game.frame_time_ms)?;
        let index = assets::ready_or_fallback_index(
            &game.images,
            standing_frames.iter().map(|frame| frame.asset_id.as_str()),
            default_index,
        )?;
        return standing_frames.get(index);
    }
    None
}

pub(super) fn standing_frames(npc: &Npc) -> Option<&[NpcFrame]> {
    npc.animations
        .iter()
        .find(|animation| animation.name == "stand" && !animation.frames.is_empty())
        .or_else(|| {
            npc.animations
                .iter()
                .find(|animation| !animation.frames.is_empty())
        })
        .map(|animation| animation.frames.as_slice())
}

#[derive(Clone, Copy)]
struct FrameSequence<'a> {
    frames: &'a [NpcFrame],
    elapsed_ms: f64,
    one_shot: bool,
}

fn frame_sequence<'a>(
    npc: &'a Npc,
    state: &NpcAnimationPlaybackState,
    timestamp_ms: f64,
) -> FrameSequence<'a> {
    let named = state
        .playbacks
        .get(&npc.spawn_id)
        .and_then(|playback| {
            npc.animations
                .iter()
                .find(|animation| animation.name == playback.action_name)
                .map(|animation| (playback, animation))
        })
        .filter(|(playback, animation)| {
            let elapsed_ms = (timestamp_ms - playback.started_ms).max(0.0);
            elapsed_ms < animation_duration_ms(&animation.frames) as f64
        });
    match named {
        Some((playback, animation)) => FrameSequence {
            frames: &animation.frames,
            elapsed_ms: (timestamp_ms - playback.started_ms).max(0.0),
            one_shot: true,
        },
        None => FrameSequence {
            frames: standing_frames(npc).unwrap_or(&[]),
            elapsed_ms: timestamp_ms,
            one_shot: false,
        },
    }
}

fn point_in_frame(
    point: CanvasPoint,
    left: f64,
    top: f64,
    width: f64,
    height: f64,
) -> bool {
    f64::from(point.x) >= left
        && f64::from(point.x) <= left + width
        && f64::from(point.y) >= top
        && f64::from(point.y) <= top + height
}

fn animation_frame_index(
    frames: &[NpcFrame],
    timestamp_ms: f64,
) -> Option<usize> {
    super::timed_frame_index(frames.iter().map(|frame| frame.delay_ms), timestamp_ms)
}

fn one_shot_frame_index(
    frames: &[NpcFrame],
    elapsed_ms: f64,
) -> Option<usize> {
    let mut remaining = elapsed_ms.max(0.0) as u64;
    if remaining >= animation_duration_ms(frames) {
        return None;
    }
    for (index, frame) in frames.iter().enumerate() {
        let delay = u64::from(frame.delay_ms);
        if remaining < delay {
            return Some(index);
        }
        remaining = remaining.saturating_sub(delay);
    }
    None
}

fn animation_duration_ms(frames: &[NpcFrame]) -> u64 {
    frames.iter().map(|frame| u64::from(frame.delay_ms)).sum()
}

fn frame_x(
    anchor_x: f32,
    frame: &NpcFrame,
    flip_x: bool,
) -> f32 {
    if flip_x {
        anchor_x - (frame.width - frame.origin_x)
    } else {
        anchor_x - frame.origin_x
    }
}

#[cfg(test)]
mod tests {
    use oozems_proto::v1::Map;
    use oozems_proto::v1::Npc;
    use oozems_proto::v1::NpcAnimation;
    use oozems_proto::v1::NpcAnimationEvent;
    use oozems_proto::v1::NpcFrame;

    use super::NpcAnimationPlaybackState;
    use super::animation_frame_index;
    use super::clear;
    use super::frame_sequence;
    use super::frame_x;
    use super::install_event;
    use super::one_shot_frame_index;
    use super::point_in_frame;
    use super::standing_frames;
    use crate::game_gui::CanvasPoint;

    #[test]
    fn npc_animation_uses_frame_delays() {
        let frames = vec![
            NpcFrame {
                asset_id: "first".to_owned(),
                delay_ms: 100,
                ..NpcFrame::default()
            },
            NpcFrame {
                asset_id: "second".to_owned(),
                delay_ms: 200,
                ..NpcFrame::default()
            },
        ];

        assert_eq!(
            frames[animation_frame_index(&frames, 99.0).expect("frame")].asset_id,
            "first"
        );
        assert_eq!(
            frames[animation_frame_index(&frames, 100.0).expect("frame")].asset_id,
            "second"
        );
        assert_eq!(
            frames[animation_frame_index(&frames, 300.0).expect("frame")].asset_id,
            "first"
        );
    }

    #[test]
    fn flipping_keeps_the_frame_origin_on_the_npc_anchor() {
        let frame = NpcFrame {
            width: 40.0,
            origin_x: 15.0,
            ..NpcFrame::default()
        };

        assert_eq!(frame_x(100.0, &frame, false), 85.0);
        assert_eq!(frame_x(100.0, &frame, true), 75.0);
    }

    #[test]
    fn npc_frame_bounds_include_their_edges() {
        assert!(point_in_frame(
            CanvasPoint { x: 10.0, y: 20.0 },
            10.0,
            20.0,
            30.0,
            40.0,
        ));
        assert!(!point_in_frame(
            CanvasPoint { x: 41.0, y: 20.0 },
            10.0,
            20.0,
            30.0,
            40.0,
        ));
    }

    #[test]
    fn npc_animation_events_require_the_current_map_spawn_and_npc() {
        let map = animation_map();
        for event in [
            NpcAnimationEvent {
                map_id: 2,
                ..animation_event(5)
            },
            NpcAnimationEvent {
                npc_spawn_id: 8,
                ..animation_event(5)
            },
            NpcAnimationEvent {
                npc_id: 20,
                ..animation_event(5)
            },
        ] {
            assert!(
                install_event(
                    &mut NpcAnimationPlaybackState::default(),
                    &map,
                    event,
                    5,
                    5,
                    100.0,
                )
                .is_err()
            );
        }
    }

    #[test]
    fn stale_and_duplicate_npc_animation_events_do_not_restart_playback() {
        let map = animation_map();
        let mut state = NpcAnimationPlaybackState::default();
        assert!(install_event(&mut state, &map, animation_event(5), 6, 6, 50.0).is_err());
        assert!(
            install_event(&mut state, &map, animation_event(5), 5, 5, 100.0).expect("first event")
        );
        assert!(
            !install_event(&mut state, &map, animation_event(5), 5, 5, 200.0)
                .expect("duplicate event")
        );
        assert!(
            !install_event(&mut state, &map, animation_event(4), 4, 5, 300.0).expect("stale event")
        );
        let playback = state.playbacks.get(&7).expect("active playback");
        assert_eq!(playback.player_revision, 5);
        assert_eq!(playback.started_ms, 100.0);

        assert!(
            install_event(&mut state, &map, animation_event(6), 6, 6, 400.0).expect("new event")
        );
        assert_eq!(state.playbacks[&7].started_ms, 400.0);
    }

    #[test]
    fn named_npc_animation_plays_once_then_uses_stand_animation() {
        let map = animation_map();
        let npc = &map.npcs[0];
        let mut state = NpcAnimationPlaybackState::default();
        install_event(&mut state, &map, animation_event(5), 5, 5, 100.0).expect("animation event");

        let first = frame_sequence(npc, &state, 199.0);
        assert!(first.one_shot);
        assert_eq!(
            one_shot_frame_index(first.frames, first.elapsed_ms),
            Some(0)
        );
        let second = frame_sequence(npc, &state, 200.0);
        assert_eq!(
            one_shot_frame_index(second.frames, second.elapsed_ms),
            Some(1)
        );
        let final_named = frame_sequence(npc, &state, 399.0);
        assert!(final_named.one_shot);

        let fallback = frame_sequence(npc, &state, 400.0);
        assert!(!fallback.one_shot);
        assert_eq!(fallback.frames[0].asset_id, "stand");

        clear(&mut state);
        assert!(state.playbacks.is_empty());
        assert_eq!(state.latest_revision, 0);
    }

    #[test]
    fn standing_frames_prefer_stand_then_the_first_nonempty_animation() {
        let mut npc = animation_map().npcs.remove(0);

        assert_eq!(
            standing_frames(&npc).expect("stand frames")[0].asset_id,
            "stand"
        );

        npc.animations
            .iter_mut()
            .find(|animation| animation.name == "stand")
            .expect("stand animation")
            .frames
            .clear();
        assert_eq!(
            standing_frames(&npc).expect("fallback frames")[0].asset_id,
            "named-1"
        );
    }

    #[test]
    fn missing_or_zero_duration_named_animation_keeps_stand_animation() {
        let mut map = animation_map();
        let mut state = NpcAnimationPlaybackState::default();
        let mut missing = animation_event(5);
        missing.action_name = "missing".to_owned();
        assert!(install_event(&mut state, &map, missing, 5, 5, 100.0).is_err());
        assert!(!frame_sequence(&map.npcs[0], &state, 100.0).one_shot);

        map.npcs[0].animations[0]
            .frames
            .iter_mut()
            .for_each(|frame| frame.delay_ms = 0);
        assert!(install_event(&mut state, &map, animation_event(5), 5, 5, 100.0).is_err());
        assert!(!frame_sequence(&map.npcs[0], &state, 100.0).one_shot);
    }

    #[test]
    fn active_named_frame_controls_flipped_hit_bounds() {
        let map = animation_map();
        let npc = &map.npcs[0];
        let mut state = NpcAnimationPlaybackState::default();
        install_event(&mut state, &map, animation_event(5), 5, 5, 100.0).expect("animation event");
        let sequence = frame_sequence(npc, &state, 100.0);
        let frame = &sequence.frames
            [one_shot_frame_index(sequence.frames, sequence.elapsed_ms).expect("named frame")];
        let left = f64::from(frame_x(100.0, frame, true));

        assert_eq!(left, 85.0);
        assert!(point_in_frame(
            CanvasPoint { x: 85.0, y: 80.0 },
            left,
            80.0,
            f64::from(frame.width),
            f64::from(frame.height),
        ));
        assert!(!point_in_frame(
            CanvasPoint { x: 84.0, y: 80.0 },
            left,
            80.0,
            f64::from(frame.width),
            f64::from(frame.height),
        ));
    }

    fn animation_map() -> Map {
        Map {
            id: 1,
            npcs: vec![Npc {
                spawn_id: 7,
                npc_id: 10,
                flip_x: true,
                animations: vec![
                    NpcAnimation {
                        name: "quest".to_owned(),
                        frames: vec![
                            NpcFrame {
                                asset_id: "named-1".to_owned(),
                                width: 20.0,
                                height: 30.0,
                                origin_x: 5.0,
                                origin_y: 20.0,
                                delay_ms: 100,
                            },
                            NpcFrame {
                                asset_id: "named-2".to_owned(),
                                delay_ms: 200,
                                ..NpcFrame::default()
                            },
                        ],
                    },
                    NpcAnimation {
                        name: "stand".to_owned(),
                        frames: vec![NpcFrame {
                            asset_id: "stand".to_owned(),
                            width: 10.0,
                            height: 10.0,
                            delay_ms: 50,
                            ..NpcFrame::default()
                        }],
                    },
                ],
                ..Npc::default()
            }],
            ..Map::default()
        }
    }

    fn animation_event(player_revision: u64) -> NpcAnimationEvent {
        NpcAnimationEvent {
            map_id: 1,
            npc_spawn_id: 7,
            npc_id: 10,
            action_name: "quest".to_owned(),
            player_revision,
        }
    }
}
