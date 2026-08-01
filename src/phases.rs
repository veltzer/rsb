//! Post-config hook system.
//!
//! Hooks registered via `inventory::submit!` run once at the end of
//! [`crate::config::Config::load`], before any command consumes the config,
//! in inventory registration order.

use anyhow::Result;

use crate::config::Config;

/// A post-config hook. Submit one with `inventory::submit!`.
///
/// `function` and `location` carry the registered function's fully-qualified
/// path and source location. They are populated by hand at each registration
/// site (no macro), via `concat!(module_path!(), "::", stringify!(fn))` and
/// `concat!(file!(), ":", line!())`. They are surfaced by
/// `rsconstruct hooks --verbose`.
pub struct PhaseHook {
    pub name: &'static str,
    pub description: &'static str,
    pub function: &'static str,
    pub location: &'static str,
    pub run: fn(&mut Config) -> Result<()>,
}

inventory::collect!(PhaseHook);

pub fn all_hooks() -> impl Iterator<Item = &'static PhaseHook> {
    inventory::iter::<PhaseHook>()
}

/// Run every registered post-config hook, in inventory order.
pub fn run_post_config_hooks(config: &mut Config) -> Result<()> {
    for hook in all_hooks() {
        (hook.run)(config)
            .map_err(|e| e.context(format!("post-config hook '{}'", hook.name)))?;
    }
    Ok(())
}
