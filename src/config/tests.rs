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

/// Provenance reports user-set fields as `rsconstruct.toml:<line>`, and it
/// walks the *substituted* text. If substitution changed the line count,
/// every reported line number after the substitution point would be wrong.
/// This pins the whole-pipeline invariant across the value shapes most likely
/// to break it: a multi-element array, a table, and strings containing real
/// newlines and tabs.
#[test]
fn line_preservation_invariant_holds() {
    let content = concat!(
        "[vars]\n",
        "dirs = [\"a\", \"b\", \"c\"]\n",
        "multiline = \"one\\ntwo\\nthree\"\n",
        "tbl = { x = 1, y = 2 }\n",
        "\n",
        "[processor.ruff]\n",
        "src_dirs = \"${dirs}\"\n",
        "note = \"${multiline}\"\n",
        "opts = \"${tbl}\"\n",
    );
    let before = content.lines().count();
    let result = substitute_variables(content).unwrap();
    assert_eq!(
        result.lines().count(), before,
        "substitution changed the line count, which silently corrupts every \
         provenance line number below the change:\n{result}"
    );
    // And the substituted values must still be on their original lines.
    let lines: Vec<&str> = result.lines().collect();
    assert!(lines[6].contains("[\"a\", \"b\", \"c\"]"), "line 7 was: {}", lines[6]);
    assert!(!lines[7].contains('\n'), "embedded newline leaked into the output");
    assert!(lines[7].contains("\\n"), "newlines must be escaped, line 8 was: {}", lines[7]);
}

/// The blanking half of the same invariant, isolated: `remove_vars_section`
/// must blank the `[vars]` lines rather than delete them.
#[test]
fn remove_vars_section_preserves_line_count() {
    let content = "[vars]\na = \"1\"\nb = \"2\"\n\n[processor.ruff]\nsrc_dirs = [\"src\"]\n";
    let before = content.lines().count();
    let result = remove_vars_section(content);
    assert_eq!(result.lines().count(), before, "vars removal must not shift lines");
    assert!(!result.contains("[vars]"), "the section header must be gone");
    // The surviving section must still be at its original line index.
    let lines: Vec<&str> = result.lines().collect();
    assert_eq!(lines[4], "[processor.ruff]", "line 5 moved: {lines:?}");
}

