use crate::span::{SourceFile, Span};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, Serialize)]
pub struct Annotation {
    pub span: Span,
    pub directive: Directive,
}

#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub enum Directive {
    Sig(SigDirective),
    Bind(BindDirective),
    Type(TypeDirective),
    Module(ModuleDirective),
    Proves(ProvesDirective),
    Unknown { name: String },
}

#[derive(Debug, Clone, Serialize)]
pub struct ProvesDirective {
    /// The parameter reference (e.g., "$1")
    pub param: String,
    /// The refinement established (e.g., "ExistingFile")
    pub refinement: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SigDirective {
    pub name: String,
    pub params: Vec<Param>,
    pub return_type: Option<TypeExpr>,
    pub effects: Vec<Effect>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Param {
    pub name: String,
    pub type_expr: TypeExpr,
}

#[derive(Debug, Clone, Serialize)]
#[non_exhaustive]
pub enum TypeExpr {
    Named(String),
    Parameterized { name: String, param: Box<TypeExpr> },
    Status(String),
}

#[derive(Debug, Clone, Serialize)]
pub struct Effect {
    pub name: String,
    pub span: Span,
}

#[derive(Debug, Clone, Serialize)]
pub struct BindDirective {
    pub positional: String,
    pub name: String,
    pub variadic: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct TypeDirective {
    pub name: String,
    pub type_expr: TypeExpr,
}

#[derive(Debug, Clone, Serialize)]
pub struct ModuleDirective {
    pub shell: String,
    pub version_constraint: Option<String>,
}

#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AnnotationError {
    #[error("malformed annotation at {span:?}: {message}")]
    Malformed { span: Span, message: String },
}

pub fn parse_annotations(source: &SourceFile) -> (Vec<Annotation>, Vec<AnnotationError>) {
    let mut annotations = Vec::new();
    let mut errors = Vec::new();

    for (line_idx, line) in source.source.lines().enumerate() {
        let line_start = source
            .source
            .lines()
            .take(line_idx)
            .map(|l| l.len() + 1) // +1 for newline
            .sum::<usize>() as u32;

        let trimmed = line.trim_start();
        if !trimmed.starts_with("#@") {
            continue;
        }

        let pragma_offset = line.len() - trimmed.len();
        let pragma_start = line_start + pragma_offset as u32;
        let pragma_end = line_start + line.len() as u32;
        let span = Span::new(pragma_start, pragma_end);

        let body = &trimmed[2..].trim_start();
        match parse_directive(body, span) {
            Ok(directive) => annotations.push(Annotation { span, directive }),
            Err(e) => errors.push(e),
        }
    }

    (annotations, errors)
}

fn parse_directive(body: &str, span: Span) -> Result<Directive, AnnotationError> {
    let (keyword, rest) = split_first_word(body);
    match keyword {
        "sig" => parse_sig(rest, span),
        "bind" => parse_bind(rest, span),
        "type" => parse_type_directive(rest, span),
        "module" => parse_module(rest, span),
        "proves" => parse_proves(rest, span),
        _ => Ok(Directive::Unknown {
            name: keyword.to_string(),
        }),
    }
}

fn split_first_word(s: &str) -> (&str, &str) {
    let s = s.trim();
    match s.find(char::is_whitespace) {
        Some(i) => (&s[..i], s[i..].trim_start()),
        None => (s, ""),
    }
}

fn parse_sig(rest: &str, span: Span) -> Result<Directive, AnnotationError> {
    // Format: name(param: Type, param: Type) -> ReturnType !effect1 !effect2
    let Some(paren_open) = rest.find('(') else {
        return Err(AnnotationError::Malformed {
            span,
            message: "expected '(' in sig directive".into(),
        });
    };

    let name = rest[..paren_open].trim().to_string();

    let Some(paren_close) = rest.find(')') else {
        return Err(AnnotationError::Malformed {
            span,
            message: "expected ')' in sig directive".into(),
        });
    };

    let params_str = &rest[paren_open + 1..paren_close];
    let params = parse_params(params_str, span)?;

    let after_parens = rest[paren_close + 1..].trim();

    let (return_type, effects_str) = if let Some(arrow_rest) = after_parens.strip_prefix("->") {
        let arrow_rest = arrow_rest.trim();
        // Find where effects start (first '!')
        if let Some(effect_start) = arrow_rest.find('!') {
            let ret_str = arrow_rest[..effect_start].trim();
            let ret = parse_type_expr(ret_str, span)?;
            (Some(ret), &arrow_rest[effect_start..])
        } else {
            let ret = parse_type_expr(arrow_rest, span)?;
            (Some(ret), "")
        }
    } else {
        (None, after_parens)
    };

    let effects = parse_effects(effects_str, span);

    Ok(Directive::Sig(SigDirective {
        name,
        params,
        return_type,
        effects,
    }))
}

fn parse_params(params_str: &str, span: Span) -> Result<Vec<Param>, AnnotationError> {
    let trimmed = params_str.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }

    let mut params = Vec::new();
    for part in trimmed.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let Some(colon) = part.find(':') else {
            return Err(AnnotationError::Malformed {
                span,
                message: format!("expected ':' in parameter '{}'", part),
            });
        };
        let name = part[..colon].trim().to_string();
        let type_str = part[colon + 1..].trim();
        let type_expr = parse_type_expr(type_str, span)?;
        params.push(Param { name, type_expr });
    }
    Ok(params)
}

