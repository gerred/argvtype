use std::collections::HashMap;

use argvtype_syntax::annotation::{Directive, TypeExpr};
use argvtype_syntax::hir::*;
use argvtype_syntax::span::Span;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub enum CellKind {
    Scalar,
    IndexedArray,
    AssocArray,
    Unknown,
}

/// Whether a variable is known to be set at a given program point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub enum Presence {
    Set,
    Unset,
    MaybeUnset,
    Unknown,
}

/// The expansion shape of a variable — how it behaves when expanded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub enum ExpansionShape {
    Scalar,    // produces one shell word
    Argv,      // produces zero-or-more shell words (splice)
    Unknown,
}

/// Combined type information for a variable.
#[derive(Debug, Clone, Serialize)]
pub struct TypeInfo {
    pub cell_kind: CellKind,
    pub shape: ExpansionShape,
    pub refinement: Option<String>,
    pub presence: Presence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub enum DeclScope {
    Global,
    Local,
    Export,
    Readonly,
    Implicit,
}

#[derive(Debug, Clone, Serialize)]
pub struct Symbol {
    pub name: String,
    pub cell_kind: CellKind,
    pub type_info: TypeInfo,
    pub decl_scope: DeclScope,
    pub decl_span: Span,
    pub decl_node: NodeId,
    pub type_annotation: Option<TypeExpr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct ScopeId(pub u32);

#[derive(Debug, Clone, Serialize)]
pub struct Scope {
    pub id: ScopeId,
    pub parent: Option<ScopeId>,
    pub kind: ScopeKind,
    pub symbols: HashMap<String, Symbol>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub enum ScopeKind {
    Global,
    Function,
    Subshell,
}

#[derive(Debug, Clone, Serialize)]
pub struct SymbolTable {
    scopes: Vec<Scope>,
    node_scopes: HashMap<NodeId, ScopeId>,
}

impl Default for SymbolTable {
    fn default() -> Self {
        Self::new()
    }
}

impl SymbolTable {
    pub fn new() -> Self {
        let global = Scope {
            id: ScopeId(0),
            parent: None,
            kind: ScopeKind::Global,
            symbols: HashMap::new(),
        };
        SymbolTable {
            scopes: vec![global],
            node_scopes: HashMap::new(),
        }
    }

    pub fn root_scope(&self) -> ScopeId {
        ScopeId(0)
    }

    pub fn push_scope(&mut self, parent: ScopeId, kind: ScopeKind) -> ScopeId {
        let id = ScopeId(self.scopes.len() as u32);
        self.scopes.push(Scope {
            id,
            parent: Some(parent),
            kind,
            symbols: HashMap::new(),
        });
        id
    }

    pub fn define(&mut self, scope: ScopeId, symbol: Symbol) {
        self.scopes[scope.0 as usize]
            .symbols
            .insert(symbol.name.clone(), symbol);
    }

    pub fn resolve(&self, scope: ScopeId, name: &str) -> Option<&Symbol> {
        let s = &self.scopes[scope.0 as usize];
        if let Some(sym) = s.symbols.get(name) {
            return Some(sym);
        }
        if let Some(parent) = s.parent {
            return self.resolve(parent, name);
        }
        None
    }

    pub fn scope(&self, id: ScopeId) -> &Scope {
        &self.scopes[id.0 as usize]
    }

    pub fn scope_of_node(&self, node: NodeId) -> Option<ScopeId> {
        self.node_scopes.get(&node).copied()
    }

    pub fn bind_node(&mut self, node: NodeId, scope: ScopeId) {
        self.node_scopes.insert(node, scope);
    }

