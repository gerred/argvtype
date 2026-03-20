use std::collections::{HashMap, HashSet};
use argvtype_syntax::annotation::{Directive, SigDirective, TypeExpr};
use argvtype_syntax::hir::*;
use argvtype_syntax::span::{SourceId, Span};
use crate::diagnostic::{Diagnostic, DiagnosticCode, Fix};
use crate::scope::{self, CellKind, Presence, ScopeId, SymbolTable, ExpansionShape};
use crate::stdlib::{self, Destructiveness, EffectSet};

const BT000: DiagnosticCode = DiagnosticCode { family: "BT", number: 0 };
const BT101: DiagnosticCode = DiagnosticCode { family: "BT", number: 101 };
const BT102: DiagnosticCode = DiagnosticCode { family: "BT", number: 102 };
const BT201: DiagnosticCode = DiagnosticCode { family: "BT", number: 201 };
const BT202: DiagnosticCode = DiagnosticCode { family: "BT", number: 202 };
const BT203: DiagnosticCode = DiagnosticCode { family: "BT", number: 203 };
const BT301: DiagnosticCode = DiagnosticCode { family: "BT", number: 301 };
const BT302: DiagnosticCode = DiagnosticCode { family: "BT", number: 302 };
const BT401: DiagnosticCode = DiagnosticCode { family: "BT", number: 401 };
const BT405: DiagnosticCode = DiagnosticCode { family: "BT", number: 405 };
const BT406: DiagnosticCode = DiagnosticCode { family: "BT", number: 406 };
const BT407: DiagnosticCode = DiagnosticCode { family: "BT", number: 407 };
const BT801: DiagnosticCode = DiagnosticCode { family: "BT", number: 801 };
const BT802: DiagnosticCode = DiagnosticCode { family: "BT", number: 802 };

type FunctionSigs = HashMap<String, SigDirective>;
type FunctionProves = HashMap<String, ProvesDirective>;

type PresenceMap = HashMap<String, Presence>;

/// Maps variable names to their proven type refinements (e.g., "ExistingFile").
type RefinementMap = HashMap<String, HashSet<String>>;

/// Tracks why a proof was invalidated, for targeted diagnostics.
#[derive(Debug, Clone)]
struct InvalidatedProof {
    refinement: String,
    cause: InvalidationCause,
    cause_span: Span,
}

#[derive(Debug, Clone)]
enum InvalidationCause {
    Cd,
    WritesFs(String),
    UnknownCall(String),
    Source,
}

/// Maps variable names to their invalidated proofs.
type InvalidatedMap = HashMap<String, Vec<InvalidatedProof>>;

/// A `#@proves` directive parsed from a function annotation.
#[derive(Debug, Clone)]
struct ProvesDirective {
    param_position: String,
    refinement: String,
}

/// Flow-sensitive state threaded through the checker.
#[derive(Debug, Clone)]
struct FlowState {
    presence: PresenceMap,
    refinements: RefinementMap,
    invalidated: InvalidatedMap,
}

impl FlowState {
    fn new(presence: PresenceMap) -> Self {
        FlowState {
            presence,
            refinements: RefinementMap::new(),
            invalidated: InvalidatedMap::new(),
        }
    }
}

/// A refinement extracted from a test condition.
struct TestRefinement {
    var_name: String,
    presence: Presence,
    type_refinement: Option<String>,
}

const WELL_KNOWN_SET_VARS: &[&str] = &[
    "PATH", "HOME", "USER", "SHELL", "PWD", "OLDPWD", "IFS",
    "LINENO", "BASH_SOURCE", "BASH_LINENO", "FUNCNAME",
    "HOSTNAME", "HOSTTYPE", "OSTYPE", "MACHTYPE", "BASHPID",
    "BASH_VERSION", "BASH_VERSINFO", "RANDOM", "SECONDS",
    "SHLVL", "TERM", "LANG", "LC_ALL", "TMPDIR", "EDITOR",
    "UID", "EUID", "GROUPS", "PPID",
];

const SPECIAL_VARS: &[&str] = &["?", "#", "$", "!", "-", "0", "@", "*", "_"];

fn is_positional_param(name: &str) -> bool {
    name.len() <= 3 && name.chars().all(|c| c.is_ascii_digit()) && name != "0"
}

fn is_env_like(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_ascii_uppercase() || c == '_')
}

fn init_presence_map(symbols: &SymbolTable, scope: ScopeId) -> PresenceMap {
    let mut map = PresenceMap::new();
    // Walk the scope chain collecting symbols
    let mut current = Some(scope);
    while let Some(sid) = current {
        let s = symbols.scope(sid);
        for (name, sym) in &s.symbols {
            map.entry(name.clone()).or_insert(sym.type_info.presence);
        }
        current = s.parent;
    }
    // Well-known vars are always set
    for &var in WELL_KNOWN_SET_VARS {
        map.entry(var.into()).or_insert(Presence::Set);
    }
    map
}

fn presence_join(a: Presence, b: Presence) -> Presence {
    match (a, b) {
        (Presence::Set, Presence::Set) => Presence::Set,
        (Presence::Unset, Presence::Unset) => Presence::Unset,
        _ => Presence::MaybeUnset,
    }
}

fn merge_presence_maps(a: &PresenceMap, b: &PresenceMap) -> PresenceMap {
    let mut result = PresenceMap::new();
    for (name, &a_presence) in a {
        let merged = match b.get(name) {
            Some(&b_presence) => presence_join(a_presence, b_presence),
            None => presence_join(a_presence, Presence::Unknown),
        };
        result.insert(name.clone(), merged);
    }
    for (name, &b_presence) in b {
        if !a.contains_key(name) {
            result.insert(name.clone(), presence_join(Presence::Unknown, b_presence));
        }
    }
    result
}

/// Merge refinement maps: a refinement survives only if proven in both branches.
fn merge_refinement_maps(a: &RefinementMap, b: &RefinementMap) -> RefinementMap {
    let mut result = RefinementMap::new();
    for (name, a_refs) in a {
        if let Some(b_refs) = b.get(name) {
            let intersection: HashSet<String> = a_refs.intersection(b_refs).cloned().collect();
            if !intersection.is_empty() {
                result.insert(name.clone(), intersection);
            }
        }
    }
    result
}

/// Merge invalidated maps: keep all invalidation records from both branches.
fn merge_invalidated_maps(a: &InvalidatedMap, b: &InvalidatedMap) -> InvalidatedMap {
    let mut result = a.clone();
    for (name, proofs) in b {
        result.entry(name.clone()).or_default().extend(proofs.iter().cloned());
    }
    result
}

fn merge_flow_states(a: &FlowState, b: &FlowState) -> FlowState {
    FlowState {
        presence: merge_presence_maps(&a.presence, &b.presence),
        refinements: merge_refinement_maps(&a.refinements, &b.refinements),
        invalidated: merge_invalidated_maps(&a.invalidated, &b.invalidated),
    }
}

fn collect_function_sigs(source_unit: &SourceUnit) -> FunctionSigs {
    let mut sigs = FunctionSigs::new();
    for item in &source_unit.items {
        if let Item::Function(f) = item {
            for ann in &f.annotations {
                if let Directive::Sig(sig) = &ann.directive {
                    sigs.insert(f.name.clone(), sig.clone());
                }
            }
        }
    }
    sigs
}

fn collect_function_proves(source_unit: &SourceUnit) -> FunctionProves {
    let mut proves = FunctionProves::new();
    for item in &source_unit.items {
        if let Item::Function(f) = item {
            for ann in &f.annotations {
                if let Directive::Proves(p) = &ann.directive {
                    proves.insert(f.name.clone(), ProvesDirective {
                        param_position: p.param.clone(),
                        refinement: p.refinement.clone(),
                    });
                }
            }
        }
    }
    proves
}

/// Compute the effect set for a command, using `#@sig` effects when available.
fn command_effect_set(cmd: &Command, ctx: &CheckCtx) -> EffectSet {
    let name = match command_name_str(cmd) {
        Some(n) => n,
        None => return EffectSet::UNKNOWN_EXTERNAL,
    };
    // Check if this is a function with a declared #@sig and effects
    if let Some(sig) = ctx.sigs.get(name) {
        let mut set = EffectSet::NONE;
        for effect in &sig.effects {
            if let Some(e) = EffectSet::from_effect_name(&effect.name) {
                set = set.union(e);
            }
        }
        return set;
    }
    // Functions with #@proves are known — treat as no additional effects
    if ctx.proves.contains_key(name) {
        return EffectSet::NONE;
    }
    stdlib::lookup_effects(name)
}

/// Invalidate path refinements in the flow state based on a command's effects.
/// Moves killed refinements to the invalidated map with cause information.
fn apply_effect_invalidation(
    flow: &mut FlowState,
    effects: EffectSet,
    cmd_name: &str,
    cmd_span: Span,
    ctx: &CheckCtx,
) {
    if !effects.invalidates_path_proofs() {
        return;
    }

    // Determine the cause: distinguish known commands from unknown externals.
    // Unknown externals get UNKNOWN_EXTERNAL effects conservatively — attribute
    // the invalidation to the unknown call itself, not a specific effect.
    let is_known = matches!(cmd_name,
        "cd" | "source" | "." | "eval" | "exec" | "exit" | "return"
        | "export" | "unset" | "declare" | "local" | "readonly"
        | "echo" | "printf" | "true" | "false" | ":" | "test" | "[" | "[["
        | "read" | "mapfile" | "readarray" | "shift" | "set"
    ) || stdlib::lookup_command(cmd_name).is_some()
      || ctx.sigs.contains_key(cmd_name)
      || ctx.proves.contains_key(cmd_name);

    let cause = if effects.contains(EffectSet::MAY_SOURCE) {
        InvalidationCause::Source
    } else if effects.contains(EffectSet::CHANGES_CWD) {
        InvalidationCause::Cd
    } else if !is_known {
        InvalidationCause::UnknownCall(cmd_name.to_string())
    } else if effects.contains(EffectSet::WRITES_FS) {
        InvalidationCause::WritesFs(cmd_name.to_string())
    } else {
        InvalidationCause::UnknownCall(cmd_name.to_string())
    };

    let vars_with_path_proofs: Vec<String> = flow
        .refinements
        .iter()
        .filter(|(_, refs)| refs.iter().any(|r| is_path_refinement(r)))
        .map(|(name, _)| name.clone())
        .collect();

    for var in vars_with_path_proofs {
        if let Some(refs) = flow.refinements.get_mut(&var) {
            let path_refs: Vec<String> = refs
                .iter()
                .filter(|r| is_path_refinement(r))
                .cloned()
                .collect();
            for r in &path_refs {
                refs.remove(r);
                flow.invalidated.entry(var.clone()).or_default().push(InvalidatedProof {
                    refinement: r.clone(),
                    cause: cause.clone(),
                    cause_span: cmd_span,
                });
            }
            if refs.is_empty() {
                flow.refinements.remove(&var);
            }
        }
    }
}

