use thiserror::Error;
use crate::span::{SourceFile, Span};

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ParseError {
    #[error("tree-sitter parse failed")]
    TreeSitterFailed,
    #[error("syntax error at {span:?}")]
    SyntaxError { span: Span },
}

pub struct ParseSession {
    parser: tree_sitter::Parser,
}

impl ParseSession {
    pub fn new() -> Self {
        let mut parser = tree_sitter::Parser::new();
        parser
            .set_language(&tree_sitter_bash::LANGUAGE.into())
            .expect("failed to load tree-sitter-bash grammar");
        Self { parser }
    }

    pub fn parse(&mut self, source: SourceFile) -> Result<ParsedSource, ParseError> {
        let tree = self
            .parser
            .parse(&source.source, None)
            .ok_or(ParseError::TreeSitterFailed)?;
        Ok(ParsedSource { tree, source })
    }
}

impl Default for ParseSession {
    fn default() -> Self {
        Self::new()
    }
}

pub struct ParsedSource {
    pub tree: tree_sitter::Tree,
    pub source: SourceFile,
}

impl ParsedSource {
    pub fn root_node(&self) -> tree_sitter::Node<'_> {
        self.tree.root_node()
    }

    pub fn has_errors(&self) -> bool {
        self.tree.root_node().has_error()
    }

    pub fn source_text(&self) -> &str {
        &self.source.source
    }

    pub fn collect_errors(&self) -> Vec<ParseError> {
        let mut errors = Vec::new();
        collect_error_nodes(self.tree.root_node(), &mut errors);
        errors
    }
}

fn collect_error_nodes(node: tree_sitter::Node<'_>, errors: &mut Vec<ParseError>) {
    if node.is_error() || node.is_missing() {
        errors.push(ParseError::SyntaxError {
            span: Span::new(node.start_byte() as u32, node.end_byte() as u32),
        });
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_error_nodes(child, errors);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::SourceId;

    fn parse(src: &str) -> ParsedSource {
        let source = SourceFile::new(SourceId(0), "test.sh".into(), src.into());
        let mut session = ParseSession::new();
        session.parse(source).unwrap()
    }

    #[test]
    fn parse_echo_hello() {
        let parsed = parse("echo hello");
        assert_eq!(parsed.root_node().kind(), "program");
        assert!(!parsed.has_errors());
    }

    #[test]
    fn parse_function_def() {
        let parsed = parse("greet() { echo hi; }");
        assert!(!parsed.has_errors());
        let root = parsed.root_node();
        let func = root.child(0).unwrap();
        assert_eq!(func.kind(), "function_definition");
    }

    #[test]
    fn parse_invalid_syntax() {
        let parsed = parse("if then fi ((( )))");
        assert!(parsed.has_errors());
    }
}
