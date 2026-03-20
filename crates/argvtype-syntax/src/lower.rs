use crate::annotation::{self, Annotation, AnnotationError};
use crate::hir::*;
use crate::parse::{ParseError, ParseSession, ParsedSource};
use crate::span::{SourceFile, Span};
use thiserror::Error;

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum LoweringError {
    #[error("unsupported syntax '{kind}' at {span:?}")]
    UnsupportedSyntax { kind: String, span: Span },
}

pub struct LowerResult {
    pub source_unit: SourceUnit,
    pub parse_errors: Vec<ParseError>,
    pub annotation_errors: Vec<AnnotationError>,
    pub lowering_errors: Vec<LoweringError>,
    source_text: String,
}

impl LowerResult {
    pub fn source_text(&self) -> &str {
        &self.source_text
    }
}

pub fn parse_and_lower(source: SourceFile) -> LowerResult {
    let (annotations, annotation_errors) = annotation::parse_annotations(&source);
    let saved_source_text = source.source.clone();

    let mut session = ParseSession::new();
    let parsed = match session.parse(source) {
        Ok(p) => p,
        Err(e) => {
            return LowerResult {
                source_unit: SourceUnit {
                    source_id: crate::span::SourceId(0),
                    annotations,
                    items: Vec::new(),
                },
                parse_errors: vec![e],
                annotation_errors,
                lowering_errors: Vec::new(),
                source_text: saved_source_text,
            };
        }
    };

    let parse_errors = parsed.collect_errors();
    let mut ctx = LoweringContext::new(&parsed, annotations);
    let items = ctx.lower_program();

    LowerResult {
        source_unit: SourceUnit {
            source_id: parsed.source.id,
            annotations: ctx.unattached_annotations,
            items,
        },
        parse_errors,
        annotation_errors,
        lowering_errors: ctx.errors,
        source_text: saved_source_text,
    }
}

fn line_of_byte(src: &str, byte: usize) -> usize {
    src[..byte].chars().filter(|&c| c == '\n').count()
}

struct LoweringContext<'a> {
    source: &'a ParsedSource,
    next_id: u32,
    errors: Vec<LoweringError>,
    annotations: Vec<Annotation>,
    unattached_annotations: Vec<Annotation>,
}

impl<'a> LoweringContext<'a> {
    fn new(source: &'a ParsedSource, annotations: Vec<Annotation>) -> Self {
        Self {
            source,
            next_id: 0,
            errors: Vec::new(),
            annotations,
            unattached_annotations: Vec::new(),
        }
    }