/// Static context shared across the checker — avoids passing many args.
struct CheckCtx<'a> {
    source_id: SourceId,
    symbols: &'a SymbolTable,
    sigs: &'a FunctionSigs,
    proves: &'a FunctionProves,
}

pub fn check(source_unit: &SourceUnit) -> Vec<Diagnostic> {
    check_with_imports(source_unit, &[])
}

/// Check a source unit with imported symbols from sourced files.
/// Imported symbols are added to the root scope before checking.
pub fn check_with_imports(
    source_unit: &SourceUnit,
    imported: &[&scope::Symbol],
) -> Vec<Diagnostic> {
    let mut symbols = scope::build_symbol_table(source_unit);

    // Inject imported symbols into the root scope
    let root = symbols.root_scope();
    for &sym in imported {
        if symbols.resolve(root, &sym.name).is_none() {
            symbols.define(root, sym.clone());
        }
    }

    let sigs = collect_function_sigs(source_unit);
    let proves = collect_function_proves(source_unit);
    let ctx = CheckCtx {
        source_id: source_unit.source_id,
        symbols: &symbols,
        sigs: &sigs,
        proves: &proves,
    };
    let mut diagnostics = Vec::new();
    let mut global_flow = FlowState::new(init_presence_map(&symbols, root));

    for item in &source_unit.items {
        match item {
            Item::Function(f) => {
                let scope = symbols.scope_of_node(f.id).unwrap_or(root);
                let mut func_flow = FlowState::new(init_presence_map(&symbols, scope));
                check_statements(&f.body, &ctx, scope, &mut diagnostics, &mut func_flow);
            }
            Item::Statement(s) => {
                check_statement(s, &ctx, root, &mut diagnostics, &mut global_flow);
            }
            _ => {}
        }
    }

    // BT802: check consecutive top-level items for cd;next pattern
    check_consecutive_cd_items(&source_unit.items, ctx.source_id, &mut diagnostics);

    // BT101: annotation/declaration shape mismatches
    check_type_mismatches(&symbols, ctx.source_id, &mut diagnostics);

    diagnostics
}

fn check_statements(
    stmts: &[Statement],
    ctx: &CheckCtx,
    scope: ScopeId,
    diagnostics: &mut Vec<Diagnostic>,
    flow: &mut FlowState,
) {
    for stmt in stmts {
        check_statement(stmt, ctx, scope, diagnostics, flow);
    }
}

fn is_argv_shape_in_scope(symbols: &SymbolTable, scope: ScopeId, name: &str) -> bool {
    symbols
        .resolve(scope, name)
        .is_some_and(|sym| {
            matches!(sym.type_info.cell_kind, CellKind::IndexedArray | CellKind::AssocArray)
                || sym.type_info.shape == ExpansionShape::Argv
        })
}

fn is_scalar_shape_in_scope(symbols: &SymbolTable, scope: ScopeId, name: &str) -> bool {
    symbols
        .resolve(scope, name)
        .is_some_and(|sym| {
            sym.type_info.shape == ExpansionShape::Scalar
                && matches!(sym.type_info.cell_kind, CellKind::Scalar | CellKind::Unknown)
        })
}

fn check_statement(
    stmt: &Statement,
    ctx: &CheckCtx,
    scope: ScopeId,
    diagnostics: &mut Vec<Diagnostic>,
    flow: &mut FlowState,
) {
    match stmt {
        Statement::Assignment(a) => {
            // Assignment sets the variable and clears path refinements
            if a.value.is_some() || a.array_value.is_some() {
                flow.presence.insert(a.name.clone(), Presence::Set);
                flow.refinements.remove(&a.name);
                flow.invalidated.remove(&a.name);
            }
        }
        Statement::Command(cmd) => {
            let cmd_scope = ctx.symbols.scope_of_node(cmd.id).unwrap_or(scope);
            check_word_for_bare_array(&cmd.name, ctx.source_id, ctx.symbols, cmd_scope, diagnostics);
            for arg in &cmd.args {
                check_word_for_bare_array(arg, ctx.source_id, ctx.symbols, cmd_scope, diagnostics);
            }
            if !is_test_command(cmd) {
                // BT202: unquoted expansion in command args
                for arg in &cmd.args {
                    check_word_for_unquoted_expansion(arg, ctx.source_id, diagnostics);
                }
                // BT801: destructive command with unquoted variable
                check_destructive_unquoted(cmd, ctx.source_id, diagnostics);
                // BT301/BT302: presence checks on expansions
                check_command_presence(cmd, ctx.source_id, ctx.symbols, cmd_scope, &flow.presence, diagnostics);
            }
            // Recognize `: "${x:?msg}"` guard pattern
            apply_colon_guard(cmd, &mut flow.presence);
            // Track commands that modify variable presence
            apply_command_presence_effects(cmd, &mut flow.presence);
            // Recognize `command -v`/`type`/`hash` as CommandName proof sites
            apply_command_name_proof(cmd, flow);
            // Apply #@proves from custom proof functions
            apply_proves_effects(cmd, ctx.proves, flow);
            // BT102/BT401/BT405-407: function call site checking against #@sig
            check_call_site(cmd, ctx.source_id, ctx.symbols, scope, ctx.sigs, flow, diagnostics);
            // Effect invalidation: kill path proofs after effectful commands
            let effects = command_effect_set(cmd, ctx);
            if let Some(name) = command_name_str(cmd) {
                apply_effect_invalidation(flow, effects, name, cmd.span, ctx);
            }
        }
        Statement::Pipeline(p) => {
            for cmd in &p.commands {
                check_statement(cmd, ctx, scope, diagnostics, flow);
            }
        }
        Statement::If(if_stmt) => {
            for s in &if_stmt.condition {
                check_statement(s, ctx, scope, diagnostics, flow);
            }

            // Extract test refinements from condition
            let refinements = extract_test_refinements(&if_stmt.condition);

            // Fork flow for then-branch: apply refinements
            let mut then_flow = flow.clone();
            for r in &refinements {
                then_flow.presence.insert(r.var_name.clone(), r.presence);
                if let Some(ref type_ref) = r.type_refinement {
                    then_flow.refinements.entry(r.var_name.clone()).or_default().insert(type_ref.clone());
                    then_flow.invalidated.remove(&r.var_name);
                }
            }
            check_statements(&if_stmt.then_body, ctx, scope, diagnostics, &mut then_flow);

            // Fork flow for else-branch: invert presence, no type refinements
            let mut else_flow = flow.clone();
            for r in &refinements {
                let inverted = match r.presence {
                    Presence::Set => Presence::MaybeUnset,
                    Presence::Unset => Presence::Set,
                    other => other,
                };
                else_flow.presence.insert(r.var_name.clone(), inverted);
            }
            if let Some(else_body) = &if_stmt.else_body {
                check_statements(else_body, ctx, scope, diagnostics, &mut else_flow);
            }

            // Merge at join point
            *flow = merge_flow_states(&then_flow, &else_flow);
        }
        Statement::For(for_loop) => {
            // Loop body may execute zero or more times — fork and merge
            let pre_loop = flow.clone();
            for s in &for_loop.body {
                check_statement(s, ctx, scope, diagnostics, flow);
            }
            *flow = merge_flow_states(&pre_loop, flow);
        }
        Statement::While(while_loop) => {
            // Condition always evaluates at least once
            for s in &while_loop.condition {
                check_statement(s, ctx, scope, diagnostics, flow);
            }
            // Loop body may execute zero or more times — fork and merge
            let pre_loop = flow.clone();
            for s in &while_loop.body {
                check_statement(s, ctx, scope, diagnostics, flow);
            }
            *flow = merge_flow_states(&pre_loop, flow);
        }
        Statement::List(list) => {
            check_list_flow(list, ctx, scope, diagnostics, flow);
            // BT802: cd followed by ; instead of && within a list
            check_list_for_cd_semi(list, ctx.source_id, diagnostics);
        }
        Statement::Block(b) => {
            let block_scope = ctx.symbols.scope_of_node(b.id).unwrap_or(scope);
            let body_scope = if b.subshell {
                b.body.first()
                    .and_then(stmt_node_id)
                    .and_then(|id| ctx.symbols.scope_of_node(id))
                    .unwrap_or(block_scope)
            } else {
                block_scope
            };
            for s in &b.body {
                check_statement(s, ctx, body_scope, diagnostics, flow);
            }
        }
        Statement::Case(case_stmt) => {
            let pre_case = flow.clone();
            let mut arm_flows: Vec<FlowState> = Vec::new();
            for arm in &case_stmt.arms {
                let mut arm_flow = pre_case.clone();
                for s in &arm.body {
                    check_statement(s, ctx, scope, diagnostics, &mut arm_flow);
                }
                arm_flows.push(arm_flow);
            }
            // If no wildcard/default arm, case might not match — include pre-case state
            let has_default = case_stmt.arms.iter().any(|arm| {
                arm.patterns.iter().any(|p| {
                    p.segments
                        .first()
                        .is_some_and(|s| matches!(s, WordSegment::Literal(l) if l == "*"))
                })
            });
            if !has_default {
                arm_flows.push(pre_case);
            }
            // Merge all arm flow states
            if let Some(first) = arm_flows.first().cloned() {
                let mut merged = first;
                for arm_f in &arm_flows[1..] {
                    merged = merge_flow_states(&merged, arm_f);
                }
                *flow = merged;
            }
        }
        Statement::SourceCommand(src_cmd) => {
            // Source commands are checked by the source graph layer.
            // At the single-file level, conservatively invalidate all path proofs
            // since sourced code may have arbitrary effects.
            if !src_cmd.dynamic {
                // Static source: BT701 (unresolved) is emitted by source_graph module
            }
            // Invalidate proofs: source may execute arbitrary code
            let vars: Vec<String> = flow.refinements.keys().cloned().collect();
            for var in vars {
                if let Some(refs) = flow.refinements.remove(&var) {
                    for refinement in refs {
                        flow.invalidated.entry(var.clone()).or_default().push(
                            InvalidatedProof {
                                refinement,
                                cause: InvalidationCause::Source,
                                cause_span: src_cmd.span,
                            },
                        );
                    }
                }
            }
        }
        Statement::Unmodeled(u) => {
            diagnostics.push(
                Diagnostic::warning(
                    BT000,
                    format!("unmodeled syntax: {}", u.kind),
                    ctx.source_id,
                    u.span,
                )
                .with_help("this construct is not yet analyzed by argvtype"),
            );
        }
        _ => {}
    }
}

