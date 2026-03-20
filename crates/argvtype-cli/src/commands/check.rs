use std::path::PathBuf;

use argvtype_core::check::{check, check_with_imports};
use argvtype_core::diagnostic::{render_diagnostics, Fix, Severity};
use argvtype_core::source_graph::SourceGraph;
use argvtype_syntax::lower::parse_and_lower;
use argvtype_syntax::span::{SourceFile, SourceId};
use serde::Serialize;

#[derive(Serialize)]
struct AgentReport {
    pass: bool,
    diagnostics: Vec<AgentDiagnostic>,
    summary: String,
}

#[derive(Clone, Serialize)]
struct AgentDiagnostic {
    code: String,
    severity: String,
    message: String,
    span: AgentSpan,
    fix: Option<Fix>,
    agent_context: Option<String>,
}

#[derive(Clone, Serialize)]
struct AgentSpan {
    start: u32,
    end: u32,
}

pub fn run(
    paths: &[String],
    format: &str,
    dump_hir: bool,
    command: Option<&str>,
    stdin: bool,
    agent: bool,
) -> i32 {
    // For --command and --stdin, use the simple single-file path
    if command.is_some() || stdin {
        return run_single_file(paths, format, dump_hir, command, stdin, agent);
    }

    if paths.is_empty() {
        eprintln!("error: no input specified (use paths, --command, or --stdin)");
        return 1;
    }

    // Build source graph for file paths (resolves cross-file dependencies)
    let entry_paths: Vec<PathBuf> = paths
        .iter()
        .filter_map(|p| std::fs::canonicalize(p).ok())
        .collect();

    if entry_paths.is_empty() {
        for p in paths {
            eprintln!("error: cannot read '{}'", p);
        }
        return 1;
    }

    let graph = SourceGraph::build(&entry_paths);

    let mut has_errors = false;
    let mut all_agent_diagnostics = Vec::new();

    // Emit source graph diagnostics (BT701, BT702)
    for (file_path, diag) in graph.diagnostics() {
        if agent {
            all_agent_diagnostics.push(to_agent_diagnostic(diag));
        } else {
            // Render with the source file from the graph node
            if let Some(node) = graph.node(file_path) {
                let render_source = SourceFile::new(
                    node.source_id,
                    file_path.to_string_lossy().to_string(),
                    node.lower_result.source_text().to_string(),
                );
                match format {
                    "json" => {
                        println!("{}", serde_json::to_string_pretty(&[diag]).unwrap());
                    }
                    _ => {
                        let reports = render_diagnostics(std::slice::from_ref(diag), &render_source);
                        for report in reports {
                            eprintln!("{:?}", report);
                        }
                    }
                }
            }
        }
        if diag.severity == Severity::Error {
            has_errors = true;
        }
    }

    // Check files in topological order (dependencies first)
    for file_path in graph.topo_order() {
        let node = match graph.node(file_path) {
            Some(n) => n,
            None => continue,
        };

        let name = file_path.to_string_lossy().to_string();

        if dump_hir {
            match format {
                "json" => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&node.lower_result.source_unit).unwrap()
                    );
                }
                _ => {
                    println!("--- HIR for {} ---", name);
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&node.lower_result.source_unit).unwrap()
                    );
                }
            }
        }

        for err in &node.lower_result.parse_errors {
            eprintln!("{}: parse error: {}", name, err);
            has_errors = true;
        }

        for err in &node.lower_result.annotation_errors {
            eprintln!("{}: annotation error: {}", name, err);
            has_errors = true;
        }

        for err in &node.lower_result.lowering_errors {
            eprintln!("{}: lowering warning: {}", name, err);
        }

        // Get imported symbols from sourced files
        let imported = graph.imported_symbols(file_path);
        let diagnostics = check_with_imports(
            &node.lower_result.source_unit,
            &imported,
        );

        if !diagnostics.is_empty() {
            if agent {
                for d in &diagnostics {
                    all_agent_diagnostics.push(to_agent_diagnostic(d));
                }
            } else {
                let render_source = SourceFile::new(
                    node.source_id,
                    name.clone(),
                    node.lower_result.source_text().to_string(),
                );

                match format {
                    "json" => {
                        println!("{}", serde_json::to_string_pretty(&diagnostics).unwrap());
                    }
                    _ => {
                        let reports = render_diagnostics(&diagnostics, &render_source);
                        for report in reports {
                            eprintln!("{:?}", report);
                        }
                    }
                }
            }

            if diagnostics
                .iter()
                .any(|d| d.severity == Severity::Error)
            {
                has_errors = true;
            }
        }
    }

    if agent {
        emit_agent_report(&all_agent_diagnostics, has_errors);
    }

    if has_errors { 1 } else { 0 }
}

