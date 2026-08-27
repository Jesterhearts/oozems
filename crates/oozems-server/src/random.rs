pub fn map_spawn_seed(
    map_id: u32,
    spawn_id: u32,
) -> u64 {
    let seed = (u64::from(map_id) << 32) | u64::from(spawn_id);
    seed ^ 0x9e37_79b9_7f4a_7c15
}

pub fn next_u64(state: &mut u64) -> u64 {
    let mut value = *state;
    value ^= value << 13;
    value ^= value >> 7;
    value ^= value << 17;
    *state = value;
    value
}

#[cfg(test)]
mod tests {
    use super::map_spawn_seed;
    use super::next_u64;

    #[test]
    fn map_spawn_seeds_are_stable_and_distinct() {
        assert_eq!(map_spawn_seed(1, 2), map_spawn_seed(1, 2));
        assert_ne!(map_spawn_seed(1, 2), map_spawn_seed(1, 3));
        assert_ne!(map_spawn_seed(1, 2), map_spawn_seed(2, 2));
    }

    #[test]
    fn xorshift_sequence_is_deterministic_and_updates_state() {
        let mut state = 1;

        for expected in [
            1_082_269_761,
            1_152_992_998_833_853_505,
            11_177_516_664_432_764_457,
            17_678_023_832_001_937_445,
            9_659_130_143_999_365_733,
        ] {
            assert_eq!(next_u64(&mut state), expected);
            assert_eq!(state, expected);
        }
    }

    #[test]
    fn zero_is_the_xorshift_fixed_point() {
        let mut state = 0;

        assert_eq!(next_u64(&mut state), 0);
        assert_eq!(state, 0);
    }
}
