use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use argvtype_syntax::hir::*;
use argvtype_syntax::lower::{parse_and_lower, LowerResult};
use argvtype_syntax::span::{SourceFile, SourceId, Span};

use crate::diagnostic::{Diagnostic, DiagnosticCode};
use crate::scope::{build_symbol_table, Symbol, SymbolTable};

const BT701: DiagnosticCode = DiagnosticCode { family: "BT", number: 701 };
const BT702: DiagnosticCode = DiagnosticCode { family: "BT", number: 702 };

/// A resolved source dependency edge.
#[derive(Debug, Clone)]
pub struct SourceEdge {
    /// The resolved absolute path of the sourced file.
    pub target: PathBuf,
    /// Span of the `source`/`.` command in the sourcing file.
    pub span: Span,
    /// Whether this was a dynamic source (contains expansions).
    pub dynamic: bool,
}

/// A node in the source graph — one parsed+lowered file.
pub struct SourceNode {
    pub path: PathBuf,
    pub source_id: SourceId,
    pub lower_result: LowerResult,
    pub symbols: SymbolTable,
    pub edges: Vec<SourceEdge>,
}

/// The source dependency graph for a set of shell scripts.
pub struct SourceGraph {
    nodes: HashMap<PathBuf, SourceNode>,
    /// Topological order (sourced files before sourcing files).
    topo_order: Vec<PathBuf>,
    /// Diagnostics produced during graph construction.
    diagnostics: Vec<(PathBuf, Diagnostic)>,
}

impl SourceGraph {
    /// Build a source graph starting from the given entry files.
    pub fn build(entry_paths: &[PathBuf]) -> Self {
        let mut builder = SourceGraphBuilder::new();
        for path in entry_paths {
            if let Ok(abs) = std::fs::canonicalize(path) {
                builder.add_file(&abs);
            }
        }
        builder.finish()
    }

    /// Returns files in topological order (dependencies before dependents).
    pub fn topo_order(&self) -> &[PathBuf] {
        &self.topo_order
    }

    /// Get a node by path.
    pub fn node(&self, path: &Path) -> Option<&SourceNode> {
        self.nodes.get(path)
    }

    /// Returns all diagnostics (BT701/BT702) produced during graph construction.
    pub fn diagnostics(&self) -> &[(PathBuf, Diagnostic)] {
        &self.diagnostics
    }

    /// Collect exported symbols from all files that `path` transitively sources.
    /// Returns symbols from sourced files that should be visible in the sourcing file.
    pub fn imported_symbols(&self, path: &Path) -> Vec<&Symbol> {
        let mut result = Vec::new();
        let mut visited = HashSet::new();
        self.collect_imported_symbols(path, &mut result, &mut visited);
        result
    }

    fn collect_imported_symbols<'a>(
        &'a self,
        path: &Path,
        result: &mut Vec<&'a Symbol>,
        visited: &mut HashSet<PathBuf>,
    ) {
        if !visited.insert(path.to_path_buf()) {
            return;
        }
        if let Some(node) = self.nodes.get(path) {
            for edge in &node.edges {
                if !edge.dynamic {
                    // Recurse: sourced file's own imports come first
                    self.collect_imported_symbols(&edge.target, result, visited);
                    // Then add the sourced file's global symbols
                    if let Some(target_node) = self.nodes.get(&edge.target) {
                        result.extend(target_node.symbols.global_symbols());
                    }
                }
            }
        }
    }
}

struct SourceGraphBuilder {
    nodes: HashMap<PathBuf, SourceNode>,
    next_id: u32,
    diagnostics: Vec<(PathBuf, Diagnostic)>,
}

impl SourceGraphBuilder {
    fn new() -> Self {
        SourceGraphBuilder {
            nodes: HashMap::new(),
            next_id: 0,
            diagnostics: Vec::new(),
        }
    }

    fn alloc_id(&mut self) -> SourceId {
        let id = SourceId(self.next_id);
        self.next_id += 1;
        id
    }

    fn add_file(&mut self, abs_path: &Path) {
        if self.nodes.contains_key(abs_path) {
            return;
        }

        let source_text = match std::fs::read_to_string(abs_path) {
            Ok(s) => s,
            Err(_) => return,
        };

        let source_id = self.alloc_id();
        let source = SourceFile::new(
            source_id,
            abs_path.to_string_lossy().to_string(),
            source_text,
        );
        let lower_result = parse_and_lower(source);
        let symbols = build_symbol_table(&lower_result.source_unit);

        // Extract source edges
        let edges = self.extract_source_edges(abs_path, &lower_result.source_unit);

        // Insert the node (before resolving targets to avoid borrow issues)
        self.nodes.insert(
            abs_path.to_path_buf(),
            SourceNode {
                path: abs_path.to_path_buf(),
                source_id,
                lower_result,
                symbols,
                edges,
            },
        );

        // Recursively add sourced files
        let targets: Vec<(PathBuf, Span, bool)> = self.nodes[abs_path]
            .edges
            .iter()
            .filter(|e| !e.dynamic)
            .map(|e| (e.target.clone(), e.span, e.dynamic))
            .collect();

        for (target_path, span, _) in targets {
            if target_path.exists() {
                self.add_file(&target_path);
            } else {
                let source_id = self.nodes[abs_path].source_id;
                self.diagnostics.push((
                    abs_path.to_path_buf(),
                    Diagnostic::warning(
                        BT701,
                        format!(
                            "source target not found: {}",
                            target_path.display()
                        ),
                        source_id,
                        span,
                    )
                    .with_help("the sourced file does not exist or cannot be resolved"),
                ));
            }
        }
    }

