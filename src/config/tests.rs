use crate::config::variables::{
    value_to_toml_inline, remove_vars_section, extract_var_names, substitute_variables,
};

// Tests for value_to_toml_inline

#[test]
fn value_to_toml_inline_string() {
    let value = toml::Value::String("hello".into());
    assert_eq!(value_to_toml_inline(&value), "\"hello\"");
}

#[test]
fn value_to_toml_inline_string_with_quotes() {
    let value = toml::Value::String("say \"hello\"".into());
    assert_eq!(value_to_toml_inline(&value), "\"say \\\"hello\\\"\"");
}

#[test]
fn value_to_toml_inline_string_with_backslash() {
    let value = toml::Value::String("path\\to\\file".into());
    assert_eq!(value_to_toml_inline(&value), "\"path\\\\to\\\\file\"");
}

#[test]
fn value_to_toml_inline_integer() {
    let value = toml::Value::Integer(42);
    assert_eq!(value_to_toml_inline(&value), "42");
}

#[test]
fn value_to_toml_inline_negative_integer() {
    let value = toml::Value::Integer(-123);
    assert_eq!(value_to_toml_inline(&value), "-123");
}

#[test]
fn value_to_toml_inline_float() {
    let value = toml::Value::Float(2.72);
    assert_eq!(value_to_toml_inline(&value), "2.72");
}

#[test]
fn value_to_toml_inline_boolean_true() {
    let value = toml::Value::Boolean(true);
    assert_eq!(value_to_toml_inline(&value), "true");
}

#[test]
fn value_to_toml_inline_boolean_false() {
    let value = toml::Value::Boolean(false);
    assert_eq!(value_to_toml_inline(&value), "false");
}

#[test]
fn value_to_toml_inline_array_of_strings() {
    let value = toml::Value::Array(vec![
        toml::Value::String("a".into()),
        toml::Value::String("b".into()),
        toml::Value::String("c".into()),
    ]);
    assert_eq!(value_to_toml_inline(&value), "[\"a\", \"b\", \"c\"]");
}

#[test]
fn value_to_toml_inline_array_of_integers() {
    let value = toml::Value::Array(vec![
        toml::Value::Integer(1),
        toml::Value::Integer(2),
        toml::Value::Integer(3),
    ]);
    assert_eq!(value_to_toml_inline(&value), "[1, 2, 3]");
}

#[test]
fn value_to_toml_inline_empty_array() {
    let value = toml::Value::Array(vec![]);
    assert_eq!(value_to_toml_inline(&value), "[]");
}

#[test]
fn value_to_toml_inline_table() {
    let mut table = toml::map::Map::new();
    table.insert("key".into(), toml::Value::String("value".into()));
    let value = toml::Value::Table(table);
    assert_eq!(value_to_toml_inline(&value), "{ key = \"value\" }");
}

// Tests for remove_vars_section

#[test]
fn remove_vars_section_basic() {
    let content = "[vars]\nfoo = \"bar\"\n\n[other]\nkey = \"value\"\n";
    let result = remove_vars_section(content);
    assert!(!result.contains("[vars]"));
    assert!(!result.contains("foo = \"bar\""));
    assert!(result.contains("[other]"));
    assert!(result.contains("key = \"value\""));
}

#[test]
fn remove_vars_section_at_end() {
    let content = "[other]\nkey = \"value\"\n\n[vars]\nfoo = \"bar\"\n";
    let result = remove_vars_section(content);
    assert!(!result.contains("[vars]"));
    assert!(!result.contains("foo = \"bar\""));
    assert!(result.contains("[other]"));
    assert!(result.contains("key = \"value\""));
}

#[test]
fn remove_vars_section_no_vars() {
    let content = "[other]\nkey = \"value\"\n";
    let result = remove_vars_section(content);
    assert_eq!(result, "[other]\nkey = \"value\"\n");
}

#[test]
fn remove_vars_section_multiple_vars() {
    let content = "[vars]\nfoo = \"bar\"\nbaz = [1, 2, 3]\n\n[other]\nkey = \"value\"\n";
    let result = remove_vars_section(content);
    assert!(!result.contains("[vars]"));
    assert!(!result.contains("foo = \"bar\""));
    assert!(!result.contains("baz = [1, 2, 3]"));
    assert!(result.contains("[other]"));
}

