//! The visible and reportable identity of this build.

/// Stable across retries of the same source; the short commit ID distinguishes
/// different builds that intentionally keep the same semantic version and codename.
pub(crate) const LABEL: &str = concat!(
    "ALPHA · ",
    env!("CARGO_PKG_VERSION"),
    " ",
    env!("GLUE5_VERSION_CODENAME"),
    " · ",
    env!("GLUE5_BUILD_ID"),
);

#[cfg(test)]
mod tests {
    use super::LABEL;

    #[test]
    fn build_label_contains_version_codename_and_build_id() {
        assert!(LABEL.starts_with(concat!(
            "ALPHA · ",
            env!("CARGO_PKG_VERSION"),
            " ",
            env!("GLUE5_VERSION_CODENAME"),
            " · "
        )));
        assert!(LABEL.ends_with(env!("GLUE5_BUILD_ID")));
    }
}
