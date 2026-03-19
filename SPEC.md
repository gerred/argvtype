# ArgvType

A typed analysis and language-server stack for Bash with expansion-aware types, gradual refinement, and effect-sensitive path proofs.

## 1. Executive summary

**ArgvType** is a static analysis and developer tooling project for Bash that adds a gradual type system, a shell-native annotation language, and an IDE/LSP experience focused on what shell code actually gets wrong:

- scalar vs argv confusion
- unsafe expansion and quoting misuse
- unset/null flow errors
- array/indexed/assoc misuse
- extern command contract mismatches
- refinement invalidation from shell effects like `cd`, `rm`, and unknown commands

The central insight is that a Bash checker cannot model only values. It must model:

1. **cells**: what kind of variable storage we have (`scalar`, `indexed array`, `assoc`, `nameref`, dynamic)
2. **values**: refinements such as `String`, `Path`, `ExistingFile`, `Int`, `NonEmptyString`
3. **expansion shape**: whether a thing is a **single shell word** or an **argv splice**
4. **effects**: whether a command or function may mutate cwd, filesystem, environment, control flow, or process execution

That makes `ArgvType` much closer to a shell-specific type-and-effect checker than a conventional language type checker.

## 2. Why this project should exist

Bash has excellent linting and formatting, but still lacks a real correctness layer around typed intent and flow. ShellCheck is the dominant static analyzer for shell scripts, while `mvdan/sh` is the canonical parser/formatter/interpreter library behind `shfmt`, and `bash-language-server` provides editor integration around shell tooling. Those projects form the ecosystem context, but none of them provide a full shell-native gradual type system focused on expansion semantics. citeturn916116search2turn916116search3

The opportunity is to build a checker that answers questions like:

- Is this function expecting one path or many paths?
- Has this variable been proven set and non-empty yet?
- Did a previous `[[ -f ]]` proof get invalidated by `cd` or `rm`?
- Is this array being expanded as a scalar by mistake?
- Is an external command being called with the right argument shape?
- Did a dynamic feature like `${!x}` or `eval` cross a soundness boundary?

## 3. Project name and positioning

**Project name:** `ArgvType`

Why this name:

- it highlights the most important original idea: **argv shape is part of type semantics in shell**
- it is compact, memorable, and technical
- it naturally supports a CLI called `argvtype`
- it avoids sounding like a generic academic type checker

Suggested branding:

- **Project:** `ArgvType`
- **CLI:** `argvtype`
- **Language server:** `argvtype-lsp`
- **Annotation files:** `.bti` (Bash Type Interface)

## 4. Language and implementation choice

**Pick: Rust.**

### Why Rust over Zig

Rust is the better implementation language for v1 because it has a substantially stronger ecosystem for:

- parser and syntax tooling integration
- LSP and editor tooling
- graph algorithms and incremental data structures
- serialization, config, diagnostics, and testing infrastructure
- mature async/concurrency primitives when needed for LSP and workspace indexing

The current shell ecosystem also makes Rust the more practical choice for interop. `tree-sitter-bash` exists and has active upstream maintenance, and `bash-language-server` already demonstrates the editor/LSP demand for shell tooling. Meanwhile the strongest full-fidelity shell parser remains `mvdan/sh`, which is written in Go and can be used as an oracle or normalization tool in tests. citeturn916116search2turn916116search3turn916116search15

### Why not Zig first

Zig is attractive for low-level control and binary simplicity, but for this project it would raise unnecessary schedule risk:

- fewer mature language-server and IDE infrastructure options
- less ready-made tooling around diagnostic pipelines and workspace analysis
- more custom work to get equivalent parser and incremental-analysis ergonomics

Zig remains a plausible future target for a lightweight secondary frontend or embedded engine, but **Rust is the right choice for first implementation**.

## 5. Product goals

### Primary goals

