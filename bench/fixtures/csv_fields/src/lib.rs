//! RFC 4180-style single-record CSV field parsing with quoting.

/// Parse one CSV record (a single line, no embedded newlines) into
/// fields.
///
/// Rules:
/// - fields are separated by commas
/// - a field wrapped in double quotes may contain commas and escaped
///   quotes (`""` inside a quoted field is a literal `"`)
/// - unquoted fields are taken verbatim (no trimming)
/// - an empty input line yields one empty field
///
/// Returns `Err` with a message when quoting is malformed (an unterminated
/// quoted field, or a quote appearing after a quoted field's closing
/// quote without a comma following it).
pub fn parse_record(line: &str) -> Result<Vec<String>, String> {
    todo!("Step 1")
}

/// Parse a record and re-serialize it canonically: every field that
/// contains a comma, a quote, or needs quoting is quoted, inner quotes
/// doubled. Round-trips through `parse_record`.
pub fn encode_record(fields: &[String]) -> String {
    todo!("Step 2")
}
