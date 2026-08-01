use anyhow::{Context, Result};
use regex::Regex;
use std::sync::OnceLock;

use crate::errors;

/// Convert a toml::Value to its inline TOML string representation.
/// This is used for variable substitution to insert values into the config.
pub(super) fn value_to_toml_inline(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n").replace('\r', "\\r").replace('\t', "\\t")),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Array(arr) => {
            let items: Vec<String> = arr.iter().map(value_to_toml_inline).collect();
            format!("[{}]", items.join(", "))
        }
        toml::Value::Table(table) => {
            let items: Vec<String> = table
                .iter()
                .map(|(k, v)| format!("{} = {}", k, value_to_toml_inline(v)))
                .collect();
            format!("{{ {} }}", items.join(", "))
        }
        toml::Value::Datetime(dt) => dt.to_string(),
    }
}

/// Check if a trimmed line is a TOML section header (e.g., `[section]` or `[section] # comment`).
fn is_section_header(trimmed: &str) -> bool {
    if !trimmed.starts_with('[') {
        return false;
    }
    // Strip trailing comment: "[section] # comment" -> "[section]"
    let header_part = trimmed.split('#').next().unwrap_or(trimmed).trim_end();
    header_part.ends_with(']')
}

/// Check if a trimmed line is specifically the `[vars]` section header.
fn is_vars_header(trimmed: &str) -> bool {
    if !trimmed.starts_with("[vars]") {
        return false;
    }
    // Allow trailing whitespace/comments: "[vars]", "[vars] # comment"
    let rest = trimmed["[vars]".len()..].trim_start();
    rest.is_empty() || rest.starts_with('#')
}

/// Remove the [vars] section from TOML content by blanking its lines.
/// Blanking (instead of deleting) keeps every remaining line at its original
/// line number, so provenance spans built from the result stay correct.
pub(super) fn remove_vars_section(content: &str) -> String {
    let mut in_vars_section = false;
    let lines: Vec<&str> = content.lines()
        .map(|line| {
            let trimmed = line.trim();
            if is_vars_header(trimmed) {
                in_vars_section = true;
                return "";
            }
            if in_vars_section && is_section_header(trimmed) {
                in_vars_section = false;
            }
            if in_vars_section { "" } else { line }
        })
        .collect();
    let mut result = lines.join("\n");
    result.push('\n');
    result
}

/// Strip a TOML line comment (a `#` outside of any string literal).
/// Used only for the undefined-variable scan, so `${...}` inside comments
/// doesn't fail config loading.
fn strip_toml_comment(line: &str) -> &str {
    let mut in_basic = false;   // "..."
    let mut in_literal = false; // '...'
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' if in_basic => escaped = true,
            '"' if !in_literal => in_basic = !in_basic,
            '\'' if !in_basic => in_literal = !in_literal,
            '#' if !in_basic && !in_literal => return &line[..i],
            _ => {}
        }
    }
    line
}

/// Resolve a var value, following `${name}` references to other vars.
/// Nested references resolve regardless of definition order; cycles error out.
fn resolve_var_value(
    value: &toml::Value,
    vars: &toml::map::Map<String, toml::Value>,
    depth: usize,
) -> Result<toml::Value> {
    if depth > 32 {
        return Err(crate::exit_code::RsconstructError::new(
            crate::exit_code::RsconstructExitCode::ConfigError,
            "Variable reference cycle in [vars] (nesting exceeds 32 levels)".to_string(),
        ).into());
    }
    match value {
        toml::Value::String(s) => {
            if let Some(name) = s.strip_prefix("${").and_then(|r| r.strip_suffix('}'))
                && !name.contains('}') {
                let Some(referenced) = vars.get(name) else {
                    return Err(crate::exit_code::RsconstructError::new(
                        crate::exit_code::RsconstructExitCode::ConfigError,
                        format!("Undefined variable: ${{{name}}}"),
                    ).into());
                };
                return resolve_var_value(referenced, vars, depth + 1);
            }
            Ok(value.clone())
        }
        toml::Value::Array(arr) => arr.iter()
            .map(|v| resolve_var_value(v, vars, depth + 1))
            .collect::<Result<Vec<_>>>()
            .map(toml::Value::Array),
        toml::Value::Table(table) => table.iter()
            .map(|(k, v)| resolve_var_value(v, vars, depth + 1).map(|rv| (k.clone(), rv)))
            .collect::<Result<toml::map::Map<_, _>>>()
            .map(toml::Value::Table),
        _ => Ok(value.clone()),
    }
}