- Catch real Bash correctness bugs with minimal annotation burden
- Make shell semantics explicit without requiring a new shell language
- Work in existing `.sh` files via comments and sidecar interfaces
- Provide immediate IDE feedback via LSP
- Scale from single-file checks to workspace-wide `source` graph analysis
- Be honest about unsound areas and surface them as explicit soundness boundaries

### Non-goals for v1

- Replacing ShellCheck
- Executing shell scripts or becoming a shell runtime
- Fully sound modeling of all Bash dynamic features
- Full symbolic filesystem or process simulation
- Supporting every shell dialect equally; v1 is **Bash-first**

## 6. User-facing architecture

There are three major deliverables:

1. **CLI** for batch analysis and CI
2. **Core checker** with parser, HIR, CFG, type/effect engine, and stub resolution
3. **LSP** for diagnostics, hover, code actions, symbol info, and quick fixes

### CLI commands

```text
argvtype check [paths...]
argvtype lsp
argvtype stubgen [paths...]
argvtype fmt-annotations [paths...]
argvtype graph [paths...]
argvtype explain <diagnostic-code>
```

### File formats

- `.sh`: regular Bash scripts with `#@` comment annotations
- `.bti`: sidecar interface files for sourced libraries, generated code, and external command contracts
- `argvtype.toml`: project configuration

## 7. Annotation language design

The annotation language must be:

- valid Bash source when embedded inline
- readable by humans
- easy to parse independently
- expressive enough for gradual types, contracts, refinement, and effects

### Inline comment annotations

Use `#@` pragmas.

Example:

```bash
#!/usr/bin/env bash
#@module bash>=5.2
#@strict gradual

#@sig install(dest: Scalar[DirPath], srcs: Argv[ExistingFile]) -> Status[0] !writes_fs
install() {
  #@bind $1 dest
  #@bind $2.. srcs

  local dest=$1
  shift
  local -a srcs=("$@")

  : "${dest:?destination required}"
  mkdir -p -- "$dest"
  cp -- "${srcs[@]}" "$dest"
}
```

### Sidecar interface files: `.bti`

Use a compact interface language for library APIs and externs.

Example:

```text
module deploy

env KUBECONFIG: Scalar[ExistingFile]

sig render_chart(chart: Scalar[ExistingDir], values: Scalar[ExistingFile]) -> Scalar[YamlText] !reads_fs
sig apply_manifest(doc: Scalar[YamlText]) -> Status[0] !may_exec

extern kubectl_apply(file: Scalar[ExistingFile]) -> Status[0] !may_exec
extern yq_eval(expr: Scalar[YqExpr], file: Scalar[ExistingFile]) -> Scalar[YamlText] !may_exec
```

### v1 directives

- `module`
- `strict`
- `type`
- `sig`
- `bind`
- `extern`
- `env`
- `assert`
- `allow`

### v2 directives

- `refine`
- `alias`
- `generic`
- `trait`-like command capability descriptions
- `pathset` / resource regions for more precise invalidation

## 8. Type system design

### 8.1 Core model

Every variable reference is analyzed across four axes:

#### Cell kind

- `Scalar[T]`
- `IndexedArray[T]`
- `Assoc[K, V]`
- `Ref[T]`
- `Dyn`

#### Value refinement

- `String`
- `NonEmptyString`
- `Int`
- `Boolish`
- `Path`
- `AbsPath`
- `RelPath`
- `ExistingPath`
- `ExistingFile`
- `ExistingDir`
- `CommandName`
- `Regex`
- `Glob`
- `Fd`
- `Pid`
- `EnvVarName`
- `JsonText`
- `YamlText`
- user-defined nominal refinements

#### Expansion shape

- `Scalar[T]`: one shell word when safely expanded
- `Argv[T]`: zero or more shell words intended to be spliced
- `Words[N, T]`: optional future exact-shape refinement

#### Presence state

- `Set`
- `Unset`
- `Null`
- `NonNull`

### 8.2 Why `Argv` matters

In Bash, expansion form is semantics. The difference between:

```bash
"${arr[@]}"
```

and

```bash
$arr
```

