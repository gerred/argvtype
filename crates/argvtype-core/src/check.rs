use std::collections::HashMap;
use argvtype_syntax::hir::*;
use argvtype_syntax::span::SourceId;
use crate::diagnostic::{Diagnostic, DiagnosticCode, Fix};
use crate::scope::{self, CellKind, Presence, ScopeId, SymbolTable, ExpansionShape};
use crate::stdlib::{self, Destructiveness};

const BT000: DiagnosticCode = DiagnosticCode { family: "BT", number: 0 };
const BT101: DiagnosticCode = DiagnosticCode { family: "BT", number: 101 };
const BT201: DiagnosticCode = DiagnosticCode { family: "BT", number: 201 };
const BT202: DiagnosticCode = DiagnosticCode { family: "BT", number: 202 };
const BT301: DiagnosticCode = DiagnosticCode { family: "BT", number: 301 };
const BT302: DiagnosticCode = DiagnosticCode { family: "BT", number: 302 };
const BT801: DiagnosticCode = DiagnosticCode { family: "BT", number: 801 };
const BT802: DiagnosticCode = DiagnosticCode { family: "BT", number: 802 };

type PresenceMap = HashMap<String, Presence>;

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
    let mut result = a.clone();
    for (name, &b_presence) in b {
        let merged = match result.get(name) {
            Some(&a_presence) => presence_join(a_presence, b_presence),
            None => b_presence,
        };
        result.insert(name.clone(), merged);
    }
    result
}

pub fn check(source_unit: &SourceUnit) -> Vec<Diagnostic> {
    let symbols = scope::build_symbol_table(source_unit);
    let mut diagnostics = Vec::new();
    let source_id = source_unit.source_id;
    let root = symbols.root_scope();
    let mut global_presence = init_presence_map(&symbols, root);

    for item in &source_unit.items {
        match item {
            Item::Function(f) => {
                let scope = symbols.scope_of_node(f.id).unwrap_or(root);
                let mut func_presence = init_presence_map(&symbols, scope);
                check_statements(&f.body, source_id, &symbols, scope, &mut diagnostics, &mut func_presence);
            }
            Item::Statement(s) => {
                check_statement(s, source_id, &symbols, root, &mut diagnostics, &mut global_presence);
            }
            _ => {}
        }
    }

    // BT802: check consecutive top-level items for cd;next pattern
    check_consecutive_cd_items(&source_unit.items, source_id, &mut diagnostics);

    // BT101: annotation/declaration shape mismatches
    check_type_mismatches(&symbols, source_id, &mut diagnostics);

    diagnostics
}

fn check_statements(
    stmts: &[Statement],
    source_id: SourceId,
    symbols: &SymbolTable,
    scope: ScopeId,
    diagnostics: &mut Vec<Diagnostic>,
    presence: &mut PresenceMap,
) {
    for stmt in stmts {
        check_statement(stmt, source_id, symbols, scope, diagnostics, presence);
    }
}

fn is_array_in_scope(symbols: &SymbolTable, scope: ScopeId, name: &str) -> bool {
    symbols
        .resolve(scope, name)
        .is_some_and(|sym| matches!(sym.type_info.cell_kind, CellKind::IndexedArray | CellKind::AssocArray))
}