/// Blanked (not deleted) vars lines keep every following line at its original
/// line number, so provenance spans stay correct.
#[test]
fn remove_vars_section_preserves_line_numbers() {
    let content = "[vars]\nfoo = \"bar\"\n\n[other]\nkey = \"value\"\n";
    let result = remove_vars_section(content);
    let lines: Vec<&str> = result.lines().collect();
    assert_eq!(lines.len(), 5, "line count must be unchanged");
    assert_eq!(lines[3], "[other]", "[other] must stay on line 4");
    assert_eq!(lines[4], "key = \"value\"", "key must stay on line 5");
}

/// A var referencing another var must resolve regardless of definition order
/// (previously only alphabetically-earlier references happened to work).
#[test]
fn substitute_variables_nested_reference_any_order() {
    let content = "[vars]\nb = \"x\"\nz = \"${b}\"\n\n[other]\nkey = \"${z}\"\n";
    let result = substitute_variables(content).unwrap();
    assert!(result.contains("key = \"x\""), "nested var must resolve: {result}");
    assert!(!result.contains("${"), "no raw references may remain: {result}");
}

/// A reference cycle in [vars] must be a clear error, not a hang.
#[test]
fn substitute_variables_cycle_errors() {
    let content = "[vars]\na = \"${b}\"\nb = \"${a}\"\n\n[other]\nkey = \"${a}\"\n";
    let err = substitute_variables(content).unwrap_err();
    assert!(err.to_string().contains("cycle"), "should mention cycle: {err}");
}

/// `${...}` inside a comment must not fail the undefined-variable check.
#[test]
fn substitute_variables_ignores_comments() {
    let content = "# example: x = \"${my_var}\"\n[other]\nkey = \"value\"\n";
    let result = substitute_variables(content).unwrap();
    assert!(result.contains("key = \"value\""));
}

// Tests for extract_var_names

/// Order is not part of the contract — the names are only ever used for
/// membership tests in the undefined-variable check.
fn sorted_var_names(content: &str) -> Vec<String> {
    let mut names = extract_var_names(content);
    names.sort();
    names
}

#[test]
fn extract_var_names_basic() {
    let content = "[vars]\nfoo = \"bar\"\nbaz = [1, 2]\n\n[other]\nkey = \"value\"\n";
    assert_eq!(sorted_var_names(content), vec!["baz", "foo"]);
}

/// A multi-line array whose items contain `=` must not contribute bogus
/// names. The old line-based scanner split each line on the first `=` and
/// recorded `"a` as a variable, which made the undefined-variable check
/// too permissive — a genuinely undefined `${a}` would pass.
#[test]
fn extract_var_names_ignores_equals_inside_array_items() {
    let content = "[vars]\npatterns = [\n  \"a=b\",\n  \"c=d\",\n]\nreal = \"x\"\n";
    assert_eq!(sorted_var_names(content), vec!["patterns", "real"]);
}

/// The consequence of the above, end to end: `${a}` is undefined even
/// though an array item happens to start with `a=`.
#[test]
fn equals_in_array_item_does_not_define_a_variable() {
    let content = "[vars]\npatterns = [\n  \"a=b\",\n]\n\n[processor.tera]\nsrc_dirs = [\"${a}\"]\n";
    let err = substitute_variables(content).unwrap_err().to_string();
    assert!(err.contains("Undefined variable"), "expected undefined-variable error, got: {err}");
}

#[test]
fn extract_var_names_no_vars_section() {
    let content = "[other]\nkey = \"value\"\n";
    let names = extract_var_names(content);
    assert!(names.is_empty());
}

#[test]
fn extract_var_names_empty_vars_section() {
    let content = "[vars]\n\n[other]\nkey = \"value\"\n";
    let names = extract_var_names(content);
    assert!(names.is_empty());
}

#[test]
fn extract_var_names_with_comments() {
    let content = "[vars]\n# This is a comment\nfoo = \"bar\"\n# Another comment\nbaz = 42\n";
    assert_eq!(sorted_var_names(content), vec!["baz", "foo"]);
}

#[test]
fn extract_var_names_with_whitespace() {
    let content = "[vars]\n  foo   =   \"bar\"\n\tbaz\t=\t42\n";
    assert_eq!(sorted_var_names(content), vec!["baz", "foo"]);
}

// Tests for substitute_variables

#[test]
fn substitute_variables_string() {
    let content = "[vars]\nmy_dir = \"templates\"\n\n[processor]\nsome_field = \"${my_dir}\"\n";
    let result = substitute_variables(content).expect("variable substitution failed");
    assert!(result.contains("some_field = \"templates\""));
    assert!(!result.contains("${my_dir}"));
    assert!(!result.contains("[vars]"));
}