is not stylistic. It changes word boundaries, emptiness behavior, and process argv. This is the heart of the project.

### 8.3 Path refinements and proofs

`ExistingFile` and `ExistingDir` are not timeless facts. They are **ephemeral refinements** produced by proof sites such as:

- `[[ -f $p ]]`
- `[[ -d $p ]]`
- trusted helper functions
- validated environment variables

These proofs are invalidated by effects such as:

- `cd`, `pushd`, `popd` for relative paths
- `rm`, `mv`, `mkdir`, `rmdir`, extraction tools, unknown filesystem-mutating commands
- unknown function calls lacking effect summaries

The checker must treat these as **flow-sensitive path proofs**, not absolute facts.

## 9. Effect system design

The checker needs a small but useful effect system to track when refinements survive.

### v1 effects

- `reads_fs`
- `writes_fs`
- `changes_cwd`
- `may_exec`
- `may_source`
- `mutates_env`
- `may_exit`
- `may_split`
- `may_glob`

### Why effects matter

Example:

```bash
[[ -f $cfg ]] || return 1
load_cfg "$cfg"      # survives only if load_cfg doesn't invalidate proof
```

If `load_cfg` has `!reads_fs`, the proof likely survives. If it has `!writes_fs` or is unknown, the checker widens `cfg` back to `Path`.

## 10. Core architecture

### 10.1 High-level pipeline

```text
source files
  -> parser frontend
  -> AST normalization
  -> expansion-aware HIR
  -> symbol + scope resolution
  -> source graph + interface resolution
  -> CFG construction
  -> dataflow + refinement engine
  -> constraint solving / type checking
  -> diagnostics + quick-fix suggestions
  -> CLI output / LSP publishDiagnostics
```

### 10.2 Parser strategy

Use a **hybrid parser strategy**:

- primary Rust parser frontend via `tree-sitter-bash` bindings for incremental editor-friendly parsing
- normalization layer to produce ArgvType HIR
- compatibility and test oracle path using `mvdan/sh` output in fixture-based differential testing

This is the right trade-off because `tree-sitter-bash` is editor-friendly but known to have parser edge cases in some Bash constructs, while `mvdan/sh` remains a valuable high-fidelity reference implementation in the shell ecosystem. citeturn916116search2turn916116search7

### 10.3 Internal IR stack

#### CST/AST

Raw syntax from parser.

#### HIR (High-level Intermediate Representation)

This is the first **ArgvType-owned** representation. It must make shell expansions explicit.

Key HIR nodes:

- `LiteralWord`
- `WordConcat`
- `ParamExpand`
- `ArrayExpand`
- `CommandSub`
- `ArithExpand`
- `TestExpr`
- `SimpleCommand`
- `Assignment`
- `FunctionDef`
- `Pipeline`
- `Redirect`
- `Source`
- `Case`
- `ForEach`
- `CStyleFor`

Each command argument should be represented as a sequence of word segments so diagnostics can pinpoint exactly where shape/refinement breaks.

#### MIR / CFG IR

Build a control-flow-oriented mid-level IR for dataflow.

Block contents should use normalized operations like:

- `AssignCell`
- `RefineSet`
- `RefineType`
- `InvalidatePathProofs`
- `CallKnown`
- `CallUnknown`
- `SourceModule`
- `ExitLike`

This will make the solver dramatically simpler.

## 11. Constraint and inference engine

### 11.1 Inference sources

Infer from native Bash constructs wherever possible:

- `declare -a` -> `IndexedArray`
- `declare -A` -> `Assoc`
- `declare -i` -> integer-biased scalar
- `local -n` -> `Ref[...]`
- `("$@")` -> `IndexedArray[String]` with argv-origin semantics
- `(( ... ))` -> arithmetic constraints
- `: "${x:?msg}"` -> refine to `Set & NonNull`
- `[[ -f $x ]]` -> refine to `ExistingFile`
- `[[ -d $x ]]` -> refine to `ExistingDir`

### 11.2 Gradual typing

Unannotated code should remain useful.

