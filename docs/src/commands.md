# Command Reference

## Global Flags

These flags can be used with any command:

| Flag | Description |
|------|-------------|
| `--verbose`, `-v` | Show skip/restore/cache messages during build |
| `--output-display`, `-O` | What to show for output files (`none`, `basename`, `path`; default: `none`) |
| `--input-display`, `-I` | What to show for input files (`none`, `source`, `all`; default: `source`) |
| `--path-format`, `-P` | Path format for displayed files (`basename`, `path`; default: `path`) |
| `--show-child-processes` | Print each child process command before execution |
| `--show-output` | Show tool output even on success (default: only show on failure) |
| `--json` | Output in JSON Lines format (machine-readable) |
| `--quiet`, `-q` | Suppress all output except errors (useful for CI) |
| `--phases` | Show build phase messages (discover, add_dependencies, etc.) |

Example:

```bash
rsconstruct --phases build                    # Show phase messages during build
rsconstruct --show-child-processes build      # Show each command being executed
rsconstruct --show-output build               # Show compiler/linter output even on success
rsconstruct --phases --show-child-processes build # Show both phases and commands
rsconstruct -O path build                     # Show output file paths in build messages
rsconstruct -I all build                      # Show all input files (including headers)
```

## `rsconstruct build`

**Requires config.** (no subcommands)

Incremental build — only rebuilds products whose inputs have changed.

```bash
rsconstruct build                              # Incremental build
rsconstruct build --force                      # Force full rebuild
rsconstruct build -j4                          # Build with 4 parallel jobs
rsconstruct build --dry-run                    # Show what would be built without executing
rsconstruct build --keep-going                 # Continue after errors
rsconstruct build --timings                    # Show per-product and total timing info
rsconstruct build --stop-after discover        # Stop after product discovery
rsconstruct build --stop-after add-dependencies # Stop after dependency scanning
rsconstruct build --stop-after resolve         # Stop after graph resolution
rsconstruct build --stop-after classify        # Stop after classifying products
rsconstruct build --show-output                # Show compiler/linter output even on success
rsconstruct build --auto-add-words             # Add misspelled words to .zspell-words instead of failing
rsconstruct build --auto-add-words -p zspell   # Run only zspell and auto-add words
rsconstruct build -p ruff,pylint               # Run only specific processors
rsconstruct build --explain                    # Show why each product is skipped/restored/rebuilt
rsconstruct build --retry 3                    # Retry failed products up to 3 times
rsconstruct build --no-mtime                   # Disable mtime pre-check, always compute checksums
rsconstruct build --no-summary                 # Suppress the build summary
rsconstruct build --batch-size 10              # Limit batch size for batch-capable processors
rsconstruct build --verify-tool-versions       # Verify tool versions against .tools.versions
rsconstruct build -t "src/*.c"                 # Only build products matching this glob pattern
rsconstruct build -d src                       # Only build products under this directory
rsconstruct build --show-all-config-changes    # Show all config changes, not just output-affecting
```

By default, tool output (compiler messages, linter output) is only shown when a command fails. Use `--show-output` to see all output.

### Incremental recovery and batch behavior

By default (fail-fast mode), rsconstruct executes each product independently, even for batch-capable processors. Successfully completed products are cached immediately, so if a build fails or is interrupted, the next run only rebuilds what wasn't completed.

With `--keep-going`, batch-capable processors group all their products into a single tool invocation. If the tool fails, all products in the batch are marked failed and must be rebuilt. Use `--batch-size N` to limit batch chunks and improve recovery granularity.

### Processor Shortcuts (`@` aliases)

The `-p` flag supports `@`-prefixed shortcuts that expand to groups of processors:

**By type:**
- `@checkers` — all checker processors (ruff, pylint, shellcheck, etc.)
- `@generators` — all generator processors (tera, cc_single_file, etc.)
- `@creators` — all creator processors (pip, npm, cargo, etc.)