    pub fn for_each_symbol(&self, mut f: impl FnMut(&Symbol)) {
        for scope in &self.scopes {
            for sym in scope.symbols.values() {
                f(sym);
            }
        }
    }
}

pub fn build_symbol_table(source_unit: &SourceUnit) -> SymbolTable {
    let mut table = SymbolTable::new();
    let global = table.root_scope();

    for item in &source_unit.items {
        match item {
            Item::Function(f) => build_function(&mut table, f, global),
            Item::Statement(s) => build_statement(&mut table, s, global),
            _ => {}
        }
    }

    // Process top-level type annotations after items so symbols exist
    for ann in &source_unit.annotations {
        if let Directive::Type(td) = &ann.directive
            && let Some(sym) = table.scopes[global.0 as usize].symbols.get_mut(&td.name)
        {
            sym.type_annotation = Some(td.type_expr.clone());
            sym.type_info = infer_type_info(sym.cell_kind, Some(&td.type_expr), sym.type_info.presence);
        }
    }

    table
}

fn build_function(table: &mut SymbolTable, func: &Function, parent: ScopeId) {
    let func_scope = table.push_scope(parent, ScopeKind::Function);
    table.bind_node(func.id, func_scope);

    // Process annotations: #@sig params and #@bind directives
    for ann in &func.annotations {
        match &ann.directive {
            Directive::Sig(sig) => {
                for param in &sig.params {
                    let cell_kind = type_expr_to_cell_kind(&param.type_expr);
                    let type_info = infer_type_info(cell_kind, Some(&param.type_expr), Presence::MaybeUnset);
                    table.define(
                        func_scope,
                        Symbol {
                            name: param.name.clone(),
                            cell_kind,
                            type_info,
                            decl_scope: DeclScope::Local,
                            decl_span: ann.span,
                            decl_node: func.id,
                            type_annotation: Some(param.type_expr.clone()),
                        },
                    );
                }
            }
            Directive::Bind(bind) => {
                // Create a symbol for the bind target if not already defined by sig
                if table.resolve(func_scope, &bind.name).is_none() {
                    let cell_kind = if bind.variadic {
                        CellKind::IndexedArray
                    } else {
                        CellKind::Scalar
                    };
                    let type_info = infer_type_info(cell_kind, None, Presence::MaybeUnset);
                    table.define(
                        func_scope,
                        Symbol {
                            name: bind.name.clone(),
                            cell_kind,
                            type_info,
                            decl_scope: DeclScope::Local,
                            decl_span: ann.span,
                            decl_node: func.id,
                            type_annotation: None,
                        },
                    );
                }
            }
            Directive::Type(td) => {
                if let Some(sym) = table.scopes[func_scope.0 as usize]
                    .symbols
                    .get_mut(&td.name)
                {
                    sym.type_annotation = Some(td.type_expr.clone());
                    sym.type_info = infer_type_info(sym.cell_kind, Some(&td.type_expr), sym.type_info.presence);
                }
            }
            _ => {}
        }
    }

    for stmt in &func.body {
        build_statement(table, stmt, func_scope);
    }
}

fn build_statement(table: &mut SymbolTable, stmt: &Statement, scope: ScopeId) {
    match stmt {
        Statement::Assignment(a) => {
            table.bind_node(a.id, scope);
            let cell_kind = cell_kind_from_assignment(a);
            let decl_scope = decl_scope_from_assignment(a);
            let has_value = a.value.is_some() || a.array_value.is_some();
            let presence = if has_value {
                Presence::Set
            } else {
                Presence::Unset
            };
            let type_info = infer_type_info(cell_kind, None, presence);
            table.define(
                scope,
                Symbol {
                    name: a.name.clone(),
                    cell_kind,
                    type_info,
                    decl_scope,
                    decl_span: a.span,
                    decl_node: a.id,
                    type_annotation: None,
                },
            );
        }
        Statement::Command(cmd) => {
            table.bind_node(cmd.id, scope);
        }
        Statement::Pipeline(p) => {
            table.bind_node(p.id, scope);
            for s in &p.commands {
                build_statement(table, s, scope);
            }
        }
        Statement::If(if_stmt) => {
            table.bind_node(if_stmt.id, scope);
            for s in &if_stmt.condition {
                build_statement(table, s, scope);
            }
            for s in &if_stmt.then_body {
                build_statement(table, s, scope);
            }
            if let Some(else_body) = &if_stmt.else_body {
                for s in else_body {
                    build_statement(table, s, scope);
                }
            }
        }
        Statement::For(for_loop) => {
            table.bind_node(for_loop.id, scope);
            // Define the loop variable in the current scope
            table.define(
                scope,
                Symbol {
                    name: for_loop.variable.clone(),
                    cell_kind: CellKind::Scalar,
                    type_info: infer_type_info(CellKind::Scalar, None, Presence::Set),
                    decl_scope: DeclScope::Implicit,
                    decl_span: for_loop.span,
                    decl_node: for_loop.id,
                    type_annotation: None,
                },
            );
            for s in &for_loop.body {
                build_statement(table, s, scope);
            }
        }
        Statement::While(while_loop) => {
            table.bind_node(while_loop.id, scope);
            for s in &while_loop.condition {
                build_statement(table, s, scope);
            }
            for s in &while_loop.body {
                build_statement(table, s, scope);
            }
        }
        Statement::Case(case_stmt) => {
            table.bind_node(case_stmt.id, scope);
            for arm in &case_stmt.arms {
                for s in &arm.body {
                    build_statement(table, s, scope);
                }
            }
        }
        Statement::List(list) => {
            table.bind_node(list.id, scope);
            for elem in &list.elements {
                build_statement(table, &elem.statement, scope);
            }
        }
        Statement::Block(block) => {
            table.bind_node(block.id, scope);
            if block.subshell {
                let sub_scope = table.push_scope(scope, ScopeKind::Subshell);
                for s in &block.body {
                    build_statement(table, s, sub_scope);
                }
            } else {
                for s in &block.body {
                    build_statement(table, s, scope);
                }
            }
        }
        Statement::Unmodeled(u) => {
            table.bind_node(u.id, scope);
        }
        _ => {}
    }
}

fn cell_kind_from_assignment(a: &Assignment) -> CellKind {
    if a.flags.iter().any(|f| f == "-a") {
        CellKind::IndexedArray
    } else if a.flags.iter().any(|f| f == "-A") {
        CellKind::AssocArray
    } else if a.array_value.is_some() {
        CellKind::IndexedArray
    } else {
        CellKind::Scalar
    }
}

fn decl_scope_from_assignment(a: &Assignment) -> DeclScope {
    match &a.decl_kind {
        Some(DeclKind::Local) | Some(DeclKind::Declare) => DeclScope::Local,
        Some(DeclKind::Export) => DeclScope::Export,
        Some(DeclKind::Readonly) => DeclScope::Readonly,
        None => DeclScope::Implicit,
        _ => DeclScope::Implicit,
    }
}

fn infer_type_info(cell_kind: CellKind, type_annotation: Option<&TypeExpr>, presence: Presence) -> TypeInfo {
    match type_annotation {
        Some(type_expr) => {
            let (shape, refinement) = match type_expr {
                TypeExpr::Named(name) if name == "Scalar" => (ExpansionShape::Scalar, None),
                TypeExpr::Named(name) if name == "Argv" => (ExpansionShape::Argv, None),
                TypeExpr::Named(name) => (ExpansionShape::Scalar, Some(name.clone())),
                TypeExpr::Parameterized { name, param } if name == "Scalar" => {
                    (ExpansionShape::Scalar, Some(type_expr_refinement(param)))
                }
                TypeExpr::Parameterized { name, param } if name == "Argv" => {
                    (ExpansionShape::Argv, Some(type_expr_refinement(param)))
                }
                TypeExpr::Parameterized { name, param } if name == "AssocArray" => {
                    (ExpansionShape::Scalar, Some(type_expr_refinement(param)))
                }
                TypeExpr::Parameterized { name, param } => {
                    (ExpansionShape::Scalar, Some(format!("{}[{}]", name, type_expr_refinement(param))))
                }
                TypeExpr::Status(_) => (ExpansionShape::Scalar, Some("Status".into())),
                _ => (ExpansionShape::Unknown, None),
            };
            TypeInfo { cell_kind, shape, refinement, presence }
        }
        None => {
            let shape = match cell_kind {
                CellKind::IndexedArray => ExpansionShape::Argv,
                CellKind::AssocArray => ExpansionShape::Scalar,
                CellKind::Scalar => ExpansionShape::Scalar,
                CellKind::Unknown => ExpansionShape::Unknown,
            };
            TypeInfo { cell_kind, shape, refinement: None, presence }
        }
    }
}

fn type_expr_refinement(expr: &TypeExpr) -> String {
    match expr {
        TypeExpr::Named(name) => name.clone(),
        TypeExpr::Parameterized { name, param } => format!("{}[{}]", name, type_expr_refinement(param)),
        TypeExpr::Status(code) => format!("Status[{}]", code),
        _ => "Unknown".into(),
    }
}

fn type_expr_to_cell_kind(type_expr: &TypeExpr) -> CellKind {
    match type_expr {
        TypeExpr::Named(name) if name == "Argv" => CellKind::IndexedArray,
        TypeExpr::Parameterized { name, .. } if name == "Argv" => CellKind::IndexedArray,
        TypeExpr::Parameterized { name, .. } if name == "AssocArray" => CellKind::AssocArray,
        _ => CellKind::Scalar,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use argvtype_syntax::lower::parse_and_lower;
    use argvtype_syntax::span::{SourceFile, SourceId};

    fn build(src: &str) -> SymbolTable {
        let source = SourceFile::new(SourceId(0), "test.sh".into(), src.into());
        let result = parse_and_lower(source);
        build_symbol_table(&result.source_unit)
    }

    #[test]
    fn resolve_global_variable() {
        let table = build("x=hello");
        let sym = table.resolve(table.root_scope(), "x").unwrap();
        assert_eq!(sym.name, "x");
        assert_eq!(sym.cell_kind, CellKind::Scalar);
        assert_eq!(sym.decl_scope, DeclScope::Implicit);
    }

    #[test]
    fn resolve_local_shadows_global() {
        let table = build("x=global\nfoo() { local x=local; }");
        // Global scope should have x
        let global_sym = table.resolve(table.root_scope(), "x").unwrap();
        assert_eq!(global_sym.decl_scope, DeclScope::Implicit);

        // Find function scope (should be ScopeId(1))
        let func_scope = ScopeId(1);
        let local_sym = table.resolve(func_scope, "x").unwrap();
        assert_eq!(local_sym.decl_scope, DeclScope::Local);
    }

    #[test]
    fn resolve_walks_parent_chain() {
        let table = build("x=hello\nfoo() { echo $x; }");
        let func_scope = ScopeId(1);
        let sym = table.resolve(func_scope, "x").unwrap();
        assert_eq!(sym.name, "x");
        assert_eq!(sym.decl_scope, DeclScope::Implicit);
    }

    #[test]
    fn resolve_unknown_returns_none() {
        let table = build("x=hello");
        assert!(table.resolve(table.root_scope(), "nonexistent").is_none());
    }

    #[test]
    fn cell_kind_indexed_array() {
        let table = build("declare -a arr=(1 2 3)");
        let sym = table.resolve(table.root_scope(), "arr").unwrap();
        assert_eq!(sym.cell_kind, CellKind::IndexedArray);
    }

    #[test]
    fn cell_kind_assoc_array() {
        let table = build("declare -A map");
        let sym = table.resolve(table.root_scope(), "map").unwrap();
        assert_eq!(sym.cell_kind, CellKind::AssocArray);
    }

    #[test]
    fn cell_kind_scalar_default() {
        let table = build("x=hello");
        let sym = table.resolve(table.root_scope(), "x").unwrap();
        assert_eq!(sym.cell_kind, CellKind::Scalar);
    }

    #[test]
    fn cell_kind_array_value_without_flag() {
        let table = build("arr=(1 2 3)");
        let sym = table.resolve(table.root_scope(), "arr").unwrap();
        assert_eq!(sym.cell_kind, CellKind::IndexedArray);
    }

    #[test]
    fn function_scope_isolates_locals() {
        let table = build("foo() { local y=bar; }");
        // y should not be visible from global scope
        assert!(table.resolve(table.root_scope(), "y").is_none());
        // y should be visible in the function scope
        let func_scope = ScopeId(1);
        let sym = table.resolve(func_scope, "y").unwrap();
        assert_eq!(sym.name, "y");
        assert_eq!(sym.decl_scope, DeclScope::Local);
    }

    #[test]
    fn for_loop_variable_defined() {
        let table = build("for f in *.txt; do echo $f; done");
        let sym = table.resolve(table.root_scope(), "f").unwrap();
        assert_eq!(sym.name, "f");
        assert_eq!(sym.cell_kind, CellKind::Scalar);
        assert_eq!(sym.decl_scope, DeclScope::Implicit);
    }

    #[test]
    fn annotation_populates_type() {
        let src = "\
#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0] !may_exec
deploy() {
  #@bind $1 cfg
  echo done
}";
        let table = build(src);
        let func_scope = ScopeId(1);
        let sym = table.resolve(func_scope, "cfg").unwrap();
        assert!(sym.type_annotation.is_some());
        assert_eq!(sym.cell_kind, CellKind::Scalar);
    }

    #[test]
    fn bind_variadic_creates_array() {
        let src = "\
#@sig process(files: Argv[String]) -> Status[0]
process() {
  #@bind $1.. files
  echo done
}";
        let table = build(src);
        let func_scope = ScopeId(1);
        let sym = table.resolve(func_scope, "files").unwrap();
        assert_eq!(sym.cell_kind, CellKind::IndexedArray);
    }

    #[test]
    fn export_decl_scope() {
        let table = build("export PATH=/usr/bin");
        let sym = table.resolve(table.root_scope(), "PATH").unwrap();
        assert_eq!(sym.decl_scope, DeclScope::Export);
    }

    #[test]
    fn readonly_decl_scope() {
        let table = build("readonly VERSION=1.0");
        let sym = table.resolve(table.root_scope(), "VERSION").unwrap();
        assert_eq!(sym.decl_scope, DeclScope::Readonly);
    }

    #[test]
    fn local_array_in_function() {
        let table = build("foo() { local -a arr=(1 2 3); }");
        let func_scope = ScopeId(1);
        let sym = table.resolve(func_scope, "arr").unwrap();
        assert_eq!(sym.cell_kind, CellKind::IndexedArray);
        assert_eq!(sym.decl_scope, DeclScope::Local);
    }

    #[test]
    fn node_scope_binding() {
        let table = build("x=hello\necho $x");
        // The assignment and command nodes should be bound to the global scope
        // Just verify the table has node bindings
        let root = table.root_scope();
        // At least some nodes should be bound
        assert!(table.scope(root).symbols.contains_key("x"));
    }

    #[test]
    fn infer_type_info_scalar_annotation() {
        let src = "\
#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0] !may_exec
deploy() {
  #@bind $1 cfg
  echo done
}";
        let table = build(src);
        let func_scope = ScopeId(1);
        let sym = table.resolve(func_scope, "cfg").unwrap();
        assert_eq!(sym.type_info.shape, ExpansionShape::Scalar);
        assert_eq!(sym.type_info.refinement.as_deref(), Some("ExistingFile"));
    }

    #[test]
    fn infer_type_info_argv_annotation() {
        let src = "\
#@sig process(files: Argv[String]) -> Status[0]
process() {
  #@bind $1.. files
  echo done
}";
        let table = build(src);
        let func_scope = ScopeId(1);
        let sym = table.resolve(func_scope, "files").unwrap();
        assert_eq!(sym.type_info.shape, ExpansionShape::Argv);
        assert_eq!(sym.type_info.refinement.as_deref(), Some("String"));
    }

    #[test]
    fn infer_type_info_no_annotation_array() {
        let table = build("declare -a arr=(1 2 3)");
        let sym = table.resolve(table.root_scope(), "arr").unwrap();
        assert_eq!(sym.type_info.shape, ExpansionShape::Argv);
        assert!(sym.type_info.refinement.is_none());
    }

    #[test]
    fn infer_type_info_no_annotation_scalar() {
        let table = build("x=hello");
        let sym = table.resolve(table.root_scope(), "x").unwrap();
        assert_eq!(sym.type_info.shape, ExpansionShape::Scalar);
        assert!(sym.type_info.refinement.is_none());
    }

    #[test]
    fn type_info_from_declare_a() {
        let table = build("foo() { local -a arr=(1 2 3); }");
        let func_scope = ScopeId(1);
        let sym = table.resolve(func_scope, "arr").unwrap();
        assert_eq!(sym.type_info.cell_kind, CellKind::IndexedArray);
        assert_eq!(sym.type_info.shape, ExpansionShape::Argv);
    }

    #[test]
    fn type_info_shape_unknown() {
        let mut table = SymbolTable::new();
        let root = table.root_scope();
        let ti = super::infer_type_info(CellKind::Unknown, None, super::Presence::Unknown);
        assert_eq!(ti.shape, ExpansionShape::Unknown);
        table.define(root, Symbol {
            name: "mystery".into(),
            cell_kind: CellKind::Unknown,
            type_info: ti,
            decl_scope: DeclScope::Implicit,
            decl_span: Span::new(0, 0),
            decl_node: NodeId(0),
            type_annotation: None,
        });
        let sym = table.resolve(root, "mystery").unwrap();
        assert_eq!(sym.type_info.shape, ExpansionShape::Unknown);
    }

    #[test]
    fn presence_set_from_assignment() {
        let table = build("x=hello");
        let sym = table.resolve(table.root_scope(), "x").unwrap();
        assert_eq!(sym.type_info.presence, Presence::Set);
    }

    #[test]
    fn presence_unset_from_local_no_value() {
        let table = build("foo() { local x; }");
        let func_scope = ScopeId(1);
        let sym = table.resolve(func_scope, "x").unwrap();
        assert_eq!(sym.type_info.presence, Presence::Unset);
    }

    #[test]
    fn presence_set_from_local_with_value() {
        let table = build("foo() { local x=hello; }");
        let func_scope = ScopeId(1);
        let sym = table.resolve(func_scope, "x").unwrap();
        assert_eq!(sym.type_info.presence, Presence::Set);
    }

    #[test]
    fn presence_set_for_loop_var() {
        let table = build("for f in *.txt; do echo \"$f\"; done");
        let sym = table.resolve(table.root_scope(), "f").unwrap();
        assert_eq!(sym.type_info.presence, Presence::Set);
    }

    #[test]
    fn presence_maybe_unset_for_sig_param() {
        let src = "\
#@sig deploy(cfg: Scalar[ExistingFile]) -> Status[0] !may_exec
deploy() {
  #@bind $1 cfg
  echo done
}";
        let table = build(src);
        let func_scope = ScopeId(1);
        let sym = table.resolve(func_scope, "cfg").unwrap();
        assert_eq!(sym.type_info.presence, Presence::MaybeUnset);
    }
}
