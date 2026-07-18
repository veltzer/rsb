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

#[test]
fn extract_var_names_basic() {
    let content = "[vars]\nfoo = \"bar\"\nbaz = [1, 2]\n\n[other]\nkey = \"value\"\n";
    let names = extract_var_names(content);
    assert_eq!(names, vec!["foo", "baz"]);
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
    let names = extract_var_names(content);
    assert_eq!(names, vec!["foo", "baz"]);
}

#[test]
fn extract_var_names_with_whitespace() {
    let content = "[vars]\n  foo   =   \"bar\"\n\tbaz\t=\t42\n";
    let names = extract_var_names(content);
    assert_eq!(names, vec!["foo", "baz"]);
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
    let raw = toml_of(r#"
[analyzer.python]
enabeld = false

[analyzer.nonsense]
"#);
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
