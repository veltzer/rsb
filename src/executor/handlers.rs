use crate::color;
use crate::json_output::{emit_product_complete, ProductStatus};
use crate::processors::{FailedProduct, ProcessStats};

use super::{Executor, HandlerContext, RestoreOutcome, SharedState};

impl Executor<'_> {
    /// Lock shared stats and apply an update function to the processor's stats entry.
    fn update_stats(shared: &SharedState, proc_name: &str, f: impl FnOnce(&mut ProcessStats)) {
        let mut stats = shared.stats.lock();
        f(stats.entry(proc_name.to_string()).or_default());
    }

    /// Record a product failure into shared state.
    ///
    /// In keep-going mode: prints the error, records the message for the summary.
    /// In fail-fast mode: stores the error for later propagation.
    /// If `mark_processor_failed` is true, also records the processor name
    /// so its products are skipped in subsequent levels.
    fn record_failure(
        &self,
        ctx: &HandlerContext,
        error: anyhow::Error,
        mark_processor_failed: bool,
    ) {
        // Always mark the product as failed
        ctx.shared.failed_products.lock().insert(ctx.id);

        // Record structured failure detail for `rsconstruct edit`
        let primary_file = ctx.product.inputs.first()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        ctx.shared.failed_details.lock().push(FailedProduct {
            file: primary_file,
            processor: ctx.proc_name.to_string(),
            error: format!("{error:#}"),
        });

        // Wrap the error with the processor name so users can always identify the source.
        // Use {:#} to include the full anyhow error chain (e.g. tera rendering details).
        let prefixed_error = anyhow::anyhow!("[{}] {:#}", ctx.proc_name, error);

        if ctx.keep_going {
            let msg = format!("{}: {}", self.product_display(ctx.product), prefixed_error);
            // stderr: errors must survive --json, and must not corrupt the
            // JSON event stream on stdout.
            crate::output::error(&msg);
            ctx.shared.failed_messages.lock().push(msg);
        } else {
            if mark_processor_failed {
                ctx.shared.failed_processors.lock().insert(ctx.proc_name.to_string());
            }
            ctx.shared.errors.lock().push(prefixed_error);
        }
    }

    /// Handle the "skip (unchanged)" case for a product.
    /// Logs, emits JSON event, increments stats. Does NOT advance the progress bar
    /// since skips are instant and the bar total excludes them.
    pub(super) fn handle_skip(
        &self,
        product: &crate::graph::Product,
        shared: &SharedState,
    ) {
        crate::output::detail(self.verbose, &format!("[{}] {} {}", product.processor,
            color::dim("Skipping (unchanged):"),
            self.product_display(product)));
        emit_product_complete(
            &self.product_display(product),
            &product.processor,
            ProductStatus::Skipped,
            None,
            None,
        );
        Self::update_stats(shared, &product.processor, |s| s.skipped += 1);
        Self::inc_global(shared);
    }

    /// Handle cache restore for a product.
    /// Try to restore a product from cache.
    /// When `emit_fail_event` is true, emits a product_complete "failed" JSON event on error.
    pub(super) fn handle_restore(
        &self,
        ctx: &HandlerContext,
        object_store: &crate::object_store::ObjectStore,
        force: bool,
        emit_fail_event: bool,
    ) -> RestoreOutcome {
        if force {
            return RestoreOutcome::NotRestorable;
        }
        let desc_key = ctx.product.descriptor_key(ctx.input_checksum);
        let restore_result = object_store.restore_from_descriptor(self.build_ctx, &desc_key, &ctx.product.outputs);
        match restore_result {
            Ok(true) => {
                crate::output::detail(self.verbose, &format!("[{}] {} {}", ctx.product.processor,
                    color::cyan("Restored from cache:"),
                    self.product_display(ctx.product)));
                emit_product_complete(
                    &self.product_display(ctx.product),
                    &ctx.product.processor,
                    ProductStatus::Restored,
                    None,
                    None,
                );
                let output_count = ctx.product.outputs.len();
                Self::update_stats(ctx.shared, &ctx.product.processor, |s| {
                    s.restored += 1;
                    s.files_restored += output_count;
                });
                Self::inc_progress(ctx.pb, ctx.shared);
                RestoreOutcome::Restored
            }
            Err(e) => {
                if emit_fail_event {
                    emit_product_complete(
                        &self.product_display(ctx.product),
                        &ctx.product.processor,
                        ProductStatus::Failed,
                        None,
                        Some(&e.to_string()),
                    );
                }
                self.record_failure(ctx, e, false);
                Self::inc_progress(ctx.pb, ctx.shared);
                RestoreOutcome::Failed
            }
            Ok(false) => RestoreOutcome::NotRestorable,
        }
    }

    /// Handle a product execution error.
    /// Emits a JSON event, records failure stats, and records keep-going / non-keep-going state.
    pub(super) fn handle_error(
        &self,
        ctx: &HandlerContext,
        error: anyhow::Error,
        duration: Option<std::time::Duration>,
    ) {
        emit_product_complete(
            &self.product_display(ctx.product),
            &ctx.product.processor,
            ProductStatus::Failed,
            duration,
            Some(&format!("{error:#}")),
        );
        Self::update_stats(ctx.shared, ctx.proc_name, |s| s.failed += 1);
        self.record_failure(ctx, error, true);
    }

    /// Handle caching outputs and recording stats after successful execution.
    /// Returns `true` if caching succeeded, `false` if it failed (error is handled internally).
    /// On success, emits a product_complete "success" JSON event and increments processed/files_created.
    pub(super) fn handle_success(
        &self,
        ctx: &HandlerContext,
        object_store: &crate::object_store::ObjectStore,
        graph: &crate::graph::BuildGraph,
        duration: Option<std::time::Duration>,
    ) -> bool {
        // Recompute the input checksum NOW (post-execution) rather than reusing
        // the classify-time value. The cache key must match what the *next*
        // classify will compute from on-disk state, which means hashing inputs
        // that exist right now:
        //   - upstream-chain case (e.g. ipdfunite consuming marp's PDF on a
        //     clean build): at classify time the upstream output didn't exist
        //     yet and the checksum had MISSING:; here it does, and the hash
        //     reflects real content.
        //   - self-reference / output-also-as-input case (e.g. tera template
        //     whose dep_inputs lists its own output): the product just rewrote
        //     the file, so the post-execution hash matches what next classify
        //     will see.
        // Both cases break if we use the classify-time checksum.
        let post_input_checksum = match crate::checksum::combined_input_checksum(self.build_ctx, &ctx.product.inputs) {
            Ok(cs) => cs,
            Err(e) => {
                emit_product_complete(
                    &self.product_display(ctx.product),
                    &ctx.product.processor,
                    ProductStatus::Failed,
                    duration,
                    Some(&format!("Failed to compute post-execution input checksum: {e}")),
                );
                self.record_failure(ctx, e, false);
                return false;
            }
        };
        let desc_key = ctx.product.descriptor_key(&post_input_checksum);
        let cache_result = if ctx.product.outputs.is_empty() && !ctx.product.has_output_dirs() {
            // Checker: no outputs, just mark as passed
            object_store.store_marker(self.build_ctx, &desc_key).map(|()| false)
        } else if ctx.product.outputs.len() == 1 && !ctx.product.has_output_dirs() {
            // Generator: single output file → blob
            object_store.store_blob_descriptor(self.build_ctx, &desc_key, &ctx.product.outputs[0])
        } else {
            // Creator/Explicit or multi-output: always tree.
            // When walking output_dirs, skip paths declared as outputs of OTHER products —
            // they're owned by some other processor that contributes to the shared directory.
            let is_foreign = |path: &std::path::Path| -> bool {
                matches!(graph.path_owner(path), Some(owner) if owner != ctx.id)
            };
            object_store.store_tree_descriptor(self.build_ctx, &desc_key, &ctx.product.output_dirs, &ctx.product.outputs, &is_foreign)
        };
        match cache_result {
            Ok(changed) => {
                if !changed {
                    ctx.shared.unchanged_products.lock().insert(ctx.id);
                }
            }
            Err(e) => {
                emit_product_complete(
                    &self.product_display(ctx.product),
                    &ctx.product.processor,
                    ProductStatus::Failed,
                    duration,
                    Some(&e.to_string()),
                );
                self.record_failure(ctx, e, false);
                return false;
            }
        }
        emit_product_complete(
            &self.product_display(ctx.product),
            &ctx.product.processor,
            ProductStatus::Success,
            duration,
            None,
        );
        let output_count = ctx.product.outputs.len();
        Self::update_stats(ctx.shared, ctx.proc_name, |s| {
            s.processed += 1;
            s.files_created += output_count;
        });
        true
    }
}
