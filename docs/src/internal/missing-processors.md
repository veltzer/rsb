# Missing Processors

Tools found in Makefiles across `../*/` sibling projects that rsconstruct does not yet have processors for.
Organized by category, with priority based on breadth of usage.

## High Priority — Linters and Validators

### eslint
- **What it does:** JavaScript/TypeScript linter (industry standard).
- **Projects:** demos-lang-js
- **Invocation:** `eslint $(ALL_JS)` or `node_modules/.bin/eslint $<`
- **Processor type:** Checker

### jshint
- **What it does:** JavaScript linter — detects errors and potential problems.
- **Projects:** demos-lang-js, gcp-gemini-cli, gcp-machines, gcp-miflaga, gcp-nikuda, gcp-randomizer, schemas, veltzer.github.io
- **Invocation:** `node_modules/.bin/jshint $<`
- **Processor type:** Checker

### tidy (HTML Tidy)
- **What it does:** HTML/XHTML validator and formatter.
- **Projects:** demos-lang-js, gcp-gemini-cli, gcp-machines, gcp-miflaga, gcp-nikuda, gcp-randomizer, openbook, riddles-book
- **Invocation:** `tidy -errors -quiet -config .tidy.config $<`
- **Processor type:** Checker

### check-jsonschema
- **What it does:** Validates YAML/JSON files against JSON Schema (distinct from rsconstruct's json_schema which validates JSON against schemas found via `$schema` key).
- **Projects:** data, schemas, veltzer.github.io
- **Invocation:** `check-jsonschema --schemafile $(yq -r '.["$schema"]' $<) $<`
- **Processor type:** Checker

### cpplint
- **What it does:** C++ linter enforcing Google C++ style guide.
- **Projects:** demos-os-linux
- **Invocation:** `cpplint $<`
- **Processor type:** Checker

### checkpatch.pl
- **What it does:** Linux kernel coding style checker.
- **Projects:** kcpp
- **Invocation:** `$(KDIR)/scripts/checkpatch.pl --file $(C_SOURCES) --no-tree`
- **Processor type:** Checker

### standard (StandardJS)
- **What it does:** JavaScript style guide, linter, and formatter — zero config.
- **Projects:** demos-lang-js
- **Invocation:** `node_modules/.bin/standard $<`
- **Processor type:** Checker

### jslint
- **What it does:** JavaScript code quality linter (Douglas Crockford).
- **Projects:** demos-lang-js
- **Invocation:** `node_modules/.bin/jslint $<`
- **Processor type:** Checker

### jsl (JavaScript Lint)
- **What it does:** JavaScript lint tool.
- **Projects:** keynote, myworld-php
- **Invocation:** `jsl --conf=support/jsl.conf --quiet --nologo --nosummary --nofilelisting $(SOURCES_JS)`
- **Processor type:** Checker

### gjslint (Google Closure Linter)
- **What it does:** JavaScript style checker following Google JS style guide.
- **Projects:** keynote, myworld-php
- **Invocation:** `$(TOOL_GJSLINT) --flagfile support/gjslint.cfg $(JS_SRC)`
- **Processor type:** Checker

### checkstyle
- **What it does:** Java source code style checker.
- **Projects:** demos-lang-java, keynote
- **Invocation:** `java -cp $(scripts/cp.py) $(MAINCLASS_CHECKSTYLE) -c support/checkstyle_config.xml $(find . -name "*.java")`
- **Processor type:** Checker

### pyre
- **What it does:** Python type checker from Facebook/Meta.
- **Projects:** archive.apiiro.TrainingDataLaboratory, archive.work-amdocs-py
- **Invocation:** `pyre check`
- **Processor type:** Checker

## High Priority — Formatters

### black
- **What it does:** Opinionated Python code formatter.
- **Projects:** archive.apiiro.TrainingDataLaboratory, archive.work-amdocs-py
- **Invocation:** `black --target-version py36 $(ALL_PACKAGES)`
- **Processor type:** Checker (using `--check` mode) or Formatter

### uncrustify
- **What it does:** C/C++/Java source code formatter.
- **Projects:** demos-os-linux, xmeltdown
- **Invocation:** `uncrustify -c support/uncrustify.cfg --no-backup -l C $(ALL_US_C)`
- **Processor type:** Formatter

### astyle (Artistic Style)
- **What it does:** C/C++/Java source code indenter and formatter.
- **Projects:** demos-os-linux
- **Invocation:** `astyle --verbose --suffix=none --formatted --preserve-date --options=support/astyle.cfg $(ALL_US)`
- **Processor type:** Formatter

### indent (GNU Indent)
- **What it does:** C source code formatter (GNU style).
- **Projects:** demos-os-linux
- **Invocation:** `indent $(ALL_US)`
- **Processor type:** Formatter

## High Priority — Testing

### pytest
- **What it does:** Python test framework.
- **Projects:** 50+ py* projects (pyanyzip, pyapikey, pyapt, pyawskit, pyblueprint, pybookmarks, pyclassifiers, pycmdtools, pyconch, pycontacts, pycookie, pydatacheck, pydbmtools, pydockerutils, pyeventroute, pyeventsummary, pyfakeuse, pyflexebs, pyfoldercheck, pygcal, pygitpub, pygooglecloud, pygooglehelper, pygpeople, pylogconf, pymakehelper, pymount, pymultienv, pymultigit, pymyenv, pynetflix, pyocutil, pypathutil, pypipegzip, pypitools, pypluggy, pypowerline, pypptkit, pyrelist, pyscrapers, pysigfd, pyslider, pysvgview, pytagimg, pytags, pytconf, pytimer, pytsv, pytubekit, pyunique, pyvardump, pyweblight, and archive.*)
- **Invocation:** `pytest tests` or `python -m pytest tests`
- **Processor type:** Checker (mass, per-directory)

## High Priority — YAML/JSON Processing

### yq
- **What it does:** YAML/JSON processor (like jq but for YAML).
- **Projects:** data, demos-lang-yaml, schemas, veltzer.github.io
- **Invocation:** `yq < $< > $@` (format/validate) or `yq -r '.key' $<` (extract)
- **Processor type:** Checker or Generator

## Medium Priority — Compilers

### javac
- **What it does:** Java compiler.
- **Projects:** demos-lang-java, jenable, keynote
- **Invocation:** `javac -Werror -Xlint:all $(JAVA_SOURCES) -d out/classes`
- **Processor type:** Generator

### go build
- **What it does:** Go language compiler.
- **Projects:** demos-lang-go
- **Invocation:** `go build -o $@ $<`
- **Processor type:** Generator (single-file, like cc_single_file)

### kotlinc
- **What it does:** Kotlin compiler.
- **Projects:** demos-lang-kotlin
- **Invocation:** `kotlinc $< -include-runtime -d $@`
- **Processor type:** Generator (single-file)

### ghc
- **What it does:** Glasgow Haskell Compiler.
- **Projects:** demos-lang-haskell
- **Invocation:** `ghc -v0 -o $@ $<`
- **Processor type:** Generator (single-file)

### ldc2
- **What it does:** D language compiler (LLVM-based).
- **Projects:** demos-lang-d
- **Invocation:** `ldc2 $(FLAGS) $< -of=$@`
- **Processor type:** Generator (single-file)

### nasm
- **What it does:** Netwide Assembler (x86/x64).
- **Projects:** demos-lang-nasm
- **Invocation:** `nasm -f $(ARCH) -o $@ $<`
- **Processor type:** Generator (single-file)

### rustc
- **What it does:** Rust compiler for single-file programs (as opposed to cargo for projects).
- **Projects:** demos-lang-rust
- **Invocation:** `rustc $(FLAGS_DBG) $< -o $@`
- **Processor type:** Generator (single-file)

### dotnet
- **What it does:** .NET SDK CLI — builds C#/F# projects.
- **Projects:** demos-lang-cs
- **Invocation:** `dotnet build --nologo --verbosity quiet`
- **Processor type:** MassGenerator

### dtc (Device Tree Compiler)
- **What it does:** Compiles device tree source (.dts) to device tree blob (.dtb) for embedded Linux.
- **Projects:** clients-heqa (8 subdirectories)
- **Invocation:** `dtc -I dts -O dtb -o $@ $<`
- **Processor type:** Generator (single-file)

## Medium Priority — Build Systems

### cmake
- **What it does:** Cross-platform build system generator.
- **Projects:** demos-build-cmake
- **Invocation:** `cmake -B $@ && cmake --build $@`
- **Processor type:** MassGenerator

### mvn (Apache Maven)
- **What it does:** Java project build and dependency management.
- **Projects:** demos-lang-java/maven
- **Invocation:** `mvn compile`
- **Processor type:** MassGenerator

### ant (Apache Ant)
- **What it does:** Java build tool (XML-based).
- **Projects:** demos-lang-java, keynote
- **Invocation:** `ant checkstyle`
- **Processor type:** MassGenerator

## Medium Priority — Converters and Generators

### pygmentize
- **What it does:** Syntax highlighter — converts source code to HTML, SVG, PNG.
- **Projects:** demos-misc-highlight
- **Invocation:** `pygmentize -f html -O full -o $@ $<`
- **Processor type:** Generator (single-file)

### slidev
- **What it does:** Markdown-based presentation tool — exports to PDF.
- **Projects:** demos-lang-slidev
- **Invocation:** `node_modules/.bin/slidev export $< --with-clicks --output $@`
- **Processor type:** Generator (single-file)

### jekyll
- **What it does:** Static site generator (Ruby-based, used by GitHub Pages).
- **Projects:** site-personal-jekyll
- **Invocation:** `jekyll build --source $(SOURCE_FOLDER) --destination $(DESTINATION_FOLDER)`
- **Processor type:** MassGenerator

### lilypond
- **What it does:** Music engraving program — compiles .ly files to PDF sheet music.
- **Projects:** demos-lang-lilypond, openbook
- **Invocation:** `scripts/wrapper_lilypond.py ... $<`
- **Processor type:** Generator (single-file)

### wkhtmltoimage
- **What it does:** Renders HTML to image using WebKit engine.
- **Projects:** demos-misc-highlight
- **Invocation:** `wkhtmltoimage $(WK_OPTIONS) $< $@`
- **Processor type:** Generator (single-file)

## Medium Priority — Documentation

### jsdoc
- **What it does:** API documentation generator for JavaScript.
- **Projects:** jschess, keynote
- **Invocation:** `node_modules/.bin/jsdoc -d $(JSDOC_FOLDER) -c support/jsdoc.json out/src`
- **Processor type:** MassGenerator

## Low Priority — Minifiers

### jsmin
- **What it does:** JavaScript minifier (removes whitespace and comments).
- **Projects:** jschess
- **Invocation:** `node_modules/.bin/jsmin < $< > $(JSMIN_JSMIN)`
- **Processor type:** Generator (single-file)

### yuicompressor
- **What it does:** JavaScript/CSS minifier and compressor (Yahoo).
- **Projects:** jschess
- **Invocation:** `node_modules/.bin/yuicompressor $< -o $(JSMIN_YUI)`
- **Processor type:** Generator (single-file)

### closure compiler
- **What it does:** JavaScript optimizer and minifier (Google Closure).
- **Projects:** keynote
- **Invocation:** `tools/closure.jar $< --js_output_file $@`
- **Processor type:** Generator (single-file)

## Low Priority — Preprocessors

### gpp (Generic Preprocessor)
- **What it does:** General-purpose text preprocessor with macro expansion.
- **Projects:** demos/gpp
- **Invocation:** `gpp -o $@ $<`
- **Processor type:** Generator (single-file)

### m4
- **What it does:** Traditional Unix macro processor.
- **Projects:** demos/m4
- **Invocation:** `m4 $< > $@`
- **Processor type:** Generator (single-file)

## Low Priority — Binary Analysis

### objdump
- **What it does:** Disassembles object files (displays assembly code).
- **Projects:** demos-os-linux
- **Invocation:** `objdump --disassemble --source $< > $@`
- **Processor type:** Generator (single-file, post-compile)

## Low Priority — Packaging

### dpkg-deb
- **What it does:** Builds Debian .deb packages.
- **Projects:** archive.myrepo
- **Invocation:** `dpkg-deb --build deb/mypackage ~/packages`
- **Processor type:** Generator

### reprepro
- **What it does:** Manages Debian APT package repositories.
- **Projects:** archive.myrepo
- **Invocation:** `reprepro --basedir $(config.apt.service_dir) export $(config.apt.codename)`
- **Processor type:** Generator

## Low Priority — Profiling

### pyinstrument
- **What it does:** Python profiler with HTML output.
- **Projects:** archive.apiiro.TrainingDataLaboratory, archive.work-amdocs-py
- **Invocation:** `pyinstrument --renderer=html -m $(MAIN_MODULE)`
- **Processor type:** Generator

## Low Priority — Code Metrics

### sloccount
- **What it does:** Counts source lines of code and estimates development cost.
- **Projects:** demos-lang-java, demos-lang-r, demos-os-linux, jschess
- **Invocation:** `sloccount .`
- **Processor type:** Checker (whole-project)

## Low Priority — Dependency Generation

### makedepend
- **What it does:** Generates C/C++ header dependency rules for Makefiles.
- **Projects:** xmeltdown
- **Invocation:** `makedepend -I... -- $(CFLAGS) -- $(SRC)`
- **Notes:** rsconstruct's built-in C/C++ dependency analyzer already handles this.

## Low Priority — Embedded

### fdtoverlay
- **What it does:** Applies device tree overlays to a base device tree blob.
- **Projects:** clients-heqa/come_overlay
- **Invocation:** `fdtoverlay -i $@ -o $@.tmp $$overlay && mv $@.tmp $@`
- **Processor type:** Generator

### fdtput
- **What it does:** Modifies properties in a device tree blob.
- **Projects:** clients-heqa/come_overlay
- **Invocation:** `fdtput -r $@ $$node`
- **Processor type:** Generator
