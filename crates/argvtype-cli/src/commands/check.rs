use argvtype_core::check::check;
use argvtype_core::diagnostic::render_diagnostics;
use argvtype_syntax::lower::parse_and_lower;
use argvtype_syntax::span::{SourceFile, SourceId};

pub fn run(paths: &[String], format: &str, dump_hir: bool) -> i32 {
    if paths.is_empty() {
        eprintln!("error: no files specified");
        return 1;
    }

    let mut has_errors = false;

    for (idx, path) in paths.iter().enumerate() {
        let source_text = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("error: cannot read '{}': {}", path, e);
                has_errors = true;
                continue;
            }
        };

        let source = SourceFile::new(SourceId(idx as u32), path.clone(), source_text);
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
                    println!("--- HIR for {} ---", path);
                    println!(
                        "{}",
                        serde_json::to_string_pretty(&result.source_unit).unwrap()
                    );
                }
            }
        }

        for err in &result.parse_errors {
            eprintln!("{}: parse error: {}", path, err);
            has_errors = true;
        }

        for err in &result.annotation_errors {
            eprintln!("{}: annotation error: {}", path, err);
            has_errors = true;
        }

        for err in &result.lowering_errors {
            eprintln!("{}: lowering warning: {}", path, err);
        }

        let diagnostics = check(&result.source_unit);

        if !diagnostics.is_empty() {
            // Re-read source for rendering (we need it for miette)
            let source_text = std::fs::read_to_string(path).unwrap();
            let render_source =
                SourceFile::new(SourceId(idx as u32), path.clone(), source_text);

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