fn parse_type_expr(s: &str, span: Span) -> Result<TypeExpr, AnnotationError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(AnnotationError::Malformed {
            span,
            message: "empty type expression".into(),
        });
    }

    // Status[code]
    if let Some(inner) = try_parameterized(s, "Status") {
        return Ok(TypeExpr::Status(inner.to_string()));
    }

    // Parameterized: Name[Param]
    if let Some(bracket_open) = s.find('[')
        && s.ends_with(']')
    {
        let name = s[..bracket_open].trim().to_string();
        let inner = &s[bracket_open + 1..s.len() - 1];
        let param = parse_type_expr(inner, span)?;
        return Ok(TypeExpr::Parameterized {
            name,
            param: Box::new(param),
        });
    }

    Ok(TypeExpr::Named(s.to_string()))
}

fn try_parameterized<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = s.strip_prefix(prefix)?;
    let rest = rest.strip_prefix('[')?;
    let rest = rest.strip_suffix(']')?;
    Some(rest)
}

fn parse_effects(s: &str, span: Span) -> Vec<Effect> {
    let mut effects = Vec::new();
    for word in s.split_whitespace() {
        if let Some(name) = word.strip_prefix('!') {
            effects.push(Effect {
                name: name.to_string(),
                span,
            });
        }
    }
    effects
}

fn parse_bind(rest: &str, span: Span) -> Result<Directive, AnnotationError> {
    // Format: $N name or $N.. name (variadic)
    let (positional_raw, name) = split_first_word(rest);
    if positional_raw.is_empty() || name.is_empty() {
        return Err(AnnotationError::Malformed {
            span,
            message: "expected '$N name' in bind directive".into(),
        });
    }

    let variadic = positional_raw.ends_with("..");
    let positional = if variadic {
        positional_raw[..positional_raw.len() - 2].to_string()
    } else {
        positional_raw.to_string()
    };

    Ok(Directive::Bind(BindDirective {
        positional,
        name: name.to_string(),
        variadic,
    }))
}

fn parse_type_directive(rest: &str, span: Span) -> Result<Directive, AnnotationError> {
    // Format: NAME: TypeExpr
    let Some(colon) = rest.find(':') else {
        return Err(AnnotationError::Malformed {
            span,
            message: "expected ':' in type directive".into(),
        });
    };
    let name = rest[..colon].trim().to_string();
    let type_str = rest[colon + 1..].trim();
    let type_expr = parse_type_expr(type_str, span)?;
    Ok(Directive::Type(TypeDirective { name, type_expr }))
}

fn parse_proves(rest: &str, span: Span) -> Result<Directive, AnnotationError> {
    // Format: $N RefinementType (e.g., "$1 ExistingFile")
    let (param, refinement) = split_first_word(rest);
    if param.is_empty() || refinement.is_empty() {
        return Err(AnnotationError::Malformed {
            span,
            message: "expected '$N RefinementType' in proves directive".into(),
        });
    }
    Ok(Directive::Proves(ProvesDirective {
        param: param.to_string(),
        refinement: refinement.to_string(),
    }))
}

