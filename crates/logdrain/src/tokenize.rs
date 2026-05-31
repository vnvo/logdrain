//! Tokenizer: whitespace splitting plus optional path-delimiter splitting that
//! records the delimiter surrounding each sub-token.

use std::sync::Arc;

use smallvec::SmallVec;

/// A borrowed token: a slice of the input line plus path-delimiter flags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token<'a> {
    /// The token text, borrowed from the input.
    pub text: &'a str,
    /// Path delimiter immediately preceding this sub-token, if any.
    pub leading_delim: Option<char>,
    /// Path delimiter immediately following this sub-token, if any.
    pub trailing_delim: Option<char>,
}

/// Owned token stored inside a [`crate::Cluster`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OwnedToken {
    /// The token text, reference-counted for cheap cloning across clusters.
    pub text: Arc<str>,
    /// Path delimiter immediately preceding this sub-token, if any.
    pub leading_delim: Option<char>,
    /// Path delimiter immediately following this sub-token, if any.
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

/// Split a line into whitespace-delimited tokens (no path splitting). Test-only
/// convenience; production code calls [`tokenize_with`] with the active delimiters.
#[cfg(test)]
pub(crate) fn tokenize(line: &str) -> Tokens<'_> {
    tokenize_with(line, &[])
}

/// Split a line into whitespace tokens, then split each on `delims` into path
/// sub-tokens carrying leading/trailing delimiter flags. With empty `delims`
/// this is identical to [`tokenize`].
pub(crate) fn tokenize_with<'a>(line: &'a str, delims: &[char]) -> Tokens<'a> {
    let mut out: Tokens<'a> = SmallVec::new();
    for piece in line.split_whitespace() {
        push_path_tokens(piece, delims, &mut out);
    }
    out
}

/// Split one whitespace token on delimiter chars, appending sub-tokens. A run of
/// delimiters collapses; a piece that is only delimiters is kept verbatim as one
/// plain token so no input is silently dropped.
fn push_path_tokens<'a>(piece: &'a str, delims: &[char], out: &mut Tokens<'a>) {
    let before = out.len();
    let mut cur_leading: Option<char> = None;
    let mut seg_start = 0usize;
    for (i, ch) in piece.char_indices() {
        if delims.contains(&ch) {
            let seg = &piece[seg_start..i];
            if !seg.is_empty() {
                out.push(Token {
                    text: seg,
                    leading_delim: cur_leading,
                    trailing_delim: Some(ch),
                });
            }
            cur_leading = Some(ch);
            seg_start = i + ch.len_utf8();
        }
    }
    let seg = &piece[seg_start..];
    if !seg.is_empty() {
        out.push(Token {
            text: seg,
            leading_delim: cur_leading,
            trailing_delim: None,
        });
    }
    if out.len() == before && !piece.is_empty() {
        // The piece was entirely delimiters; preserve it as a literal token.
        out.push(Token {
            text: piece,
            leading_delim: None,
            trailing_delim: None,
        });
    }
}

/// Split off the first line; return `(first_line, suffix_after_newline)`.
pub(crate) fn split_first_line(line: &str) -> (&str, Option<&str>) {
    match line.find('\n') {
        Some(i) => (&line[..i], Some(&line[i + 1..])),
        None => (line, None),
    }
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

    fn flags<'a>(t: &Token<'a>) -> (&'a str, Option<char>, Option<char>) {
        (t.text, t.leading_delim, t.trailing_delim)
    }

    #[test]
    fn path_splits_with_delim_flags() {
        let toks = tokenize_with("/servers/409/foo", &['/']);
        assert_eq!(toks.len(), 3);
        assert_eq!(flags(&toks[0]), ("servers", Some('/'), Some('/')));
        assert_eq!(flags(&toks[1]), ("409", Some('/'), Some('/')));
        assert_eq!(flags(&toks[2]), ("foo", Some('/'), None));
    }

    #[test]
    fn mixed_whitespace_and_path() {
        let toks = tokenize_with("GET /servers/409 ok", &['/']);
        assert_eq!(toks.len(), 4);
        assert_eq!(flags(&toks[0]), ("GET", None, None));
        assert_eq!(flags(&toks[1]), ("servers", Some('/'), Some('/')));
        assert_eq!(flags(&toks[2]), ("409", Some('/'), None));
        assert_eq!(flags(&toks[3]), ("ok", None, None));
    }

    #[test]
    fn leading_and_trailing_delims() {
        let a = tokenize_with("a/b", &['/']);
        assert_eq!(flags(&a[0]), ("a", None, Some('/')));
        assert_eq!(flags(&a[1]), ("b", Some('/'), None));

        let lead = tokenize_with("/a", &['/']);
        assert_eq!(flags(&lead[0]), ("a", Some('/'), None));

        let trail = tokenize_with("a/", &['/']);
        assert_eq!(trail.len(), 1);
        assert_eq!(flags(&trail[0]), ("a", None, Some('/')));
    }

    #[test]
    fn consecutive_delims_collapse() {
        let toks = tokenize_with("a//b", &['/']);
        assert_eq!(toks.len(), 2);
        assert_eq!(flags(&toks[0]), ("a", None, Some('/')));
        assert_eq!(flags(&toks[1]), ("b", Some('/'), None));
    }

    #[test]
    fn lone_delimiter_is_preserved() {
        let toks = tokenize_with("GET / HTTP", &['/']);
        let texts: Vec<&str> = toks.iter().map(|t| t.text).collect();
        assert_eq!(texts, vec!["GET", "/", "HTTP"]);
    }

    #[test]
    fn empty_delims_behaves_like_v0_1() {
        let toks = tokenize_with("/servers/409/foo", &[]);
        assert_eq!(toks.len(), 1);
        assert_eq!(flags(&toks[0]), ("/servers/409/foo", None, None));
    }

    #[test]
    fn split_first_line_works() {
        assert_eq!(split_first_line("a b\nc\nd"), ("a b", Some("c\nd")));
        assert_eq!(split_first_line("single"), ("single", None));
        assert_eq!(split_first_line("first\n"), ("first", Some("")));
    }
}