/// `value_to_toml_inline` is the function that would break the invariant
/// first, so assert its no-newline contract directly over every value shape.
#[test]
fn value_to_toml_inline_never_emits_newlines() {
    let cases = vec![
        toml::Value::String("has\nnewline\rand\ttab".to_string()),
        toml::Value::Array(vec![
            toml::Value::String("a\nb".to_string()),
            toml::Value::Integer(2),
        ]),
        toml::Value::Table({
            let mut t = toml::map::Map::new();
            t.insert("k".to_string(), toml::Value::String("v\nw".to_string()));
            t
        }),
        toml::Value::Boolean(true),
        toml::Value::Integer(42),
    ];
    for case in cases {
        let out = value_to_toml_inline(&case);
        assert!(
            !out.contains('\n') && !out.contains('\r'),
            "value_to_toml_inline emitted a line break for {case:?}: {out:?}"
        );
    }
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

// Schema tests.
//
// The field schema is the plugin's FieldSpec list — known/checksum/must
// fields, descriptions, and expected types are all projections of it, so
// the old cross-list drift class (dead type arms, checksum entries naming
// removed fields, undocumented fields) is impossible by construction. The
// properties that remain checkable are the ones the data itself can get
// wrong.

use crate::config::SCAN_CONFIG_FIELDS;
use crate::registries::processor::all_plugins;

/// `FieldSpec` entries must be well-formed: non-empty name, non-empty doc
/// (a blank doc is a blank cell in `processors defconfig`), no duplicate
/// names within a plugin, and no collision with scan fields (scan fields
/// are validated generically; a spec shadowing one would create two
/// authorities for its type).
#[test]
fn every_field_spec_is_well_formed() {
    let mut bad: Vec<String> = Vec::new();
    for plugin in all_plugins() {
        let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for spec in plugin.fields {
            if spec.name.is_empty() {
                bad.push(format!("{}: empty field name", plugin.name));
            }
            if spec.doc.is_empty() {
                bad.push(format!("{}.{}: empty doc", plugin.name, spec.name));
            }
            if !seen.insert(spec.name) {
                bad.push(format!("{}.{}: duplicate FieldSpec", plugin.name, spec.name));
            }
            if SCAN_CONFIG_FIELDS.contains(&spec.name) {
                bad.push(format!("{}.{}: FieldSpec shadows a scan field", plugin.name, spec.name));
            }
        }
        for omitted in plugin.omit_standard_fields {
            use crate::config::KnownFields as _;
            if !crate::config::StandardConfig::known_fields().contains(omitted) {
                bad.push(format!("{}: omit_standard_fields names non-standard field '{omitted}'", plugin.name));
            }
        }
    }
    bad.sort();
    assert!(bad.is_empty(), "malformed FieldSpec entries: {bad:#?}");
}

/// A processor is one file — but three touch-points necessarily live
/// outside it, and nothing but this test enforces them: the docs page, the
/// integration test file, and (checked separately below) the `mod`
/// declaration. A missing docs page or test file is invisible to the
/// compiler; `prettier` once shipped silently half-registered exactly this
/// way. Grandfathered gaps are allowlisted — shrink the lists, never grow
/// them.
#[test]
fn every_plugin_has_docs_and_tests() {
    const DOCS_ALLOWLIST: &[&str] = &[
        "creator", "duplicate_files", "encoding", "ijq", "ijsonlint",
        "ipdfunite", "isass", "itaplo", "iyamllint", "license_header",
        "marp_images", "prettier", "svglint", "svgo",
    ];
    const TESTS_ALLOWLIST: &[&str] = &[
        "checkpatch", "chromium", "cpplint", "encoding", "explicit",
        "ijq", "ijsonlint", "imarkdown2html", "ipdfunite", "isass",
        "itaplo", "iyamllint", "license_header", "linux_module",
        "markdown2html", "marp_images", "objdump", "prettier", "yaml2json",
    ];

    let mut missing: Vec<String> = Vec::new();
    for plugin in all_plugins() {
        let name = plugin.name;
        if !DOCS_ALLOWLIST.contains(&name)
            && !std::path::Path::new(&format!("docs/src/processors/{name}.md")).exists()
        {
            missing.push(format!("{name}: no docs/src/processors/{name}.md"));
        }
        if !TESTS_ALLOWLIST.contains(&name)
            && !std::path::Path::new(&format!("tests/processors/{name}.rs")).exists()
        {
            missing.push(format!("{name}: no tests/processors/{name}.rs"));
        }
    }
    missing.sort();
    assert!(missing.is_empty(),
        "processors missing their out-of-file touch-points (add the file, or — for \
         pre-existing gaps only — the allowlist entry): {missing:#?}");
}

/// Every `.rs` file under `src/processors/<category>/` must be declared in
/// that category's `mod.rs`. A forgotten `mod` line is the one-file
/// design's silent kill switch: the file compiles as dead code, submits
/// nothing to inventory, and the processor simply does not exist.
#[test]
fn every_processor_file_is_declared() {
    let mut missing: Vec<String> = Vec::new();
    for dir in ["src/processors/checkers", "src/processors/generators",
                "src/processors/creators", "src/processors/explicit",
                "src/processors/lua"] {
        let mod_src = std::fs::read_to_string(format!("{dir}/mod.rs")).unwrap();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else { continue };
            if stem == "mod" || path.extension().is_none_or(|e| e != "rs") {
                continue;
            }
            if !mod_src.contains(&format!("mod {stem};")) {
                missing.push(format!("{dir}/{stem}.rs not declared in {dir}/mod.rs"));
            }
        }
    }
    missing.sort();
    assert!(missing.is_empty(),
        "processor files invisible to the build (missing mod declaration): {missing:#?}");
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

/// `required_tools` reaches `required_tools()`, so a wrapper script's real
/// tool is installable and version-lockable.
///
/// Without it, a `command` that shells out to something else leaves that tool
/// invisible: `tools install` reports nothing missing and the build fails
/// later, inside the wrapper. veltzer.github.io hit exactly this -- its
/// `command` is a Python script that runs zola.
#[test]
fn required_tools_is_a_known_standard_field() {
    use crate::config::KnownFields as _;
    assert!(
        crate::config::StandardConfig::known_fields().contains(&"required_tools"),
        "required_tools must be a known field or the validator rejects it"
    );
}

/// The field round-trips from TOML rather than being silently dropped by serde.
#[test]
fn required_tools_deserializes_from_toml() {
    let cfg: crate::config::StandardConfig =
        toml::from_str("command = \"scripts/build.py\"\nrequired_tools = [\"zola\"]\n")
            .expect("should deserialize");
    assert_eq!(cfg.required_tools, vec!["zola".to_string()]);
}

/// Defaults to empty, so every existing stanza keeps its current behaviour.
#[test]
fn required_tools_defaults_to_empty() {
    let cfg: crate::config::StandardConfig =
        toml::from_str("command = \"eslint\"\n").expect("should deserialize");
    assert!(cfg.required_tools.is_empty());
}

// Tests for pyproject.toml dependency reading (effective_pip and friends)

#[test]
fn normalized_distribution_name_strips_specifiers_extras_markers() {
    use crate::config::normalized_distribution_name as norm;
    assert_eq!(norm("Flask"), "flask");
    assert_eq!(norm("types_PyYAML"), "types-pyyaml");
    assert_eq!(norm("ruamel.yaml"), "ruamel-yaml");
    assert_eq!(norm("requests>=2.0"), "requests");
    assert_eq!(norm("uvicorn[standard]==0.30"), "uvicorn");
    assert_eq!(norm("tomli; python_version < \"3.11\""), "tomli");
    assert_eq!(norm("A.-_b"), "a-b");
}

#[test]
fn pyproject_python_deps_missing_file_is_empty() {
    let tmp = tempfile::TempDir::new().unwrap();
    let deps = crate::config::pyproject_python_deps(&tmp.path().join("pyproject.toml"))
        .expect("missing file should not error");
    assert!(deps.is_empty());
}

#[test]
fn pyproject_python_deps_collects_all_sections() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("pyproject.toml");
    std::fs::write(&path, r#"
[project]
name = "demo"
dependencies = ["flask", "requests>=2.0"]

[project.optional-dependencies]
docs = ["sphinx"]

[dependency-groups]
dev = ["mypy", {include-group = "test"}]
test = ["pytest"]
"#).unwrap();
    let deps = crate::config::pyproject_python_deps(&path).expect("should parse");
    let mut sorted = deps.clone();
    sorted.sort();
    assert_eq!(sorted, vec!["flask", "mypy", "pytest", "requests>=2.0", "sphinx"]);
    // include-group tables are skipped, not errors, and groups are read anyway
    assert!(deps.contains(&"pytest".to_string()));
}

#[test]
fn pyproject_python_deps_invalid_toml_errors() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("pyproject.toml");
    std::fs::write(&path, "not [ valid toml").unwrap();
    assert!(crate::config::pyproject_python_deps(&path).is_err());
}