/// Extract the variable names defined in the `[vars]` section.
///
/// Parses the TOML rather than scanning lines. An unsubstituted `${var}` is
/// just string content as far as TOML is concerned, so parsing here is safe
/// — the older line scanner predates that realization and split each line on
/// the first `=`, which recorded junk names for any multi-line array whose
/// items contained one:
///
/// ```toml
/// [vars]
/// patterns = [
///     "a=b",     # recorded a bogus variable named `"a`
/// ]
/// ```
///
/// Junk names made the undefined-variable check too permissive: a genuinely
/// undefined `${a}` would pass validation because a bogus `a` was "defined".
///
/// Returns an empty list when the content does not parse or has no `[vars]`
/// section. A parse error is not reported here — the caller parses again
/// immediately after and produces a far better message.
pub(super) fn extract_var_names(content: &str) -> Vec<String> {
    let Ok(parsed) = toml::from_str::<toml::Value>(content) else {
        return Vec::new();
    };
    let Some(vars) = parsed.get("vars").and_then(toml::Value::as_table) else {
        return Vec::new();
    };
    vars.keys().cloned().collect()
}

/// Substitute variables defined in [vars] section throughout the config.
/// Variables are referenced using `${var_name}` syntax.
/// The entire `"${var_name}"` (including quotes) is replaced with the TOML-serialized value.
/// After substitution, the [vars] section is removed from the output.
pub(super) fn substitute_variables(content: &str) -> Result<String> {
    // Check for undefined variables first (before any TOML parsing)
    // This gives a clear error message for undefined vars even without a [vars] section
    // Matches quoted variable references like "${var_name}" (including the surrounding double quotes,
    // since variables in TOML values are written as "value" = "${var}").
    static VAR_PATTERN: OnceLock<Regex> = OnceLock::new();
    let var_pattern = VAR_PATTERN.get_or_init(|| Regex::new(r#""\$\{([^}]+)\}""#).expect(errors::INVALID_REGEX));

    // Extract defined variable names before TOML parsing
    let defined_vars = extract_var_names(content);

    // Check for undefined variable references, ignoring `${...}` in comments
    for line in content.lines() {
        for captures in var_pattern.captures_iter(strip_toml_comment(line)) {
            let var_name = captures.get(1).expect(errors::CAPTURE_GROUP_MISSING).as_str();
            if !defined_vars.iter().any(|v| v == var_name) {
                return Err(crate::exit_code::RsconstructError::new(
                    crate::exit_code::RsconstructExitCode::ConfigError,
                    format!("Undefined variable: ${{{var_name}}}"),
                ).into());
            }
        }
    }

    // If no vars defined, return content as-is (we already checked for undefined refs above)
    if defined_vars.is_empty() {
        check_no_residual_references(content)?;
        return Ok(content.to_string());
    }

    // Parse just to extract [vars] section values
    let parsed: toml::Value = toml::from_str(content)
        .context("Failed to parse TOML for variable extraction")?;

    let Some(vars) = parsed.get("vars").and_then(|v| v.as_table()) else {
        check_no_residual_references(content)?;
        return Ok(content.to_string());
    };

    let mut result = content.to_string();

    // Replace "${var_name}" (including quotes) with the TOML-serialized value.
    // Values are fully resolved first so a var referencing another var works
    // regardless of definition order.
    for (name, value) in vars {
        let pattern = format!("\"${{{name}}}\"");
        let resolved = resolve_var_value(value, vars, 0)?;
        let replacement = value_to_toml_inline(&resolved);
        result = result.replace(&pattern, &replacement);
    }

    // Remove the [vars] section from the result
    let result = remove_vars_section(&result);

    check_no_residual_references(&result)?;
    Ok(result)
}

/// Error on any `${...}` left after substitution. The engine only replaces
/// whole quoted values (`"${var}"`), so a partial reference like
/// `"${base}/src"` matches neither the substitution nor the quoted
/// undefined-var scan and would otherwise flow through silently as a
/// literal. Comments are exempt, like everywhere else in this pipeline.
fn check_no_residual_references(content: &str) -> Result<()> {
    static RESIDUAL_PATTERN: OnceLock<Regex> = OnceLock::new();
    let residual = RESIDUAL_PATTERN.get_or_init(|| Regex::new(r"\$\{([^}]+)\}").expect(errors::INVALID_REGEX));
    for line in content.lines() {
        if let Some(captures) = residual.captures(strip_toml_comment(line)) {
            let var_name = captures.get(1).expect(errors::CAPTURE_GROUP_MISSING).as_str();
            return Err(crate::exit_code::RsconstructError::new(
                crate::exit_code::RsconstructExitCode::ConfigError,
                format!(
                    "Unresolved variable reference ${{{var_name}}}: a reference must be the entire quoted value (\"${{{var_name}}}\"), not embedded in a larger string"
                ),
            ).into());
        }
    }
    Ok(())
}
