use std::collections::HashMap;
use std::fs;
use std::path::Path;

use anyhow::Result;
use crate::tables;

use crate::file_index::FileIndex;
use crate::json_output::{self, SlocCocomoEstimate, SlocLanguageEntry, SlocOutput, SlocTotals};

struct CommentStyle {
    single: &'static [&'static str],
    block_start: Option<&'static str>,
    block_end: Option<&'static str>,
}

static COMMENT_C_STYLE: CommentStyle = CommentStyle {
    single: &["//"],
    block_start: Some("/*"),
    block_end: Some("*/"),
};

static COMMENT_HASH: CommentStyle = CommentStyle {
    single: &["#"],
    block_start: None,
    block_end: None,
};

static COMMENT_HTML: CommentStyle = CommentStyle {
    single: &[],
    block_start: Some("<!--"),
    block_end: Some("-->"),
};

static COMMENT_DASH: CommentStyle = CommentStyle {
    single: &["--"],
    block_start: None,
    block_end: None,
};

static COMMENT_PERCENT: CommentStyle = CommentStyle {
    single: &["%"],
    block_start: None,
    block_end: None,
};

static COMMENT_SEMICOLON: CommentStyle = CommentStyle {
    single: &[";"],
    block_start: None,
    block_end: None,
};

static COMMENT_NONE: CommentStyle = CommentStyle {
    single: &[],
    block_start: None,
    block_end: None,
};

struct LanguageInfo {
    name: &'static str,
    comment: &'static CommentStyle,
}

fn language_map() -> HashMap<&'static str, LanguageInfo> {
    let mut m = HashMap::new();

    let c_style = &COMMENT_C_STYLE;
    let hash = &COMMENT_HASH;
    let html = &COMMENT_HTML;
    let dash = &COMMENT_DASH;
    let percent = &COMMENT_PERCENT;
    let semicolon = &COMMENT_SEMICOLON;
    let none = &COMMENT_NONE;

    let entries: &[(&[&str], &str, &CommentStyle)] = &[
        (&[".rs"], "Rust", c_style),
        (&[".c", ".h"], "C", c_style),
        (&[".cc", ".cpp", ".cxx", ".hh", ".hpp", ".hxx"], "C++", c_style),
        (&[".cs"], "C#", c_style),
        (&[".java"], "Java", c_style),
        (&[".js", ".mjs", ".cjs"], "JavaScript", c_style),
        (&[".ts", ".mts", ".cts"], "TypeScript", c_style),
        (&[".tsx", ".jsx"], "TSX/JSX", c_style),
        (&[".go"], "Go", c_style),
        (&[".swift"], "Swift", c_style),
        (&[".kt", ".kts"], "Kotlin", c_style),
        (&[".scala"], "Scala", c_style),
        (&[".zig"], "Zig", c_style),
        (&[".d"], "D", c_style),
        (&[".css", ".scss", ".less"], "CSS", c_style),
        (&[".proto"], "Protobuf", c_style),
        (&[".py", ".pyi"], "Python", hash),
        (&[".rb"], "Ruby", hash),
        (&[".sh", ".bash", ".zsh"], "Shell", hash),
        (&[".pl", ".pm"], "Perl", hash),
        (&[".r", ".R"], "R", hash),
        (&[".nim"], "Nim", hash),
        (&[".jl"], "Julia", hash),
        (&[".toml"], "TOML", hash),
        (&[".yaml", ".yml"], "YAML", hash),
        (&[".php"], "PHP", c_style),
        (&[".lua"], "Lua", dash),
        (&[".sql"], "SQL", dash),
        (&[".hs"], "Haskell", dash),
        (&[".ex", ".exs"], "Elixir", hash),
        (&[".erl", ".hrl"], "Erlang", percent),
        (&[".tex", ".sty"], "LaTeX", percent),
        (&[".el", ".lisp", ".cl", ".scm"], "Lisp", semicolon),
        (&[".html", ".htm"], "HTML", html),
        (&[".xml", ".xsl", ".xsd", ".svg"], "XML", html),
        (&[".vue"], "Vue", html),
        (&[".md", ".markdown"], "Markdown", none),
        (&[".json"], "JSON", none),
        (&[".tera"], "Tera", c_style),
        (&[".mako", ".mak"], "Mako", hash),
        (&[".cmake"], "CMake", hash),
        (&[".gradle"], "Gradle", c_style),
    ];

    for (exts, name, comment) in entries {
        for ext in *exts {
            m.insert(*ext, LanguageInfo { name, comment });
        }
    }

    // Special filename mappings (no extension dot prefix — matched by exact filename)
    m.insert("Makefile", LanguageInfo { name: "Makefile", comment: hash });
    m.insert("Dockerfile", LanguageInfo { name: "Dockerfile", comment: hash });
    m.insert("CMakeLists.txt", LanguageInfo { name: "CMake", comment: hash });
    m.insert("Vagrantfile", LanguageInfo { name: "Ruby", comment: hash });
    m.insert("Rakefile", LanguageInfo { name: "Ruby", comment: hash });
    m.insert("Gemfile", LanguageInfo { name: "Ruby", comment: hash });

    m
}