fn parse_module(rest: &str, span: Span) -> Result<Directive, AnnotationError> {
    let rest = rest.trim();
    if rest.is_empty() {
        return Err(AnnotationError::Malformed {
            span,
            message: "expected shell name in module directive".into(),
        });
    }

    // Try to split on >= or other operators
    let (shell, version_constraint) = if let Some(idx) = rest.find(">=") {
        (
            rest[..idx].trim().to_string(),
            Some(rest[idx..].trim().to_string()),
        )
    } else if let Some(idx) = rest.find("<=") {
        (
            rest[..idx].trim().to_string(),
            Some(rest[idx..].trim().to_string()),
        )
    } else if let Some(idx) = rest.find('=') {
        (
            rest[..idx].trim().to_string(),
            Some(rest[idx..].trim().to_string()),
        )
    } else {
        (rest.to_string(), None)
    };

    Ok(Directive::Module(ModuleDirective {
        shell,
        version_constraint,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::span::SourceId;

    fn parse(src: &str) -> (Vec<Annotation>, Vec<AnnotationError>) {
        let source = SourceFile::new(SourceId(0), "test.sh".into(), src.into());
        parse_annotations(&source)
    }

    #[test]
    fn parse_sig_full() {
        let (anns, errs) = parse(
            "#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0] !may_exec",
        );
        assert!(errs.is_empty(), "errors: {:?}", errs);
        assert_eq!(anns.len(), 1);
        match &anns[0].directive {
            Directive::Sig(sig) => {
                assert_eq!(sig.name, "deploy");
                assert_eq!(sig.params.len(), 1);
                assert_eq!(sig.params[0].name, "cfg");
                assert!(matches!(&sig.params[0].type_expr, TypeExpr::Parameterized { name, .. } if name == "Scalar"));
                assert!(matches!(&sig.return_type, Some(TypeExpr::Status(code)) if code == "0"));
                assert_eq!(sig.effects.len(), 1);
                assert_eq!(sig.effects[0].name, "may_exec");
            }
            other => panic!("expected Sig, got {:?}", other),
        }
    }

    #[test]
    fn parse_bind_simple() {
        let (anns, errs) = parse("#@bind $1 cfg");
        assert!(errs.is_empty());
        assert_eq!(anns.len(), 1);
        match &anns[0].directive {
            Directive::Bind(bind) => {
                assert_eq!(bind.positional, "$1");
                assert_eq!(bind.name, "cfg");
                assert!(!bind.variadic);
            }
            other => panic!("expected Bind, got {:?}", other),
        }
    }

    #[test]
    fn parse_bind_variadic() {
        let (anns, errs) = parse("#@bind $2.. manifests");
        assert!(errs.is_empty());
        match &anns[0].directive {
            Directive::Bind(bind) => {
                assert_eq!(bind.positional, "$2");
                assert_eq!(bind.name, "manifests");
                assert!(bind.variadic);
            }
            other => panic!("expected Bind, got {:?}", other),
        }
    }

    #[test]
    fn parse_type_directive() {
        let (anns, errs) = parse("#@type KUBECONFIG: Scalar[ExistingFile]");
        assert!(errs.is_empty());
        match &anns[0].directive {
            Directive::Type(td) => {
                assert_eq!(td.name, "KUBECONFIG");
                assert!(matches!(&td.type_expr, TypeExpr::Parameterized { name, .. } if name == "Scalar"));
            }
            other => panic!("expected Type, got {:?}", other),
        }
    }

    #[test]
    fn parse_module_directive() {
        let (anns, errs) = parse("#@module bash>=5.2");
        assert!(errs.is_empty());
        match &anns[0].directive {
            Directive::Module(m) => {
                assert_eq!(m.shell, "bash");
                assert_eq!(m.version_constraint.as_deref(), Some(">=5.2"));
            }
            other => panic!("expected Module, got {:?}", other),
        }
    }

    #[test]
    fn unknown_directive() {
        let (anns, errs) = parse("#@foobar something");
        assert!(errs.is_empty());
        assert!(matches!(&anns[0].directive, Directive::Unknown { name } if name == "foobar"));
    }

    #[test]
    fn malformed_sig() {
        let (anns, errs) = parse("#@sig (");
        assert!(anns.is_empty());
        assert_eq!(errs.len(), 1);
        assert!(matches!(&errs[0], AnnotationError::Malformed { .. }));
    }

    #[test]
    fn parse_proves_directive() {
        let (anns, errs) = parse("#@proves $1 ExistingFile");
        assert!(errs.is_empty(), "errors: {:?}", errs);
        assert_eq!(anns.len(), 1);
        match &anns[0].directive {
            Directive::Proves(p) => {
                assert_eq!(p.param, "$1");
                assert_eq!(p.refinement, "ExistingFile");
            }
            other => panic!("expected Proves, got {:?}", other),
        }
    }

    #[test]
    fn parse_proves_malformed() {
        let (anns, errs) = parse("#@proves $1");
        assert!(anns.is_empty());
        assert_eq!(errs.len(), 1);
    }

    #[test]
    fn multi_annotation_file() {
        let src = "\
#!/usr/bin/env bash
#@module bash>=5.2
#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0] !may_exec
deploy() {
  #@bind $1 cfg
  #@bind $2.. manifests
  echo done
}
";
        let (anns, errs) = parse(src);
        assert!(errs.is_empty(), "errors: {:?}", errs);
        assert_eq!(anns.len(), 4);
        assert!(matches!(&anns[0].directive, Directive::Module(_)));
        assert!(matches!(&anns[1].directive, Directive::Sig(_)));
        assert!(matches!(&anns[2].directive, Directive::Bind(_)));
        assert!(matches!(&anns[3].directive, Directive::Bind(_)));
    }
}