fn check_statement(
    stmt: &Statement,
    source_id: SourceId,
    symbols: &SymbolTable,
    scope: ScopeId,
    diagnostics: &mut Vec<Diagnostic>,
    presence: &mut PresenceMap,
) {
    match stmt {
        Statement::Assignment(a) => {
            // Assignment sets the variable
            if a.value.is_some() || a.array_value.is_some() {
                presence.insert(a.name.clone(), Presence::Set);
            }
        }
        Statement::Command(cmd) => {
            let cmd_scope = symbols.scope_of_node(cmd.id).unwrap_or(scope);
            check_word_for_bare_array(&cmd.name, source_id, symbols, cmd_scope, diagnostics);
            for arg in &cmd.args {
                check_word_for_bare_array(arg, source_id, symbols, cmd_scope, diagnostics);
            }
            if !is_test_command(cmd) {
                // BT202: unquoted expansion in command args
                for arg in &cmd.args {
                    check_word_for_unquoted_expansion(arg, source_id, diagnostics);
                }
                // BT801: destructive command with unquoted variable
                check_destructive_unquoted(cmd, source_id, diagnostics);
                // BT301/BT302: presence checks on expansions
                check_command_presence(cmd, source_id, symbols, cmd_scope, presence, diagnostics);
            }
            // Recognize `: "${x:?msg}"` guard pattern
            apply_colon_guard(cmd, presence);
        }
        Statement::Pipeline(p) => {
            for cmd in &p.commands {
                check_statement(cmd, source_id, symbols, scope, diagnostics, presence);
            }
        }
        Statement::If(if_stmt) => {
            for s in &if_stmt.condition {
                check_statement(s, source_id, symbols, scope, diagnostics, presence);
            }

            // Extract test refinements from condition
            let refinements = extract_test_refinements(&if_stmt.condition);

            // Fork presence for then-branch
            let mut then_presence = presence.clone();
            for (name, p) in &refinements {
                then_presence.insert(name.clone(), *p);
            }
            check_statements(&if_stmt.then_body, source_id, symbols, scope, diagnostics, &mut then_presence);

            // Fork presence for else-branch (inverted refinements)
            let mut else_presence = presence.clone();
            for (name, p) in &refinements {
                let inverted = match p {
                    Presence::Set => Presence::MaybeUnset,
                    Presence::Unset => Presence::Set,
                    other => *other,
                };
                else_presence.insert(name.clone(), inverted);
            }
            if let Some(else_body) = &if_stmt.else_body {
                check_statements(else_body, source_id, symbols, scope, diagnostics, &mut else_presence);
            }

            // Merge at join point
            *presence = merge_presence_maps(&then_presence, &else_presence);
        }
        Statement::For(for_loop) => {
            for s in &for_loop.body {
                check_statement(s, source_id, symbols, scope, diagnostics, presence);
            }
        }
        Statement::While(while_loop) => {
            for s in &while_loop.condition {
                check_statement(s, source_id, symbols, scope, diagnostics, presence);
            }
            for s in &while_loop.body {
                check_statement(s, source_id, symbols, scope, diagnostics, presence);
            }
        }
        Statement::List(list) => {
            check_list_presence(list, source_id, symbols, scope, diagnostics, presence);
            // BT802: cd followed by ; instead of && within a list
            check_list_for_cd_semi(list, source_id, diagnostics);
        }
        Statement::Block(b) => {
            let block_scope = symbols.scope_of_node(b.id).unwrap_or(scope);
            let body_scope = if b.subshell {
                b.body.first()
                    .and_then(stmt_node_id)
                    .and_then(|id| symbols.scope_of_node(id))
                    .unwrap_or(block_scope)
            } else {
                block_scope
            };
            for s in &b.body {
                check_statement(s, source_id, symbols, body_scope, diagnostics, presence);
            }
        }
        Statement::Case(case_stmt) => {
            for arm in &case_stmt.arms {
                for s in &arm.body {
                    check_statement(s, source_id, symbols, scope, diagnostics, presence);
                }
            }
        }
        Statement::Unmodeled(u) => {
            diagnostics.push(
                Diagnostic::warning(
                    BT000,
                    format!("unmodeled syntax: {}", u.kind),
                    source_id,
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

/// Extract test refinements from an if-condition.
/// Recognizes `[[ -n "$x" ]]` → x is Set in then-branch,
/// `[[ -z "$x" ]]` → x is Unset in then-branch.
fn extract_test_refinements(condition: &[Statement]) -> Vec<(String, Presence)> {
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
                    "-n" => refinements.push((name, Presence::Set)),
                    "-z" => refinements.push((name, Presence::Unset)),
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

/// Handle list elements with presence tracking, including `|| return`/`|| exit` patterns.
fn check_list_presence(
    list: &List,
    source_id: SourceId,
    symbols: &SymbolTable,
    scope: ScopeId,
    diagnostics: &mut Vec<Diagnostic>,
    presence: &mut PresenceMap,
) {
    for (i, elem) in list.elements.iter().enumerate() {
        check_statement(&elem.statement, source_id, symbols, scope, diagnostics, presence);

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
                for (name, p) in refinements {
                    presence.insert(name, p);
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
            if pe.operator.is_none() && is_array_in_scope(symbols, scope, &pe.name) {
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
}