/// Single-file check path for --command and --stdin inputs.
fn run_single_file(
    paths: &[String],
    format: &str,
    dump_hir: bool,
    command: Option<&str>,
    stdin: bool,
    agent: bool,
) -> i32 {
    let sources = collect_sources(paths, command, stdin);
    if sources.is_empty() {
        eprintln!("error: no input specified (use paths, --command, or --stdin)");
        return 1;
    }

    let mut has_errors = false;
    let mut all_agent_diagnostics = Vec::new();

    for source in sources {
        let name = source.name.clone();
        let result = parse_and_lower(source);

        if dump_hir {
            match format {
                "json" => {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&result.source_unit).unwrap()
                    );
                }
                _ => {
                    println!("--- HIR for {} ---", name);
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&result.source_unit).unwrap()
                    );
                }
            }
        }

        for err in &result.parse_errors {
            eprintln!("{}: parse error: {}", name, err);
            has_errors = true;
        }

        for err in &result.annotation_errors {
            eprintln!("{}: annotation error: {}", name, err);
            has_errors = true;
        }

        for err in &result.lowering_errors {
            eprintln!("{}: lowering warning: {}", name, err);
        }

        let diagnostics = check(&result.source_unit);

        if !diagnostics.is_empty() {
            if agent {
                for d in &diagnostics {
                    all_agent_diagnostics.push(to_agent_diagnostic(d));
                }
            } else {
                let render_source = SourceFile::new(
                    result.source_unit.source_id,
                    name.clone(),
                    result.source_text().to_string(),
                );

                match format {
                    "json" => {
                        println!("{}", serde_json::to_string_pretty(&diagnostics).unwrap());
                    }
                    _ => {
                        let reports = render_diagnostics(&diagnostics, &render_source);
                        for report in reports {
                            eprintln!("{:?}", report);
                        }
                    }
                }
            }

            if diagnostics
                .iter()
                .any(|d| d.severity == Severity::Error)
            {
                has_errors = true;
            }
        }
    }

    if agent {
        emit_agent_report(&all_agent_diagnostics, has_errors);
    }

    if has_errors { 1 } else { 0 }
}

fn to_agent_diagnostic(d: &argvtype_core::diagnostic::Diagnostic) -> AgentDiagnostic {
    AgentDiagnostic {
        code: d.code.to_string(),
        severity: match d.severity {
            Severity::Error => "error".into(),
            Severity::Warning => "warning".into(),
            Severity::Info => "info".into(),
            Severity::Hint => "hint".into(),
            _ => "unknown".into(),
        },
        message: d.message.clone(),
        span: AgentSpan {
            start: d.primary_span.start,
            end: d.primary_span.end,
        },
        fix: d.fix.clone(),
        agent_context: d.agent_context.clone(),
    }
}

fn emit_agent_report(all_agent_diagnostics: &[AgentDiagnostic], has_errors: bool) {
    let error_count = all_agent_diagnostics.iter().filter(|d| d.severity == "error").count();
    let warning_count = all_agent_diagnostics.iter().filter(|d| d.severity == "warning").count();
    let pass = !has_errors;
    let summary = if pass && all_agent_diagnostics.is_empty() {
        "No issues found.".into()
    } else {
        format!("{} error(s), {} warning(s)", error_count, warning_count)
    };
    let report = AgentReport {
        pass,
        diagnostics: all_agent_diagnostics.to_vec(),
        summary,
    };
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
}

fn collect_sources(
    paths: &[String],
    command: Option<&str>,
    stdin: bool,
) -> Vec<SourceFile> {
    let mut sources = Vec::new();
    let mut next_id = 0u32;

    if let Some(cmd) = command {
        sources.push(SourceFile::new(
            SourceId(next_id),
            "<command>".to_string(),
            cmd.to_string(),
        ));
        next_id += 1;
    }

    if stdin {
        let mut input = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)
            .expect("failed to read from stdin");
        sources.push(SourceFile::new(
            SourceId(next_id),
            "<stdin>".to_string(),
            input,
        ));
        next_id += 1;
    }

    for path in paths {
        match std::fs::read_to_string(path) {
            Ok(s) => {
                sources.push(SourceFile::new(
                    SourceId(next_id),
                    path.clone(),
                    s,
                ));
                next_id += 1;
            }
            Err(e) => {
                eprintln!("error: cannot read '{}': {}", path, e);
            }
        }
    }

    sources
}
