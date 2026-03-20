pub use argvtype_core;
pub use argvtype_syntax;

use argvtype_core::check::check;
use argvtype_core::diagnostic::Diagnostic;
use argvtype_core::source_graph::SourceGraph;
use argvtype_syntax::lower::{parse_and_lower, LowerResult};
use argvtype_syntax::span::{SourceFile, SourceId};

pub struct FullResult {
    pub lower: LowerResult,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn check_fixture(path: &str) -> FullResult {
    let source_text = std::fs::read_to_string(path).unwrap_or_else(|e| {
        panic!("cannot read fixture '{}': {}", path, e);
    });
    let source = SourceFile::new(SourceId(0), path.to_string(), source_text);
    let lower = parse_and_lower(source);
    let diagnostics = check(&lower.source_unit);
    FullResult { lower, diagnostics }
}

/// Build a source graph from a fixture path and return all diagnostics
/// (both graph-level BT701/BT702 and per-file check diagnostics).
pub fn check_fixture_graph(path: &str) -> Vec<Diagnostic> {
    let abs = std::fs::canonicalize(path).unwrap_or_else(|e| {
        panic!("cannot canonicalize fixture '{}': {}", path, e);
    });
    let graph = SourceGraph::build(std::slice::from_ref(&abs));

    let mut all_diagnostics = Vec::new();

    // Graph-level diagnostics (BT701, BT702)
    for (_, diag) in graph.diagnostics() {
        all_diagnostics.push(diag.clone());
    }

    // Per-file check diagnostics in topo order
    for file_path in graph.topo_order() {
        if let Some(node) = graph.node(file_path) {
            let imported = graph.imported_symbols(file_path);
            let diagnostics = argvtype_core::check::check_with_imports(
                &node.lower_result.source_unit,
                &imported,
            );
            all_diagnostics.extend(diagnostics);
        }
    }

    all_diagnostics
}

#[cfg(test)]
mod tests {
    use super::*;
    use argvtype_syntax::hir::Item;

    #[test]
    fn basic_parses_without_errors() {
        let result = check_fixture("../../fixtures/basic.sh");
        assert!(
            result.lower.parse_errors.is_empty(),
            "parse errors: {:?}",
            result.lower.parse_errors
        );
        assert!(
            result.lower.annotation_errors.is_empty(),
            "annotation errors: {:?}",
            result.lower.annotation_errors
        );
    }

    #[test]
    fn annotated_has_annotations_on_function() {
        let result = check_fixture("../../fixtures/annotated.sh");
        assert!(result.lower.parse_errors.is_empty());
        assert!(result.lower.annotation_errors.is_empty());

        let has_annotated_fn = result.lower.source_unit.items.iter().any(|item| {
            matches!(item, Item::Function(f) if !f.annotations.is_empty())
        });
        assert!(has_annotated_fn, "expected function with annotations");
    }

    #[test]
    fn array_misuse_produces_diagnostic() {
        let result = check_fixture("../../fixtures/array_misuse.sh");
        assert!(
            !result.diagnostics.is_empty(),
            "expected diagnostics for array misuse"
        );
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.code.number == 201),
            "expected BT201 diagnostic"
        );
    }

    #[test]
    fn destructive_unquoted_produces_bt801() {
        let result = check_fixture("../../fixtures/destructive_unquoted.sh");
        assert!(
            result.diagnostics.iter().any(|d| d.code.number == 801),
            "expected BT801 diagnostic"
        );
    }

    #[test]
    fn cd_without_guard_produces_bt802() {
        let result = check_fixture("../../fixtures/cd_without_guard.sh");
        assert!(
            result.diagnostics.iter().any(|d| d.code.number == 802),
            "expected BT802 diagnostic"
        );
    }

    #[test]
    fn unquoted_expansion_produces_diagnostic() {
        let result = check_fixture("../../fixtures/unquoted_expansion.sh");
        assert!(
            result
                .diagnostics
                .iter()
                .any(|d| d.code.number == 202),
            "expected BT202 diagnostic"
        );
    }

    #[test]
    fn clean_produces_no_diagnostics() {
        let result = check_fixture("../../fixtures/clean.sh");
        assert!(
            result.diagnostics.is_empty(),
            "expected no diagnostics for clean.sh, got: {:?}",
            result.diagnostics
        );
    }

    #[test]
    fn presence_unset_produces_bt302() {
        let result = check_fixture("../../fixtures/presence_unset.sh");
        assert!(
            result.diagnostics.iter().any(|d| d.code.number == 302),
            "expected BT302 diagnostic for unset variable use"
        );
    }

    #[test]
    fn presence_guarded_no_bt302() {
        let result = check_fixture("../../fixtures/presence_guarded.sh");
        let bt302s: Vec<_> = result.diagnostics.iter().filter(|d| d.code.number == 302).collect();
        assert!(
            bt302s.is_empty(),
            "expected no BT302 for guarded variables, got: {:?}",
            bt302s.iter().map(|d| &d.message).collect::<Vec<_>>()
        );
    }

    #[test]
    fn presence_undeclared_produces_bt301() {
        let result = check_fixture("../../fixtures/presence_undeclared.sh");
        assert!(
            result.diagnostics.iter().any(|d| d.code.number == 301),
            "expected BT301 diagnostic for undeclared variable"
        );
    }

    // Source graph integration tests

    #[test]
    fn source_graph_resolves_imported_symbols() {
        // main.sh sources lib.sh — LIB_VERSION should be resolved, no BT301
        let diagnostics = check_fixture_graph("../../fixtures/source_graph/main.sh");
        let bt301_lib_version = diagnostics
            .iter()
            .any(|d| d.code.number == 301 && d.message.contains("LIB_VERSION"));
        assert!(
            !bt301_lib_version,
            "LIB_VERSION should be imported from lib.sh, not BT301. Got: {:?}",
            diagnostics.iter().map(|d| (&d.code, &d.message)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn source_graph_missing_source_bt701() {
        let diagnostics = check_fixture_graph("../../fixtures/source_graph/missing_source.sh");
        assert!(
            diagnostics.iter().any(|d| d.code.number == 701),
            "expected BT701 for missing source target"
        );
    }

    #[test]
    fn source_graph_cycle_bt702() {
        let diagnostics = check_fixture_graph("../../fixtures/source_graph/cycle_a.sh");
        assert!(
            diagnostics.iter().any(|d| d.code.number == 702),
            "expected BT702 for circular source dependency"
        );
    }

    #[test]
    fn source_graph_dynamic_source_no_bt701() {
        // Dynamic source paths should not produce BT701
        let diagnostics = check_fixture_graph("../../fixtures/source_graph/dynamic_source.sh");
        let bt701 = diagnostics.iter().any(|d| d.code.number == 701);
        assert!(
            !bt701,
            "dynamic source should not produce BT701"
        );
    }

    #[test]
    fn source_graph_dot_syntax_resolves() {
        // `. lib.sh` should resolve the same as `source lib.sh`
        let diagnostics = check_fixture_graph("../../fixtures/source_graph/dot_source.sh");
        let bt301_lib_name = diagnostics
            .iter()
            .any(|d| d.code.number == 301 && d.message.contains("LIB_NAME"));
        assert!(
            !bt301_lib_name,
            "LIB_NAME should be imported via . lib.sh"
        );
    }
}
