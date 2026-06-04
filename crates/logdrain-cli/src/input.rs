//! Extracting a log message from each input line.

use serde_json::Value;

/// Extract a dot-path field from a JSON line. Returns `None` if the line is not
/// valid JSON or the path is absent. String values are returned unquoted; other
/// JSON scalars/containers are returned in their compact JSON form.
pub fn extract_field(line: &str, path: &str) -> Option<String> {
    let root: Value = serde_json::from_str(line).ok()?;
    let mut cur = &root;
    for key in path.split('.') {
        cur = cur.get(key)?;
    }
    match cur {
        Value::String(s) => Some(s.clone()),
        Value::Null => None,
        other => Some(other.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_top_level_string() {
        let line = r#"{"message":"hello world","level":"info"}"#;
        assert_eq!(
            extract_field(line, "message").as_deref(),
            Some("hello world")
        );
    }

    #[test]
    fn extracts_nested_dot_path() {
        let line = r#"{"event":{"type":"login","user":{"id":42}}}"#;
        assert_eq!(extract_field(line, "event.type").as_deref(), Some("login"));
        assert_eq!(extract_field(line, "event.user.id").as_deref(), Some("42"));
    }

    #[test]
    fn missing_field_is_none() {
        let line = r#"{"message":"x"}"#;
        assert_eq!(extract_field(line, "nope"), None);
        assert_eq!(extract_field(line, "a.b.c"), None);
    }

    #[test]
    fn non_json_is_none() {
        assert_eq!(extract_field("just a plain log line", "message"), None);
    }
}