Defaults:

- plain scalar assignment: `Scalar[String | Null?]`
- unknown sourced symbols: `Dyn`
- unsafely dynamic constructs (`eval`, `${!x}`, dynamic `source`) widen to `Dyn`

The checker should preserve utility in legacy shell code by surfacing precision loss rather than failing outright.

### 11.3 Diagnostic classes

Core diagnostic families:

- `BT1xx`: cell-kind errors
- `BT2xx`: expansion-shape errors
- `BT3xx`: unset/null flow errors
- `BT4xx`: path proof/refinement errors
- `BT5xx`: extern contract mismatches
- `BT6xx`: effect invalidation / unsound boundary
- `BT7xx`: interface or source graph resolution issues

Examples:

- `BT201`: `Argv` used in scalar expansion site
- `BT302`: possibly unset variable at required expansion site
- `BT405`: `ExistingFile` proof invalidated by `cd`
- `BT507`: extern command argument 2 expected `Scalar[ExistingDir]`, got `Scalar[Path]`
- `BT601`: soundness boundary crossed via `eval`

## 12. CLI design

### `argvtype check`

Purpose: batch analysis for local development and CI.

Example:

```bash
argvtype check scripts/**/*.sh
argvtype check --project .
argvtype check --format sarif
```

Key flags:

- `--project <path>`
- `--strict <level>`
- `--format text|json|sarif`
- `--explain <code>`
- `--ignore-shellcheck-overlap`
- `--max-dyn <threshold>`
- `--emit-graph`

### `argvtype stubgen`

Purpose: generate starter `.bti` files from shell libraries or command help/manpage patterns.

Examples:

```bash
argvtype stubgen lib/*.sh
argvtype stubgen --extern cp mkdir jq kubectl
```

This should remain conservative in v1; it mostly bootstraps interfaces.

### `argvtype fmt-annotations`

Purpose: normalize inline `#@` syntax and `.bti` formatting.

### `argvtype graph`

Purpose: show `source` relationships, interface resolution, and dynamic boundaries.

## 13. LSP design

The LSP should be a first-class product, not an afterthought.

### 13.1 Baseline LSP capabilities

- diagnostics
- hover
- go-to definition
- find references
- document symbols
- semantic tokens for typed regions
- completion for annotation directives and `.bti` syntax
- code actions / quick fixes

### 13.2 Hover behavior

Hover over a symbol should show:

- current cell kind
- current refined type at cursor point
- presence state
- proof sources, if any
- invalidation notes, if relevant

Example hover:

```text
cfg
  cell: Scalar
  type: ExistingFile
  state: Set, NonNull
  proof: [[ -f $cfg ]] on line 14
  invalidates on: cd, rm, mv, unknown writes_fs calls
```

### 13.3 Code actions

High-value quick fixes:

- wrap array expansion as `"${arr[@]}"`
- insert `: "${var:?message}"`
- convert scalar assignment to array capture
- add missing `#@bind`
- generate local `.bti` stub for unresolved sourced function
- suppress or acknowledge dynamic boundary with `#@allow`

### 13.4 Workspace analysis

The language server should maintain:

- source graph cache
- interface cache
- file parse cache
- HIR cache
- cross-file symbol index

Incremental performance is essential. `tree-sitter` makes this practical in editor scenarios. citeturn916116search3turn916116search15

## 14. Repository layout

```text
argvtype/
  Cargo.toml
  crates/
    argvtype-cli/
    argvtype-core/
    argvtype-parser/
    argvtype-hir/
    argvtype-cfg/
    argvtype-types/
    argvtype-effects/
    argvtype-interfaces/
    argvtype-diagnostics/
    argvtype-lsp/
    argvtype-config/
    argvtype-test-harness/
  stdlib/
    bash/
      builtins.bti
      test.bti
      filesystem.bti
    extern/
      coreutils.bti
      jq.bti
      git.bti
      kubectl.bti
  docs/
    language.md
    type-system.md
    lsp.md
    architecture.md
  fixtures/
    parser/
    inference/
    diagnostics/
    workspace/
  scripts/
  xtask/
```

