pub mod auth;
pub mod evidence;
pub mod provider;
pub mod script;

mod text {
    pub(crate) fn truncate(
        value: &str,
        maximum_chars: usize,
    ) -> String {
        value.chars().take(maximum_chars).collect()
    }
}