**By tool:**
- `@python3` — all processors that require `python3`
- `@node` — all processors that require `node`
- Any tool name works (matched against each processor's `required_tools()`)

**By processor name:**
- `@ruff` — equivalent to `ruff` (strips the `@` prefix)

Examples:

```bash
rsconstruct build -p @checkers              # Run only checker processors
rsconstruct build -p @generators            # Run only generator processors
rsconstruct build -p @python3               # Run all Python-based processors
rsconstruct build -p @checkers,tera         # Mix shortcuts with processor names
```

The `--stop-after` flag allows stopping the build at a specific phase:
- `discover` — stop after discovering products (before dependency scanning)
- `add-dependencies` — stop after adding dependencies (before resolving graph)
- `resolve` — stop after resolving the dependency graph (before execution)
- `classify` — stop after classifying products (show skip/restore/build counts)
- `build` — run the full build (default)

## `rsconstruct clean`

Clean build artifacts. When run without a subcommand, removes build output files (same as `rsconstruct clean outputs`).

| Subcommand | Config required? |
|------------|-----------------|
| `outputs` | Yes |
| `all` | Yes |
| `git` | Yes |
| `unknown` | Yes |

```bash
rsconstruct clean                # Remove build output files (preserves cache) [default]
rsconstruct clean outputs        # Remove build output files (preserves cache)
rsconstruct clean outputs -p tera,tags    # Only clean outputs from named processors
rsconstruct clean outputs --no-empty-dirs # Skip the empty-directory sweep
rsconstruct clean all            # Remove out/ and .rsconstruct/ directories
rsconstruct clean git            # Hard clean using git clean -qffxd (requires git repository)
rsconstruct clean unknown        # Remove files not tracked by git and not known as build outputs
rsconstruct clean unknown --dry-run      # Show what would be removed without deleting
rsconstruct clean unknown --no-gitignore # Include gitignored files as unknown
```

### `rsconstruct clean outputs`

Removes build output files. The cache (`.rsconstruct/`) is preserved.

What gets removed depends on each product's processor type:

- **Checkers** (ruff, cppcheck, shellcheck, terms, …) — produce no outputs, so nothing to delete.
- **Generators** (tera, tags, mako, jinja2, mermaid, pdflatex, pdfunite, …) — declare concrete output files. Each declared output file is removed individually with `fs::remove_file`. **Directories are never recursed into.**
- **Creators** (mdbook, sphinx, jekyll, cargo, npm, gem, cc, …) — invoke an external build that produces an unknown set of files inside a declared output directory (e.g. `book/`, `_site/`, `target/`). The whole declared `output_dir` is removed recursively. This is the only path that recursively deletes a directory, and it only ever affects directories the creator itself declared.
- **Explicit** — declares both `output_files` and `output_dirs` directly in `rsconstruct.toml`. Files are removed individually; declared `output_dirs` are removed recursively (because the user asked for that).
- **Lua plugins** — call the plugin's own `clean()` function if defined; otherwise fall back to file-only removal.

#### Empty-directory sweep

After every product's `clean()` has run, `clean outputs` walks every directory that previously contained an output file (and the parents of every removed `output_dir`), bottom-up, and removes any that are now empty. The sweep:

- Tries `fs::remove_dir` (non-recursive) — only succeeds when the directory is empty.
- Walks upward through parents until it hits a non-empty directory or the project root.
- Never removes a directory that still contains files — empty dirs only.
- Does not special-case `out/`. Any path ancestor of a cleaned output is eligible.

Pass `--no-empty-dirs` to skip the sweep entirely. Files still get cleaned; empty parent directories are left in place.

#### Filtering by processor

`-p` / `--processors` accepts a comma-separated list of processor type names (e.g. `tera`, `cargo`). Only products whose processor type matches are cleaned. Multi-instance configs share a type name, so `-p tera` cleans every `[processor.tera.*]` instance together.

#### Other clean variants

- `clean all` — removes `out/` and `.rsconstruct/` outright (cache included). Use when you want a full reset.
- `clean git` — runs `git clean -qffxd`. Requires a git repository.
- `clean unknown` — finds files in the working tree that are neither git-tracked nor declared as a build output. Defaults to dry-run unless you pass without `--dry-run`. `--no-gitignore` includes gitignored files in the search.

## `rsconstruct status`

**Requires config.** (no subcommands)

Show product status — whether each product is up-to-date, stale, or restorable from cache.

```bash
rsconstruct status                     # Show per-processor and total summary
rsconstruct status -v                  # Show per-product status
rsconstruct status --breakdown         # Show source file counts by processor and extension
```

## `rsconstruct smart auto`

Auto-detect relevant processors and add them to `rsconstruct.toml`. Scans the project for files matching each processor's conventions and checks that the required tools are installed. Only adds new sections — existing processor sections are preserved. **Requires config.**

```bash
rsconstruct smart auto
```

Example output:

```
Added 3 processor(s): pylint, ruff, shellcheck
```

## `rsconstruct init`

**No config needed.** (no subcommands)

Initialize a new rsconstruct project in the current directory.

```bash
rsconstruct init
```

## `rsconstruct watch`

**Requires config.** (no subcommands)

Watch source files and auto-rebuild on changes.

```bash
rsconstruct watch                              # Watch and rebuild on changes
rsconstruct watch --auto-add-words             # Watch with zspell auto-add words
rsconstruct watch -j4                          # Watch with 4 parallel jobs
rsconstruct watch -p ruff                      # Watch and only run the ruff processor
```

The watch command accepts the same build flags as `rsconstruct build` (e.g., `--jobs`, `--keep-going`, `--timings`, `--processors`, `--batch-size`, `--explain`, `--retry`, `--no-mtime`, `--no-summary`).

## `rsconstruct graph`

Display the build dependency graph.

| Subcommand | Config required? |
|------------|-----------------|
| `show` | Yes |
| `view` | Yes |
| `stats` | Yes |

```bash
rsconstruct graph show                    # Default SVG format
rsconstruct graph show --format dot       # Graphviz DOT format
rsconstruct graph show --format mermaid   # Mermaid format
rsconstruct graph show --format json      # JSON format
rsconstruct graph show --format text      # Plain text hierarchical view
rsconstruct graph show --format svg       # SVG format (requires Graphviz dot)
rsconstruct graph view                    # Open as SVG (default viewer)
rsconstruct graph view --viewer mermaid   # Open as HTML with Mermaid in browser
rsconstruct graph view --viewer svg       # Generate and open SVG using Graphviz dot
rsconstruct graph stats                   # Show graph statistics (products, processors, dependencies)
```

## `rsconstruct cache`

Manage the build cache.

| Subcommand | Config required? |
|------------|-----------------|
| `clear` | No |
| `size` | Yes |
| `trim` | Yes |
| `list` | Yes |
| `stale` | Yes |
| `stats` | Yes |
| `remove-stale` | Yes |

```bash
rsconstruct cache clear         # Clear the entire cache
rsconstruct cache size          # Show cache size
rsconstruct cache trim          # Remove unreferenced objects
rsconstruct cache list          # List all cache entries and their status
rsconstruct cache stale         # Show which cache entries are stale vs current
rsconstruct cache stats         # Show per-processor cache statistics
rsconstruct cache remove-stale  # Remove stale index entries not matching any current product
```

## `rsconstruct webcache`

Manage the web request cache. Schemas fetched by `iyamlschema` (and any future processors that fetch URLs) are cached in `.rsconstruct/webcache.redb`.

| Subcommand | Config required? |
|------------|-----------------|
| `clear` | No |
| `stats` | No |
| `list` | No |

```bash
rsconstruct webcache clear   # Clear all cached web responses
rsconstruct webcache stats   # Show cache size and entry count
rsconstruct webcache list    # List all cached URLs and their sizes
```

## `rsconstruct deps`

Show or manage source file dependencies from the dependency cache. The cache is populated during builds when dependency analyzers scan source files (e.g., C/C++ headers, Python imports).

| Subcommand | Config required? |
|------------|-----------------|
| `list` | No |
| `used` | Yes |
| `build` | Yes |
| `config` | Yes |
| `show` | Yes |
| `stats` | Yes |
| `clean` | Yes |

```bash
rsconstruct deps list                          # List all available dependency analyzers
rsconstruct deps build                         # Run dependency analysis without building
rsconstruct deps show all                    # Show all cached dependencies
rsconstruct deps show files src/main.c       # Show dependencies for a specific file
rsconstruct deps show files src/a.c src/b.c  # Show dependencies for multiple files
rsconstruct deps show analyzers cpp          # Show dependencies from the C/C++ analyzer
rsconstruct deps show analyzers cpp python   # Show dependencies from multiple analyzers
rsconstruct deps stats                       # Show statistics by analyzer
rsconstruct deps clean                       # Clear the entire dependency cache
rsconstruct deps clean --analyzer cpp        # Clear only C/C++ dependencies
rsconstruct deps clean --analyzer python     # Clear only Python dependencies
```

Example output for `rsconstruct deps show all`:

```
src/main.c: [cpp] (no dependencies)
src/test.c: [cpp]
  src/utils.h
  src/config.h
config/settings.py: [python]
  config/base.py
```

Example output for `rsconstruct deps stats`:

```
cpp: 15 files, 42 dependencies
python: 8 files, 12 dependencies

Total: 23 files, 54 dependencies
```

Note: This command reads directly from the dependency cache (`.rsconstruct/deps.redb`). If the cache is empty, run a build first to populate it.

This command is useful for:
- Debugging why a file is being rebuilt
- Understanding the include/import structure of your project
- Verifying that dependency analyzers are finding the right files
- Viewing statistics about cached dependencies by analyzer
- Clearing dependencies for a specific analyzer without affecting others

## `rsconstruct smart`

Smart config manipulation commands for managing processor sections in `rsconstruct.toml`.

| Subcommand | Config required? |
|------------|-----------------|
| `disable-all` | No |
| `enable-all` | No |
| `enable` | No |
| `disable` | No |
| `only` | No |
| `reset` | No |
| `enable-detected` | Yes |
| `enable-if-available` | Yes |
| `minimal` | Yes |
| `auto` | Yes |
| `remove-no-file-processors` | Yes |

```bash
rsconstruct smart enable pylint          # Add [processor.pylint] section
rsconstruct smart disable pylint         # Remove [processor.pylint] section
rsconstruct smart enable-all             # Add sections for all builtin processors
rsconstruct smart disable-all            # Remove all processor sections
rsconstruct smart enable-detected        # Add sections for auto-detected processors
rsconstruct smart enable-if-available    # Add sections for detected processors with tools installed
rsconstruct smart minimal                # Remove all, then add only detected processors
rsconstruct smart only ruff pylint       # Remove all, then add only listed processors
rsconstruct smart reset                  # Remove all processor sections
rsconstruct smart remove-no-file-processors  # Remove processors that don't match any files
```

## `rsconstruct processors`

| Subcommand | Config required? |
|------------|-----------------|
| `list --all` | No |
| `list` | Yes (without `--all`) |
| `defconfig` | No |
| `config` | Uses config if available |
| `used` | Yes |
| `files` | Yes |
| `allowlist` | Yes |
| `graph` | Yes |

```bash
rsconstruct processors list              # List declared processors and descriptions
rsconstruct processors list -a           # Show all built-in processors
rsconstruct processors files             # Show source and target files for each declared processor
rsconstruct processors files ruff        # Show files for a specific processor
rsconstruct processors files              # Show files for enabled processors
rsconstruct processors config ruff       # Show resolved configuration for a processor
rsconstruct processors config --diff     # Show only fields that differ from defaults
rsconstruct processors defconfig ruff    # Show default configuration for a processor
rsconstruct processors add ruff          # Append [processor.ruff] to rsconstruct.toml (fields pre-populated + comments)
rsconstruct processors add ruff --dry-run  # Preview the snippet without writing
rsconstruct processors allowlist         # Show the current processor allowlist
rsconstruct processors graph             # Show inter-processor dependencies
rsconstruct processors graph --format dot    # Graphviz DOT format
rsconstruct processors graph --format mermaid # Mermaid format
rsconstruct processors files --headers   # Show files with processor headers
```

## `rsconstruct tools`

List or check external tools required by declared processors. All subcommands use config if available; without config, they operate on all built-in processors.

| Subcommand | Config required? |
|------------|-----------------|
| `list` | Uses config if available |
| `check` | Uses config if available |
| `lock` | Uses config if available |
| `install` | Uses config if available |
| `install-deps` | Uses config if available |
| `stats` | Uses config if available |
| `graph` | Uses config if available |

```bash
rsconstruct tools list              # List all tools known to rsconstruct (from the central registry)
rsconstruct tools list -M           # ...with every install method for each tool
rsconstruct tools list-configured   # List tools this project's processors require, and which needs them
rsconstruct tools list-configured -a   # Include tools from disabled processors
rsconstruct tools check             # Verify tool versions against .tools.versions lock file
rsconstruct tools lock              # Lock tool versions to .tools.versions
rsconstruct tools install           # Install missing external tools for enabled processors
rsconstruct tools install ruff      # Install a specific tool by name
rsconstruct tools install --all     # Install every tool in the registry, ignoring the config (no config needed)
rsconstruct tools install -i        # Ask for confirmation first (default: install without asking)
rsconstruct tools install --no-eatmydata        # Don't wrap apt/dnf/pacman with eatmydata
rsconstruct tools install-deps      # Install declared [dependencies] in fixed order: system → pip → npm → gem
rsconstruct tools install-deps -i   # Ask for confirmation first (default: install without asking)
rsconstruct tools install-deps --no-eatmydata   # Don't wrap apt/dnf/pacman with eatmydata
rsconstruct tools stats             # Show tool availability and language runtime breakdown
rsconstruct tools stats --json      # Show tool stats in JSON format
rsconstruct tools graph             # Show tool-to-processor dependency graph (DOT format)
rsconstruct tools graph --format mermaid  # Mermaid format
rsconstruct tools graph --view      # Open tool graph in browser
```

`install --all` walks the central registry instead of the project config, so it
needs no `rsconstruct.toml`. It never skips: a tool with no automatable install
method is a hard error, not a warning. This makes it the intended way to
provision CI — one command covers the whole matrix, and a registry entry that
loses its install method fails the provisioning step instead of quietly
shrinking the matrix. `--all` cannot be combined with a tool name.

## `rsconstruct tags`

Search and query frontmatter tags from markdown files.

| Subcommand | Config required? |
|------------|-----------------|
| `list` | Yes |
| `count` | Yes |
| `tree` | Yes |
| `stats` | Yes |
| `files` | Yes |
| `grep` | Yes |
| `for-file` | Yes |
| `frontmatter` | Yes |
| `unused` | Yes |
| `validate` | Yes |
| `matrix` | Yes |
| `coverage` | Yes |
| `orphans` | Yes |
| `check` | Yes |
| `suggest` | Yes |
| `merge` | Yes |
| `collect` | Yes |

```bash
rsconstruct tags list                        # List all unique tags
rsconstruct tags count                       # Show each tag with file count, sorted by frequency
rsconstruct tags tree                        # Show tags grouped by prefix/category
rsconstruct tags stats                       # Show statistics about the tags database
rsconstruct tags files docker                # List files matching a tag (AND semantics)
rsconstruct tags files docker --or k8s       # List files matching any tag (OR semantics)
rsconstruct tags files level:advanced        # Match key:value tags
rsconstruct tags grep deploy                 # Search for tags containing a substring
rsconstruct tags grep deploy -i              # Case-insensitive tag search
rsconstruct tags for-file src/main.md        # List all tags for a specific file
rsconstruct tags frontmatter src/main.md     # Show raw frontmatter for a file
rsconstruct tags validate                    # Validate tags against tags_dir allowlist
rsconstruct tags unused                      # List tags in tags_dir not used by any file
rsconstruct tags unused --strict             # Exit with error if unused tags found (CI)
rsconstruct tags check                       # Run all tag validations without building
rsconstruct tags suggest src/main.md         # Suggest tags for a file based on similarity
rsconstruct tags coverage                    # Show percentage of files with each tag category
rsconstruct tags matrix                      # Show coverage matrix of tag categories per file
rsconstruct tags orphans                     # Find markdown files with no tags
rsconstruct tags merge ../other/tags         # Merge tags from another project
rsconstruct tags collect                     # Add missing tags from source files to tag collection
```

## `rsconstruct complete`

Generate shell completions. No config needed when shell is specified as argument; uses config to read default shells if no argument given.

```bash
rsconstruct complete bash    # Generate bash completions
rsconstruct complete zsh     # Generate zsh completions
rsconstruct complete fish    # Generate fish completions
```

## `rsconstruct terms`

Manage term checking and fixing in markdown files.

| Subcommand | Config required? |
|------------|-----------------|
| `fix` | Yes |
| `merge` | Yes |
| `stats` | Yes |

### `rsconstruct terms fix`

Add backticks around terms from the terms directory that appear unquoted in markdown files.

```bash
rsconstruct terms fix
rsconstruct terms fix --remove-non-terms   # also remove backticks from non-terms
```

### `rsconstruct terms merge`

Merge terms from another project's terms directory. Unions matching files and copies missing files in both directions.

```bash
rsconstruct terms merge ../other-project/terms
```

## `rsconstruct doctor`

**Requires config.** (no subcommands)

Diagnose build environment — checks config, tools, versions, and declared
`[dependencies]`. Tools are probed by running them; `system` dependencies are
packages, not tools, so they are probed through the platform's package manager
(dpkg-query/rpm/pacman/brew) — a binary-less package like `aspell-en` counts
as installed even though no binary carries its name.

```bash
rsconstruct doctor
```

## `rsconstruct info`

Show project information.

| Subcommand | Config required? |
|------------|-----------------|
| `source` | Yes |

```bash
rsconstruct info source          # Show source file counts by extension
```

## `rsconstruct sloc`

**No config needed.** (no subcommands)

Count source lines of code (SLOC) by language, with optional COCOMO effort/cost estimation.

```bash
rsconstruct sloc                 # Show SLOC by language
rsconstruct sloc --cocomo        # Include COCOMO effort/cost estimation
rsconstruct sloc --cocomo --salary 80000  # Custom annual salary for COCOMO
```

## `rsconstruct version`

**No config needed.** (no subcommands)

Print version information.

```bash
rsconstruct version
```