struct LineCounts {
    blank: usize,
    comment: usize,
    code: usize,
}

fn count_lines(path: &Path, comment: &CommentStyle) -> LineCounts {
    let Ok(content) = fs::read_to_string(path) else {
        return LineCounts { blank: 0, comment: 0, code: 0 };
    };

    let mut blank = 0usize;
    let mut comment_lines = 0usize;
    let mut code = 0usize;
    let mut in_block = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            blank += 1;
            continue;
        }

        if in_block {
            comment_lines += 1;
            if let Some(end) = comment.block_end
                && trimmed.contains(end)
            {
                in_block = false;
            }
            continue;
        }

        // Check single-line comments
        if comment.single.iter().any(|prefix| trimmed.starts_with(prefix)) {
            comment_lines += 1;
            continue;
        }

        // Check block comment start
        if let Some(start) = comment.block_start
            && let Some(rest) = trimmed.strip_prefix(start)
        {
            comment_lines += 1;
            if let Some(end) = comment.block_end
                && !rest.contains(end)
            {
                in_block = true;
            }
            continue;
        }

        code += 1;
    }

    LineCounts { blank, comment: comment_lines, code }
}

struct LanguageStats {
    files: usize,
    blank: usize,
    comment: usize,
    code: usize,
}

pub fn run_sloc(file_index: &FileIndex, cocomo: bool, salary: u64) -> Result<()> {
    let lang_map = language_map();
    let mut stats: HashMap<&str, LanguageStats> = HashMap::new();

    for path in file_index.files() {
        // Try extension first, then exact filename
        let info = path.extension()
            .and_then(|ext| ext.to_str())
            .and_then(|ext| {
                let dot_ext = format!(".{ext}");
                // We need to look up by the dot-prefixed extension
                // but our map keys include the dot already
                lang_map.get(dot_ext.as_str())
            })
            .or_else(|| {
                path.file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|name| lang_map.get(name))
            });

        let Some(info) = info else { continue };

        let counts = count_lines(path, info.comment);

        let entry = stats.entry(info.name).or_insert(LanguageStats {
            files: 0, blank: 0, comment: 0, code: 0,
        });
        entry.files += 1;
        entry.blank += counts.blank;
        entry.comment += counts.comment;
        entry.code += counts.code;
    }

    // Sort by code lines descending
    let mut sorted: Vec<(&str, &LanguageStats)> = stats.iter().map(|(k, v)| (*k, v)).collect();
    sorted.sort_by_key(|(_, stats)| std::cmp::Reverse(stats.code));

    // Totals
    let total_files: usize = sorted.iter().map(|(_, s)| s.files).sum();
    let total_blank: usize = sorted.iter().map(|(_, s)| s.blank).sum();
    let total_comment: usize = sorted.iter().map(|(_, s)| s.comment).sum();
    let total_code: usize = sorted.iter().map(|(_, s)| s.code).sum();

    // COCOMO
    let cocomo_estimate = if cocomo {
        let ksloc = total_code as f64 / 1000.0;
        let effort = 2.4 * ksloc.powf(1.05);
        let schedule = 2.5 * effort.powf(0.38);
        let people = if schedule > 0.0 { effort / schedule } else { 0.0 };
        let cost = effort * (salary as f64 / 12.0);
        Some(SlocCocomoEstimate {
            effort_person_months: effort,
            schedule_months: schedule,
            people,
            cost,
            salary,
        })
    } else {
        None
    };

    if json_output::is_json_mode() {
        let output = SlocOutput {
            languages: sorted.iter().map(|(name, s)| SlocLanguageEntry {
                language: name.to_string(),
                files: s.files,
                blank: s.blank,
                comment: s.comment,
                code: s.code,
            }).collect(),
            total: SlocTotals {
                files: total_files,
                blank: total_blank,
                comment: total_comment,
                code: total_code,
            },
            cocomo: cocomo_estimate,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        let rows: Vec<Vec<String>> = sorted.iter().map(|(name, s)| {
            vec![
                name.to_string(), s.files.to_string(), s.blank.to_string(),
                s.comment.to_string(), s.code.to_string(),
            ]
        }).collect();
        let total = vec![
            "Total".to_string(), total_files.to_string(), total_blank.to_string(),
            total_comment.to_string(), total_code.to_string(),
        ];
        tables::print_table_with_total(
            &["Language", "Files", "Blank", "Comment", "Code"],
            &rows,
            &total,
        );

        if let Some(ref est) = cocomo_estimate {
            println!();
            println!("COCOMO Estimation (Basic Organic):");
            println!("  Effort:   {:.1} person-months", est.effort_person_months);
            println!("  Schedule: {:.1} months", est.schedule_months);
            println!("  People:   {:.1}", est.people);
            println!("  Cost:     ${:.0} (at ${}/yr salary)", est.cost, est.salary);
        }
    }

    Ok(())
}