    fn extract_source_edges(&self, file_path: &Path, source_unit: &SourceUnit) -> Vec<SourceEdge> {
        let dir = file_path.parent().unwrap_or(Path::new("."));
        let mut edges = Vec::new();
        Self::walk_items_for_sources(&source_unit.items, dir, &mut edges);
        edges
    }

    fn walk_items_for_sources(items: &[Item], dir: &Path, edges: &mut Vec<SourceEdge>) {
        for item in items {
            match item {
                Item::Function(f) => {
                    Self::walk_stmts_for_sources(&f.body, dir, edges);
                }
                Item::Statement(s) => {
                    Self::walk_stmt_for_sources(s, dir, edges);
                }
                _ => {}
            }
        }
    }

    fn walk_stmts_for_sources(stmts: &[Statement], dir: &Path, edges: &mut Vec<SourceEdge>) {
        for stmt in stmts {
            Self::walk_stmt_for_sources(stmt, dir, edges);
        }
    }

    fn walk_stmt_for_sources(stmt: &Statement, dir: &Path, edges: &mut Vec<SourceEdge>) {
        match stmt {
            Statement::SourceCommand(src) => {
                let target = if src.dynamic {
                    // Dynamic source — can't resolve at analysis time
                    PathBuf::new()
                } else if let Some(lit) = src.path.literal_str() {
                    let p = Path::new(lit);
                    if p.is_absolute() {
                        p.to_path_buf()
                    } else {
                        dir.join(p)
                    }
                } else {
                    PathBuf::new()
                };

                // Canonicalize if it exists (handles symlinks, ..)
                let target = if !src.dynamic && target.exists() {
                    std::fs::canonicalize(&target).unwrap_or(target)
                } else {
                    target
                };

                edges.push(SourceEdge {
                    target,
                    span: src.span,
                    dynamic: src.dynamic,
                });
            }
            Statement::If(if_stmt) => {
                Self::walk_stmts_for_sources(&if_stmt.condition, dir, edges);
                Self::walk_stmts_for_sources(&if_stmt.then_body, dir, edges);
                if let Some(else_body) = &if_stmt.else_body {
                    Self::walk_stmts_for_sources(else_body, dir, edges);
                }
            }
            Statement::For(for_loop) => {
                Self::walk_stmts_for_sources(&for_loop.body, dir, edges);
            }
            Statement::While(while_loop) => {
                Self::walk_stmts_for_sources(&while_loop.condition, dir, edges);
                Self::walk_stmts_for_sources(&while_loop.body, dir, edges);
            }
            Statement::Case(case_stmt) => {
                for arm in &case_stmt.arms {
                    Self::walk_stmts_for_sources(&arm.body, dir, edges);
                }
            }
            Statement::List(list) => {
                for elem in &list.elements {
                    Self::walk_stmt_for_sources(&elem.statement, dir, edges);
                }
            }
            Statement::Pipeline(p) => {
                Self::walk_stmts_for_sources(&p.commands, dir, edges);
            }
            Statement::Block(b) => {
                Self::walk_stmts_for_sources(&b.body, dir, edges);
            }
            _ => {}
        }
    }

    fn finish(mut self) -> SourceGraph {
        // Detect cycles and compute topological order
        let topo_order = self.compute_topo_order();

        SourceGraph {
            nodes: self.nodes,
            topo_order,
            diagnostics: self.diagnostics,
        }
    }

    fn compute_topo_order(&mut self) -> Vec<PathBuf> {
        let mut visited = HashSet::new();
        let mut in_stack = HashSet::new();
        let mut order = Vec::new();

        let paths: Vec<PathBuf> = self.nodes.keys().cloned().collect();
        for path in &paths {
            self.topo_visit(path, &mut visited, &mut in_stack, &mut order);
        }

        order
    }

    fn topo_visit(
        &mut self,
        path: &Path,
        visited: &mut HashSet<PathBuf>,
        in_stack: &mut HashSet<PathBuf>,
        order: &mut Vec<PathBuf>,
    ) {
        if visited.contains(path) {
            return;
        }
        if in_stack.contains(path) {
            // Cycle detected — emit BT702
            if let Some(node) = self.nodes.get(path) {
                let source_id = node.source_id;
                // Find the edge that creates the cycle
                let cycle_span = node
                    .edges
                    .iter()
                    .find(|e| !e.dynamic && in_stack.contains(&e.target))
                    .map(|e| e.span)
                    .unwrap_or(Span::new(0, 0));
                self.diagnostics.push((
                    path.to_path_buf(),
                    Diagnostic::error(
                        BT702,
                        format!(
                            "circular source dependency involving {}",
                            path.file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| path.to_string_lossy().to_string())
                        ),
                        source_id,
                        cycle_span,
                    )
                    .with_help("break the cycle by removing one of the mutual source commands"),
                ));
            }
            return;
        }

