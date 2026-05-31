//! Whitespace tokenizer. Path-delimiter handling lands in v0.2; the delimiter
//! fields exist now for forward compatibility and are always `None`.

use std::sync::Arc;

use smallvec::SmallVec;

/// A borrowed token: a slice of the input line plus (future) delimiter flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token<'a> {
    /// The token text, borrowed from the input.
    pub text: &'a str,
    /// Leading path delimiter (always `None` in v0.1).
    pub leading_delim: Option<char>,
    /// Trailing path delimiter (always `None` in v0.1).
    pub trailing_delim: Option<char>,
}

/// Owned token stored inside a [`crate::Cluster`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedToken {
    /// The token text, reference-counted for cheap cloning across clusters.
    pub text: Arc<str>,
    /// Leading path delimiter (always `None` in v0.1).
    pub leading_delim: Option<char>,
    /// Trailing path delimiter (always `None` in v0.1).
    pub trailing_delim: Option<char>,
}

impl From<&Token<'_>> for OwnedToken {
    fn from(t: &Token<'_>) -> Self {
        OwnedToken {
            text: Arc::from(t.text),
            leading_delim: t.leading_delim,
            trailing_delim: t.trailing_delim,
        }
    }
}

/// Inline storage for the common case of <= 16 tokens per line.
pub type Tokens<'a> = SmallVec<[Token<'a>; 16]>;

/// Split a line into whitespace-delimited tokens. Empty tokens are dropped.
pub fn tokenize(line: &str) -> Tokens<'_> {
    let mut out: Tokens<'_> = SmallVec::new();
    for piece in line.split_whitespace() {
        out.push(Token {
            text: piece,
            leading_delim: None,
            trailing_delim: None,
        });
    }
    out
}

/// A token is numeric iff it is non-empty and every byte is an ASCII digit.
pub fn is_numeric_token(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_on_whitespace() {
        let toks = tokenize("GET /api/v1 200");
        let texts: Vec<&str> = toks.iter().map(|t| t.text).collect();
        assert_eq!(texts, vec!["GET", "/api/v1", "200"]);
    }

    #[test]
    fn collapses_runs_of_whitespace() {
        let toks = tokenize("  a   b\tc  ");
        let texts: Vec<&str> = toks.iter().map(|t| t.text).collect();
        assert_eq!(texts, vec!["a", "b", "c"]);
    }

    #[test]
    fn empty_input_yields_no_tokens() {
        assert_eq!(tokenize("").len(), 0);
        assert_eq!(tokenize("   ").len(), 0);
    }

    #[test]
    fn delims_are_none_in_v0_1() {
        let toks = tokenize("x");
        assert!(toks[0].leading_delim.is_none());
        assert!(toks[0].trailing_delim.is_none());
    }

    #[test]
    fn numeric_detection() {
        assert!(is_numeric_token("12345"));
        assert!(is_numeric_token("0"));
        assert!(!is_numeric_token(""));
        assert!(!is_numeric_token("12a"));
        assert!(!is_numeric_token("-3"));
        assert!(!is_numeric_token("1.5"));
    }

    #[test]
    fn owned_token_round_trips() {
        let toks = tokenize("hello");
        let owned = OwnedToken::from(&toks[0]);
        assert_eq!(&*owned.text, "hello");
    }
}