## 15. Data model sketch

### 15.1 Core Rust types

```rust
pub enum CellKind {
    Scalar,
    IndexedArray,
    AssocArray,
    Ref,
    Dynamic,
}

pub enum Presence {
    Unset,
    MaybeUnset,
    SetNull,
    SetNonNull,
    Unknown,
}

pub enum BaseType {
    String,
    Int,
    Path,
    AbsPath,
    RelPath,
    ExistingFile,
    ExistingDir,
    JsonText,
    YamlText,
    CommandName,
    Dyn,
}

pub enum Shape {
    Scalar,
    Argv,
}

pub struct TypeInfo {
    pub cell: CellKind,
    pub base: BaseType,
    pub shape: Shape,
    pub presence: Presence,
}
```

This should evolve into richer unions/intersections and proof tokens, but it is enough to drive an initial solver.

## 16. Soundness boundaries

ArgvType must be explicit about what it cannot know.

### Unsound or precision-losing features

- `eval`
- `${!x}` indirect expansion
- dynamic `source "$path"`
- nameref aliasing across unknown boundaries
- heavy metaprogramming via generated shell code
- command substitution with unknown output shape

The engine should widen to `Dyn` and issue a structured diagnostic rather than fabricating precision.

## 17. Testing strategy

### 17.1 Test layers

1. parser fixtures
2. HIR lowering fixtures
3. CFG/dataflow fixtures
4. type inference fixtures
5. diagnostic golden tests
6. workspace multi-file tests
7. differential tests against real Bash behavior where possible
8. comparison tests against `mvdan/sh` parse structure for sanity on representative samples

### 17.2 Corpus priorities

Focus first on shell constructs that matter most operationally:

- arrays and `$@`
- `[[ ... ]]` tests
- `case`
- redirections
- `source`
- `set -e`-adjacent control flow
- environment-variable usage
- path-sensitive operations

## 18. Incremental roadmap

## Milestone 0: parser + HIR skeleton

Deliverables:

- Rust workspace
- parser integration via `tree-sitter-bash`
- annotation lexer/parser
- HIR for assignments, words, simple commands, functions
- baseline CLI wiring

Exit criteria:

- can parse annotated shell files
- can print HIR
- can map diagnostics to source spans

## Milestone 1: minimal useful checker

Deliverables:

- symbol tables and lexical scope
- `Scalar` vs `Argv` distinction
- set/unset/null flow tracking
- `#@sig`, `#@bind`, `#@type`
- `BT201` / `BT302` style diagnostics

Exit criteria:

- catches array/scalar misuse and unset-variable errors in real scripts

## Milestone 2: refinement and path proofs

Deliverables:

- `[[ -f ]]`, `[[ -d ]]`, `${x:?}` refinements
- path proof invalidation by `cd`, `rm`, unknown writes
- `ExistingFile` / `ExistingDir`
- basic effect annotations

Exit criteria:

- can enforce a typed path contract across a small sourced workspace

## Milestone 3: extern contracts and `.bti`

Deliverables:

- `.bti` parser
- builtins/coreutils standard library
- extern command checking
- `stubgen` prototype

Exit criteria:

- can type-check real orchestration scripts against common command stubs

## Milestone 4: LSP alpha

Deliverables:

- diagnostics
- hover
- code actions for top 3 fixes
- cross-file source graph

Exit criteria:

- usable in VS Code/Neovim as an early-access extension

## Milestone 5: workspace-scale stabilization

Deliverables:

- caching and incremental invalidation
- more builtins and extern libraries
- suppression model
- SARIF and CI integration

Exit criteria:

- stable enough for pilot usage on medium-sized Bash-heavy repos

## 19. Risk register

### Risk: parser fidelity for Bash edge cases

Mitigation:

- use `tree-sitter-bash` for incremental editing
- differential and oracle testing against `mvdan/sh`
- create explicit unsupported syntax boundaries where needed