    fn alloc_id(&mut self) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        id
    }

    fn node_span(&self, node: &tree_sitter::Node) -> Span {
        Span::new(node.start_byte() as u32, node.end_byte() as u32)
    }

    fn node_text(&self, node: &tree_sitter::Node) -> &str {
        &self.source.source_text()[node.start_byte()..node.end_byte()]
    }

    fn lower_program(&mut self) -> Vec<Item> {
        let root = self.source.root_node();
        let mut items = Vec::new();
        let mut cursor = root.walk();
        for child in root.children(&mut cursor) {
            if let Some(item) = self.lower_top_level(&child) {
                items.push(item);
            }
        }
        // Any remaining annotations that weren't attached
        let remaining = std::mem::take(&mut self.annotations);
        self.unattached_annotations.extend(remaining);
        items
    }

    fn lower_top_level(&mut self, node: &tree_sitter::Node) -> Option<Item> {
        match node.kind() {
            "function_definition" => {
                let func = self.lower_function(node);
                Some(Item::Function(func))
            }
            "\n" | ";" | "comment" => None,
            _ => {
                let stmt = self.lower_statement(node);
                Some(Item::Statement(stmt))
            }
        }
    }

    fn collect_preceding_annotations(&mut self, node: &tree_sitter::Node) -> Vec<Annotation> {
        let src = self.source.source_text();
        let node_start_line = line_of_byte(src, node.start_byte());
        let mut attached = Vec::new();
        let mut remaining = Vec::new();

        for ann in self.annotations.drain(..) {
            let ann_line = line_of_byte(src, ann.span.start as usize);
            if ann_line < node_start_line {
                attached.push(ann);
            } else {
                remaining.push(ann);
            }
        }
        self.annotations = remaining;
        attached
    }

    fn lower_function(&mut self, node: &tree_sitter::Node) -> Function {
        let id = self.alloc_id();
        let span = self.node_span(node);
        let annotations = self.collect_preceding_annotations(node);

        let name = node
            .child_by_field_name("name")
            .map(|n| self.node_text(&n).to_string())
            .or_else(|| {
                // tree-sitter-bash: function name is the first `word` child
                let mut cursor = node.walk();
                node.children(&mut cursor)
                    .find(|c| c.kind() == "word")
                    .map(|n| self.node_text(&n).to_string())
            })
            .unwrap_or_default();

        let mut body = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "compound_statement" {
                body = self.lower_compound_statement(&child);
            }
        }

        // Collect any annotations inside the function body
        let inner_annotations = self.collect_inner_annotations(node);
        let mut all_annotations = annotations;
        all_annotations.extend(inner_annotations);

        Function {
            id,
            span,
            name,
            body,
            annotations: all_annotations,
        }
    }

    fn collect_inner_annotations(&mut self, node: &tree_sitter::Node) -> Vec<Annotation> {
        let node_end = node.end_byte() as u32;
        let mut inner = Vec::new();
        let mut remaining = Vec::new();

        for ann in self.annotations.drain(..) {
            if ann.span.start < node_end {
                inner.push(ann);
            } else {
                remaining.push(ann);
            }
        }
        self.annotations = remaining;
        inner
    }

    fn lower_compound_statement(&mut self, node: &tree_sitter::Node) -> Vec<Statement> {
        let mut stmts = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "{" | "}" | ";" | "\n" | "comment" => {}
                _ => stmts.push(self.lower_statement(&child)),
            }
        }
        stmts
    }

    fn lower_statement(&mut self, node: &tree_sitter::Node) -> Statement {
        match node.kind() {
            "command" => self.lower_command(node),
            "variable_assignment" => self.lower_variable_assignment(node),
            "declaration_command" => self.lower_declaration(node),
            "pipeline" => self.lower_pipeline(node),
            "if_statement" => self.lower_if(node),
            "for_statement" => self.lower_for(node),
            "while_statement" => self.lower_while(node),
            "case_statement" => self.lower_case(node),
            "compound_statement" => {
                let id = self.alloc_id();
                let span = self.node_span(node);
                let body = self.lower_compound_statement(node);
                Statement::Block(Block { id, span, body, subshell: false })
            }
            "list" => self.lower_list(node),
            "subshell" => {
                let id = self.alloc_id();
                let span = self.node_span(node);
                let mut body = Vec::new();
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    match child.kind() {
                        "(" | ")" | ";" | "\n" => {}
                        _ => body.push(self.lower_statement(&child)),
                    }
                }
                Statement::Block(Block { id, span, body, subshell: true })
            }
            "redirected_statement" => self.lower_redirected_statement(node),
            "negated_command" => self.lower_negated_command(node),
            "test_command" => self.lower_test_command(node),
            "unset_command" => self.lower_unset_command(node),
            kind => {
                let id = self.alloc_id();
                let span = self.node_span(node);
                let text = self.node_text(node).to_string();
                self.errors.push(LoweringError::UnsupportedSyntax {
                    kind: kind.to_string(),
                    span,
                });
                Statement::Unmodeled(Unmodeled {
                    id,
                    span,
                    kind: kind.to_string(),
                    text,
                })
            }
        }
    }

    fn lower_command(&mut self, node: &tree_sitter::Node) -> Statement {
        let id = self.alloc_id();
        let span = self.node_span(node);
        let mut name = None;
        let mut args = Vec::new();
        let mut redirects = Vec::new();

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "command_name" => {
                    // command_name has a child that is the actual word/string
                    let inner = child.child(0).unwrap_or(child);
                    name = Some(self.lower_word(&inner));
                }
                "file_redirect" | "heredoc_redirect" => {
                    redirects.push(self.lower_redirect(&child));
                }
                _ => {
                    if name.is_some() {
                        args.push(self.lower_word(&child));
                    }
                }
            }
        }

        let name = name.unwrap_or(Word {
            span,
            segments: vec![WordSegment::Literal(self.node_text(node).to_string())],
        });

        Statement::Command(Command {
            id,
            span,
            name,
            args,
            redirects,
        })
    }

    fn lower_redirect(&mut self, node: &tree_sitter::Node) -> Redirect {
        let span = self.node_span(node);
        let mut fd = None;
        let mut op = String::new();
        let mut target = Word {
            span,
            segments: Vec::new(),
        };

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "file_descriptor" => fd = Some(self.node_text(&child).to_string()),
                ">" | ">>" | "<" | "<<" | "&>" | "&>>" | "2>" | "2>>" => {
                    op = self.node_text(&child).to_string();
                }
                _ => target = self.lower_word(&child),
            }
        }

        if op.is_empty() {
            op = self.node_text(node).to_string();
        }

        Redirect { span, fd, op, target }
    }

    fn lower_variable_assignment(&mut self, node: &tree_sitter::Node) -> Statement {
        let id = self.alloc_id();
        let span = self.node_span(node);
        let mut name = String::new();
        let mut value = None;
        let mut array_value = None;

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "variable_name" => name = self.node_text(&child).to_string(),
                "=" => {}
                "array" => {
                    array_value = Some(self.lower_array(&child));
                }
                _ => {
                    value = Some(self.lower_word(&child));
                }
            }
        }

        Statement::Assignment(Assignment {
            id,
            span,
            name,
            value,
            decl_kind: None,
            flags: Vec::new(),
            array_value,
        })
    }

    fn lower_declaration(&mut self, node: &tree_sitter::Node) -> Statement {
        let id = self.alloc_id();
        let span = self.node_span(node);
        let mut decl_kind = None;
        let mut flags = Vec::new();
        let mut name = String::new();
        let mut value = None;
        let mut array_value = None;

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "local" => decl_kind = Some(DeclKind::Local),
                "declare" => decl_kind = Some(DeclKind::Declare),
                "export" => decl_kind = Some(DeclKind::Export),
                "readonly" => decl_kind = Some(DeclKind::Readonly),
                "word" => {
                    let text = self.node_text(&child).to_string();
                    if text.starts_with('-') {
                        flags.push(text);
                    } else if name.is_empty() {
                        name = text;
                    }
                }
                "variable_name" => {
                    name = self.node_text(&child).to_string();
                }
                "variable_assignment" => {
                    let mut inner_cursor = child.walk();
                    for inner_child in child.children(&mut inner_cursor) {
                        match inner_child.kind() {
                            "variable_name" => name = self.node_text(&inner_child).to_string(),
                            "=" => {}
                            "array" => {
                                array_value = Some(self.lower_array(&inner_child));
                            }
                            _ => {
                                value = Some(self.lower_word(&inner_child));
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        Statement::Assignment(Assignment {
            id,
            span,
            name,
            value,
            decl_kind,
            flags,
            array_value,
        })
    }

    fn lower_array(&mut self, node: &tree_sitter::Node) -> Vec<Word> {
        let mut elements = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "(" | ")" => {}
                _ => elements.push(self.lower_word(&child)),
            }
        }
        elements
    }

    fn lower_pipeline(&mut self, node: &tree_sitter::Node) -> Statement {
        let id = self.alloc_id();
        let span = self.node_span(node);
        let mut commands = Vec::new();

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "|" | "|&" => {}
                _ => commands.push(self.lower_statement(&child)),
            }
        }

        Statement::Pipeline(Pipeline {
            id,
            span,
            commands,
            negated: false,
        })
    }

    fn lower_if(&mut self, node: &tree_sitter::Node) -> Statement {
        let id = self.alloc_id();
        let span = self.node_span(node);
        let mut condition = Vec::new();
        let mut then_body = Vec::new();
        let mut else_body = None;

        let mut in_condition = false;
        let mut in_then = false;

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "if" => in_condition = true,
                "then" => {
                    in_condition = false;
                    in_then = true;
                }
                "else_clause" => {
                    in_then = false;
                    else_body = Some(self.lower_else_clause(&child));
                }
                "elif_clause" => {
                    in_then = false;
                    else_body = Some(vec![self.lower_elif_clause(&child)]);
                }
                "fi" | ";" | "\n" => {}
                _ => {
                    if in_condition {
                        condition.push(self.lower_statement(&child));
                    } else if in_then {
                        then_body.push(self.lower_statement(&child));
                    }
                }
            }
        }

        Statement::If(IfStatement {
            id,
            span,
            condition,
            then_body,
            else_body,
        })
    }

    fn lower_else_clause(&mut self, node: &tree_sitter::Node) -> Vec<Statement> {
        let mut body = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "else" | ";" | "\n" => {}
                _ => body.push(self.lower_statement(&child)),
            }
        }
        body
    }

    fn lower_elif_clause(&mut self, node: &tree_sitter::Node) -> Statement {
        let id = self.alloc_id();
        let span = self.node_span(node);
        let mut condition = Vec::new();
        let mut then_body = Vec::new();
        let mut else_body = None;
        let mut in_condition = false;
        let mut in_then = false;

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "elif" => in_condition = true,
                "then" => {
                    in_condition = false;
                    in_then = true;
                }
                "else_clause" => {
                    in_then = false;
                    else_body = Some(self.lower_else_clause(&child));
                }
                "elif_clause" => {
                    in_then = false;
                    else_body = Some(vec![self.lower_elif_clause(&child)]);
                }
                ";" | "\n" => {}
                _ => {
                    if in_condition {
                        condition.push(self.lower_statement(&child));
                    } else if in_then {
                        then_body.push(self.lower_statement(&child));
                    }
                }
            }
        }

        Statement::If(IfStatement {
            id,
            span,
            condition,
            then_body,
            else_body,
        })
    }

    fn lower_for(&mut self, node: &tree_sitter::Node) -> Statement {
        let id = self.alloc_id();
        let span = self.node_span(node);
        let mut variable = String::new();
        let mut items = Vec::new();
        let mut body = Vec::new();

        let mut in_items = false;

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "for" => {}
                "variable_name" => variable = self.node_text(&child).to_string(),
                "in" => in_items = true,
                "do_group" => {
                    in_items = false;
                    body = self.lower_do_group(&child);
                }
                ";" | "\n" => {}
                _ => {
                    if in_items {
                        items.push(self.lower_word(&child));
                    }
                }
            }
        }

        Statement::For(ForLoop {
            id,
            span,
            variable,
            items,
            body,
        })
    }

    fn lower_while(&mut self, node: &tree_sitter::Node) -> Statement {
        let id = self.alloc_id();
        let span = self.node_span(node);
        let mut condition = Vec::new();
        let mut body = Vec::new();

        let mut in_condition = false;

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "while" | "until" => in_condition = true,
                "do_group" => {
                    in_condition = false;
                    body = self.lower_do_group(&child);
                }
                ";" | "\n" => {}
                _ => {
                    if in_condition {
                        condition.push(self.lower_statement(&child));
                    }
                }
            }
        }

        Statement::While(WhileLoop {
            id,
            span,
            condition,
            body,
        })
    }

    fn lower_case(&mut self, node: &tree_sitter::Node) -> Statement {
        let id = self.alloc_id();
        let span = self.node_span(node);
        let mut subject = Word {
            span,
            segments: Vec::new(),
        };
        let mut arms = Vec::new();

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "case" | "in" | "esac" | ";" | "\n" => {}
                "case_item" => arms.push(self.lower_case_arm(&child)),
                _ => {
                    if subject.segments.is_empty() {
                        subject = self.lower_word(&child);
                    }
                }
            }
        }

        Statement::Case(CaseStatement {
            id,
            span,
            subject,
            arms,
        })
    }

    fn lower_case_arm(&mut self, node: &tree_sitter::Node) -> CaseArm {
        let span = self.node_span(node);
        let mut patterns = Vec::new();
        let mut body = Vec::new();
        let mut past_paren = false;

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                ")" => past_paren = true,
                ";;" | ";&" | ";;&" | "(" => {}
                _ => {
                    if past_paren {
                        body.push(self.lower_statement(&child));
                    } else {
                        patterns.push(self.lower_word(&child));
                    }
                }
            }
        }

        CaseArm {
            span,
            patterns,
            body,
        }
    }

    fn lower_do_group(&mut self, node: &tree_sitter::Node) -> Vec<Statement> {
        let mut stmts = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "do" | "done" | ";" | "\n" => {}
                _ => stmts.push(self.lower_statement(&child)),
            }
        }
        stmts
    }

    fn lower_list(&mut self, node: &tree_sitter::Node) -> Statement {
        let id = self.alloc_id();
        let span = self.node_span(node);
        let mut elements: Vec<ListElement> = Vec::new();
        let mut pending_op: Option<ListOperator> = None;

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "&&" => pending_op = Some(ListOperator::And),
                "||" => pending_op = Some(ListOperator::Or),
                ";" => pending_op = Some(ListOperator::Semi),
                "\n" => {}
                _ => {
                    // Attach pending operator to previous element
                    if let Some(op) = pending_op.take()
                        && let Some(last) = elements.last_mut()
                    {
                        last.operator = Some(op);
                    }
                    elements.push(ListElement {
                        statement: self.lower_statement(&child),
                        operator: None,
                    });
                }
            }
        }

        Statement::List(List { id, span, elements })
    }

    fn lower_redirected_statement(&mut self, node: &tree_sitter::Node) -> Statement {
        // Lower the inner statement (the redirects will be attached if it's a command)
        let mut cursor = node.walk();
        let mut inner_stmt = None;
        let mut redirects = Vec::new();

        for child in node.children(&mut cursor) {
            match child.kind() {
                "file_redirect" | "heredoc_redirect" => {
                    redirects.push(self.lower_redirect(&child));
                }
                _ => {
                    inner_stmt = Some(self.lower_statement(&child));
                }
            }
        }

        let mut stmt = inner_stmt.unwrap_or_else(|| {
            let id = self.alloc_id();
            let span = self.node_span(node);
            Statement::Unmodeled(Unmodeled {
                id,
                span,
                kind: "empty_redirected".into(),
                text: self.node_text(node).to_string(),
            })
        });

        // Attach redirects to inner command if possible
        if let Statement::Command(ref mut cmd) = stmt {
            cmd.redirects.extend(redirects);
        }

        stmt
    }

    fn lower_negated_command(&mut self, node: &tree_sitter::Node) -> Statement {
        let id = self.alloc_id();
        let span = self.node_span(node);
        let mut commands = Vec::new();

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "!" => {}
                _ => commands.push(self.lower_statement(&child)),
            }
        }

        Statement::Pipeline(Pipeline {
            id,
            span,
            commands,
            negated: true,
        })
    }

    fn lower_test_command(&mut self, node: &tree_sitter::Node) -> Statement {
        // [[ ... ]] or [ ... ] — lower as a command for M0
        let id = self.alloc_id();
        let span = self.node_span(node);
        let name_word = Word {
            span,
            segments: vec![WordSegment::Literal("[[".to_string())],
        };

        // Collect args from test expressions
        let mut args = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "[[" | "]]" | "[" | "]" => {}
                _ => self.collect_test_args(&child, &mut args),
            }
        }

        Statement::Command(Command {
            id,
            span,
            name: name_word,
            args,
            redirects: Vec::new(),
        })
    }

    fn lower_unset_command(&mut self, node: &tree_sitter::Node) -> Statement {
        // `unset [-fv] var [var...]` — lower as a Command with name "unset"
        let id = self.alloc_id();
        let span = self.node_span(node);
        let mut args = Vec::new();

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "unset" => {}
                _ => args.push(self.lower_word(&child)),
            }
        }

        Statement::Command(Command {
            id,
            span,
            name: Word {
                span,
                segments: vec![WordSegment::Literal("unset".to_string())],
            },
            args,
            redirects: Vec::new(),
        })
    }

    fn collect_test_args(&mut self, node: &tree_sitter::Node, args: &mut Vec<Word>) {
        match node.kind() {
            "unary_expression" => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    match child.kind() {
                        "test_operator" => {
                            args.push(Word {
                                span: self.node_span(&child),
                                segments: vec![WordSegment::Literal(
                                    self.node_text(&child).to_string(),
                                )],
                            });
                        }
                        _ => args.push(self.lower_word(&child)),
                    }
                }
            }
            "binary_expression" => {
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    match child.kind() {
                        "test_operator" | "==" | "!=" | "=~" | "<" | ">" => {
                            args.push(Word {
                                span: self.node_span(&child),
                                segments: vec![WordSegment::Literal(
                                    self.node_text(&child).to_string(),
                                )],
                            });
                        }
                        _ => args.push(self.lower_word(&child)),
                    }
                }
            }
            _ => args.push(self.lower_word(node)),
        }
    }

    fn lower_word(&mut self, node: &tree_sitter::Node) -> Word {
        let span = self.node_span(node);
        let segments = self.lower_word_segments(node);
        Word { span, segments }
    }

    fn lower_word_segments(&mut self, node: &tree_sitter::Node) -> Vec<WordSegment> {
        match node.kind() {
            "word" | "number" => {
                vec![WordSegment::Literal(self.node_text(node).to_string())]
            }
            "raw_string" => {
                let text = self.node_text(node);
                // Strip surrounding single quotes
                let inner = text
                    .strip_prefix('\'')
                    .and_then(|s| s.strip_suffix('\''))
                    .unwrap_or(text);
                vec![WordSegment::SingleQuoted(inner.to_string())]
            }
            "string" | "translated_string" => {
                // Double-quoted string
                let mut segments = Vec::new();
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    match child.kind() {
                        "\"" | "$\"" => {}
                        "string_content" => {
                            segments.push(WordSegment::Literal(
                                self.node_text(&child).to_string(),
                            ));
                        }
                        _ => {
                            let inner = self.lower_word_segments(&child);
                            segments.extend(inner);
                        }
                    }
                }
                vec![WordSegment::DoubleQuoted(segments)]
            }
            "simple_expansion" => {
                // $x or $1 etc.
                let name = self.extract_expansion_name(node);
                vec![WordSegment::ParamExpand(ParamExpand {
                    span: self.node_span(node),
                    name,
                    operator: None,
                    operand: None,
                })]
            }
            "expansion" => {
                // ${...}
                self.lower_expansion(node)
            }
            "command_substitution" => {
                let span = self.node_span(node);
                let mut body = Vec::new();
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    match child.kind() {
                        "$(" | ")" | "`" => {}
                        _ => body.push(self.lower_statement(&child)),
                    }
                }
                vec![WordSegment::CommandSub(CommandSub { span, body })]
            }
            "arithmetic_expansion" => {
                let span = self.node_span(node);
                let text = self.node_text(node);
                let expr = text
                    .strip_prefix("$((")
                    .and_then(|s| s.strip_suffix("))"))
                    .unwrap_or(text)
                    .to_string();
                vec![WordSegment::ArithExpand(ArithExpand {
                    span,
                    expression: expr,
                })]
            }
            "concatenation" => {
                let mut segments = Vec::new();
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    segments.extend(self.lower_word_segments(&child));
                }
                segments
            }
            _ => {
                // Fallback: treat as literal
                vec![WordSegment::Literal(self.node_text(node).to_string())]
            }
        }
    }

    fn extract_expansion_name(&self, node: &tree_sitter::Node) -> String {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            match child.kind() {
                "variable_name" | "special_variable_name" => {
                    return self.node_text(&child).to_string();
                }
                "$" | "${" | "}" => {}
                _ => {}
            }
        }
        // Fallback: text minus the $
        let text = self.node_text(node);
        text.strip_prefix('$')
            .unwrap_or(text)
            .trim_matches(|c| c == '{' || c == '}')
            .to_string()
    }

    fn lower_expansion(&mut self, node: &tree_sitter::Node) -> Vec<WordSegment> {
        // ${...} can be param expansion or array subscript
        let mut cursor = node.walk();
        let mut has_subscript = false;

        for child in node.children(&mut cursor) {
            if child.kind() == "subscript" {
                has_subscript = true;
                break;
            }
        }

        if has_subscript {
            return self.lower_array_expansion(node);
        }

        let span = self.node_span(node);
        let mut name = String::new();
        let mut operator = None;
        let mut operand = None;

        let mut cursor = node.walk();
        let children: Vec<_> = node.children(&mut cursor).collect();

        for child in &children {
            match child.kind() {
                "${" | "}" | "$" => {}
                "variable_name" | "special_variable_name" => {
                    name = self.node_text(child).to_string();
                }
                "#" => {
                    // Could be ${#var} (length)
                    if name.is_empty() {
                        operator = Some(ParamOperator::Length);
                    }
                }
                "!" => {
                    if name.is_empty() {
                        operator = Some(ParamOperator::Indirect);
                    }
                }
                ":-" => operator = Some(ParamOperator::Default),
                ":=" => operator = Some(ParamOperator::Assign),
                ":?" => operator = Some(ParamOperator::Error),
                ":+" => operator = Some(ParamOperator::Alternate),
                "-" => {
                    if operator.is_none() {
                        operator = Some(ParamOperator::Default);
                    }
                }
                "=" => {
                    if operator.is_none() {
                        operator = Some(ParamOperator::Assign);
                    }
                }
                "?" => {
                    if operator.is_none() {
                        operator = Some(ParamOperator::Error);
                    }
                }
                "+" => {
                    if operator.is_none() {
                        operator = Some(ParamOperator::Alternate);
                    }
                }
                _ => {
                    if operator.is_some() && operand.is_none() {
                        operand = Some(Box::new(self.lower_word(child)));
                    } else if name.is_empty() {
                        name = self.node_text(child).to_string();
                    }
                }
            }
        }

        vec![WordSegment::ParamExpand(ParamExpand {
            span,
            name,
            operator,
            operand,
        })]
    }

    fn lower_array_expansion(&mut self, node: &tree_sitter::Node) -> Vec<WordSegment> {
        let span = self.node_span(node);
        let mut name = String::new();
        let mut subscript = ArraySubscript::At;

        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "subscript" {
                let mut sub_cursor = child.walk();
                for sub_child in child.children(&mut sub_cursor) {
                    match sub_child.kind() {
                        "variable_name" => name = self.node_text(&sub_child).to_string(),
                        "[" | "]" => {}
                        "word" | "number" => {
                            let text = self.node_text(&sub_child);
                            subscript = match text {
                                "@" => ArraySubscript::At,
                                "*" => ArraySubscript::Star,
                                _ => ArraySubscript::Index(text.to_string()),
                            };
                        }
                        _ => {
                            let text = self.node_text(&sub_child);
                            subscript = match text {
                                "@" => ArraySubscript::At,
                                "*" => ArraySubscript::Star,
                                _ => ArraySubscript::Index(text.to_string()),
                            };
                        }
                    }
                }
            }
        }

        vec![WordSegment::ArrayExpand(ArrayExpand {
            span,
            name,
            subscript,
        })]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::SourceId;

    fn lower(src: &str) -> LowerResult {
        let source = SourceFile::new(SourceId(0), "test.sh".into(), src.into());
        parse_and_lower(source)
    }

    #[test]
    fn lower_echo_hello() {
        let result = lower("echo hello world");
        assert!(result.parse_errors.is_empty());
        assert!(result.lowering_errors.is_empty());
        assert_eq!(result.source_unit.items.len(), 1);
        match &result.source_unit.items[0] {
            Item::Statement(Statement::Command(cmd)) => {
                assert_eq!(cmd.args.len(), 2);
            }
            other => panic!("expected Command, got {:?}", other),
        }
        insta::assert_yaml_snapshot!("echo_hello", result.source_unit);
    }

    #[test]
    fn lower_assignment() {
        let result = lower("FOO=bar");
        assert!(result.lowering_errors.is_empty());
        match &result.source_unit.items[0] {
            Item::Statement(Statement::Assignment(a)) => {
                assert_eq!(a.name, "FOO");
                assert!(a.value.is_some());
            }
            other => panic!("expected Assignment, got {:?}", other),
        }
        insta::assert_yaml_snapshot!("assignment", result.source_unit);
    }

    #[test]
    fn lower_function() {
        let result = lower("greet() { echo hi; }");
        assert!(result.lowering_errors.is_empty());
        match &result.source_unit.items[0] {
            Item::Function(f) => {
                assert_eq!(f.name, "greet");
                assert_eq!(f.body.len(), 1);
            }
            other => panic!("expected Function, got {:?}", other),
        }
        insta::assert_yaml_snapshot!("function", result.source_unit);
    }

    #[test]
    fn lower_annotated_function() {
        let src = "\
#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0] !may_exec
deploy() {
  #@bind $1 cfg
  #@bind $2.. manifests
  local cfg=$1
  echo done
}";
        let result = lower(src);
        assert!(result.parse_errors.is_empty());
        assert!(result.annotation_errors.is_empty());
        assert!(result.lowering_errors.is_empty(), "lowering errors: {:?}", result.lowering_errors);
        match &result.source_unit.items[0] {
            Item::Function(f) => {
                assert_eq!(f.name, "deploy");
                assert!(!f.annotations.is_empty());
            }
            other => panic!("expected Function, got {:?}", other),
        }
        insta::assert_yaml_snapshot!("annotated_function", result.source_unit);
    }

    #[test]
    fn lower_param_expansion() {
        let src = r#"echo "${x:-default}""#;
        let result = lower(src);
        assert!(result.lowering_errors.is_empty());
        insta::assert_yaml_snapshot!("param_expansion", result.source_unit);
    }

    #[test]
    fn lower_array_expansion() {
        let src = r#"echo "${arr[@]}""#;
        let result = lower(src);
        assert!(result.lowering_errors.is_empty());
        insta::assert_yaml_snapshot!("array_expansion", result.source_unit);
    }

    #[test]
    fn lower_pipeline() {
        let result = lower("cat file | grep pattern | wc -l");
        assert!(result.lowering_errors.is_empty());
        match &result.source_unit.items[0] {
            Item::Statement(Statement::Pipeline(p)) => {
                assert_eq!(p.commands.len(), 3);
            }
            other => panic!("expected Pipeline, got {:?}", other),
        }
        insta::assert_yaml_snapshot!("pipeline", result.source_unit);
    }

    #[test]
    fn lower_if_statement() {
        let src = "if [[ -f $x ]]; then echo yes; else echo no; fi";
        let result = lower(src);
        assert!(result.lowering_errors.is_empty());
        match &result.source_unit.items[0] {
            Item::Statement(Statement::If(if_stmt)) => {
                assert!(!if_stmt.condition.is_empty());
                assert!(!if_stmt.then_body.is_empty());
                assert!(if_stmt.else_body.is_some());
            }
            other => panic!("expected If, got {:?}", other),
        }
        insta::assert_yaml_snapshot!("if_statement", result.source_unit);
    }

    #[test]
    fn lower_for_loop() {
        let src = r#"for f in *.txt; do echo "$f"; done"#;
        let result = lower(src);
        assert!(result.lowering_errors.is_empty());
        match &result.source_unit.items[0] {
            Item::Statement(Statement::For(for_loop)) => {
                assert_eq!(for_loop.variable, "f");
                assert!(!for_loop.items.is_empty());
                assert!(!for_loop.body.is_empty());
            }
            other => panic!("expected For, got {:?}", other),
        }
        insta::assert_yaml_snapshot!("for_loop", result.source_unit);
    }

    #[test]
    fn lower_local_array() {
        let result = lower("local -a arr=(1 2 3)");
        assert!(result.lowering_errors.is_empty());
        match &result.source_unit.items[0] {
            Item::Statement(Statement::Assignment(a)) => {
                assert_eq!(a.name, "arr");
                assert!(matches!(a.decl_kind, Some(DeclKind::Local)));
                assert!(a.flags.contains(&"-a".to_string()));
                assert!(a.array_value.is_some());
            }
            other => panic!("expected Assignment, got {:?}", other),
        }
    }

    #[test]
    fn unmodeled_does_not_crash() {
        // c-style for loop might not be handled
        let result = lower("coproc { echo hello; }");
        // Should not panic, may produce Unmodeled or handle as-is
        assert!(!result.source_unit.items.is_empty());
    }
}
