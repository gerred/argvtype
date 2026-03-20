# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

ArgvType is a static type-and-effect checker for Bash. It parses `.sh` files with `#@` annotation pragmas, lowers them to an expansion-aware HIR, runs checks, and renders diagnostics with source spans. The key insight: **expansion form is semantics in Bash** — `"${arr[@]}"` and `$arr` produce fundamentally different argv. ArgvType tracks this as part of the type via `ExpansionShape` (Scalar vs Argv).

## Commands

```bash
cargo check --workspace          # type-check all crates
cargo test --workspace           # run all tests (90 currently)
cargo clippy --workspace -- -D warnings  # lint (must pass clean)
cargo test -p argvtype-syntax    # test one crate
cargo test -p argvtype-core -- scope::tests::infer_type_info_scalar_annotation  # single test
cargo insta test --workspace --accept    # run tests and accept new snapshots
cargo run -- check fixtures/annotated.sh           # check a file
cargo run -- check --dump-hir fixtures/annotated.sh  # dump HIR as JSON
```

## Architecture

Five crates in a workspace. Data flows in one direction: **syntax → core → cli**.

### argvtype-syntax (the big one)
- `span.rs` — `SourceId`, `Span` (byte offsets), `SourceFile`. Spans convert to miette `SourceSpan` via `to_miette()`.
- `parse.rs` — `ParseSession` wraps tree-sitter-bash. `ParsedSource` holds the tree + source text.
- `annotation.rs` — Hand-written recursive descent parser for `#@` pragmas. Extracts `Directive` variants (`Sig`, `Bind`, `Type`, `Module`, `Unknown`). Unknown directives are not errors.
- `hir.rs` — All HIR node types. The core abstraction is `Word { segments: Vec<WordSegment> }` which makes shell expansions explicit. `WordSegment` variants: `Literal`, `SingleQuoted`, `DoubleQuoted`, `ParamExpand`, `CommandSub`, `ArithExpand`, `ArrayExpand`. All types derive `Serialize` for insta snapshots and `--dump-hir`.
- `lower.rs` — `parse_and_lower()` is the main entry point. `LoweringContext` dispatches on tree-sitter `node.kind()`. Unknown node kinds become `Statement::Unmodeled`, not panics. Annotations attach to the next function/statement by line proximity.

### argvtype-core
- `scope.rs` — `SymbolTable` with lexical scoping (`ScopeId`, `Scope`, `Symbol`). `build_symbol_table()` walks the HIR. Each `Symbol` has `CellKind` (Scalar/IndexedArray/AssocArray/Unknown), `TypeInfo` (cell_kind + `ExpansionShape` + refinement), `DeclScope`, and optional `TypeExpr` annotation. `infer_type_info()` derives shape/refinement from CellKind + annotation. Items are processed before `#@type` directives so symbols exist when annotations are applied.
- `diagnostic.rs` — `DiagnosticCode { family, number }` formats as `"BT201"`. `Diagnostic` builder with `.with_label()` / `.with_help()` / `.with_fix()` / `.with_agent_context()`. `render_diagnostics()` converts to miette `Report`s.
- `check.rs` — Structural checks against HIR using the symbol table. Active diagnostics: BT000 (unmodeled syntax), BT101 (annotation/declaration shape mismatch), BT201 (bare array scalar expansion), BT202 (unquoted expansion), BT801 (destructive command with unquoted var), BT802 (cd without error guard).
- `stdlib.rs` — Built-in command library with `Destructiveness` levels and flag metadata for coreutils/devops commands.

### argvtype-cli
Binary crate. Clap derive. `check` subcommand runs: read file → `parse_and_lower()` → `check()` → render diagnostics. Exit 0 if clean, 1 if errors.

### argvtype-lsp
Stub. `run_server()` returns `Err(LspError::NotImplemented)`.

### argvtype-test-harness
`check_fixture(path)` helper for end-to-end tests against `fixtures/*.sh`.

## Key patterns

- **All public HIR/annotation enums are `#[non_exhaustive]`** — match arms need wildcard fallbacks.
- **Snapshot tests use insta with YAML** — run `cargo insta test --accept` after changing lowering output. Snapshots live in `src/snapshots/`.
- **tree-sitter node kinds drive lowering** — when adding new syntax support, explore the node structure first (tree-sitter-bash docs or a test that prints the tree).
- **Dependencies use `workspace = true`** — version pins are in the root `Cargo.toml`.
- **Diagnostic codes**: `BT0xx` = internal/meta, `BT1xx` = cell-kind, `BT2xx` = expansion-shape, `BT3xx` = unset/null, `BT4xx` = path proofs, `BT5xx` = extern contracts, `BT6xx` = effects/soundness, `BT7xx` = source graph, `BT8xx` = control-flow safety.
- **Symbol construction**: every `Symbol` must include a `type_info` field populated via `infer_type_info(cell_kind, type_annotation)`. The `cell_kind` field on `Symbol` is the bash-inferred kind; `type_info` combines it with annotation-derived shape and refinement.

## Current status

M0 is complete. M1 work is in progress: symbol tables and Scalar vs Argv type distinction are implemented, set/unset flow and CFG remain. See SPEC.md for the full design and roadmap.