### Risk: too much annotation burden

Mitigation:

- strong inference first
- sidecar `.bti` files for library typing
- code actions and stub generation

### Risk: false precision around filesystem/path proofs

Mitigation:

- model proofs as ephemeral refinements
- invalidate aggressively on suspicious effects
- document TOCTOU limitations clearly

### Risk: overlap confusion with ShellCheck

Mitigation:

- focus docs and marketing on what is new: types, argv shape, refinements, effects
- optionally ingest ShellCheck output in editor integrations later rather than competing at first

## 20. Initial standard library plan

### Builtins

- `test`, `[`, `[[` contracts
- `declare`, `local`, `readonly`, `export`
- `source`, `.`, `return`, `exit`
- `printf`, `read`, `mapfile`, `cd`, `pushd`, `popd`

### Common extern sets

- coreutils: `cp`, `mv`, `rm`, `mkdir`, `ln`, `cat`, `test`
- text tools: `grep`, `sed`, `awk`, `jq`, `yq`
- devops tools: `git`, `docker`, `kubectl`

The standard library should start small and precise rather than broad and vague.

## 21. Example end-to-end workflow

```bash
# deploy.sh
#@sig deploy(cfg: Scalar[ExistingFile], manifests: Argv[ExistingFile]) -> Status[0] !may_exec
deploy() {
  #@bind $1 cfg
  #@bind $2.. manifests

  local cfg=$1
  shift
  local -a manifests=("$@")

  : "${cfg:?cfg required}"
  [[ -f $cfg ]] || return 1

  kubectl_apply "$cfg"        # ok if stub expects ExistingFile
  kubectl_apply $manifests      # BT201 Argv used as scalar
}
```

And in `.bti`:

```text
extern kubectl_apply(file: Scalar[ExistingFile]) -> Status[0] !may_exec
```

## 22. Codex handoff: immediate implementation tasks

This is the order I would hand to Codex.

### Task 1: scaffold workspace

- create Rust workspace
- add crates listed in repository layout
- wire CLI with `check` and `lsp` subcommands
- add config loading and fixture harness

### Task 2: annotation parsing

- implement lexer/parser for `#@` lines
- implement `.bti` parser
- attach parsed annotations to source spans

### Task 3: parser adapter

- integrate `tree-sitter-bash`
- map CST to minimal AST/HIR
- support functions, assignments, words, simple commands, tests

### Task 4: HIR and diagnostics plumbing

- define HIR node enums
- define source-span IDs
- define diagnostics model and pretty printer

### Task 5: minimal type engine

- lexical scopes
- symbol table
- `Scalar` vs `Argv`
- set/unset/null lattice
- implement diagnostics `BT201`, `BT302`

### Task 6: CFG + refinement engine

- conditional branches for `[[ ... ]]`
- `${x:?}` set/non-null refinement
- `[[ -f ]]`, `[[ -d ]]` path refinement
- invalidation on `cd`, `rm`, unknown calls

### Task 7: LSP alpha

- launchable server
- diagnostics
- hover typed info
- one or two code actions

## 23. What success looks like

ArgvType succeeds if, within a few months, it can analyze real Bash in a repo and catch bugs like:

- a function expecting one config path but being passed an argv-shaped list
- a sourced helper assuming `$KUBECONFIG` is set and existing when it is not yet validated
- a relative path proven under one cwd then used after `cd`
- an array being incorrectly scalar-expanded into a command invocation
- a dynamic boundary being introduced without the author noticing

If it does that reliably, it becomes the first genuinely valuable type-and-correctness layer for production Bash.

## 24. Final recommendation

Build **ArgvType** in **Rust**, with:

- `tree-sitter-bash` for editor-friendly parsing
- an ArgvType-owned expansion-aware HIR
- a flow-sensitive gradual type-and-effect system
- `.bti` sidecar contracts for sourced code and external commands
- a first-class LSP experience from the start

That is the right architecture, the right scope, and the right path to a credible v1.
