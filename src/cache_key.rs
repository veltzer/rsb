//! The single owner of cache-key composition.
//!
//! Before this module, key material was assembled in five uncoordinated
//! places — `Product::descriptor_key`, the `output_config_hash` allowlist,
//! `extend_config_hash`, `apply_tool_version_hashes` (which used a different
//! concatenation style than the others), and each processor's
//! `checksum_fields()` — all funnelling into one opaque `Option<String>`.
//! Nothing recorded *what* had gone into that string, so a component that
//! silently failed to be mixed in (tool versions without a lock file) was
//! invisible, and `--explain` could not attribute a key to its causes.
//!
//! `CacheKey` replaces the opaque string with an ordered, named component
//! list. Every contributor appends a `(Component, value)` pair; one function
//! folds them into the final digest. The components survive into
//! `rsconstruct product show`, so a changed key can always be attributed to
//! the exact component that changed.

use std::fmt;

/// What kind of state a key component captures. The discriminant is mixed
/// into the digest, so two components with equal values but different kinds
/// produce different keys — a config hash of `"abc"` is not a tool version
/// of `"abc"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Component {
    /// Hash of the processor's checksum-relevant config fields
    /// (`output_config_hash` over `checksum_fields()`).
    Config,
    /// A piece contributed by an analyzer — non-content state that still
    /// affects the output, e.g. the sorted set of paths matching a glob.
    Analyzer,
    /// Hash of the versions of the external tools the processor invokes.
    ToolVersion,
    /// A variant/profile name (e.g. a compiler profile).
    Variant,
}

impl Component {
    /// Stable tag mixed into the digest and shown by `product show`.
    /// These strings are part of the on-disk cache key: changing one
    /// invalidates every cache entry that has that component.
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::Analyzer => "analyzer",
            Self::ToolVersion => "tool",
            Self::Variant => "variant",
        }
    }
}

impl fmt::Display for Component {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.tag())
    }
}

/// The ordered set of non-input state that contributes to a product's cache
/// key. Input *content* is not stored here — it is passed to
/// [`CacheKey::descriptor_key`] at the point of use, because it is computed
/// lazily (and expensively) from the filesystem.
///
/// Components are kept in insertion order rather than sorted: the order in
/// which contributors run is deterministic (discovery, then analyzers, then
/// tool versions), and preserving it keeps the digest stable while making
/// the `product show` output read in causal order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CacheKey {
    components: Vec<(Component, String)>,
}

impl CacheKey {
    /// An empty key — a product whose output depends on nothing but its inputs.
    pub const fn new() -> Self {
        Self { components: Vec::new() }
    }

    /// Build a key from a single config hash, the common case at discovery time.
    pub fn from_config_hash(hash: Option<String>) -> Self {
        let mut key = Self::new();
        if let Some(hash) = hash {
            key.push(Component::Config, hash);
        }
        key
    }

    /// Append a component. Contributors call this instead of hand-splicing
    /// strings, which is what let three different separator conventions
    /// coexist previously.
    pub fn push(&mut self, component: Component, value: impl Into<String>) {
        self.components.push((component, value.into()));
    }

    /// Whether any component has been contributed.
    pub const fn is_empty(&self) -> bool {
        self.components.is_empty()
    }

    /// The components in contribution order, for display and testing.
    pub fn components(&self) -> &[(Component, String)] {
        &self.components
    }

    /// The serialized component list that feeds the digest. Each component is
    /// `tag=value`, joined by `|`. Both separators are excluded from tags, and
    /// values are hex digests or config-supplied names, so the encoding is
    /// unambiguous in practice for every current contributor.
    fn material(&self) -> String {
        self.components.iter()
            .map(|(c, v)| format!("{}={}", c.tag(), v))
            .collect::<Vec<_>>()
            .join("|")
    }

    /// A stable digest of the components alone, with no input content.
    /// `None` when there are no components, so that a product with no
    /// non-input state produces a key identical in shape to the old
    /// `config_hash: None` case.
    pub fn digest(&self) -> Option<String> {
        if self.components.is_empty() {
            return None;
        }
        Some(crate::checksum::bytes_checksum(self.material().as_bytes()))
    }

    /// Compute the content-addressed descriptor key for a product.
    ///
    /// This key does NOT include file paths — renaming a file with identical
    /// content produces the same key. The blob in the cache is path-free; the
    /// product knows where to restore it.
    ///
    /// The key mixes in the processor's implementation `version` (from its
    /// plugin registration). Bumping that version invalidates every cache
    /// entry produced by this processor — see docs/src/processor-versioning.md
    /// for the bump rule. For processors not in the builtin registry (e.g. Lua
    /// plugins), `v0` is used.
    pub fn descriptor_key(&self, processor: &str, input_checksum: &str) -> String {
        let mut parts = String::new();
        parts.push_str(processor);
        parts.push_str(":v");
        let version = crate::registries::processor_version(processor).unwrap_or(0);
        parts.push_str(&version.to_string());
        if let Some(digest) = self.digest() {
            parts.push(':');
            parts.push_str(&digest);
        }
        parts.push(':');
        parts.push_str(input_checksum);
        crate::checksum::bytes_checksum(parts.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_key_has_no_digest() {
        assert_eq!(CacheKey::new().digest(), None);
        assert!(CacheKey::new().is_empty());
    }

    #[test]
    fn from_config_hash_none_is_empty() {
        assert!(CacheKey::from_config_hash(None).is_empty());
        assert!(!CacheKey::from_config_hash(Some("x".into())).is_empty());
    }

    #[test]
    fn component_kind_is_part_of_the_digest() {
        // The bug this prevents: a config hash and a tool hash with the same
        // value must not collapse to the same key.
        let mut a = CacheKey::new();
        a.push(Component::Config, "abc");
        let mut b = CacheKey::new();
        b.push(Component::ToolVersion, "abc");
        assert_ne!(a.digest(), b.digest());
    }

    #[test]
    fn order_is_significant() {
        let mut a = CacheKey::new();
        a.push(Component::Config, "one");
        a.push(Component::Analyzer, "two");
        let mut b = CacheKey::new();
        b.push(Component::Analyzer, "two");
        b.push(Component::Config, "one");
        assert_ne!(a.digest(), b.digest());
    }

    #[test]
    fn digest_is_stable_across_calls() {
        let mut key = CacheKey::new();
        key.push(Component::Config, "abc");
        key.push(Component::ToolVersion, "def");
        assert_eq!(key.digest(), key.digest());
    }

    #[test]
    fn adding_a_component_changes_the_digest() {
        let mut key = CacheKey::new();
        key.push(Component::Config, "abc");
        let before = key.digest();
        key.push(Component::ToolVersion, "def");
        assert_ne!(before, key.digest());
    }

    #[test]
    fn descriptor_key_varies_with_every_input() {
        let mut key = CacheKey::new();
        key.push(Component::Config, "abc");
        let base = key.descriptor_key("proc", "chk");
        assert_ne!(base, key.descriptor_key("other", "chk"));
        assert_ne!(base, key.descriptor_key("proc", "other"));
        assert_ne!(base, CacheKey::new().descriptor_key("proc", "chk"));
    }

    #[test]
    fn components_are_reported_in_contribution_order() {
        let mut key = CacheKey::new();
        key.push(Component::Config, "c");
        key.push(Component::Analyzer, "a");
        key.push(Component::ToolVersion, "t");
        let tags: Vec<_> = key.components().iter().map(|(c, _)| c.tag()).collect();
        assert_eq!(tags, vec!["config", "analyzer", "tool"]);
    }
}