fn stmt_node_id(stmt: &Statement) -> Option<NodeId> {
    match stmt {
        Statement::Assignment(a) => Some(a.id),
        Statement::Command(c) => Some(c.id),
        Statement::SourceCommand(s) => Some(s.id),
        Statement::Pipeline(p) => Some(p.id),
        Statement::If(i) => Some(i.id),
        Statement::For(f) => Some(f.id),
        Statement::While(w) => Some(w.id),
        Statement::Case(c) => Some(c.id),
        Statement::List(l) => Some(l.id),
        Statement::Block(b) => Some(b.id),
        Statement::Unmodeled(u) => Some(u.id),
        _ => None,
    }
}

/// Check all expansions in a command for BT301 (undeclared) and BT302 (maybe-unset).
fn check_command_presence(
    cmd: &Command,
    source_id: SourceId,
    symbols: &SymbolTable,
    scope: ScopeId,
    presence: &PresenceMap,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for arg in &cmd.args {
        check_word_presence(arg, source_id, symbols, scope, presence, diagnostics);
    }
    check_word_presence(&cmd.name, source_id, symbols, scope, presence, diagnostics);
}

fn check_word_presence(
    word: &Word,
    source_id: SourceId,
    symbols: &SymbolTable,
    scope: ScopeId,
    presence: &PresenceMap,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for segment in &word.segments {
        check_segment_presence(segment, source_id, symbols, scope, presence, diagnostics);
    }
}

fn check_segment_presence(
    segment: &WordSegment,
    source_id: SourceId,
    symbols: &SymbolTable,
    scope: ScopeId,
    presence: &PresenceMap,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match segment {
        WordSegment::ParamExpand(pe) => {
            // Skip special vars, positional params
            if SPECIAL_VARS.contains(&pe.name.as_str()) || is_positional_param(&pe.name) {
                return;
            }
            // Skip if expansion has a guard operator
            if matches!(pe.operator, Some(ParamOperator::Default | ParamOperator::Error | ParamOperator::Alternate | ParamOperator::Assign)) {
                return;
            }

            let var_presence = presence.get(&pe.name).copied();
            let in_symbol_table = symbols.resolve(scope, &pe.name).is_some();

            if !in_symbol_table && var_presence.is_none() {
                // BT301: undeclared variable
                // Skip env-like (all-uppercase) names and well-known vars
                if !is_env_like(&pe.name) && !WELL_KNOWN_SET_VARS.contains(&pe.name.as_str()) {
                    diagnostics.push(
                        Diagnostic::warning(
                            BT301,
                            format!("use of undeclared variable '{}'", pe.name),
                            source_id,
                            pe.span,
                        )
                        .with_help(format!(
                            "declare '{}' before use, or add a #@type annotation",
                            pe.name
                        )),
                    );
                }
            } else {
                // BT302: maybe-unset variable
                let p = var_presence.unwrap_or_else(|| {
                    symbols.resolve(scope, &pe.name)
                        .map(|s| s.type_info.presence)
                        .unwrap_or(Presence::Unknown)
                });
                if matches!(p, Presence::Unset | Presence::MaybeUnset) {
                    diagnostics.push(
                        Diagnostic::warning(
                            BT302,
                            format!(
                                "variable '{}' may be unset when expanded",
                                pe.name
                            ),
                            source_id,
                            pe.span,
                        )
                        .with_help(format!(
                            "guard with `: \"${{{}:?msg}}\"` or use a default: \"${{{}:-default}}\"",
                            pe.name, pe.name
                        ))
                        .with_agent_context(format!(
                            "Variable '{}' has not been assigned a value on all paths reaching this point. \
                             Expanding an unset variable yields empty string (or triggers an error with set -u). \
                             Use a guard pattern like `: \"${{{}:?required}}\"` or provide a default.",
                            pe.name, pe.name
                        )),
                    );
                }
            }
        }
        WordSegment::DoubleQuoted(inner) => {
            for seg in inner {
                check_segment_presence(seg, source_id, symbols, scope, presence, diagnostics);
            }
        }
        _ => {}
    }
}

/// Recognize `: "${x:?msg}"` — the colon command with an Error-operator expansion marks x as Set.
fn apply_colon_guard(cmd: &Command, presence: &mut PresenceMap) {
    if command_name_str(cmd) != Some(":") {
        return;
    }
    for arg in &cmd.args {
        for segment in &arg.segments {
            apply_colon_guard_segment(segment, presence);
        }
    }
}

fn apply_colon_guard_segment(segment: &WordSegment, presence: &mut PresenceMap) {
    match segment {
        WordSegment::ParamExpand(pe) if pe.operator == Some(ParamOperator::Error) => {
            presence.insert(pe.name.clone(), Presence::Set);
        }
        WordSegment::DoubleQuoted(inner) => {
            for seg in inner {
                apply_colon_guard_segment(seg, presence);
            }
        }
        _ => {}
    }
}

/// Track commands that modify variable presence:
/// - `read var` / `read -r var` → Set
/// - `unset var` → Unset
/// - `mapfile var` / `readarray var` → Set
/// - `printf -v var ...` → Set
fn apply_command_presence_effects(cmd: &Command, presence: &mut PresenceMap) {
    let name = match command_name_str(cmd) {
        Some(n) => n,
        None => return,
    };
    match name {
        "read" => {
            // `read [-r] [-d delim] [-n count] [-p prompt] [-t timeout] [-u fd] var [var...]`
            // Variable names are the non-flag arguments at the end
            for arg in cmd.args.iter().rev() {
                if let Some(var_name) = word_as_literal(arg) {
                    if var_name.starts_with('-') {
                        break;
                    }
                    presence.insert(var_name.to_string(), Presence::Set);
                } else {
                    break;
                }
            }
        }
        "unset" => {
            // `unset [-fv] var [var...]`
            for arg in &cmd.args {
                if let Some(var_name) = word_as_literal(arg) {
                    if var_name.starts_with('-') {
                        continue;
                    }
                    presence.insert(var_name.to_string(), Presence::Unset);
                }
            }
        }
        "mapfile" | "readarray" => {
            // `mapfile [-t] [-n count] [-O origin] [-s count] [-C callback] [-c quantum] [array]`
            // The array name is the last non-flag argument, or defaults to MAPFILE
            if let Some(last) = cmd.args.last()
                && let Some(var_name) = word_as_literal(last)
                && !var_name.starts_with('-')
            {
                presence.insert(var_name.to_string(), Presence::Set);
            }
        }
        "printf" => {
            // `printf -v var format [args...]` — assigns to var
            let mut i = 0;
            while i < cmd.args.len() {
                if let Some(flag) = word_as_literal(&cmd.args[i])
                    && flag == "-v"
                {
                    if let Some(next) = cmd.args.get(i + 1)
                        && let Some(var_name) = word_as_literal(next)
                    {
                        presence.insert(var_name.to_string(), Presence::Set);
                    }
                    break;
                }
                i += 1;
            }
        }
        _ => {}
    }
}

fn word_as_literal(word: &Word) -> Option<&str> {
    if word.segments.len() == 1
        && let WordSegment::Literal(s) = &word.segments[0]
    {
        return Some(s.as_str());
    }
    None
}

/// Recognize `command -v NAME`, `type NAME`, `hash NAME` as CommandName proof sites.
/// These prove the named command exists when they succeed.
fn apply_command_name_proof(cmd: &Command, flow: &mut FlowState) {
    let name = match command_name_str(cmd) {
        Some(n) => n,
        None => return,
    };
    match name {
        "command" => {
            // `command -v name` → CommandName proof on `name`
            if cmd.args.len() >= 2
                && word_as_literal(&cmd.args[0]) == Some("-v")
                && let Some(target) = word_as_literal(&cmd.args[1])
            {
                flow.refinements
                    .entry(target.to_string())
                    .or_default()
                    .insert("CommandName".to_string());
                flow.presence.insert(target.to_string(), Presence::Set);
            }
        }
        "type" | "hash" => {
            // `type name` / `hash name` → CommandName proof
            if let Some(first_arg) = cmd.args.first()
                && let Some(target) = word_as_literal(first_arg)
                && !target.starts_with('-')
            {
                flow.refinements
                    .entry(target.to_string())
                    .or_default()
                    .insert("CommandName".to_string());
                flow.presence.insert(target.to_string(), Presence::Set);
            }
        }
        _ => {}
    }
}

/// Apply refinement effects from `#@proves` custom proof functions.
/// When a function annotated with `#@proves $1 ExistingFile` is called,
/// the corresponding argument gets the specified refinement.
fn apply_proves_effects(cmd: &Command, proves: &FunctionProves, flow: &mut FlowState) {
    let func_name = match command_name_str(cmd) {
        Some(n) => n,
        None => return,
    };
    let proves_dir = match proves.get(func_name) {
        Some(p) => p,
        None => return,
    };
    // Parse the param position: "$1" → index 0, "$2" → index 1, etc.
    let param_idx = proves_dir
        .param_position
        .strip_prefix('$')
        .and_then(|s| s.parse::<usize>().ok())
        .map(|n| n.saturating_sub(1));
    if let Some(idx) = param_idx
        && let Some(arg) = cmd.args.get(idx)
        && let Some(var_name) = extract_single_var_name(arg)
    {
        flow.refinements
            .entry(var_name.clone())
            .or_default()
            .insert(proves_dir.refinement.clone());
        flow.invalidated.remove(&var_name);
    }
}