#[test]
fn substitute_variables_array() {
    let content = "[vars]\nexcludes = [\"/a/\", \"/b/\"]\n\n[processor]\nsrc_exclude_dirs = \"${excludes}\"\n";
    let result = substitute_variables(content).expect("variable substitution failed");
    assert!(result.contains("src_exclude_dirs = [\"/a/\", \"/b/\"]"));
    assert!(!result.contains("${excludes}"));
}

#[test]
fn substitute_variables_multiple_uses() {
    let content = "[vars]\nval = \"shared\"\n\n[a]\nx = \"${val}\"\n\n[b]\ny = \"${val}\"\n";
    let result = substitute_variables(content).expect("variable substitution failed");
    assert!(result.contains("x = \"shared\""));
    assert!(result.contains("y = \"shared\""));
}

#[test]
fn substitute_variables_no_vars_section() {
    let content = "[processor]\nsome_field = \"src\"\n";
    let result = substitute_variables(content).expect("variable substitution failed");
    assert_eq!(result, content);
}

#[test]
fn substitute_variables_undefined_error() {
    let content = "[processor]\nsome_field = \"${undefined}\"\n";
    let result = substitute_variables(content);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("Undefined variable"));
    assert!(err.contains("undefined"));
}

#[test]
fn substitute_variables_undefined_with_vars_section() {
    let content = "[vars]\nfoo = \"bar\"\n\n[processor]\nx = \"${foo}\"\ny = \"${missing}\"\n";
    let result = substitute_variables(content);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(err.contains("missing"));
}

#[test]
fn substitute_variables_integer() {
    let content = "[vars]\ncount = 42\n\n[processor]\nvalue = \"${count}\"\n";
    let result = substitute_variables(content).expect("variable substitution failed");
    assert!(result.contains("value = 42"));
}

#[test]
fn substitute_variables_boolean() {
    let content = "[vars]\nenabled = true\n\n[processor]\nflag = \"${enabled}\"\n";
    let result = substitute_variables(content).expect("variable substitution failed");
    assert!(result.contains("flag = true"));
}

// Tests for the pre-construction config validators. These are the pass that
// runs before any processor or analyzer is created, so regressions here would
// push schema errors past config-load and into the Builder where they produce
// worse messages.

use crate::config::{validate_processor_fields_raw, validate_analyzer_fields_raw};

fn toml_of(s: &str) -> toml::Value {
    toml::from_str(s).expect("test fixture must be valid TOML")
}

#[test]
fn analyzer_validator_accepts_known_fields() {
    let raw = toml_of("[analyzer.python]\nenabled = false\n");
    let errors = validate_analyzer_fields_raw(&raw);
    assert!(errors.is_empty(), "unexpected errors: {errors:?}");
}

#[test]
fn analyzer_validator_accepts_empty_section() {
    // `[analyzer.python]` with no fields is valid — everything defaults.
    let raw = toml_of("[analyzer.python]\n");
    let errors = validate_analyzer_fields_raw(&raw);
    assert!(errors.is_empty(), "unexpected errors: {errors:?}");
}

#[test]
fn analyzer_validator_rejects_unknown_type() {
    let raw = toml_of("[analyzer.nonsense]\n");
    let errors = validate_analyzer_fields_raw(&raw);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("nonsense"));
    assert!(errors[0].contains("unknown analyzer type"));
}

#[test]
fn analyzer_validator_rejects_unknown_field() {
    let raw = toml_of("[analyzer.python]\nenabeld = false\n");
    let errors = validate_analyzer_fields_raw(&raw);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("enabeld"));
    assert!(errors[0].contains("unknown field"));
    // Error should list valid fields to help the user fix it.
    assert!(errors[0].contains("enabled"));
}

