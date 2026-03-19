pub use argvtype_core;
pub use argvtype_syntax;

use argvtype_core::check::check;
use argvtype_core::diagnostic::Diagnostic;
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
    fn clean_produces_no_diagnostics() {
        let result = check_fixture("../../fixtures/clean.sh");
        assert!(
            result.diagnostics.is_empty(),
            "expected no diagnostics for clean.sh, got: {:?}",
            result.diagnostics
        );
    }
}
