# ArgvType

A static type-and-effect checker for Bash. Catches real bugs — scalar/argv confusion, unset variable flows, invalidated path proofs, unsafe expansions — without requiring a new shell language.

## The problem

Bash has linters and formatters, but no correctness layer that understands *typed intent*. ShellCheck catches style issues and common pitfalls. ArgvType catches a different class of bug:

- A function receives an argv-shaped list where it expects a single path
- A variable proven to be an existing file gets used after `cd` invalidates the proof
- An array is scalar-expanded into a command, silently dropping elements
- A sourced helper assumes `$KUBECONFIG` is set when no guard has run yet
- A `${!x}` crosses a soundness boundary the author didn't notice

These are the bugs that break production deploys, not the ones that break lint checks.

## Agent-first design

ArgvType is designed to work as a guardrail for AI agents that generate and execute shell commands. Agents using Bash tools (Claude Code, custom agent frameworks, CI pipelines) produce shell code without the muscle memory that keeps experienced developers out of trouble. ArgvType catches the mistakes before they execute.

**Zero-annotation mode** is the primary agent use case. The checker infers types from native Bash constructs — `declare -a`, `[[ -f ]]`, `${x:?}`, assignment patterns — and ships a built-in command library with type signatures for coreutils, common devops tools, and Bash builtins. No `#@` pragmas needed. An agent piping commands through argvtype gets useful diagnostics out of the box.

Annotations exist for human-authored code that wants stronger contracts. Agents don't write them; they benefit from the inference and the stdlib.

## Core idea

Every variable in Bash has four properties that matter for correctness:

| Axis | What it tracks | Examples |
|------|---------------|----------|
| **Cell** | Storage kind | `Scalar`, `IndexedArray`, `Assoc`, `Ref`, `Dyn` |
| **Value** | Refinement type | `String`, `Int`, `ExistingFile`, `Path`, `JsonText` |
| **Shape** | Expansion semantics | `Scalar` (one word) vs `Argv` (zero-or-more words) |
| **Presence** | Set/unset/null state | `Unset`, `SetNull`, `SetNonNull`, `MaybeUnset` |

The key insight: **expansion form is semantics in Bash.** `"${arr[@]}"` and `$arr` produce fundamentally different argv to a command. This isn't style — it changes program behavior. ArgvType tracks this as part of the type.

Path refinements like `ExistingFile` are **ephemeral proofs**, not permanent facts. A `[[ -f $cfg ]]` guard produces a proof. A subsequent `cd` or `rm` invalidates it. The checker models this with a small effect system.

## Annotations

ArgvType reads existing `.sh` files. Type information comes from inference and optional `#@` comment pragmas:

```bash
#!/usr/bin/env bash

#@sig deploy(cfg: Scalar[ExistingFile], manifests: Argv[ExistingFile]) -> Status[0] !may_exec
deploy() {
  #@bind $1 cfg
  #@bind $2.. manifests

  local cfg=$1
  shift
  local -a manifests=("$@")

  : "${cfg:?cfg required}"
  [[ -f $cfg ]] || return 1

  kubectl_apply "$cfg"          # ok: Scalar[ExistingFile]
  kubectl_apply $manifests      # BT201: Argv used in scalar expansion
}
```

Most code needs zero annotations. The checker infers from `declare -a`, `[[ -f ]]`, `${x:?}`, and other native Bash constructs.

## Diagnostics

```
BT1xx  cell-kind errors         (array treated as scalar, etc.)
BT2xx  expansion-shape errors   (Argv in scalar site, etc.)
BT3xx  unset/null flow errors   (possibly unset at required site)
BT4xx  path proof errors        (ExistingFile invalidated by cd)
BT5xx  extern contract errors   (wrong arg type to external command)
BT6xx  effect/soundness errors  (eval, ${!x}, dynamic source)
BT7xx  source graph errors      (unresolved source, missing interface)
```

## CLI

```
argvtype check [paths...]              # batch analysis, CI-friendly
argvtype check --format sarif          # SARIF output for CI integration
argvtype lsp                           # language server
argvtype explain <diagnostic-code>     # explain a diagnostic in detail
```

## LSP

First-class editor experience, not an afterthought:

- **Diagnostics** with source spans and fix suggestions
- **Hover** shows cell kind, refined type, presence state, and active proofs
- **Go-to-definition** across `source` boundaries
- **Code actions**: wrap array expansion, insert `${var:?}` guard, generate stub

Example hover:

