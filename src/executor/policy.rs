use crate::graph::Product;
use crate::object_store::{ExplainAction, ObjectStore};

/// What action should be taken for a product.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProductAction {
    /// Outputs are up-to-date — skip execution entirely.
    Skip,
    /// Outputs can be restored from the cache without re-executing.
    Restore,
    /// Product must be executed (new, changed, or forced).
    Build,
}

/// Decides whether each product should be skipped, restored, or built.
///
/// The default implementation ([`IncrementalPolicy`]) encodes the current
/// behavior: skip if outputs match, restore if cache has blobs, else build.
/// Future implementations could add time-based expiry, always-rebuild,
/// demand-driven filtering, or deterministic-verification modes.
pub trait BuildPolicy: Sync + Send {
    /// Classify a single product given the current cache state.
    ///
    /// `dep_changed` is true if any dependency of this product will be rebuilt
    /// or restored in this run (i.e. its outputs cannot be trusted even if the
    /// descriptor matches).
    fn classify(
        &self,
        product: &Product,
        object_store: &ObjectStore,
        input_checksum: &str,
        dep_changed: bool,
        force: bool,
    ) -> ProductAction;

    /// Return a human-readable explanation of what action would be taken and why.
    /// Used by `--explain`.
    fn explain(
        &self,
        product: &Product,
        object_store: &ObjectStore,
        input_checksum: &str,
        force: bool,
    ) -> ExplainAction;
}

/// The standard incremental build policy: skip unchanged, restore from cache
/// when possible, otherwise build.
pub struct IncrementalPolicy;

impl BuildPolicy for IncrementalPolicy {
    fn classify(
        &self,
        product: &Product,
        object_store: &ObjectStore,
        input_checksum: &str,
        dep_changed: bool,
        force: bool,
    ) -> ProductAction {
        let desc_key = product.descriptor_key(input_checksum);
        let needs_rebuild = object_store.needs_rebuild_descriptor(&desc_key, &product.outputs);
        let can_restore = object_store.can_restore_descriptor(&desc_key);

        if !force && !dep_changed && !needs_rebuild {
            ProductAction::Skip
        } else if !force && !dep_changed && can_restore {
            ProductAction::Restore
        } else {
            ProductAction::Build
        }
    }

    fn explain(
        &self,
        product: &Product,
        object_store: &ObjectStore,
        input_checksum: &str,
        force: bool,
    ) -> ExplainAction {
        let desc_key = product.descriptor_key(input_checksum);
        object_store.explain_descriptor(&desc_key, &product.outputs, force)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::build_context::BuildContext;
    use crate::graph::BuildGraph;
    use std::fs;

    /// The classify decision table over descriptor state, `dep_changed`, and
    /// `force`. `dep_changed` and `force` must each beat both a matching
    /// descriptor (no stale skip) and a restorable cache (no stale restore).
    #[test]
    fn classify_decision_table() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = ObjectStore::new_in(tmp.path());
        let policy = IncrementalPolicy;
        let chk = "cafe0123";

        let mut g = BuildGraph::new();
        let out = tmp.path().join("out.txt");
        let gen_id = g.add_product(vec![tmp.path().join("in.txt")], vec![out.clone()], "gen", None).unwrap();
        let chk_id = g.add_product(vec![tmp.path().join("in2.txt")], vec![], "check", None).unwrap();

        // No descriptor at all: must build, whatever the flags say.
        let generator = g.get_product(gen_id).unwrap();
        assert_eq!(policy.classify(generator, &store, chk, false, false), ProductAction::Build);

        // Checker with a stored PASS marker: skip — unless a dependency
        // changed or the build is forced.
        let checker = g.get_product(chk_id).unwrap();
        store.store_marker(&checker.descriptor_key(chk)).unwrap();
        assert_eq!(policy.classify(checker, &store, chk, false, false), ProductAction::Skip);
        assert_eq!(policy.classify(checker, &store, chk, true, false), ProductAction::Build,
            "dep_changed must invalidate a matching marker");
        assert_eq!(policy.classify(checker, &store, chk, false, true), ProductAction::Build,
            "force must beat a matching marker");

        // Generator with cached blob and intact output: skip.
        let ctx = BuildContext::new();
        fs::write(&out, b"built output").unwrap();
        store.store_blob_descriptor(&ctx, &generator.descriptor_key(chk), &out).unwrap();
        assert_eq!(policy.classify(generator, &store, chk, false, false), ProductAction::Skip);

        // Output gone but the object is in the cache: restore, not build —
        // unless a dependency changed or the build is forced.
        fs::remove_file(&out).unwrap();
        assert_eq!(policy.classify(generator, &store, chk, false, false), ProductAction::Restore);
        assert_eq!(policy.classify(generator, &store, chk, true, false), ProductAction::Build,
            "dep_changed must beat a restorable cache");
        assert_eq!(policy.classify(generator, &store, chk, false, true), ProductAction::Build,
            "force must beat a restorable cache");

        // A different input checksum is a different descriptor key: build.
        assert_eq!(policy.classify(generator, &store, "other993", false, false), ProductAction::Build);
    }

    /// A corrupted output (exists, wrong content) with the blob still cached
    /// must classify as Restore — the executor re-materializes it from the
    /// cache instead of re-running the tool.
    #[test]
    fn classify_corrupted_output_restores() {
        let tmp = tempfile::TempDir::new().unwrap();
        let store = ObjectStore::new_in(tmp.path());
        let policy = IncrementalPolicy;
        let ctx = BuildContext::new();
        let chk = "beef4567";

        let mut g = BuildGraph::new();
        let out = tmp.path().join("out.bin");
        let id = g.add_product(vec![tmp.path().join("in.bin")], vec![out.clone()], "gen", None).unwrap();
        let p = g.get_product(id).unwrap();

        fs::write(&out, b"good").unwrap();
        store.store_blob_descriptor(&ctx, &p.descriptor_key(chk), &out).unwrap();
        fs::write(&out, b"corrupted").unwrap();

        assert_eq!(policy.classify(p, &store, chk, false, false), ProductAction::Restore);
    }
}