/// Extract test refinements from an if-condition.
/// Recognizes presence refinements (-n → Set, -z → Unset) and
/// path refinements (-f → ExistingFile, -d → ExistingDir, -e → ExistingPath).
fn extract_test_refinements(condition: &[Statement]) -> Vec<TestRefinement> {
    let mut refinements = Vec::new();
    for stmt in condition {
        if let Statement::Command(cmd) = stmt
            && is_test_command(cmd) && cmd.args.len() >= 2
        {
            let flag = cmd.args.first().and_then(|w| {
                w.segments.first().and_then(|s| match s {
                    WordSegment::Literal(lit) => Some(lit.as_str()),
                    _ => None,
                })
            });
            let var_name = cmd.args.get(1).and_then(extract_single_var_name);
            if let (Some(flag), Some(name)) = (flag, var_name) {
                match flag {
                    "-n" => refinements.push(TestRefinement {
                        var_name: name,
                        presence: Presence::Set,
                        type_refinement: None,
                    }),
                    "-z" => refinements.push(TestRefinement {
                        var_name: name,
                        presence: Presence::Unset,
                        type_refinement: None,
                    }),
                    "-f" => refinements.push(TestRefinement {
                        var_name: name,
                        presence: Presence::Set,
                        type_refinement: Some("ExistingFile".into()),
                    }),
                    "-d" => refinements.push(TestRefinement {
                        var_name: name,
                        presence: Presence::Set,
                        type_refinement: Some("ExistingDir".into()),
                    }),
                    "-e" => refinements.push(TestRefinement {
                        var_name: name,
                        presence: Presence::Set,
                        type_refinement: Some("ExistingPath".into()),
                    }),
                    _ => {}
                }
            }
        }
    }
    refinements
}