        in_stack.insert(path.to_path_buf());

        // Collect targets before recursive call
        let targets: Vec<PathBuf> = self
            .nodes
            .get(path)
            .map(|n| {
                n.edges
                    .iter()
                    .filter(|e| !e.dynamic && self.nodes.contains_key(&e.target))
                    .map(|e| e.target.clone())
                    .collect()
            })
            .unwrap_or_default();

        for target in targets {
            self.topo_visit(&target, visited, in_stack, order);
        }

        in_stack.remove(path);
        visited.insert(path.to_path_buf());
        order.push(path.to_path_buf());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write_file(dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, content).unwrap();
        fs::canonicalize(&path).unwrap()
    }

    #[test]
    fn single_file_no_sources() {
        let tmp = TempDir::new().unwrap();
        let main = write_file(tmp.path(), "main.sh", "echo hello");
        let graph = SourceGraph::build(&[main.clone()]);
        assert_eq!(graph.topo_order().len(), 1);
        assert!(graph.diagnostics().is_empty());
    }

    #[test]
    fn simple_source_chain() {
        let tmp = TempDir::new().unwrap();
        let lib = write_file(tmp.path(), "lib.sh", "helper() { echo hi; }");
        let main = write_file(tmp.path(), "main.sh", "source lib.sh\nhelper");
        let graph = SourceGraph::build(&[main.clone()]);
        assert_eq!(graph.topo_order().len(), 2);
        // lib.sh should come before main.sh in topo order
        let lib_idx = graph.topo_order().iter().position(|p| p == &lib).unwrap();
        let main_idx = graph.topo_order().iter().position(|p| p == &main).unwrap();
        assert!(lib_idx < main_idx);
        assert!(graph.diagnostics().is_empty());
    }

    #[test]
    fn dot_source_syntax() {
        let tmp = TempDir::new().unwrap();
        let _lib = write_file(tmp.path(), "utils.sh", "x=1");
        let main = write_file(tmp.path(), "main.sh", ". utils.sh\necho $x");
        let graph = SourceGraph::build(&[main.clone()]);
        assert_eq!(graph.topo_order().len(), 2);
        assert!(graph.diagnostics().is_empty());
    }

    #[test]
    fn missing_source_target_emits_bt701() {
        let tmp = TempDir::new().unwrap();
        let main = write_file(tmp.path(), "main.sh", "source nonexistent.sh");
        let graph = SourceGraph::build(&[main.clone()]);
        assert_eq!(graph.diagnostics().len(), 1);
        let (_, diag) = &graph.diagnostics()[0];
        assert_eq!(diag.code.number, 701);
    }

    #[test]
    fn circular_dependency_emits_bt702() {
        let tmp = TempDir::new().unwrap();
        let _a = write_file(tmp.path(), "a.sh", "source b.sh");
        let _b = write_file(tmp.path(), "b.sh", "source a.sh");
        let a_path = tmp.path().join("a.sh");
        let graph = SourceGraph::build(&[fs::canonicalize(&a_path).unwrap()]);
        let bt702 = graph.diagnostics().iter().any(|(_, d)| d.code.number == 702);
        assert!(bt702, "should emit BT702 for circular dependency");
    }

    #[test]
    fn dynamic_source_not_resolved() {
        let tmp = TempDir::new().unwrap();
        let main = write_file(tmp.path(), "main.sh", "source \"$MYLIB\"");
        let graph = SourceGraph::build(&[main.clone()]);
        assert_eq!(graph.topo_order().len(), 1);
        // No BT701 for dynamic sources — they're soundness boundaries, not errors
        let bt701 = graph.diagnostics().iter().any(|(_, d)| d.code.number == 701);
        assert!(!bt701);
    }

    #[test]
    fn transitive_source_chain() {
        let tmp = TempDir::new().unwrap();
        let _c = write_file(tmp.path(), "c.sh", "z=1");
        let _b = write_file(tmp.path(), "b.sh", "source c.sh\ny=1");
        let main = write_file(tmp.path(), "main.sh", "source b.sh\necho $y $z");
        let graph = SourceGraph::build(&[main.clone()]);
        assert_eq!(graph.topo_order().len(), 3);
    }

    #[test]
    fn imported_symbols_from_sourced_file() {
        let tmp = TempDir::new().unwrap();
        write_file(tmp.path(), "lib.sh", "MY_VAR=hello");
        let main = write_file(tmp.path(), "main.sh", "source lib.sh\necho $MY_VAR");
        let graph = SourceGraph::build(&[main.clone()]);
        let imported = graph.imported_symbols(&main);
        assert!(imported.iter().any(|s| s.name == "MY_VAR"));
    }
}
