# No-Shell Policy

## Rule

When rsconstruct spawns a subprocess, it MUST do so via direct `argv` execution
— `Command::new(prog).args([...])` — and not via a shell interpreter such as
`sh -c`, `bash -c`, `zsh -c`, or any equivalent. Routing a command through a
shell is forbidden by default. New code that adds a `Command::new("sh")`
(or other shell binary) call WILL be rejected in code review unless it falls
under one of the named exceptions in this document.

## Why

A shell exists to interpret a *string* as a *program*: it splits on
whitespace, expands variables, performs glob matching, evaluates pipelines,
substitutes backticks, redirects file descriptors, and so on. None of that
behavior is free — every metacharacter in the input is a hazard if any part
of the input was not authored by us.

There are three concrete problems with routing subprocess calls through a
shell.

### 1. It is a code-injection surface

If any segment of the command string came from a config file, an environment
variable, a file the user wrote, or even something that *looks* benign like
a package name, a single shell metacharacter in that segment lets the
substring break out and run arbitrary code with rsconstruct's privileges.

This is not theoretical. The bug that motivated the policy was this:

```
Running: [pip] setuptools<82, manim, manim_voiceover (pip install setuptools<82 manim manim_voiceover)
```

`setuptools<82` is a perfectly normal pip version specifier. When we ran the
install command via `sh -c "pip install setuptools<82 manim ..."`, the shell
interpreted `<82` as a redirection from a file named `82`, the command
failed, and a "harmless" config string had effectively become a
shell-redirection request. With argv execution, `setuptools<82` is one
argv element and reaches pip verbatim — no ambiguity, no injection
surface.

The same pattern shows up with package names containing `<`, `>`, `|`, `&`,
`;`, `$`, `` ` ``, `(`, `)`, `*`, `?`, `[`, `]`, newlines, or spaces. A
defense-in-depth project does not enumerate "the dangerous characters"; it
removes the interpreter that gives them meaning.

### 2. It hides bugs as features

When a shell silently expands a glob, evaluates a variable, or splits an
argument, the surrounding Rust code has no way to know that happened. The
program continues, and the side effect is invisible until something downstream
notices the wrong files were processed, the wrong package was installed, or
the wrong path was deleted. Argv execution makes the actual bytes that reach
the target program identical to the bytes our code constructed.

### 3. We do not need it

Every structured package manager rsconstruct talks to (apt, dnf, pacman,
brew, pip, npm, gem, cargo, snap) accepts package names and flags as separate
argv elements. A shell adds nothing to those calls except risk.

## Exceptions

This policy is "argv by default" not "argv always". A small number of
features in rsconstruct are *contractually* shell-shaped: the user is
deliberately writing a shell snippet and expects shell semantics. Removing
the shell from those features would break the feature.

The complete list of allowed shell call sites, as of this writing, is:

| Location                                                | Why a shell is required                                                                                   |
| ------------------------------------------------------- | --------------------------------------------------------------------------------------------------------- |
| `src/processors/generators/cc_single_file.rs` (two)     | `EXTRA_COMPILE_SHELL` / `EXTRA_LINK_SHELL` and backtick expansion in C source pragmas. Users write things like `pkg-config --cflags gtk+-3.0` in source comments and expect shell semantics. |
| `src/analyzers/mod.rs`                                  | `include_path_commands` config in the icpp analyzer. Documented to run via `sh -c` so users can write commands like `gcc -print-file-name=plugin`.                                          |
| `src/processors/generators/tera.rs`                     | The `shell_output()` Tera template function. The function name *is* the contract — it executes a shell command and returns the output for use in templates.                               |
| `src/processors/mod.rs` (`run` / `run_binary` in `tools install`) | Not a shell exception: installers run argv-style via `Command`, never through `sh -c`. They are the documented exception to the *central-runner* rule instead — they inherit the terminal so `sudo` can prompt and apt/dnf can render progress. |

Each exception above is **opt-in** from the user's perspective: the user
writes a string into a place documented as "this is shell syntax". None of
them route untrusted *names* (package names, file paths, identifiers)
through a shell.

## Adding a new exception

Don't, if you can help it. Before adding `Command::new("sh")` to the
codebase, ask:

1. **Can I express this as argv?** Almost always yes. A shell is rarely the
   shortest path; it is usually the most familiar one.
2. **Is the input a single argv token?** Then argv is strictly simpler than
   shelling out.
3. **Do I need pipes, redirects, or `&&`?** Then argv is harder, but Rust
   gives you `Stdio::piped()`, `Stdio::from()`, and full control over fds
   without invoking a shell. Use those.
4. **Is the input a user-authored shell snippet (template function, source
   pragma, config field documented as shell syntax)?** Then a shell is
   legitimate — but document the contract at the call site and add the
   call site to the table above in the same PR.

If the answer to (4) is yes, the call site MUST be commented to reference
this document, e.g.:

```rust
// Shell required: <feature name> contract — user writes shell syntax.
// See docs/src/internal/no-shell-policy.md.
Command::new("sh").arg("-c").arg(user_snippet).status()?
```

The reviewer's job is to decide whether the new exception is justified, and
the comment is what they read first.

## Mechanics

For the common case, prefer the structured types in `src/processors/mod.rs`:

- `InstallPlan::Argv(Vec<String>)` — direct argv execution. Default.
- `InstallPlan::Shell(String)` — explicit shell execution. Used only by the
  registry entries that store full shell pipelines as static data.

The variant tags the *intent* at construction time, not at execution time.
A reviewer reading `InstallPlan::Shell(...)` knows immediately that this
plan was authored as shell syntax and understands the trust boundary; a
reviewer reading `InstallPlan::Argv(...)` knows nothing went through an
interpreter. This separation is the whole point — it makes the rare,
audited shell case visually distinct from the safe default.

When constructing argv, do not interpolate untrusted data into a single
string and then split on whitespace. Push each logical argument as a
separate `Vec<String>` element. Spaces, quotes, version specifiers, and
glob characters in those elements are then carried verbatim to the target
program with no further interpretation.

## Audit

The repository is audited periodically with:

```bash
rg -n 'Command::new\("(sh|bash|zsh|/bin/sh)"\)' src/
```

Every match must correspond to a row in the exceptions table above. If a
match exists that is not in the table, either remove the shell call or
add the row (with a justification) in the same PR.
