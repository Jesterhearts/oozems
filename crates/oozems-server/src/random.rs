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
    use super::next_u64;

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
