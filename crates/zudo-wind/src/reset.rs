use crate::ResetMode;

/// Original reset bytes from spec 1, revision 2, before layer indentation.
pub fn reset_css(mode: ResetMode) -> &'static str {
    match mode {
        ResetMode::None => include_str!("../assets/reset/none.css"),
        ResetMode::MinimalV1 => include_str!("../assets/reset/minimal-v1.css"),
        ResetMode::OwnedV1 => include_str!("../assets/reset/owned-v1.css"),
    }
}