fn extract_single_var_name(word: &Word) -> Option<String> {
    for seg in &word.segments {
        match seg {
            WordSegment::ParamExpand(pe) if pe.operator.is_none() => return Some(pe.name.clone()),
            WordSegment::DoubleQuoted(inner) => {
                for s in inner {
                    if let WordSegment::ParamExpand(pe) = s
                        && pe.operator.is_none()
                    {
                        return Some(pe.name.clone());
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Handle list elements with flow tracking, including `|| return`/`|| exit` patterns.
fn check_list_flow(
    list: &List,
    ctx: &CheckCtx,
    scope: ScopeId,
    diagnostics: &mut Vec<Diagnostic>,
    flow: &mut FlowState,
) {
    for (i, elem) in list.elements.iter().enumerate() {
        check_statement(&elem.statement, ctx, scope, diagnostics, flow);

        // After `cmd || return/exit`, the left-side's refinements carry forward
        // because if cmd failed, we'd have returned/exited
        if elem.operator == Some(ListOperator::Or)
            && let Some(next) = list.elements.get(i + 1)
            && is_exit_or_return(&next.statement)
        {
            // The guard pattern passed — any test refinements from elem.statement carry forward
            if let Statement::Command(cmd) = &elem.statement
                && is_test_command(cmd)
            {
                let refinements = extract_test_refinements(std::slice::from_ref(&elem.statement));
                for r in refinements {
                    flow.presence.insert(r.var_name.clone(), r.presence);
                    if let Some(ref type_ref) = r.type_refinement {
                        flow.refinements.entry(r.var_name).or_default().insert(type_ref.clone());
                    }
                }
            }
        }
    }
}

fn is_exit_or_return(stmt: &Statement) -> bool {
    match stmt {
        Statement::Command(cmd) => {
            matches!(command_name_str(cmd), Some("return" | "exit"))
        }
        _ => false,
    }
}

/// BT102/BT401: Check function call site against #@sig contract.
fn check_call_site(
    cmd: &Command,
    source_id: SourceId,
    symbols: &SymbolTable,
    scope: ScopeId,
    sigs: &FunctionSigs,
    flow: &FlowState,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let func_name = match command_name_str(cmd) {
        Some(n) => n,
        None => return,
    };
    let sig = match sigs.get(func_name) {
        Some(s) => s,
        None => return,
    };

    let has_variadic = sig.params.iter().any(|p| {
        matches!(&p.type_expr, TypeExpr::Named(n) if n == "Argv")
            || matches!(&p.type_expr, TypeExpr::Parameterized { name, .. } if name == "Argv")
    });

    // Check argument count
    let expected = sig.params.len();
    let actual = cmd.args.len();
    if !has_variadic && actual != expected {
        diagnostics.push(
            Diagnostic::error(
                BT102,
                format!(
                    "function '{}' expects {} argument{} but got {}",
                    func_name,
                    expected,
                    if expected == 1 { "" } else { "s" },
                    actual,
                ),
                source_id,
                cmd.span,
            )
            .with_help(format!(
                "signature: {}({})",
                func_name,
                sig.params.iter().map(|p| format!("{}: {}", p.name, format_type_expr(&p.type_expr))).collect::<Vec<_>>().join(", ")
            )),
        );
    } else if has_variadic && actual < expected.saturating_sub(1) {
        // Variadic: need at least the non-variadic params
        let required = expected - 1;
        diagnostics.push(
            Diagnostic::error(
                BT102,
                format!(
                    "function '{}' expects at least {} argument{} but got {}",
                    func_name,
                    required,
                    if required == 1 { "" } else { "s" },
                    actual,
                ),
                source_id,
                cmd.span,
            )
            .with_help(format!(
                "signature: {}({})",
                func_name,
                sig.params.iter().map(|p| format!("{}: {}", p.name, format_type_expr(&p.type_expr))).collect::<Vec<_>>().join(", ")
            )),
        );
    }

    // Check expansion shape of each argument against the declared param type
    for (i, param) in sig.params.iter().enumerate() {
        if i >= cmd.args.len() {
            break;
        }
        let arg = &cmd.args[i];
        let param_shape = type_expr_shape(&param.type_expr);

        match param_shape {
            ExpansionShape::Scalar => {
                // Param expects scalar — check if arg is an array expansion
                if word_is_array_expansion(arg) {
                    diagnostics.push(
                        Diagnostic::error(
                            BT102,
                            format!(
                                "argument '{}' to '{}' expects Scalar but got array expansion",
                                param.name, func_name,
                            ),
                            source_id,
                            arg.span,
                        )
                        .with_help(format!(
                            "parameter '{}' is declared as {} — pass a single value, not an array",
                            param.name, format_type_expr(&param.type_expr)
                        )),
                    );
                }
            }
            ExpansionShape::Argv => {
                // Param expects argv — check if arg is a bare scalar (not array expansion)
                if word_is_bare_scalar_expansion(arg, symbols, scope) {
                    diagnostics.push(
                        Diagnostic::warning(
                            BT102,
                            format!(
                                "argument '{}' to '{}' expects Argv but got scalar expansion",
                                param.name, func_name,
                            ),
                            source_id,
                            arg.span,
                        )
                        .with_help(format!(
                            "parameter '{}' is declared as {} — consider passing an array expansion like \"${{arr[@]}}\"",
                            param.name, format_type_expr(&param.type_expr)
                        )),
                    );
                }
            }
            _ => {}
        }

        // BT401/BT405-407: check refinement requirements
        if let Some(required_refinement) = type_expr_inner_refinement(&param.type_expr)
            && is_path_refinement(&required_refinement)
            && let Some(var_name) = extract_single_var_name(arg)
        {
            let has_proof = flow
                .refinements
                .get(&var_name)
                .is_some_and(|refs| refs.contains(&required_refinement));
            if !has_proof {
                // Check if the proof was invalidated — emit targeted diagnostics
                let invalidation = flow
                    .invalidated
                    .get(&var_name)
                    .and_then(|inv| inv.iter().find(|p| p.refinement == required_refinement));

                if let Some(inv) = invalidation {
                    let (code, cause_desc) = match &inv.cause {
                        InvalidationCause::Cd => (BT405, "'cd' changed the working directory".to_string()),
                        InvalidationCause::WritesFs(cmd) => (BT406, format!("'{}' may modify the filesystem", cmd)),
                        InvalidationCause::UnknownCall(cmd) => (BT407, format!("unknown function '{}' may have side effects", cmd)),
                        InvalidationCause::Source => (BT407, "'source' may execute arbitrary code".to_string()),
                    };
                    diagnostics.push(
                        Diagnostic::warning(
                            code,
                            format!(
                                "{} proof for '{}' was invalidated before use in '{}'",
                                required_refinement, var_name, func_name,
                            ),
                            source_id,
                            arg.span,
                        )
                        .with_label(inv.cause_span, cause_desc.clone())
                        .with_help(format!(
                            "re-check with `[[ {} \"${}\" ]] || return 1` after the invalidating command",
                            match required_refinement.as_str() {
                                "ExistingFile" => "-f",
                                "ExistingDir" => "-d",
                                _ => "-e",
                            },
                            var_name
                        ))
                        .with_agent_context(format!(
                            "The {} proof for '{}' was established earlier but then {} before the call to '{}'. \
                             The file may no longer exist. Re-verify after the effectful command.",
                            required_refinement, var_name, cause_desc, func_name,
                        )),
                    );
                } else {
                    let guard_flag = match required_refinement.as_str() {
                        "ExistingFile" => "-f",
                        "ExistingDir" => "-d",
                        "ExistingPath" => "-e",
                        _ => "-e",
                    };
                    diagnostics.push(
                        Diagnostic::warning(
                            BT401,
                            format!(
                                "argument '{}' to '{}' requires {} but no proof found",
                                param.name, func_name, required_refinement,
                            ),
                            source_id,
                            arg.span,
                        )
                        .with_help(format!(
                            "guard with `[[ {} \"${}\" ]] || return 1` before the call",
                            guard_flag, var_name
                        ))
                        .with_agent_context(format!(
                            "Parameter '{}' is declared as {} which requires a runtime proof. \
                             Use a test guard like `[[ {} \"${}\" ]]` before calling '{}' \
                             so the checker can verify the path exists.",
                            param.name,
                            format_type_expr(&param.type_expr),
                            guard_flag,
                            var_name,
                            func_name,
                        )),
                    );
                }
            }
        }
    }
}

fn is_path_refinement(name: &str) -> bool {
    matches!(name, "ExistingFile" | "ExistingDir" | "ExistingPath")
}

/// Extract the inner refinement name from a type expression.
/// e.g., Scalar[ExistingFile] → Some("ExistingFile"), Scalar → None
fn type_expr_inner_refinement(type_expr: &TypeExpr) -> Option<String> {
    match type_expr {
        TypeExpr::Parameterized { param, .. } => match param.as_ref() {
            TypeExpr::Named(name) => Some(name.clone()),
            _ => None,
        },
        _ => None,
    }
}

fn type_expr_shape(type_expr: &TypeExpr) -> ExpansionShape {
    match type_expr {
        TypeExpr::Named(name) if name == "Scalar" => ExpansionShape::Scalar,
        TypeExpr::Named(name) if name == "Argv" => ExpansionShape::Argv,
        TypeExpr::Parameterized { name, .. } if name == "Scalar" => ExpansionShape::Scalar,
        TypeExpr::Parameterized { name, .. } if name == "Argv" => ExpansionShape::Argv,
        _ => ExpansionShape::Scalar, // default: scalar
    }
}

fn format_type_expr(type_expr: &TypeExpr) -> String {
    match type_expr {
        TypeExpr::Named(name) => name.clone(),
        TypeExpr::Parameterized { name, param } => format!("{}[{}]", name, format_type_expr(param)),
        TypeExpr::Status(code) => format!("Status[{}]", code),
        _ => "Unknown".into(),
    }
}

fn word_is_array_expansion(word: &Word) -> bool {
    word.segments.iter().any(|seg| matches!(seg, WordSegment::ArrayExpand(_)))
        || word.segments.iter().any(|seg| {
            if let WordSegment::DoubleQuoted(inner) = seg {
                inner.iter().any(|s| matches!(s, WordSegment::ArrayExpand(_)))
            } else {
                false
            }
        })
}

fn word_is_bare_scalar_expansion(word: &Word, symbols: &SymbolTable, scope: ScopeId) -> bool {
    for seg in &word.segments {
        match seg {
            WordSegment::ParamExpand(pe) if pe.operator.is_none() => {
                if let Some(sym) = symbols.resolve(scope, &pe.name)
                    && sym.type_info.shape == ExpansionShape::Scalar
                {
                    return true;
                }
            }
            WordSegment::DoubleQuoted(inner) => {
                for s in inner {
                    if let WordSegment::ParamExpand(pe) = s
                        && pe.operator.is_none()
                        && let Some(sym) = symbols.resolve(scope, &pe.name)
                        && sym.type_info.shape == ExpansionShape::Scalar
                    {
                        return true;
                    }
                }
            }
            _ => {}
        }
    }
    false
}

fn check_word_for_bare_array(
    word: &Word,
    source_id: SourceId,
    symbols: &SymbolTable,
    scope: ScopeId,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for segment in &word.segments {
        check_segment_for_bare_array(segment, source_id, symbols, scope, diagnostics);
    }
}

fn check_segment_for_bare_array(
    segment: &WordSegment,
    source_id: SourceId,
    symbols: &SymbolTable,
    scope: ScopeId,
    diagnostics: &mut Vec<Diagnostic>,
) {
    match segment {
        WordSegment::ParamExpand(pe) => {
            // Bare $arr where arr is a declared array
            if pe.operator.is_none() && is_argv_shape_in_scope(symbols, scope, &pe.name) {
                diagnostics.push(
                    Diagnostic::error(
                        BT201,
                        format!(
                            "array '{}' used in scalar expansion — only first element will be used",
                            pe.name
                        ),
                        source_id,
                        pe.span,
                    )
                    .with_help(format!(
                        "use \"${{{}[@]}}\" to expand all elements",
                        pe.name
                    ))
                    .with_fix(Fix {
                        description: "Expand as array".into(),
                        replacement: Some(format!("\"${{{}[@]}}\"", pe.name)),
                    }),
                );
            }
        }
        WordSegment::ArrayExpand(ae) => {
            // ${var[@]} or ${var[*]} where var is a scalar — BT203
            if is_scalar_shape_in_scope(symbols, scope, &ae.name) {
                diagnostics.push(
                    Diagnostic::error(
                        BT203,
                        format!(
                            "scalar '{}' used in array expansion — variable is not an array",
                            ae.name
                        ),
                        source_id,
                        ae.span,
                    )
                    .with_help(format!(
                        "use \"${}\" for scalar expansion, or declare '{}' as an array",
                        ae.name, ae.name
                    )),
                );
            }
        }
        WordSegment::DoubleQuoted(inner) => {
            for seg in inner {
                check_segment_for_bare_array(seg, source_id, symbols, scope, diagnostics);
            }
        }
        _ => {}
    }
}

fn check_type_mismatches(
    symbols: &SymbolTable,
    source_id: SourceId,
    diagnostics: &mut Vec<Diagnostic>,
) {
    symbols.for_each_symbol(|sym| {
        if sym.type_annotation.is_none() {
            return;
        }
        let mismatch = match (sym.type_info.shape, sym.type_info.cell_kind) {
            (ExpansionShape::Scalar, CellKind::IndexedArray) => Some((
                "Scalar",
                "IndexedArray",
                "annotation declares Scalar but variable is an indexed array",
            )),
            (ExpansionShape::Argv, CellKind::Scalar) => Some((
                "Argv",
                "Scalar",
                "annotation declares Argv but variable is a scalar",
            )),
            _ => None,
        };
        if let Some((ann_shape, bash_kind, message)) = mismatch {
            diagnostics.push(
                Diagnostic::error(
                    BT101,
                    format!(
                        "type mismatch for '{}': {}",
                        sym.name, message
                    ),
                    source_id,
                    sym.decl_span,
                )
                .with_help(format!(
                    "change the annotation to match the declaration, or vice versa (annotation shape: {}, bash kind: {})",
                    ann_shape, bash_kind
                )),
            );
        }
    });
}

fn is_test_command(cmd: &Command) -> bool {
    cmd.name.segments.first().is_some_and(|seg| {
        matches!(seg, WordSegment::Literal(s) if s == "[[" || s == "[" || s == "test")
    })
}

fn is_special_var(name: &str) -> bool {
    matches!(name, "?" | "#" | "$" | "!" | "-" | "0")
}

fn check_word_for_unquoted_expansion(
    word: &Word,
    source_id: SourceId,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for segment in &word.segments {
        check_segment_for_unquoted_expansion(segment, source_id, diagnostics, false);
    }
}

fn check_segment_for_unquoted_expansion(
    segment: &WordSegment,
    source_id: SourceId,
    diagnostics: &mut Vec<Diagnostic>,
    quoted: bool,
) {
    match segment {
        WordSegment::ParamExpand(pe) => {
            if !quoted && pe.operator.is_none() && !is_special_var(&pe.name) {
                diagnostics.push(
                    Diagnostic::warning(
                        BT202,
                        format!("unquoted expansion '${}' may undergo word splitting and globbing", pe.name),
                        source_id,
                        pe.span,
                    )
                    .with_help(format!("wrap in double quotes: \"${}\"", pe.name))
                    .with_fix(Fix {
                        description: "Quote the variable".into(),
                        replacement: Some(format!("\"${}\"", pe.name)),
                    })
                    .with_agent_context(
                        "Unquoted variable expansions undergo word splitting and pathname expansion. \
                         If the variable contains spaces or glob characters, this will produce unexpected arguments."
                    ),
                );
            }
        }
        WordSegment::DoubleQuoted(inner) => {
            for seg in inner {
                check_segment_for_unquoted_expansion(seg, source_id, diagnostics, true);
            }
        }
        _ => {}
    }
}

fn command_name_str(cmd: &Command) -> Option<&str> {
    cmd.name.segments.first().and_then(|seg| match seg {
        WordSegment::Literal(s) => Some(s.as_str()),
        _ => None,
    })
}

fn word_has_unquoted_expansion(word: &Word) -> bool {
    word.segments.iter().any(|seg| segment_has_unquoted_expansion(seg, false))
}

fn segment_has_unquoted_expansion(segment: &WordSegment, quoted: bool) -> bool {
    match segment {
        WordSegment::ParamExpand(pe) => {
            !quoted && pe.operator.is_none() && !is_special_var(&pe.name)
        }
        WordSegment::DoubleQuoted(inner) => {
            inner.iter().any(|seg| segment_has_unquoted_expansion(seg, true))
        }
        _ => false,
    }
}

fn check_destructive_unquoted(
    cmd: &Command,
    source_id: SourceId,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let name = match command_name_str(cmd) {
        Some(n) => n,
        None => return,
    };
    let sig = match stdlib::lookup_command(name) {
        Some(s) => s,
        None => return,
    };
    if sig.destructiveness < Destructiveness::Destructive {
        return;
    }
    for arg in &cmd.args {
        if word_has_unquoted_expansion(arg) {
            for seg in &arg.segments {
                if let WordSegment::ParamExpand(pe) = seg
                    && pe.operator.is_none()
                    && !is_special_var(&pe.name)
                {
                    diagnostics.push(
                        Diagnostic::error(
                            BT801,
                            format!(
                                "destructive command '{}' with unquoted variable '${}' — risk of unintended targets",
                                name, pe.name
                            ),
                            source_id,
                            cmd.span,
                        )
                        .with_label(pe.span, format!("unquoted '${}'", pe.name))
                        .with_help(format!(
                            "quote the variable: \"${}\" — or validate it before use",
                            pe.name
                        ))
                        .with_fix(Fix {
                            description: "Quote the variable".into(),
                            replacement: Some(format!("\"${}\"", pe.name)),
                        })
                        .with_agent_context(format!(
                            "'{}' is a {} command. An unquoted variable may expand to unexpected \
                             filenames via word splitting or globbing, potentially destroying the wrong files.",
                            name,
                            match sig.destructiveness {
                                Destructiveness::Destructive => "destructive",
                                Destructiveness::SystemAltering => "system-altering",
                                _ => "dangerous",
                            }
                        )),
                    );
                }
            }
        }
    }
}

fn is_cd_command(stmt: &Statement) -> Option<&Command> {
    match stmt {
        Statement::Command(cmd) => {
            if command_name_str(cmd) == Some("cd") {
                Some(cmd)
            } else {
                None
            }
        }
        _ => None,
    }
}

fn check_list_for_cd_semi(
    list: &List,
    source_id: SourceId,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for (i, elem) in list.elements.iter().enumerate() {
        if let Some(cd_cmd) = is_cd_command(&elem.statement)
            && (elem.operator == Some(ListOperator::Semi)
                || (elem.operator.is_none() && i + 1 < list.elements.len()))
            && let Some(next) = list.elements.get(i + 1)
        {
            emit_bt802(cd_cmd, &next.statement, source_id, diagnostics);
        }
    }
}

fn check_consecutive_cd_items(
    items: &[Item],
    source_id: SourceId,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for window in items.windows(2) {
        if let (Item::Statement(stmt_a), Item::Statement(stmt_b)) = (&window[0], &window[1])
            && let Some(cd_cmd) = is_cd_command(stmt_a)
        {
            emit_bt802(cd_cmd, stmt_b, source_id, diagnostics);
        }
    }
}

fn emit_bt802(
    cd_cmd: &Command,
    next_stmt: &Statement,
    source_id: SourceId,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let next_name = match next_stmt {
        Statement::Command(cmd) => command_name_str(cmd).unwrap_or("..."),
        Statement::Pipeline(_) => "pipeline",
        _ => return,
    };
    diagnostics.push(
        Diagnostic::error(
            BT802,
            format!(
                "'cd' followed by '{}' without error check — if 'cd' fails, '{}' runs in the wrong directory",
                next_name, next_name
            ),
            source_id,
            cd_cmd.span,
        )
        .with_help("use 'cd /path && cmd' or 'cd /path || exit 1' to guard against cd failure")
        .with_fix(Fix {
            description: "Use && instead of ;".into(),
            replacement: None,
        })
        .with_agent_context(
            "If 'cd' fails (directory doesn't exist, no permission), the next command runs in the \
             original directory. For destructive commands like 'rm', this can delete files in the wrong location. \
             Use '&&' so the next command only runs if 'cd' succeeds."
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use argvtype_syntax::lower::parse_and_lower;
    use argvtype_syntax::span::{SourceFile, SourceId};

    fn check_src(src: &str) -> Vec<Diagnostic> {
        let source = SourceFile::new(SourceId(0), "test.sh".into(), src.into());
        let result = parse_and_lower(source);
        check(&result.source_unit)
    }

    #[test]
    fn bare_array_expansion_detected() {
        let diagnostics = check_src("local -a arr=(1 2 3)\necho $arr");
        assert!(!diagnostics.is_empty());
        assert_eq!(diagnostics[0].code, BT201);
    }

    #[test]
    fn proper_array_expansion_ok() {
        let diagnostics = check_src("local -a arr=(1 2 3)\necho \"${arr[@]}\"");
        let bt201s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT201).collect();
        assert!(bt201s.is_empty());
    }

    #[test]
    fn clean_code_no_diagnostics() {
        let diagnostics = check_src("x=hello\necho \"$x\"");
        assert!(diagnostics.is_empty());
    }

    // BT202 tests

    #[test]
    fn unquoted_expansion_detected() {
        let diagnostics = check_src("x=hello\necho $x");
        let bt202s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT202).collect();
        assert_eq!(bt202s.len(), 1);
        assert!(bt202s[0].message.contains("unquoted expansion"));
    }

    #[test]
    fn quoted_expansion_ok() {
        let diagnostics = check_src("x=hello\necho \"$x\"");
        let bt202s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT202).collect();
        assert!(bt202s.is_empty());
    }

    #[test]
    fn assignment_rhs_no_bt202() {
        let diagnostics = check_src("x=hello\ny=$x");
        let bt202s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT202).collect();
        assert!(bt202s.is_empty());
    }

    #[test]
    fn for_items_no_bt202() {
        let diagnostics = check_src("x=hello\nfor f in $x; do echo \"$f\"; done");
        let bt202s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT202).collect();
        assert!(bt202s.is_empty());
    }

    #[test]
    fn test_command_no_bt202() {
        let diagnostics = check_src("x=hello\n[[ -f $x ]]");
        let bt202s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT202).collect();
        assert!(bt202s.is_empty());
    }

    #[test]
    fn special_var_no_bt202() {
        let diagnostics = check_src("echo $?");
        let bt202s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT202).collect();
        assert!(bt202s.is_empty());
    }

    #[test]
    fn multiple_unquoted_args() {
        let diagnostics = check_src("a=x\nb=y\ncp $a $b");
        let bt202s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT202).collect();
        assert_eq!(bt202s.len(), 2);
    }

    #[test]
    fn pipeline_unquoted() {
        let diagnostics = check_src("x=hello\ny=world\necho $x | grep $y");
        let bt202s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT202).collect();
        assert_eq!(bt202s.len(), 2);
    }

    #[test]
    fn bt202_has_fix() {
        let diagnostics = check_src("x=hello\necho $x");
        let bt202s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT202).collect();
        assert!(bt202s[0].fix.is_some());
        assert_eq!(bt202s[0].fix.as_ref().unwrap().description, "Quote the variable");
    }

    // BT801 tests

    #[test]
    fn bt801_rm_unquoted() {
        let diagnostics = check_src("f=test\nrm $f");
        let bt801s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT801).collect();
        assert_eq!(bt801s.len(), 1);
        assert!(bt801s[0].message.contains("destructive command"));
    }

    #[test]
    fn bt801_rm_quoted_ok() {
        let diagnostics = check_src("f=test\nrm \"$f\"");
        let bt801s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT801).collect();
        assert!(bt801s.is_empty());
    }

    #[test]
    fn bt801_mv_unquoted() {
        let diagnostics = check_src("a=src\nb=dst\nmv $a $b");
        let bt801s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT801).collect();
        assert_eq!(bt801s.len(), 2);
    }

    #[test]
    fn bt801_echo_no_trigger() {
        let diagnostics = check_src("x=hello\necho $x");
        let bt801s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT801).collect();
        assert!(bt801s.is_empty());
    }

    #[test]
    fn bt801_cp_no_trigger() {
        // cp is Modifying, not Destructive
        let diagnostics = check_src("f=test\ncp $f /tmp/");
        let bt801s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT801).collect();
        assert!(bt801s.is_empty());
    }

    #[test]
    fn bt801_has_agent_context() {
        let diagnostics = check_src("f=test\nrm $f");
        let bt801s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT801).collect();
        assert!(bt801s[0].agent_context.is_some());
        assert!(bt801s[0].agent_context.as_ref().unwrap().contains("destructive"));
    }

    // BT802 tests

    #[test]
    fn bt802_cd_semi_rm() {
        // tree-sitter splits top-level ; into separate items
        let diagnostics = check_src("cd /tmp; rm file");
        let bt802s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT802).collect();
        assert_eq!(bt802s.len(), 1);
        assert!(bt802s[0].message.contains("cd"));
    }

    #[test]
    fn bt802_cd_and_ok() {
        let diagnostics = check_src("cd /tmp && rm file");
        let bt802s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT802).collect();
        assert!(bt802s.is_empty());
    }

    #[test]
    fn bt802_cd_or_exit_ok() {
        let diagnostics = check_src("cd /tmp || exit 1");
        let bt802s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT802).collect();
        assert!(bt802s.is_empty());
    }

    #[test]
    fn bt802_cd_newline_rm() {
        // Consecutive top-level statements (newline separated)
        let diagnostics = check_src("cd /tmp\nrm file");
        let bt802s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT802).collect();
        assert_eq!(bt802s.len(), 1);
    }

    #[test]
    fn bt802_no_cd_no_trigger() {
        let diagnostics = check_src("echo hello\nrm file");
        let bt802s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT802).collect();
        assert!(bt802s.is_empty());
    }

    #[test]
    fn bt802_has_fix() {
        let diagnostics = check_src("cd /tmp; rm file");
        let bt802s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT802).collect();
        assert!(bt802s[0].fix.is_some());
        assert_eq!(bt802s[0].fix.as_ref().unwrap().description, "Use && instead of ;");
    }

    // Symbol-table-aware tests

    #[test]
    fn bt201_local_array_in_function() {
        let diagnostics = check_src("foo() { local -a arr=(1 2 3); echo $arr; }");
        let bt201s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT201).collect();
        assert_eq!(bt201s.len(), 1);
    }

    #[test]
    fn bt201_function_array_not_visible_at_top() {
        // Array declared in function should not trigger BT201 at top level
        let diagnostics = check_src("foo() { local -a arr=(1 2 3); }\necho $arr");
        let bt201s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT201).collect();
        assert!(bt201s.is_empty());
    }

    // BT101 tests

    #[test]
    fn bt101_annotation_conflicts_with_declaration() {
        // Scalar annotation on an indexed array should trigger BT101
        let src = "\
#@type x: Scalar[String]
declare -a x=(1 2 3)";
        let diagnostics = check_src(src);
        let bt101s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT101).collect();
        assert_eq!(bt101s.len(), 1);
        assert!(bt101s[0].message.contains("type mismatch"));
    }

    #[test]
    fn bt101_no_conflict_when_matching() {
        // Argv annotation on an indexed array should NOT trigger BT101
        let src = "\
#@type x: Argv[String]
declare -a x=(1 2 3)";
        let diagnostics = check_src(src);
        let bt101s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT101).collect();
        assert!(bt101s.is_empty());
    }

    // BT302 tests

    #[test]
    fn bt302_unset_local_fires() {
        let diagnostics = check_src("foo() { local x; echo \"$x\"; }");
        let bt302s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT302).collect();
        assert_eq!(bt302s.len(), 1);
        assert!(bt302s[0].message.contains("may be unset"));
    }

    #[test]
    fn bt302_after_guard_no_fire() {
        let diagnostics = check_src("foo() { local x; : \"${x:?required}\"; echo \"$x\"; }");
        let bt302s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT302).collect();
        assert!(bt302s.is_empty());
    }

    #[test]
    fn bt302_default_operator_no_fire() {
        let diagnostics = check_src("foo() { local x; echo \"${x:-default}\"; }");
        let bt302s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT302).collect();
        assert!(bt302s.is_empty());
    }

    #[test]
    fn bt302_assigned_var_no_fire() {
        let diagnostics = check_src("x=hello\necho \"$x\"");
        let bt302s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT302).collect();
        assert!(bt302s.is_empty());
    }

    #[test]
    fn bt302_if_n_then_no_fire() {
        let src = "foo() {\n  local x\n  if [[ -n \"$x\" ]]; then\n    echo \"$x\"\n  fi\n}";
        let diagnostics = check_src(src);
        let bt302s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT302).collect();
        // The condition itself may fire (expanding to test), but inside then-body should not
        // Filter to only those in the echo command (after the if)
        // Since -n test is in condition (a test command), BT302 won't fire there
        // Inside then-body, x is refined to Set — no BT302
        assert!(bt302s.is_empty(), "got {} BT302s: {:?}", bt302s.len(), bt302s.iter().map(|d| &d.message).collect::<Vec<_>>());
    }

    #[test]
    fn bt302_well_known_no_fire() {
        let diagnostics = check_src("echo \"$PATH\"");
        let bt302s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT302).collect();
        assert!(bt302s.is_empty());
    }

    // BT301 tests

    #[test]
    fn bt301_undeclared_fires() {
        let diagnostics = check_src("echo \"$undeclared_var\"");
        let bt301s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT301).collect();
        assert_eq!(bt301s.len(), 1);
        assert!(bt301s[0].message.contains("undeclared"));
    }

    #[test]
    fn bt301_declared_no_fire() {
        let diagnostics = check_src("x=hello\necho \"$x\"");
        let bt301s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT301).collect();
        assert!(bt301s.is_empty());
    }

    #[test]
    fn bt301_env_like_no_fire() {
        // All-uppercase names are likely env vars — don't warn
        let diagnostics = check_src("echo \"$MY_ENV_VAR\"");
        let bt301s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT301).collect();
        assert!(bt301s.is_empty());
    }

    #[test]
    fn bt301_special_var_no_fire() {
        let diagnostics = check_src("echo \"$?\"");
        let bt301s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT301).collect();
        assert!(bt301s.is_empty());
    }

    #[test]
    fn bt301_positional_param_no_fire() {
        let diagnostics = check_src("echo \"$1\"");
        let bt301s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT301).collect();
        assert!(bt301s.is_empty());
    }

    #[test]
    fn bt302_has_agent_context() {
        let diagnostics = check_src("foo() { local x; echo \"$x\"; }");
        let bt302s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT302).collect();
        assert!(bt302s[0].agent_context.is_some());
    }

    #[test]
    fn bt302_assignment_then_use_no_fire() {
        let diagnostics = check_src("foo() { local x; x=hello; echo \"$x\"; }");
        let bt302s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT302).collect();
        assert!(bt302s.is_empty());
    }

    // Command-aware presence effect tests

    // BT203 tests: scalar used in array expansion

    #[test]
    fn bt203_scalar_array_expand() {
        let diagnostics = check_src("x=hello\necho \"${x[@]}\"");
        let bt203s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT203).collect();
        assert_eq!(bt203s.len(), 1);
        assert!(bt203s[0].message.contains("scalar"));
    }

    #[test]
    fn bt203_no_fire_on_array() {
        let diagnostics = check_src("declare -a arr=(1 2 3)\necho \"${arr[@]}\"");
        let bt203s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT203).collect();
        assert!(bt203s.is_empty());
    }

    #[test]
    fn bt203_no_fire_on_unknown() {
        // Unknown variables should not trigger BT203
        let diagnostics = check_src("echo \"${unknown_var[@]}\"");
        let bt203s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT203).collect();
        assert!(bt203s.is_empty());
    }

    #[test]
    fn bt201_annotation_argv_bare_expand() {
        // Variable annotated as Argv but expanded as bare $var — should trigger BT201
        let src = "\
#@type files: Argv[String]
declare -a files=(a b c)
echo $files";
        let diagnostics = check_src(src);
        let bt201s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT201).collect();
        assert_eq!(bt201s.len(), 1);
    }

    // BT102 tests: function call site checking

    #[test]
    fn bt102_wrong_arg_count() {
        let src = "\
#@sig deploy(cfg: Scalar[ExistingFile], env: Scalar[String]) -> Status[0]
deploy() {
  echo done
}
deploy one_arg";
        let diagnostics = check_src(src);
        let bt102s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT102).collect();
        assert_eq!(bt102s.len(), 1);
        assert!(bt102s[0].message.contains("expects 2 arguments but got 1"));
    }

    #[test]
    fn bt102_correct_arg_count_no_fire() {
        let src = "\
#@sig deploy(cfg: Scalar[ExistingFile], env: Scalar[String]) -> Status[0]
deploy() {
  echo done
}
deploy config.yaml production";
        let diagnostics = check_src(src);
        let bt102s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT102).collect();
        assert!(bt102s.is_empty());
    }

    #[test]
    fn bt102_too_many_args() {
        let src = "\
#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0]
deploy() {
  echo done
}
deploy config.yaml extra_arg";
        let diagnostics = check_src(src);
        let bt102s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT102).collect();
        assert_eq!(bt102s.len(), 1);
        assert!(bt102s[0].message.contains("expects 1 argument but got 2"));
    }

    #[test]
    fn bt102_variadic_allows_extra() {
        let src = "\
#@sig process(dir: Scalar[ExistingDir], files: Argv[String]) -> Status[0]
process() {
  echo done
}
process /tmp a.txt b.txt c.txt";
        let diagnostics = check_src(src);
        let bt102s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT102).collect();
        assert!(bt102s.is_empty());
    }

    #[test]
    fn bt102_no_sig_no_fire() {
        // Function without #@sig should not trigger BT102
        let src = "deploy() { echo done; }\ndeploy a b c";
        let diagnostics = check_src(src);
        let bt102s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT102).collect();
        assert!(bt102s.is_empty());
    }

    #[test]
    fn bt102_array_expansion_for_scalar_param() {
        let src = "\
#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0]
deploy() {
  echo done
}
declare -a arr=(a b)
deploy \"${arr[@]}\"";
        let diagnostics = check_src(src);
        let bt102s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT102).collect();
        assert_eq!(bt102s.len(), 1);
        assert!(bt102s[0].message.contains("expects Scalar but got array expansion"));
    }

    // Command-aware presence effect tests

    #[test]
    fn read_sets_variable() {
        let diagnostics = check_src("foo() { local x; read x; echo \"$x\"; }");
        let bt302s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT302).collect();
        assert!(bt302s.is_empty(), "read should mark variable as Set");
    }

    #[test]
    fn read_r_sets_variable() {
        let diagnostics = check_src("foo() { local line; read -r line; echo \"$line\"; }");
        let bt302s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT302).collect();
        assert!(bt302s.is_empty(), "read -r should mark variable as Set");
    }

    #[test]
    fn unset_makes_variable_unset() {
        let diagnostics = check_src("x=hello\nunset x\necho \"$x\"");
        let bt302s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT302).collect();
        assert_eq!(bt302s.len(), 1, "unset should mark variable as Unset");
    }

    #[test]
    fn mapfile_sets_variable() {
        let diagnostics = check_src("foo() { local lines; mapfile lines; echo \"$lines\"; }");
        let bt302s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT302).collect();
        assert!(bt302s.is_empty(), "mapfile should mark variable as Set");
    }

    #[test]
    fn printf_v_sets_variable() {
        let diagnostics = check_src("foo() { local out; printf -v out '%s' hello; echo \"$out\"; }");
        let bt302s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT302).collect();
        assert!(bt302s.is_empty(), "printf -v should mark variable as Set");
    }

    #[test]
    fn read_multiple_vars_sets_all() {
        let diagnostics = check_src("foo() { local a; local b; read a b; echo \"$a\" \"$b\"; }");
        let bt302s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT302).collect();
        assert!(bt302s.is_empty(), "read should mark all variables as Set");
    }

    // Loop convergence tests

    #[test]
    fn for_loop_var_maybe_unset_after() {
        let src = "foo() {\n  local result\n  for f in *.txt; do\n    result=\"found\"\n  done\n  echo \"$result\"\n}";
        let diagnostics = check_src(src);
        let bt302s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT302).collect();
        assert_eq!(bt302s.len(), 1, "var assigned only in for loop should be MaybeUnset after");
    }

    #[test]
    fn for_loop_var_set_before_stays_set() {
        let src = "foo() {\n  local result=default\n  for f in *.txt; do\n    result=\"found\"\n  done\n  echo \"$result\"\n}";
        let diagnostics = check_src(src);
        let bt302s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT302).collect();
        assert!(bt302s.is_empty(), "var Set before for loop and assigned in body should stay Set");
    }

    #[test]
    fn while_loop_body_assignment_maybe_unset() {
        let src = "foo() {\n  local x\n  while true; do\n    x=hello\n  done\n  echo \"$x\"\n}";
        let diagnostics = check_src(src);
        let bt302s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT302).collect();
        assert_eq!(bt302s.len(), 1, "var assigned only in while body should be MaybeUnset after");
    }

    // Case merging tests

    #[test]
    fn case_merging_without_default() {
        let src = "foo() {\n  local x\n  case \"$1\" in\n    a) x=hello ;;\n    b) x=world ;;\n  esac\n  echo \"$x\"\n}";
        let diagnostics = check_src(src);
        let bt302s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT302).collect();
        assert_eq!(bt302s.len(), 1, "case without default: var should be MaybeUnset");
    }

    #[test]
    fn case_merging_with_default() {
        let src = "foo() {\n  local x\n  case \"$1\" in\n    a) x=hello ;;\n    *) x=default ;;\n  esac\n  echo \"$x\"\n}";
        let diagnostics = check_src(src);
        let bt302s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT302).collect();
        assert!(bt302s.is_empty(), "case with default and all arms assigning: var should be Set");
    }

    // Path refinement tests (BT401)

    #[test]
    fn bt401_no_proof_fires() {
        let src = "\
#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0]
deploy() { echo done; }
cfg=/etc/config
deploy \"$cfg\"";
        let diagnostics = check_src(src);
        let bt401s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT401).collect();
        assert_eq!(bt401s.len(), 1, "calling with unproven ExistingFile should fire BT401");
        assert!(bt401s[0].message.contains("ExistingFile"));
    }

    #[test]
    fn bt401_with_f_guard_no_fire() {
        let src = "\
#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0]
deploy() { echo done; }
cfg=/etc/config
if [[ -f \"$cfg\" ]]; then
  deploy \"$cfg\"
fi";
        let diagnostics = check_src(src);
        let bt401s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT401).collect();
        assert!(bt401s.is_empty(), "ExistingFile proof via [[ -f ]] should suppress BT401");
    }

    #[test]
    fn bt401_with_or_return_guard_no_fire() {
        let src = "\
#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0]
deploy() { echo done; }
cfg=/etc/config
[[ -f \"$cfg\" ]] || return 1
deploy \"$cfg\"";
        let diagnostics = check_src(src);
        let bt401s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT401).collect();
        assert!(bt401s.is_empty(), "ExistingFile proof via || return should suppress BT401");
    }

    #[test]
    fn bt401_wrong_refinement_fires() {
        let src = "\
#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0]
deploy() { echo done; }
cfg=/etc/config
if [[ -d \"$cfg\" ]]; then
  deploy \"$cfg\"
fi";
        let diagnostics = check_src(src);
        let bt401s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT401).collect();
        assert_eq!(bt401s.len(), 1, "-d proves ExistingDir, not ExistingFile");
    }

    #[test]
    fn bt401_d_guard_for_existing_dir() {
        let src = "\
#@sig scan(dir: Scalar[ExistingDir]) -> Status[0]
scan() { echo done; }
d=/tmp/out
if [[ -d \"$d\" ]]; then
  scan \"$d\"
fi";
        let diagnostics = check_src(src);
        let bt401s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT401).collect();
        assert!(bt401s.is_empty(), "-d proof should satisfy ExistingDir");
    }

    #[test]
    fn bt401_no_fire_for_non_path_refinement() {
        let src = "\
#@sig greet(name: Scalar[String]) -> Status[0]
greet() { echo done; }
n=world
greet \"$n\"";
        let diagnostics = check_src(src);
        let bt401s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT401).collect();
        assert!(bt401s.is_empty(), "String is not a path refinement — no BT401");
    }

    #[test]
    fn bt401_refinement_lost_after_reassignment() {
        let src = "\
#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0]
deploy() { echo done; }
cfg=/etc/config
if [[ -f \"$cfg\" ]]; then
  cfg=/other/path
  deploy \"$cfg\"
fi";
        let diagnostics = check_src(src);
        let bt401s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT401).collect();
        assert_eq!(bt401s.len(), 1, "reassignment should invalidate ExistingFile proof");
    }

    #[test]
    fn path_refinement_f_sets_presence() {
        // [[ -f "$x" ]] proves x is Set in the then-branch
        let src = "foo() {\n  local x\n  if [[ -f \"$x\" ]]; then\n    echo \"$x\"\n  fi\n}";
        let diagnostics = check_src(src);
        let bt302s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT302).collect();
        assert!(bt302s.is_empty(), "-f test should prove variable is Set in then-branch");
    }

    // ===== BT405/BT406/BT407: Proof invalidation by effects =====

    #[test]
    fn bt406_rm_invalidates_existing_file_proof() {
        let src = "\
#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0]
deploy() { echo done; }
cfg=/etc/config
[[ -f \"$cfg\" ]] || return 1
rm \"$cfg\"
deploy \"$cfg\"";
        let diagnostics = check_src(src);
        let bt406s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT406).collect();
        assert_eq!(bt406s.len(), 1, "rm should invalidate ExistingFile proof → BT406");
        assert!(bt406s[0].message.contains("invalidated"));
    }

    #[test]
    fn bt405_cd_invalidates_path_proof() {
        let src = "\
#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0]
deploy() { echo done; }
cfg=config.yaml
[[ -f \"$cfg\" ]] || return 1
cd /other/dir || return 1
deploy \"$cfg\"";
        let diagnostics = check_src(src);
        let bt405s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT405).collect();
        assert_eq!(bt405s.len(), 1, "cd should invalidate ExistingFile proof → BT405");
    }

    #[test]
    fn bt407_unknown_function_invalidates_proof() {
        let src = "\
#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0]
deploy() { echo done; }
cfg=/etc/config
[[ -f \"$cfg\" ]] || return 1
some_unknown_tool
deploy \"$cfg\"";
        let diagnostics = check_src(src);
        let bt407s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT407).collect();
        assert_eq!(bt407s.len(), 1, "unknown command should invalidate proof → BT407");
    }

    #[test]
    fn no_invalidation_from_safe_command() {
        let src = "\
#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0]
deploy() { echo done; }
cfg=/etc/config
[[ -f \"$cfg\" ]] || return 1
echo \"checking config\"
deploy \"$cfg\"";
        let diagnostics = check_src(src);
        let bt401s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT401).collect();
        let bt405s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT405).collect();
        let bt406s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT406).collect();
        let bt407s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT407).collect();
        assert!(bt401s.is_empty(), "echo should not invalidate proof");
        assert!(bt405s.is_empty());
        assert!(bt406s.is_empty());
        assert!(bt407s.is_empty());
    }

    #[test]
    fn bt406_mv_invalidates_proof() {
        let src = "\
#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0]
deploy() { echo done; }
cfg=/etc/config
[[ -f \"$cfg\" ]] || return 1
mv other.txt backup.txt
deploy \"$cfg\"";
        let diagnostics = check_src(src);
        let bt406s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT406).collect();
        assert_eq!(bt406s.len(), 1, "mv (writes_fs) should invalidate proof → BT406");
    }

    #[test]
    fn invalidation_cleared_by_reproof() {
        let src = "\
#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0]
deploy() { echo done; }
cfg=/etc/config
[[ -f \"$cfg\" ]] || return 1
rm other_file
[[ -f \"$cfg\" ]] || return 1
deploy \"$cfg\"";
        let diagnostics = check_src(src);
        let bt405_7: Vec<_> = diagnostics.iter().filter(|d| {
            d.code == BT405 || d.code == BT406 || d.code == BT407
        }).collect();
        assert!(bt405_7.is_empty(), "re-proof after invalidation should suppress BT40x");
    }

    #[test]
    fn bt407_source_invalidates_everything() {
        let src = "\
#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0]
deploy() { echo done; }
cfg=/etc/config
[[ -f \"$cfg\" ]] || return 1
source other.sh
deploy \"$cfg\"";
        let diagnostics = check_src(src);
        let bt407s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT407).collect();
        assert_eq!(bt407s.len(), 1, "source should invalidate all proofs → BT407");
    }

    #[test]
    fn sig_effects_inform_invalidation() {
        let src = "\
#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0]
deploy() { echo done; }
#@sig cleanup() -> Status[0] !writes_fs
cleanup() { echo cleaning; }
cfg=/etc/config
[[ -f \"$cfg\" ]] || return 1
cleanup
deploy \"$cfg\"";
        let diagnostics = check_src(src);
        let bt406s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT406).collect();
        assert_eq!(bt406s.len(), 1, "function with !writes_fs should invalidate proof → BT406");
    }

    #[test]
    fn sig_no_effects_no_invalidation() {
        let src = "\
#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0]
deploy() { echo done; }
#@sig validate(name: Scalar[String]) -> Status[0]
validate() { echo ok; }
cfg=/etc/config
[[ -f \"$cfg\" ]] || return 1
validate test
deploy \"$cfg\"";
        let diagnostics = check_src(src);
        let bt405_7: Vec<_> = diagnostics.iter().filter(|d| {
            d.code == BT405 || d.code == BT406 || d.code == BT407
        }).collect();
        assert!(bt405_7.is_empty(), "function with no effects should not invalidate proof");
    }

    // ===== #@proves: Custom proof functions =====

    #[test]
    fn proves_annotation_establishes_proof() {
        let src = "\
#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0]
deploy() { echo done; }
#@proves $1 ExistingFile
validate_config() {
  [[ -f \"$1\" ]] || exit 1
}
cfg=/etc/config
validate_config \"$cfg\"
deploy \"$cfg\"";
        let diagnostics = check_src(src);
        let bt401s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT401).collect();
        assert!(bt401s.is_empty(), "#@proves should establish ExistingFile proof");
    }

    #[test]
    fn proves_wrong_refinement_still_fires() {
        let src = "\
#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0]
deploy() { echo done; }
#@proves $1 ExistingDir
validate_dir() {
  [[ -d \"$1\" ]] || exit 1
}
cfg=/etc/config
validate_dir \"$cfg\"
deploy \"$cfg\"";
        let diagnostics = check_src(src);
        let bt401s: Vec<_> = diagnostics.iter().filter(|d| d.code == BT401).collect();
        assert_eq!(bt401s.len(), 1, "#@proves ExistingDir does not satisfy ExistingFile requirement");
    }

    // ===== command -v / type / hash as CommandName proof sites =====

    #[test]
    fn command_v_establishes_command_name_proof() {
        let src = "\
command -v jq
echo verified";
        let diagnostics = check_src(src);
        // Just verifying it doesn't crash and that the proof is tracked
        // (we'd need a function requiring CommandName to test the proof usage)
        assert!(diagnostics.iter().all(|d| d.code != BT401));
    }

    #[test]
    fn command_v_in_guard_proves_command() {
        let src = "\
command -v jq || return 1
echo \"jq is available\"";
        let diagnostics = check_src(src);
        // The flow should have CommandName proof for "jq"
        assert!(diagnostics.iter().all(|d| d.code != BT401));
    }
}