#[test]
fn analyzer_validator_collects_multiple_errors() {
    let raw = toml_of(r"
[analyzer.python]
enabeld = false

[analyzer.nonsense]
");
    let errors = validate_analyzer_fields_raw(&raw);
    assert!(errors.len() >= 2, "expected multiple errors, got: {errors:?}");
    assert!(errors.iter().any(|e| e.contains("enabeld")));
    assert!(errors.iter().any(|e| e.contains("nonsense")));
}

#[test]
fn analyzer_validator_handles_multi_instance() {
    // `[analyzer.cpp.kernel]` and `[analyzer.cpp.userspace]` — multi-instance
    // syntax. Each sub-section must still reject unknown fields.
    let raw = toml_of(r#"
[analyzer.cpp.kernel]
include_paths = ["kernel/include"]

[analyzer.cpp.userspace]
typo_field = true
"#);
    let errors = validate_analyzer_fields_raw(&raw);
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("typo_field"));
    assert!(errors[0].contains("analyzer.cpp.userspace"));
}

#[test]
fn analyzer_validator_is_noop_without_analyzer_section() {
    let raw = toml_of("[processor.ruff]\nsrc_dirs = [\".\"]\n");
    let errors = validate_analyzer_fields_raw(&raw);
    assert!(errors.is_empty());
}

// Tests for merge_toml_values (rsconstruct.local.toml overlay semantics)

use crate::config::merge_toml_values;

#[test]
fn merge_tables_recursively() {
    let mut base = toml_of("[processor.mypy]\nsrc_dirs = [\"src\"]\nbatch = true\n");
    let overlay = toml_of("[processor.mypy]\nbatch = false\n");
    merge_toml_values(&mut base, overlay);
    let mypy = base.get("processor").unwrap().get("mypy").unwrap();
    // Untouched key survives; overlaid key is replaced.
    assert_eq!(mypy.get("src_dirs").unwrap().as_array().unwrap().len(), 1);
    assert_eq!(mypy.get("batch").unwrap().as_bool(), Some(false));
}

#[test]
fn merge_arrays_replace_wholesale() {
    let mut base = toml_of("[processor.ruff]\nsrc_dirs = [\"src\", \"config\"]\n");
    let overlay = toml_of("[processor.ruff]\nsrc_dirs = [\"scripts\"]\n");
    merge_toml_values(&mut base, overlay);
    let dirs = base.get("processor").unwrap().get("ruff").unwrap()
        .get("src_dirs").unwrap().as_array().unwrap().clone();
    assert_eq!(dirs, vec![toml::Value::String("scripts".into())]);
}

#[test]
fn merge_adds_overlay_only_sections() {
    let mut base = toml_of("[processor.tera]\n");
    let overlay = toml_of("[dependencies]\npip = [\"requests\"]\n\n[processor.ruff]\nsrc_dirs = [\"src\"]\n");
    merge_toml_values(&mut base, overlay);
    assert!(base.get("dependencies").is_some());
    assert!(base.get("processor").unwrap().get("tera").is_some());
    assert!(base.get("processor").unwrap().get("ruff").is_some());
}

#[test]
fn merge_scalar_replaces_scalar() {
    let mut base = toml_of("[build]\noutput_dir = \"out\"\n");
    let overlay = toml_of("[build]\noutput_dir = \"dist\"\n");
    merge_toml_values(&mut base, overlay);
    assert_eq!(base.get("build").unwrap().get("output_dir").unwrap().as_str(), Some("dist"));
}

#[test]
fn processor_and_analyzer_validators_are_independent() {
    // Processor errors and analyzer errors must both be reported — neither
    // short-circuits the other. This is the regression that would return if
    // somebody changed `Config::load` to `?` on the first validator.
    let raw = toml_of(r#"
[processor.ruff]
unknown_proc_field = "x"

[analyzer.python]
enabeld = false
"#);
    let proc_errors = validate_processor_fields_raw(&raw);
    let analyzer_errors = validate_analyzer_fields_raw(&raw);
    assert!(!proc_errors.is_empty(), "processor validator should have caught something");
    assert!(!analyzer_errors.is_empty(), "analyzer validator should have caught something");
}

/// A reference embedded in a larger string ("${base}/src") can't be
/// substituted (only whole quoted values are) — it must be a config error,
/// not a silent literal pass-through.
#[test]
fn substitute_variables_rejects_partial_references() {
    let content = "[vars]\nbase = \"proj\"\n\n[processor.tera]\nsrc_dirs = [\"${base}/src\"]\n";
    let err = substitute_variables(content).unwrap_err();
    assert!(err.to_string().contains("entire quoted value"),
        "expected the partial-reference error, got: {err}");
}

/// A partial reference to an *undefined* variable must also error — before
/// the residual scan it flowed through silently.
#[test]
fn substitute_variables_rejects_partial_undefined_references() {
    let content = "[processor.tera]\nsrc_dirs = [\"${nope}/src\"]\n";
    let err = substitute_variables(content).unwrap_err();
    assert!(err.to_string().contains("Unresolved variable reference"),
        "expected the residual-reference error, got: {err}");
}

// Schema-consistency tests.
//
// A processor's field schema is spread across hand-synced places: the
// config struct, `known_fields()`, `checksum_fields()`,
// `field_descriptions()`, and the `expected_field_type` table. Nothing in
// the type system keeps them in agreement, and they have drifted before
// (dead `*_bin` type arms for fields that no longer exist; checksum
// fields missing from type validation). These tests iterate every
// registered plugin and fail on any disagreement.

use crate::config::{expected_field_type, SCAN_CONFIG_FIELDS, STANDARD_EXTRA_FIELDS};
use crate::registries::processor::all_plugins;

/// Every field a processor declares must have a type-validation entry.
/// Without one, `--iset`/config type checking silently accepts anything
/// for that field.
#[test]
fn every_known_field_has_an_expected_type() {
    let mut missing: Vec<String> = Vec::new();
    for plugin in all_plugins() {
        for field in (plugin.known_fields)() {
            if expected_field_type(plugin.name, field).is_none() {
                missing.push(format!("{}.{field}", plugin.name));
            }
        }
    }
    missing.sort();
    assert!(missing.is_empty(),
        "known fields with no expected_field_type entry (type validation silently accepts anything): {missing:#?}");
}

/// Every processor-specific arm in `expected_field_type` must correspond to
/// a field some processor actually declares. A dead arm is drift: the field
/// was renamed or removed and the table was not updated.
#[test]
fn every_expected_type_arm_is_a_live_field() {
    // The table is a match, not data, so it can't be enumerated directly.
    // Instead check the inverse for the fields we can enumerate: for every
    // plugin, every field name that any OTHER plugin declares should either
    // be unknown to this processor's type table or be a field it declares.
    // This catches an arm keyed to a processor whose config lost the field.
    let mut dead: Vec<String> = Vec::new();
    for plugin in all_plugins() {
        let declared: std::collections::HashSet<&str> = (plugin.known_fields)().iter().copied()
            .chain(SCAN_CONFIG_FIELDS.iter().copied())
            .chain(STANDARD_EXTRA_FIELDS.iter().copied())
            .collect();
        // Field names that appear in any plugin's schema — the candidate set
        // the type table could plausibly be keyed on.
        for other in all_plugins() {
            for field in (other.known_fields)() {
                if !declared.contains(field)
                    && expected_field_type(plugin.name, field).is_some()
                    && !is_generic_field(field)
                {
                    dead.push(format!("{}.{field}", plugin.name));
                }
            }
        }
    }
    dead.sort();
    dead.dedup();
    assert!(dead.is_empty(),
        "expected_field_type has arms for fields the processor does not declare (renamed or removed?): {dead:#?}");
}

/// Fields handled by the generic (non-processor-specific) match arm apply
/// to every processor regardless of what it declares. Keep in sync with the
/// first `match field` block in `expected_field_type`.
fn is_generic_field(field: &str) -> bool {
    matches!(field,
        "src_dirs" | "src_extensions" | "src_exclude_dirs" | "src_exclude_files"
        | "src_exclude_paths" | "src_files" | "args" | "dep_inputs" | "dep_auto"
        | "max_jobs" | "enabled" | "batch" | "command" | "output_dir" | "formats")
}

/// Every checksum field must be a known field — a checksum entry naming a
/// field that doesn't exist silently drops out of the config hash, and a
/// missing entry for an output-affecting field means stale build outputs.
#[test]
fn every_checksum_field_is_known() {
    let mut bad: Vec<String> = Vec::new();
    for plugin in all_plugins() {
        let known: std::collections::HashSet<&str> = (plugin.known_fields)().iter().copied()
            .chain(SCAN_CONFIG_FIELDS.iter().copied())
            .chain(STANDARD_EXTRA_FIELDS.iter().copied())
            .collect();
        for field in (plugin.checksum_fields)() {
            if !known.contains(field) {
                bad.push(format!("{}.{field}", plugin.name));
            }
        }
    }
    bad.sort();
    assert!(bad.is_empty(),
        "checksum_fields naming fields not in known_fields (silently excluded from the config hash): {bad:#?}");
}

/// Every documented field must be a known field. A description for a
/// removed field is stale documentation surfaced by `processors defconfig`.
#[test]
fn every_described_field_is_known() {
    let mut bad: Vec<String> = Vec::new();
    for plugin in all_plugins() {
        let known: std::collections::HashSet<&str> = (plugin.known_fields)().iter().copied()
            .chain(SCAN_CONFIG_FIELDS.iter().copied())
            .chain(STANDARD_EXTRA_FIELDS.iter().copied())
            .collect();
        for (field, _) in (plugin.field_descriptions)() {
            if !known.contains(field) {
                bad.push(format!("{}.{field}", plugin.name));
            }
        }
    }
    bad.sort();
    assert!(bad.is_empty(),
        "field_descriptions naming fields not in known_fields (stale docs): {bad:#?}");
}

/// Every declared field needs a description — `processors defconfig` shows a
/// blank cell for anything missing one, and a user has no way to learn what
/// the field does.
#[test]
fn every_known_field_has_a_description() {
    use crate::config::SHARED_FIELD_DESCRIPTIONS;
    let mut missing: Vec<String> = Vec::new();
    for plugin in all_plugins() {
        let described: std::collections::HashSet<&str> = (plugin.field_descriptions)().iter()
            .map(|(f, _)| *f)
            .chain(SHARED_FIELD_DESCRIPTIONS.iter().map(|(f, _)| *f))
            .chain(SCAN_CONFIG_FIELDS.iter().copied())
            .collect();
        for field in (plugin.known_fields)() {
            if !described.contains(field) {
                missing.push(format!("{}.{field}", plugin.name));
            }
        }
    }
    missing.sort();
    missing.dedup();
    assert!(missing.is_empty(),
        "known fields with no description (blank cell in `processors defconfig`): {missing:#?}");
}

// Tests for processor section shape classification (finding 11)

use crate::config::{ProcessorConfig, SectionShape};

fn classify(toml_src: &str) -> SectionShape {
    let value: toml::Value = toml::from_str(toml_src).unwrap();
    let table = value.get("processor").unwrap()
        .get("pylint").unwrap()
        .as_table().unwrap();
    ProcessorConfig::classify_section("pylint", table)
}

#[test]
fn section_with_config_fields_is_single_instance() {
    assert_eq!(
        classify("[processor.pylint]\nargs = [\"--x\"]\n"),
        SectionShape::SingleInstance,
    );
}

#[test]
fn section_with_only_subtables_is_multi_instance() {
    assert_eq!(
        classify("[processor.pylint.core]\nargs = [\"--x\"]\n\n[processor.pylint.tests]\nargs = [\"--y\"]\n"),
        SectionShape::MultiInstance,
    );
}

#[test]
fn empty_section_is_single_instance() {
    assert_eq!(classify("[processor.pylint]\n"), SectionShape::SingleInstance);
}

/// An instance named after a known config field reads as both shapes. This
/// used to silently resolve to single-instance — which meant adding a config
/// field in a future release could retroactively change how an existing
/// user's config parsed. It is now rejected, naming the colliding key.
#[test]
fn instance_named_after_a_config_field_is_ambiguous() {
    let shape = classify(
        "[processor.pylint.args]\nargs = [\"--x\"]\n\n[processor.pylint.other]\nargs = [\"--y\"]\n"
    );
    match shape {
        SectionShape::Ambiguous { colliding } => {
            assert_eq!(colliding, vec!["args"]);
        }
        other => panic!("expected Ambiguous, got {other:?}"),
    }
}

/// Scan fields count as known fields too — they are appended to every
/// processor's field list during validation, so an instance named
/// `src_dirs` is just as ambiguous as one named `args`.
#[test]
fn instance_named_after_a_scan_field_is_ambiguous() {
    let shape = classify(
        "[processor.pylint.src_dirs]\nargs = [\"--x\"]\n\n[processor.pylint.other]\nargs = [\"--y\"]\n"
    );
    assert!(matches!(shape, SectionShape::Ambiguous { .. }), "got {shape:?}");
}

/// A single known field holding a table is config, not an ambiguity —
/// there is no second sub-table for it to be an instance alongside.
/// (Both readings exist in principle; the error message tells the user how
/// to disambiguate, so being conservative here would be pure noise.)
#[test]
fn mixed_scalar_and_table_values_are_single_instance() {
    assert_eq!(
        classify("[processor.pylint]\nargs = [\"--x\"]\n\n[processor.pylint.core]\nargs = [\"--y\"]\n"),
        SectionShape::SingleInstance,
    );
}
