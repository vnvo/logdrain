//! Configuration: `Options` (resolved) and `MinerBuilder`.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::LogdrainError;
use crate::mask::Mask;

/// Resolved, validated miner configuration. Immutable for a miner's lifetime.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Options {
    /// Similarity threshold in `[0.0, 1.0]` for joining an existing cluster.
    pub sim_threshold: f64,
    /// Prefix-tree depth (`>= 2`). Token levels below the shard root = `depth - 2`.
    pub depth: usize,
    /// Optional global cluster cap. Stored but not yet enforced (per-leaf LRU bounds).
    pub max_clusters: Option<usize>,
    /// Max clusters kept per leaf bucket before LRU eviction.
    pub max_clusters_per_leaf: usize,
    /// Replace pure-numeric tokens with the wildcard during tree descent.
    pub parametrize_numeric_tokens: bool,
    /// Placeholder string used for generalized positions.
    pub wildcard: Arc<str>,
    /// Characters that split a token into path sub-tokens (e.g. `'/'`).
    pub path_delimiters: Vec<char>,
    /// Path delimiters used when tokenizing the first line in `first_line_only` mode.
    pub first_line_path_delimiters: Vec<char>,
    /// Cluster on the first line only; the remainder is kept verbatim as a suffix.
    pub first_line_only: bool,
    /// Pre-tokenization masks. Not serialized (regex is not serializable; `restore`
    /// does not re-apply options).
    #[serde(skip)]
    pub masks: Vec<Mask>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            sim_threshold: 0.4,
            depth: 4,
            max_clusters: None,
            max_clusters_per_leaf: 100,
            parametrize_numeric_tokens: true,
            wildcard: Arc::from("<*>"),
            path_delimiters: Vec::new(),
            first_line_path_delimiters: Vec::new(),
            first_line_only: false,
            masks: Vec::new(),
        }
    }
}

impl Options {
    /// Number of token levels below the token-count shard root.
    pub(crate) fn prefix_len(&self) -> usize {
        self.depth - 2
    }

    /// The delimiter set used for tokenization: in `first_line_only` mode the
    /// first-line set is preferred (falling back to `path_delimiters` if empty),
    /// otherwise `path_delimiters`.
    pub(crate) fn active_path_delimiters(&self) -> &[char] {
        if self.first_line_only && !self.first_line_path_delimiters.is_empty() {
            &self.first_line_path_delimiters
        } else {
            &self.path_delimiters
        }
    }
}

/// Fluent builder for [`Options`] / [`crate::Miner`].
#[derive(Debug, Clone, Default)]
pub struct MinerBuilder {
    opts: Options,
}

impl MinerBuilder {
    /// Start from defaults.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the similarity threshold (validated to `[0.0, 1.0]` at build time).
    pub fn sim_threshold(mut self, v: f64) -> Self {
        self.opts.sim_threshold = v;
        self
    }

    /// Set the prefix-tree depth (validated `>= 2` at build time).
    pub fn depth(mut self, v: usize) -> Self {
        self.opts.depth = v;
        self
    }

    /// Set the optional global cluster cap.
    pub fn max_clusters(mut self, v: Option<usize>) -> Self {
        self.opts.max_clusters = v;
        self
    }

    /// Set the per-leaf cluster cap (validated `>= 1`).
    pub fn max_clusters_per_leaf(mut self, v: usize) -> Self {
        self.opts.max_clusters_per_leaf = v;
        self
    }

    /// Toggle numeric-token parametrization.
    pub fn parametrize_numeric_tokens(mut self, v: bool) -> Self {
        self.opts.parametrize_numeric_tokens = v;
        self
    }

    /// Set the wildcard placeholder string.
    pub fn wildcard(mut self, v: &str) -> Self {
        self.opts.wildcard = Arc::from(v);
        self
    }

    /// Set the path delimiter characters (e.g. `&['/']`).
    pub fn path_delimiters(mut self, delims: &[char]) -> Self {
        self.opts.path_delimiters = delims.to_vec();
        self
    }

    /// Set the first-line path delimiter characters (used in `first_line_only` mode).
    pub fn first_line_path_delimiters(mut self, delims: &[char]) -> Self {
        self.opts.first_line_path_delimiters = delims.to_vec();
        self
    }

    /// Enable first-line-only clustering (multi-line input keeps a verbatim suffix).
    pub fn first_line_only(mut self, v: bool) -> Self {
        self.opts.first_line_only = v;
        self
    }

