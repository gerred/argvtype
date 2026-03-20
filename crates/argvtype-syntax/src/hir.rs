use crate::annotation::Annotation;
use crate::span::{SourceId, Span};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct NodeId(pub u32);

#[derive(Debug, Clone, Serialize)]
pub struct SourceUnit {
    pub source_id: SourceId,
    pub annotations: Vec<Annotation>,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub enum Item {
    Function(Function),
    Statement(Statement),
}

#[derive(Debug, Clone, Serialize)]
pub struct Function {
    pub id: NodeId,
    pub span: Span,
    pub name: String,
    pub body: Vec<Statement>,
    pub annotations: Vec<Annotation>,
}

#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub enum Statement {
    Assignment(Assignment),
    Command(Command),
    SourceCommand(SourceCommand),
    Pipeline(Pipeline),
    If(IfStatement),
    For(ForLoop),
    While(WhileLoop),
    Case(CaseStatement),
    List(List),
    Block(Block),
    Unmodeled(Unmodeled),
}

#[derive(Debug, Clone, Serialize)]
pub struct Assignment {
    pub id: NodeId,
    pub span: Span,
    pub name: String,
    pub value: Option<Word>,
    pub decl_kind: Option<DeclKind>,
    pub flags: Vec<String>,
    pub array_value: Option<Vec<Word>>,
}

#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub enum DeclKind {
    Local,
    Declare,
    Export,
    Readonly,
}

#[derive(Debug, Clone, Serialize)]
pub struct Command {
    pub id: NodeId,
    pub span: Span,
    pub name: Word,
    pub args: Vec<Word>,
    pub redirects: Vec<Redirect>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceCommand {
    pub id: NodeId,
    pub span: Span,
    pub path: Word,
    pub dynamic: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Redirect {
    pub span: Span,
    pub fd: Option<String>,
    pub op: String,
    pub target: Word,
}

#[derive(Debug, Clone, Serialize)]
pub struct Pipeline {
    pub id: NodeId,
    pub span: Span,
    pub commands: Vec<Statement>,
    pub negated: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct IfStatement {
    pub id: NodeId,
    pub span: Span,
    pub condition: Vec<Statement>,
    pub then_body: Vec<Statement>,
    pub else_body: Option<Vec<Statement>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ForLoop {
    pub id: NodeId,
    pub span: Span,
    pub variable: String,
    pub items: Vec<Word>,
    pub body: Vec<Statement>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WhileLoop {
    pub id: NodeId,
    pub span: Span,
    pub condition: Vec<Statement>,
    pub body: Vec<Statement>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaseStatement {
    pub id: NodeId,
    pub span: Span,
    pub subject: Word,
    pub arms: Vec<CaseArm>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CaseArm {
    pub span: Span,
    pub patterns: Vec<Word>,
    pub body: Vec<Statement>,
}

#[derive(Debug, Clone, Serialize)]
pub struct List {
    pub id: NodeId,
    pub span: Span,
    pub elements: Vec<ListElement>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ListElement {
    pub statement: Statement,
    pub operator: Option<ListOperator>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub enum ListOperator {
    And,
    Or,
    Semi,
}

#[derive(Debug, Clone, Serialize)]
pub struct Block {
    pub id: NodeId,
    pub span: Span,
    pub body: Vec<Statement>,
    pub subshell: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct Unmodeled {
    pub id: NodeId,
    pub span: Span,
    pub kind: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Word {
    pub span: Span,
    pub segments: Vec<WordSegment>,
}

#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub enum WordSegment {
    Literal(String),
    SingleQuoted(String),
    DoubleQuoted(Vec<WordSegment>),
    ParamExpand(ParamExpand),
    CommandSub(CommandSub),
    ArithExpand(ArithExpand),
    ArrayExpand(ArrayExpand),
}

impl Word {
    /// Returns the literal string value if this word is a single literal or single-quoted segment.
    pub fn literal_str(&self) -> Option<&str> {
        if self.segments.len() == 1 {
            match &self.segments[0] {
                WordSegment::Literal(s) => Some(s),
                WordSegment::SingleQuoted(s) => Some(s),
                WordSegment::DoubleQuoted(inner) if inner.len() == 1 => {
                    if let WordSegment::Literal(s) = &inner[0] {
                        Some(s)
                    } else {
                        None
                    }
                }
                _ => None,
            }
        } else {
            None
        }
    }

    /// Returns true if this word contains any shell expansions (variables, command subs, etc.).
    pub fn has_expansions(&self) -> bool {
        self.segments.iter().any(|seg| seg.has_expansions())
    }
}

impl WordSegment {
    fn has_expansions(&self) -> bool {
        match self {
            WordSegment::Literal(_) | WordSegment::SingleQuoted(_) => false,
            WordSegment::DoubleQuoted(inner) => inner.iter().any(|s| s.has_expansions()),
            WordSegment::ParamExpand(_)
            | WordSegment::CommandSub(_)
            | WordSegment::ArithExpand(_)
            | WordSegment::ArrayExpand(_) => true,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ParamExpand {
    pub span: Span,
    pub name: String,
    pub operator: Option<ParamOperator>,
    pub operand: Option<Box<Word>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub enum ParamOperator {
    Default,
    Assign,
    Error,
    Alternate,
    Length,
    Indirect,
}

#[derive(Debug, Clone, Serialize)]
pub struct CommandSub {
    pub span: Span,
    pub body: Vec<Statement>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArithExpand {
    pub span: Span,
    pub expression: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ArrayExpand {
    pub span: Span,
    pub name: String,
    pub subscript: ArraySubscript,
}

#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub enum ArraySubscript {
    At,
    Star,
    Index(String),
}
