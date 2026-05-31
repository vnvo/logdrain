//! Regex pre-mask pipeline + built-in masks. Masks run before tokenization and
//! replace matched spans with a named placeholder (e.g. `<uuid>`).

use std::borrow::Cow;
use std::sync::Arc;

use regex::Regex;

use crate::error::LogdrainError;

/// A single pre-tokenization mask: a regex whose matches are replaced by a
/// named placeholder.
#[derive(Debug, Clone)]
pub struct Mask {
    /// The compiled pattern.
    pub pattern: Regex,
    /// The replacement placeholder (e.g. `<uuid>`).
    pub placeholder: Arc<str>,
}

impl Mask {
    /// Compile a custom mask from a pattern string and placeholder.
    pub fn new(pattern: &str, placeholder: &str) -> Result<Mask, LogdrainError> {
        Ok(Mask {
            pattern: Regex::new(pattern)?,
            placeholder: Arc::from(placeholder),
        })
    }
}

/// Apply masks left-to-right to `line`, replacing each pattern's matches with its
/// placeholder. Returns a borrowed `Cow` when no mask matched anything.
pub(crate) fn apply_masks<'a>(line: &'a str, masks: &[Mask]) -> Cow<'a, str> {
    let mut cur: Cow<'a, str> = Cow::Borrowed(line);
    for m in masks {
        // `replace_all` borrows `cur`; an owned result means it matched.
        if let Cow::Owned(s) = m.pattern.replace_all(&cur, m.placeholder.as_ref()) {
            cur = Cow::Owned(s);
        }
    }
    cur
}

/// Built-in masks for common high-cardinality tokens.
pub mod builtin_masks {
    use super::Mask;

    fn mask(pattern: &str, placeholder: &str) -> Mask {
        Mask::new(pattern, placeholder).expect("built-in mask pattern is valid")
    }

    /// RFC-4122 UUID -> `<uuid>`.
    pub fn uuid() -> Mask {
        mask(
            r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b",
            "<uuid>",
        )
    }

    /// 32-character hex string -> `<hex32>`.
    pub fn hex32() -> Mask {
        mask(r"\b[0-9a-fA-F]{32}\b", "<hex32>")
    }

    /// Email address -> `<email>`.
    pub fn email() -> Mask {
        mask(
            r"\b[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}\b",
            "<email>",
        )
    }

    /// IPv4 dotted-quad -> `<ipv4>`.
    pub fn ipv4() -> Mask {
        mask(r"\b(?:\d{1,3}\.){3}\d{1,3}\b", "<ipv4>")
    }

    /// JSON Web Token -> `<jwt>`.
    pub fn jwt() -> Mask {
        mask(
            r"\beyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\b",
            "<jwt>",
        )
    }
}

#[cfg(test)]
mod tests {
    use super::builtin_masks::*;
    use super::*;

    #[test]
    fn uuid_is_masked() {
        let out = apply_masks("id 550e8400-e29b-41d4-a716-446655440000 done", &[uuid()]);
        assert_eq!(out, "id <uuid> done");
    }

    #[test]
    fn email_is_masked() {
        let out = apply_masks("from alice@example.com sent", &[email()]);
        assert_eq!(out, "from <email> sent");
    }

    #[test]
    fn ipv4_is_masked() {
        let out = apply_masks("conn from 10.0.0.1 ok", &[ipv4()]);
        assert_eq!(out, "conn from <ipv4> ok");
    }

    #[test]
    fn hex32_is_masked() {
        let out = apply_masks("etag 0123456789abcdef0123456789abcdef!", &[hex32()]);
        assert_eq!(out, "etag <hex32>!");
    }

    #[test]
    fn jwt_is_masked() {
        let input = "auth eyJhbGci.eyJzdWIi.SflKxwRJ ok";
        let out = apply_masks(input, &[jwt()]);
        assert_eq!(out, "auth <jwt> ok");
    }

    #[test]
    fn no_match_borrows() {
        let out = apply_masks("nothing here", &[uuid(), email()]);
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out, "nothing here");
    }

    #[test]
    fn masks_compose_left_to_right() {
        let out = apply_masks("user alice@example.com from 10.0.0.1", &[email(), ipv4()]);
        assert_eq!(out, "user <email> from <ipv4>");
    }

    #[test]
    fn custom_mask_compiles_and_applies() {
        let m = Mask::new(r"\bport \d+\b", "<port>").unwrap();
        assert_eq!(
            apply_masks("listening port 8080 now", &[m]),
            "listening <port> now"
        );
    }

    #[test]
    fn invalid_pattern_errors() {
        assert!(Mask::new("(unclosed", "<x>").is_err());
    }
}
