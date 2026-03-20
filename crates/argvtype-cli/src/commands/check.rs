use argvtype_core::check::check;
use argvtype_core::diagnostic::render_diagnostics;
use argvtype_syntax::lower::parse_and_lower;
use argvtype_syntax::span::{SourceFile, SourceId};

pub fn run(
    paths: &[String],
    format: &str,
    dump_hir: bool,
    command: Option<&str>,
    stdin: bool,
) -> i32 {
    let sources = collect_sources(paths, command, stdin);
    if sources.is_empty() {
        eprintln!("error: no input specified (use paths, --command, or --stdin)");
        return 1;
    }

    let mut has_errors = false;

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

            if diagnostics
                .iter()
                .any(|d| d.severity == argvtype_core::diagnostic::Severity::Error)
            {
                has_errors = true;
            }
        }
    }

    if has_errors { 1 } else { 0 }
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
