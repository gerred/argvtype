use argvtype_syntax::span::{SourceFile, SourceId, Span};
use miette::{LabeledSpan, MietteDiagnostic as MietteReport, NamedSource};
use serde::Serialize;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct DiagnosticCode {
    pub family: &'static str,
    pub number: u16,
}

impl fmt::Display for DiagnosticCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{:03}", self.family, self.number)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

#[derive(Debug, Clone, Serialize)]
pub struct Label {
    pub span: Span,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub code: DiagnosticCode,
    pub severity: Severity,
    pub message: String,
    pub source_id: SourceId,
    pub primary_span: Span,
    pub labels: Vec<Label>,
    pub help: Option<String>,
}

impl Diagnostic {
    pub fn error(code: DiagnosticCode, message: impl Into<String>, source_id: SourceId, span: Span) -> Self {
        Self {
            code,
            severity: Severity::Error,
            message: message.into(),
            source_id,
            primary_span: span,
            labels: Vec::new(),
            help: None,
        }
    }

    pub fn warning(code: DiagnosticCode, message: impl Into<String>, source_id: SourceId, span: Span) -> Self {
        Self {
            code,
            severity: Severity::Warning,
            message: message.into(),
            source_id,
            primary_span: span,
            labels: Vec::new(),
            help: None,
        }
    }

    pub fn with_label(mut self, span: Span, message: impl Into<String>) -> Self {
        self.labels.push(Label {
            span,
            message: message.into(),
        });
        self
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }
}

pub fn render_diagnostics(diagnostics: &[Diagnostic], source: &SourceFile) -> Vec<miette::Report> {
    diagnostics
        .iter()
        .filter(|d| d.source_id == source.id)
        .map(|d| render_one(d, source))
        .collect()
}

fn render_one(diag: &Diagnostic, source: &SourceFile) -> miette::Report {
    let named = NamedSource::new(&source.name, source.source.clone());
    let primary = diag.primary_span.to_miette();

    let mut builder = MietteReport::new(diag.message.clone())
        .with_code(diag.code.to_string())
        .with_severity(match diag.severity {
            Severity::Error => miette::Severity::Error,
            Severity::Warning => miette::Severity::Warning,
            Severity::Info | Severity::Hint => miette::Severity::Advice,
        })
        .with_label(LabeledSpan::new(Some(diag.code.to_string()), primary.offset(), primary.len()));

    for label in &diag.labels {
        let ls = label.span.to_miette();
        builder = builder.with_label(LabeledSpan::new(Some(label.message.clone()), ls.offset(), ls.len()));
    }

    if let Some(help) = &diag.help {
        builder = builder.with_help(help);
    }

    miette::Report::new(builder).with_source_code(named)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_code_formatting() {
        let code = DiagnosticCode {
            family: "BT",
            number: 201,
        };
        assert_eq!(code.to_string(), "BT201");

        let code2 = DiagnosticCode {
            family: "BT",
            number: 0,
        };
        assert_eq!(code2.to_string(), "BT000");
    }

    #[test]
    fn construct_and_render_diagnostic() {
        let source = SourceFile::new(SourceId(0), "test.sh".into(), "echo $arr\n".into());
        let diag = Diagnostic::error(
            DiagnosticCode {
                family: "BT",
                number: 201,
            },
            "array used in scalar expansion",
            SourceId(0),
            Span::new(5, 9),
        )
        .with_help("use \"${arr[@]}\" instead");

        let reports = render_diagnostics(&[diag], &source);
        assert_eq!(reports.len(), 1);

        let rendered = format!("{:?}", reports[0]);
        assert!(rendered.contains("BT201"));
        assert!(rendered.contains("array used in scalar expansion"));
    }
}
