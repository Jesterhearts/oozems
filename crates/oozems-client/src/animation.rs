#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Playback {
    Loop,
    Once,
}

pub(crate) fn frame_index(
    delays: impl IntoIterator<Item = u32>,
    elapsed_ms: f64,
    playback: Playback,
) -> Option<usize> {
    frame_index_with(delays, elapsed_ms, playback, |delay| delay.max(1))
}

pub(crate) fn frame_index_exact(
    delays: impl IntoIterator<Item = u32>,
    elapsed_ms: f64,
    playback: Playback,
) -> Option<usize> {
    frame_index_with(delays, elapsed_ms, playback, |delay| delay)
}

fn frame_index_with(
    delays: impl IntoIterator<Item = u32>,
    elapsed_ms: f64,
    playback: Playback,
    normalize: impl Fn(u32) -> u32,
) -> Option<usize> {
    let delays = delays.into_iter().map(normalize).collect::<Vec<_>>();
    let duration_ms = delays.iter().map(|delay| u64::from(*delay)).sum::<u64>();
    if duration_ms == 0 {
        return None;
    }

    let elapsed_ms = elapsed_ms.max(0.0) as u64;
    let mut remaining_ms = match playback {
        Playback::Loop => elapsed_ms % duration_ms,
        Playback::Once if elapsed_ms < duration_ms => elapsed_ms,
        Playback::Once => return None,
    };
    for (index, delay_ms) in delays.into_iter().enumerate() {
        if remaining_ms < u64::from(delay_ms) {
            return Some(index);
        }
        remaining_ms -= u64::from(delay_ms);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::Playback;
    use super::frame_index;
    use super::frame_index_exact;

    #[test]
    fn looping_playback_wraps_at_the_total_duration() {
        let delays = [100, 200];

        assert_eq!(frame_index(delays, 99.0, Playback::Loop), Some(0));
        assert_eq!(frame_index(delays, 100.0, Playback::Loop), Some(1));
        assert_eq!(frame_index(delays, 299.0, Playback::Loop), Some(1));
        assert_eq!(frame_index(delays, 300.0, Playback::Loop), Some(0));
    }

    #[test]
    fn one_shot_playback_has_an_exclusive_end() {
        let delays = [100, 200];

        assert_eq!(frame_index(delays, 299.0, Playback::Once), Some(1));
        assert_eq!(frame_index(delays, 300.0, Playback::Once), None);
    }

    #[test]
    fn zero_delays_still_produce_reachable_frames() {
        assert_eq!(frame_index([0, 0], 0.0, Playback::Loop), Some(0));
        assert_eq!(frame_index([0, 0], 1.0, Playback::Loop), Some(1));
        assert_eq!(frame_index([0, 0], 2.0, Playback::Once), None);
        assert_eq!(frame_index_exact([0, 0], 0.0, Playback::Once), None);
    }
}
