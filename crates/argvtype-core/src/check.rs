use argvtype_syntax::hir::*;
use argvtype_syntax::span::SourceId;
use crate::diagnostic::{Diagnostic, DiagnosticCode, Fix};
use crate::stdlib::{self, Destructiveness};

const BT000: DiagnosticCode = DiagnosticCode { family: "BT", number: 0 };
const BT201: DiagnosticCode = DiagnosticCode { family: "BT", number: 201 };
const BT202: DiagnosticCode = DiagnosticCode { family: "BT", number: 202 };
const BT801: DiagnosticCode = DiagnosticCode { family: "BT", number: 801 };
const BT802: DiagnosticCode = DiagnosticCode { family: "BT", number: 802 };

pub fn check(source_unit: &SourceUnit) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let source_id = source_unit.source_id;

    // Collect top-level array names
    let mut top_array_names: Vec<String> = Vec::new();
    for item in &source_unit.items {
        if let Item::Statement(Statement::Assignment(a)) = item
            && is_array_decl(a)
        {
            top_array_names.push(a.name.clone());
        }
    }

    for item in &source_unit.items {
        match item {
            Item::Function(f) => check_statements(&f.body, source_id, &mut diagnostics),
            Item::Statement(s) => {
                check_statement_with_arrays(s, source_id, &top_array_names, &mut diagnostics);
            }
            _ => {}
        }
    }

    // BT802: check consecutive top-level items for cd;next pattern
    check_consecutive_cd_items(&source_unit.items, source_id, &mut diagnostics);

    diagnostics
}

fn check_statements(stmts: &[Statement], source_id: SourceId, diagnostics: &mut Vec<Diagnostic>) {
    // First pass: collect declared array names
    let mut array_names: Vec<String> = Vec::new();
    for stmt in stmts {
        if let Statement::Assignment(a) = stmt
            && is_array_decl(a)
        {
            array_names.push(a.name.clone());
        }
    }

    // Second pass: check for issues
    for stmt in stmts {
        check_statement_with_arrays(stmt, source_id, &array_names, diagnostics);
    }
}

fn is_array_decl(a: &Assignment) -> bool {
    // Declared with -a or -A flag, or has array_value
    a.flags.iter().any(|f| f == "-a" || f == "-A") || a.array_value.is_some()
}

fn check_statement_with_arrays(
    stmt: &Statement,
    source_id: SourceId,
    array_names: &[String],
    diagnostics: &mut Vec<Diagnostic>,
) {
    match stmt {
        Statement::Command(cmd) => {
            check_word_for_bare_array(&cmd.name, source_id, array_names, diagnostics);
            for arg in &cmd.args {
                check_word_for_bare_array(arg, source_id, array_names, diagnostics);
            }
            if !is_test_command(cmd) {
                // BT202: unquoted expansion in command args
                for arg in &cmd.args {
                    check_word_for_unquoted_expansion(arg, source_id, diagnostics);
                }
                // BT801: destructive command with unquoted variable
                check_destructive_unquoted(cmd, source_id, diagnostics);
            }
        }
        Statement::Pipeline(p) => {
            for cmd in &p.commands {
                check_statement_with_arrays(cmd, source_id, array_names, diagnostics);
            }
        }
        Statement::If(if_stmt) => {
            for s in &if_stmt.condition {
                check_statement_with_arrays(s, source_id, array_names, diagnostics);
            }
            for s in &if_stmt.then_body {
                check_statement_with_arrays(s, source_id, array_names, diagnostics);
            }
            if let Some(else_body) = &if_stmt.else_body {
                for s in else_body {
                    check_statement_with_arrays(s, source_id, array_names, diagnostics);
                }
            }
        }
        Statement::For(for_loop) => {
            for s in &for_loop.body {
                check_statement_with_arrays(s, source_id, array_names, diagnostics);
            }
        }
        Statement::While(while_loop) => {
            for s in &while_loop.condition {
                check_statement_with_arrays(s, source_id, array_names, diagnostics);
            }
            for s in &while_loop.body {
                check_statement_with_arrays(s, source_id, array_names, diagnostics);
            }
        }
        Statement::List(list) => {
            for elem in &list.elements {
                check_statement_with_arrays(&elem.statement, source_id, array_names, diagnostics);
            }
            // BT802: cd followed by ; instead of && within a list
            check_list_for_cd_semi(list, source_id, diagnostics);
        }
        Statement::Block(b) => {
            for s in &b.body {
                check_statement_with_arrays(s, source_id, array_names, diagnostics);
            }
        }
        Statement::Case(case_stmt) => {
            for arm in &case_stmt.arms {
                for s in &arm.body {
                    check_statement_with_arrays(s, source_id, array_names, diagnostics);
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

fn check_word_for_bare_array(
    word: &Word,
    source_id: SourceId,
    array_names: &[String],
    diagnostics: &mut Vec<Diagnostic>,
) {
    for segment in &word.segments {
        check_segment_for_bare_array(segment, source_id, array_names, diagnostics);
    }
}

fn check_segment_for_bare_array(
    segment: &WordSegment,
    source_id: SourceId,
    array_names: &[String],
    diagnostics: &mut Vec<Diagnostic>,
) {
    match segment {
        WordSegment::ParamExpand(pe) => {
            // Bare $arr where arr is a declared array
            if pe.operator.is_none() && array_names.contains(&pe.name) {
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
                check_segment_for_bare_array(seg, source_id, array_names, diagnostics);
            }
        }
        _ => {}
    }
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
}
