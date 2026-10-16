//! URL slug generation.

/// Convert a title to a URL slug.
///
/// Rules:
/// - lowercase everything ASCII
/// - runs of non-alphanumeric characters become a single '-'
/// - no leading or trailing '-'
/// - empty/blank input yields an empty string
pub fn slugify(title: &str) -> String {
    todo!("Step 1")
}