    /// Set the pre-tokenization masks (e.g. `[builtin_masks::uuid(), ...]`).
    pub fn masks(mut self, masks: impl IntoIterator<Item = Mask>) -> Self {
        self.opts.masks = masks.into_iter().collect();
        self
    }

    /// Validate and produce resolved [`Options`].
    pub fn build_options(self) -> Result<Options, LogdrainError> {
        let o = &self.opts;
        if o.depth < 2 {
            return Err(LogdrainError::InvalidConfig(format!(
                "depth must be >= 2, got {}",
                o.depth
            )));
        }
        if !(0.0..=1.0).contains(&o.sim_threshold) {
            return Err(LogdrainError::InvalidConfig(format!(
                "sim_threshold must be in [0.0, 1.0], got {}",
                o.sim_threshold
            )));
        }
        if o.max_clusters_per_leaf == 0 {
            return Err(LogdrainError::InvalidConfig(
                "max_clusters_per_leaf must be >= 1".into(),
            ));
        }
        Ok(self.opts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_spec() {
        let o = Options::default();
        assert_eq!(o.sim_threshold, 0.4);
        assert_eq!(o.depth, 4);
        assert_eq!(o.max_clusters, None);
        assert_eq!(o.max_clusters_per_leaf, 100);
        assert!(o.parametrize_numeric_tokens);
        assert_eq!(&*o.wildcard, "<*>");
    }

    #[test]
    fn builder_overrides_and_builds() {
        let o = MinerBuilder::new()
            .sim_threshold(0.6)
            .depth(5)
            .max_clusters(Some(10_000))
            .max_clusters_per_leaf(50)
            .parametrize_numeric_tokens(false)
            .wildcard("<?>")
            .build_options()
            .unwrap();
        assert_eq!(o.sim_threshold, 0.6);
        assert_eq!(o.depth, 5);
        assert_eq!(o.max_clusters, Some(10_000));
        assert_eq!(o.max_clusters_per_leaf, 50);
        assert!(!o.parametrize_numeric_tokens);
        assert_eq!(&*o.wildcard, "<?>");
    }

    #[test]
    fn rejects_depth_below_two() {
        let err = MinerBuilder::new().depth(1).build_options().unwrap_err();
        assert!(matches!(err, crate::LogdrainError::InvalidConfig(_)));
    }

    #[test]
    fn rejects_threshold_out_of_range() {
        assert!(MinerBuilder::new()
            .sim_threshold(1.5)
            .build_options()
            .is_err());
        assert!(MinerBuilder::new()
            .sim_threshold(-0.1)
            .build_options()
            .is_err());
    }

    #[test]
    fn rejects_zero_per_leaf() {
        assert!(MinerBuilder::new()
            .max_clusters_per_leaf(0)
            .build_options()
            .is_err());
    }

    #[test]
    fn v0_2_defaults_are_off() {
        let o = Options::default();
        assert!(o.path_delimiters.is_empty());
        assert!(o.first_line_path_delimiters.is_empty());
        assert!(!o.first_line_only);
        assert!(o.masks.is_empty());
    }

    #[test]
    fn builder_sets_v0_2_fields() {
        let o = MinerBuilder::new()
            .path_delimiters(&['/'])
            .first_line_path_delimiters(&['/', ':'])
            .first_line_only(true)
            .masks([crate::builtin_masks::uuid()])
            .build_options()
            .unwrap();
        assert_eq!(o.path_delimiters, vec!['/']);
        assert_eq!(o.first_line_path_delimiters, vec!['/', ':']);
        assert!(o.first_line_only);
        assert_eq!(o.masks.len(), 1);
    }

    #[test]
    fn active_delimiters_prefer_first_line_when_enabled() {
        // first_line_only + non-empty first-line set -> first-line set.
        let o = MinerBuilder::new()
            .path_delimiters(&['/'])
            .first_line_path_delimiters(&[':'])
            .first_line_only(true)
            .build_options()
            .unwrap();
        assert_eq!(o.active_path_delimiters(), &[':']);

        // first_line_only but empty first-line set -> fall back to path_delimiters.
        let o2 = MinerBuilder::new()
            .path_delimiters(&['/'])
            .first_line_only(true)
            .build_options()
            .unwrap();
        assert_eq!(o2.active_path_delimiters(), &['/']);

        // not first_line_only -> path_delimiters regardless.
        let o3 = MinerBuilder::new()
            .path_delimiters(&['/'])
            .first_line_path_delimiters(&[':'])
            .build_options()
            .unwrap();
        assert_eq!(o3.active_path_delimiters(), &['/']);
    }
}