```
cfg
  cell: Scalar
  type: ExistingFile
  state: Set, NonNull
  proof: [[ -f $cfg ]] on line 14
  invalidated by: cd, rm, mv, unknown writes_fs calls
```

## Effect system

Functions and commands carry effect annotations that determine when proofs survive:

```
!reads_fs      read filesystem
!writes_fs     write filesystem (invalidates path proofs)
!changes_cwd   change working directory (invalidates relative path proofs)
!may_exec      execute external process
!may_source    source shell code
!mutates_env   modify environment variables
!may_exit      may exit the shell
```

If a function between a `[[ -f $cfg ]]` guard and a use of `$cfg` has `!writes_fs`, the proof is invalidated. If it only has `!reads_fs`, the proof survives.

## Architecture

```
source.sh + #@ annotations
  -> tree-sitter-bash parser
  -> AST normalization
  -> expansion-aware HIR
  -> symbol + scope resolution
  -> CFG construction
  -> dataflow + refinement engine
  -> constraint solving
  -> diagnostics
  -> CLI output / LSP publishDiagnostics
```

### Parser strategy

**tree-sitter-bash** is the parser frontend. It provides incremental, editor-friendly parsing that the LSP requires. Known gaps in tree-sitter-bash (some here-doc forms, complex arithmetic contexts) are handled with explicit error recovery and unsupported-syntax diagnostics — not silent misparse.

### Single IR

The checker uses one internal representation: an **expansion-aware HIR** that makes word segments, parameter expansions, and shell control flow explicit. CFG construction operates on this same IR rather than requiring a separate lowering pass. If the two concerns diverge later, we split — but not before.

## Project layout

```
argvtype/
  Cargo.toml              # workspace root
  crates/
    argvtype-cli/         # CLI binary, argument parsing, output formatting
    argvtype-core/        # type engine, CFG, dataflow, diagnostics, inference
    argvtype-syntax/      # tree-sitter integration, HIR, annotation parser
    argvtype-lsp/         # language server
    argvtype-test-harness/# fixture-based test infrastructure
  stdlib/                 # .bti interface files for builtins and common commands
  fixtures/               # test fixtures organized by diagnostic family
  docs/                   # design documents
```

Five crates. Split when boundaries are proven, not before.

## Soundness boundaries

ArgvType is honest about what it cannot know. These features widen to `Dyn` with a structured diagnostic:

- `eval`
- `${!x}` indirect expansion
- Dynamic `source "$path"`
- Nameref aliasing across unknown boundaries
- Command substitution with unknown output shape

The checker surfaces precision loss rather than fabricating precision.

## Roadmap

### M0: Parser + HIR skeleton
- Rust workspace with crate structure above
- tree-sitter-bash integration
- Annotation lexer/parser for `#@` pragmas
- HIR for assignments, words, simple commands, functions
- CLI wiring for `check` subcommand
- Diagnostic span mapping

**Exit: can parse annotated shell files, print HIR, map diagnostics to source spans.**

### M1: Minimal useful checker
- Symbol tables and lexical scope
- Scalar vs Argv distinction
- Set/unset/null flow tracking
- `#@sig`, `#@bind`, `#@type` directives
- BT201, BT302 diagnostics

**Exit: catches array/scalar misuse and unset-variable errors in real scripts.**

### M2: Refinement and path proofs
- `[[ -f ]]`, `[[ -d ]]`, `${x:?}` refinements
- Path proof invalidation by `cd`, `rm`, unknown writes
- Basic effect annotations

**Exit: enforces typed path contracts across a sourced workspace.**

### M3: Extern contracts, `.bti`, and command stdlib
- `.bti` parser
- Built-in command library: Bash builtins, coreutils, common devops tools (`git`, `docker`, `kubectl`, `jq`)
- External command checking against stdlib signatures
- `stubgen` prototype for generating `.bti` from new commands

**Exit: type-checks scripts against command stubs. Zero-annotation checking works out of the box.**

### M4: Agent hook integration
- PreToolUse hook for Claude Code and agent frameworks
- Check Bash commands before execution, deny with diagnostics on type errors
- PostToolUse hook to analyze failed command output
- Ship hook scripts and configuration in the repo

**Exit: argvtype runs as a guardrail on agent-generated shell commands.**

### M5: LSP alpha
- Diagnostics, hover, code actions
- Cross-file source graph
- Incremental analysis

**Exit: usable in VS Code / Neovim.**

## Status

**M0 complete.** Parser, HIR, basic checker, and CLI working.

## License

TBD
