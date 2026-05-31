//! Token-vector similarity: fraction of positions that match, where a wildcard
//! template token matches anything.

use crate::tokenize::Token;
use crate::OwnedToken;

/// Similarity in `[0.0, 1.0]`. Differing lengths score `0.0`. Two empty vectors
/// score `1.0` (identical empty templates).
pub(crate) fn similarity(template: &[OwnedToken], tokens: &[Token<'_>], wildcard: &str) -> f64 {
    if template.len() != tokens.len() {
        return 0.0;
    }
    if template.is_empty() {
        return 1.0;
    }
    let mut matches = 0usize;
    for (t, tok) in template.iter().zip(tokens.iter()) {
        if &*t.text == wildcard || &*t.text == tok.text {
            matches += 1;
        }
    }
    matches as f64 / template.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tokenize::tokenize;
    use crate::OwnedToken;

    fn owned(line: &str) -> Vec<OwnedToken> {
        tokenize(line).iter().map(OwnedToken::from).collect()
    }

    #[test]
    fn identical_is_one() {
        let t = owned("a b c");
        let toks = tokenize("a b c");
        assert_eq!(similarity(&t, &toks, "<*>"), 1.0);
    }

    #[test]
    fn all_different_is_zero() {
        let t = owned("a b c");
        let toks = tokenize("x y z");
        assert_eq!(similarity(&t, &toks, "<*>"), 0.0);
    }

    #[test]
    fn wildcard_in_template_counts_as_match() {
        let t = owned("a <*> c");
        let toks = tokenize("a ZZ c");
        assert_eq!(similarity(&t, &toks, "<*>"), 1.0);
    }

    #[test]
    fn partial_match_is_fraction() {
        let t = owned("a b c d");
        let toks = tokenize("a b X Y");
        assert_eq!(similarity(&t, &toks, "<*>"), 0.5);
    }

    #[test]
    fn length_mismatch_is_zero() {
        let t = owned("a b c");
        let toks = tokenize("a b");
        assert_eq!(similarity(&t, &toks, "<*>"), 0.0);
    }

    #[test]
    fn two_empty_is_one() {
        let t: Vec<OwnedToken> = vec![];
        let toks = tokenize("");
        assert_eq!(similarity(&t, &toks, "<*>"), 1.0);
    }
}