#[test]
fn effective_pip_pyproject_mode_merges_and_dedupes_by_normalized_name() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(tmp.path().join("pyproject.toml"), r#"
[project]
name = "demo"
dependencies = ["Flask", "gunicorn"]
"#).unwrap();
    let deps = crate::config::DependenciesConfig {
        pip: vec!["flask==2.0".to_string(), "types-requests".to_string()],
        pip_source: crate::config::PipSource::Pyproject,
        ..Default::default()
    };
    let merged = deps.effective_pip(tmp.path()).expect("should merge");
    // [dependencies].pip first and its pinned flask wins over pyproject's Flask
    assert_eq!(merged, vec!["flask==2.0", "types-requests", "gunicorn"]);
}

#[test]
fn effective_pip_without_pyproject_is_pip_list() {
    let tmp = tempfile::TempDir::new().unwrap();
    let deps = crate::config::DependenciesConfig {
        pip: vec!["termcolor".to_string()],
        ..Default::default()
    };
    // No pyproject and no lock: nothing to install beyond the pip list, in
    // either mode — the default uv-lock mode must not error here.
    let merged = deps.effective_pip(tmp.path()).expect("no pyproject is fine");
    assert_eq!(merged, vec!["termcolor"]);
}

#[test]
fn uv_lock_pinned_deps_pins_registry_packages_and_skips_the_project() {
    let tmp = tempfile::TempDir::new().unwrap();
    let lock = tmp.path().join("uv.lock");
    std::fs::write(&lock, r#"
version = 1
requires-python = ">=3.14"

[[package]]
name = "demo"
version = "0.1.0"
source = { editable = "." }

[[package]]
name = "flask"
version = "3.1.0"
source = { registry = "https://pypi.org/simple" }

[[package]]
name = "gunicorn"
version = "23.0.0"
source = { registry = "https://pypi.org/simple" }
"#).unwrap();
    let pins = crate::config::uv_lock_pinned_deps(&lock).expect("should parse");
    assert_eq!(pins, vec!["flask==3.1.0", "gunicorn==23.0.0"]);
}

#[test]
fn uv_lock_pinned_deps_rejects_unknown_source_kinds() {
    let tmp = tempfile::TempDir::new().unwrap();
    let lock = tmp.path().join("uv.lock");
    std::fs::write(&lock, r#"
version = 1

[[package]]
name = "somelib"
version = "1.0.0"
source = { git = "https://example.com/somelib.git" }
"#).unwrap();
    assert!(crate::config::uv_lock_pinned_deps(&lock).is_err());
}

#[test]
fn effective_pip_default_mode_installs_the_lock_closure() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(tmp.path().join("pyproject.toml"), r#"
[project]
name = "demo"
dependencies = ["flask"]
"#).unwrap();
    std::fs::write(tmp.path().join("uv.lock"), r#"
version = 1

[[package]]
name = "demo"
version = "0.1.0"
source = { virtual = "." }

[[package]]
name = "flask"
version = "3.1.0"
source = { registry = "https://pypi.org/simple" }

[[package]]
name = "werkzeug"
version = "3.1.3"
source = { registry = "https://pypi.org/simple" }
"#).unwrap();
    let deps = crate::config::DependenciesConfig {
        pip: vec!["flask==2.0".to_string()],
        ..Default::default()
    };
    let merged = deps.effective_pip(tmp.path()).expect("should merge");
    // The pip-list entry wins over the lock pin; the transitive closure
    // (werkzeug) is installed even though pyproject never names it.
    assert_eq!(merged, vec!["flask==2.0", "werkzeug==3.1.3"]);
}

#[test]
fn effective_pip_default_mode_errors_without_a_lock() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(tmp.path().join("pyproject.toml"), r#"
[project]
name = "demo"
dependencies = ["flask"]
"#).unwrap();
    let deps = crate::config::DependenciesConfig::default();
    let err = deps.effective_pip(tmp.path()).unwrap_err().to_string();
    assert!(err.contains("uv lock"), "error should point at uv lock: {err}");
    assert!(err.contains("pip_source"), "error should mention the escape hatch: {err}");
}

#[test]
fn requirement_extras_parses_and_normalizes() {
    use crate::config::requirement_extras as ex;
    assert_eq!(ex("manim-voiceover[gtts]"), "gtts");
    assert_eq!(ex("uvicorn[standard,Watchfiles]>=0.30"), "standard,watchfiles");
    assert_eq!(ex("uvicorn[watchfiles, standard]"), "standard,watchfiles");
    assert_eq!(ex("flask"), "");
}

#[test]
fn effective_pip_keeps_extras_variant_distinct_from_bare_name() {
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(tmp.path().join("pyproject.toml"), r#"
[project]
name = "demo"
dependencies = ["manim_voiceover", "manim-voiceover[gtts]"]
"#).unwrap();
    let deps = crate::config::DependenciesConfig {
        pip_source: crate::config::PipSource::Pyproject,
        ..Default::default()
    };
    let merged = deps.effective_pip(tmp.path()).expect("should merge");
    assert_eq!(merged, vec!["manim_voiceover", "manim-voiceover[gtts]"]);
}
