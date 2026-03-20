use serde::Serialize;

use argvtype_syntax::hir::*;

/// Unique identifier for a basic block within a CFG.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct BlockId(pub u32);

/// The kind of edge between basic blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub enum EdgeKind {
    /// Unconditional fall-through to the next block.
    Unconditional,
    /// Taken when a condition is true (if-then, loop body entry).
    ConditionalTrue,
    /// Taken when a condition is false (if-else, loop exit).
    ConditionalFalse,
    /// Back-edge to a loop header.
    LoopBack,
}

/// A basic block: a straight-line sequence of HIR nodes with edges to successors.
#[derive(Debug, Clone, Serialize)]
pub struct BasicBlock {
    pub id: BlockId,
    /// HIR NodeIds of statements in this block, in execution order.
    pub nodes: Vec<NodeId>,
    pub successors: Vec<(BlockId, EdgeKind)>,
    pub predecessors: Vec<BlockId>,
}

/// Control flow graph for a function body or top-level script.
#[derive(Debug, Clone, Serialize)]
pub struct Cfg {
    blocks: Vec<BasicBlock>,
    pub entry: BlockId,
    pub exit: BlockId,
}

impl Cfg {
    pub fn block(&self, id: BlockId) -> &BasicBlock {
        &self.blocks[id.0 as usize]
    }

    pub fn blocks(&self) -> &[BasicBlock] {
        &self.blocks
    }

    pub fn block_count(&self) -> usize {
        self.blocks.len()
    }

    /// Returns block IDs in reverse postorder (useful for forward dataflow).
    pub fn reverse_postorder(&self) -> Vec<BlockId> {
        let mut visited = vec![false; self.blocks.len()];
        let mut order = Vec::new();
        self.dfs_postorder(self.entry, &mut visited, &mut order);
        order.reverse();
        order
    }

    fn dfs_postorder(&self, block: BlockId, visited: &mut [bool], order: &mut Vec<BlockId>) {
        if visited[block.0 as usize] {
            return;
        }
        visited[block.0 as usize] = true;
        for &(succ, _) in &self.blocks[block.0 as usize].successors {
            self.dfs_postorder(succ, visited, order);
        }
        order.push(block);
    }
}

struct CfgBuilder {
    blocks: Vec<BasicBlock>,
}

impl CfgBuilder {
    fn new() -> Self {
        CfgBuilder { blocks: Vec::new() }
    }

    fn new_block(&mut self) -> BlockId {
        let id = BlockId(self.blocks.len() as u32);
        self.blocks.push(BasicBlock {
            id,
            nodes: Vec::new(),
            successors: Vec::new(),
            predecessors: Vec::new(),
        });
        id
    }

    fn add_edge(&mut self, from: BlockId, to: BlockId, kind: EdgeKind) {
        self.blocks[from.0 as usize].successors.push((to, kind));
        self.blocks[to.0 as usize].predecessors.push(from);
    }

    fn add_node(&mut self, block: BlockId, node: NodeId) {
        self.blocks[block.0 as usize].nodes.push(node);
    }

    fn build_stmts(&mut self, stmts: &[Statement], mut current: BlockId) -> BlockId {
        for stmt in stmts {
            current = self.build_stmt(stmt, current);
        }
        current
    }

    fn build_stmt(&mut self, stmt: &Statement, mut current: BlockId) -> BlockId {
        match stmt {
            Statement::Assignment(a) => {
                self.add_node(current, a.id);
                current
            }
            Statement::Command(cmd) => {
                self.add_node(current, cmd.id);
                current
            }
            Statement::SourceCommand(src) => {
                self.add_node(current, src.id);
                current
            }
            Statement::Pipeline(p) => {
                self.add_node(current, p.id);
                current
            }
            Statement::Unmodeled(u) => {
                self.add_node(current, u.id);
                current
            }
            Statement::If(if_stmt) => {
                // Condition is evaluated in the current flow
                let cond_exit = self.build_stmts(&if_stmt.condition, current);

                let then_entry = self.new_block();
                self.add_edge(cond_exit, then_entry, EdgeKind::ConditionalTrue);
                let then_exit = self.build_stmts(&if_stmt.then_body, then_entry);

                let join = self.new_block();
                self.add_edge(then_exit, join, EdgeKind::Unconditional);

                if let Some(else_body) = &if_stmt.else_body {
                    let else_entry = self.new_block();
                    self.add_edge(cond_exit, else_entry, EdgeKind::ConditionalFalse);
                    let else_exit = self.build_stmts(else_body, else_entry);
                    self.add_edge(else_exit, join, EdgeKind::Unconditional);
                } else {
                    self.add_edge(cond_exit, join, EdgeKind::ConditionalFalse);
                }

                join
            }
            Statement::For(for_loop) => {
                let header = self.new_block();
                self.add_node(header, for_loop.id);
                self.add_edge(current, header, EdgeKind::Unconditional);

                let body_entry = self.new_block();
                self.add_edge(header, body_entry, EdgeKind::ConditionalTrue);
                let body_exit = self.build_stmts(&for_loop.body, body_entry);
                self.add_edge(body_exit, header, EdgeKind::LoopBack);

                let exit = self.new_block();
                self.add_edge(header, exit, EdgeKind::ConditionalFalse);
                exit
            }
            Statement::While(while_loop) => {
                let header = self.new_block();
                self.add_edge(current, header, EdgeKind::Unconditional);
                let cond_exit = self.build_stmts(&while_loop.condition, header);

                let body_entry = self.new_block();
                self.add_edge(cond_exit, body_entry, EdgeKind::ConditionalTrue);
                let body_exit = self.build_stmts(&while_loop.body, body_entry);
                self.add_edge(body_exit, header, EdgeKind::LoopBack);

                let exit = self.new_block();
                self.add_edge(cond_exit, exit, EdgeKind::ConditionalFalse);
                exit
            }
            Statement::Case(case_stmt) => {
                self.add_node(current, case_stmt.id);

                let join = self.new_block();
                for arm in &case_stmt.arms {
                    let arm_entry = self.new_block();
                    self.add_edge(current, arm_entry, EdgeKind::ConditionalTrue);
                    let arm_exit = self.build_stmts(&arm.body, arm_entry);
                    self.add_edge(arm_exit, join, EdgeKind::Unconditional);
                }

                let has_default = case_stmt.arms.iter().any(|arm| {
                    arm.patterns.iter().any(|p| {
                        p.segments
                            .first()
                            .is_some_and(|s| matches!(s, WordSegment::Literal(l) if l == "*"))
                    })
                });
                if !has_default {
                    self.add_edge(current, join, EdgeKind::ConditionalFalse);
                }

                join
            }
            Statement::List(list) => {
                // Model &&/|| as conditional edges. Note: for complex chains like
                // `a && b || c`, the short-circuit targets are approximate — the
                // failure edge from `a &&` goes to the list's join block rather than
                // to `c`. This is a conservative approximation that can be refined later.
                let join = self.new_block();

                for (i, elem) in list.elements.iter().enumerate() {
                    current = self.build_stmt(&elem.statement, current);
                    let is_last = i + 1 >= list.elements.len();
                    if !is_last {
                        match elem.operator {
                            Some(ListOperator::And) => {
                                let next = self.new_block();
                                self.add_edge(current, next, EdgeKind::ConditionalTrue);
                                self.add_edge(current, join, EdgeKind::ConditionalFalse);
                                current = next;
                            }
                            Some(ListOperator::Or) => {
                                let next = self.new_block();
                                self.add_edge(current, next, EdgeKind::ConditionalFalse);
                                self.add_edge(current, join, EdgeKind::ConditionalTrue);
                                current = next;
                            }
                            _ => {} // Semi or None: sequential, stay in current block
                        }
                    }
                }

                self.add_edge(current, join, EdgeKind::Unconditional);
                join
            }
            Statement::Block(b) => self.build_stmts(&b.body, current),
            _ => current,
        }
    }
}

/// Build a CFG from a sequence of HIR statements (function body or top-level).
pub fn build_cfg(stmts: &[Statement]) -> Cfg {
    let mut builder = CfgBuilder::new();
    let entry = builder.new_block();
    let exit = builder.new_block();

    let last = builder.build_stmts(stmts, entry);
    builder.add_edge(last, exit, EdgeKind::Unconditional);

    Cfg {
        blocks: builder.blocks,
        entry,
        exit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use argvtype_syntax::lower::parse_and_lower;
    use argvtype_syntax::span::{SourceFile, SourceId};

    fn build_func_cfg(src: &str) -> Cfg {
        let source = SourceFile::new(SourceId(0), "test.sh".into(), src.into());
        let result = parse_and_lower(source);
        let func = result
            .source_unit
            .items
            .iter()
            .find_map(|item| match item {
                Item::Function(f) => Some(f),
                _ => None,
            })
            .unwrap();
        build_cfg(&func.body)
    }

    fn build_script_cfg(src: &str) -> Cfg {
        let source = SourceFile::new(SourceId(0), "test.sh".into(), src.into());
        let result = parse_and_lower(source);
        let stmts: Vec<Statement> = result
            .source_unit
            .items
            .into_iter()
            .filter_map(|item| match item {
                Item::Statement(s) => Some(s),
                _ => None,
            })
            .collect();
        build_cfg(&stmts)
    }

    fn successor_kinds(cfg: &Cfg, block: BlockId) -> Vec<EdgeKind> {
        cfg.block(block).successors.iter().map(|&(_, k)| k).collect()
    }

    fn has_edge(cfg: &Cfg, from: BlockId, to: BlockId, kind: EdgeKind) -> bool {
        cfg.block(from)
            .successors
            .iter()
            .any(|&(t, k)| t == to && k == kind)
    }

    #[test]
    fn straight_line_code() {
        let cfg = build_script_cfg("x=hello\necho \"$x\"");
        // entry block (0) has both statements, exit block (1)
        assert_eq!(cfg.block_count(), 2);
        assert_eq!(cfg.block(cfg.entry).nodes.len(), 2);
        assert!(has_edge(&cfg, cfg.entry, cfg.exit, EdgeKind::Unconditional));
    }

    #[test]
    fn if_else_structure() {
        let cfg = build_func_cfg("foo() {\n  if true; then\n    echo a\n  else\n    echo b\n  fi\n}");
        // entry(0), exit(1), then_entry(2), join(3), else_entry(4)
        assert!(cfg.block_count() >= 5);
        // Entry block has condition (true command)
        let entry = cfg.entry;
        let succs = successor_kinds(&cfg, entry);
        assert!(succs.contains(&EdgeKind::ConditionalTrue));
        assert!(succs.contains(&EdgeKind::ConditionalFalse));
    }

    #[test]
    fn if_no_else() {
        let cfg = build_func_cfg("foo() {\n  if true; then\n    echo a\n  fi\n}");
        let entry = cfg.entry;
        let succs = successor_kinds(&cfg, entry);
        // ConditionalTrue → then block, ConditionalFalse → join block
        assert!(succs.contains(&EdgeKind::ConditionalTrue));
        assert!(succs.contains(&EdgeKind::ConditionalFalse));
    }

    #[test]
    fn for_loop_has_back_edge() {
        let cfg = build_func_cfg("foo() {\n  for f in *.txt; do\n    echo \"$f\"\n  done\n}");
        // Should have a LoopBack edge somewhere
        let has_loop_back = cfg
            .blocks()
            .iter()
            .any(|b| b.successors.iter().any(|&(_, k)| k == EdgeKind::LoopBack));
        assert!(has_loop_back, "for loop should have a LoopBack edge");
    }

    #[test]
    fn for_loop_structure() {
        let cfg = build_func_cfg("foo() {\n  for f in *.txt; do\n    echo \"$f\"\n  done\n}");
        // entry(0) → header(2) [Unconditional]
        // header(2) → body(3) [ConditionalTrue], header(2) → loop_exit(4) [ConditionalFalse]
        // body(3) → header(2) [LoopBack]
        // loop_exit(4) → exit(1) [Unconditional]
        let header = BlockId(2);
        let succs = successor_kinds(&cfg, header);
        assert!(succs.contains(&EdgeKind::ConditionalTrue));
        assert!(succs.contains(&EdgeKind::ConditionalFalse));
    }

    #[test]
    fn while_loop_has_back_edge() {
        let cfg = build_func_cfg("foo() {\n  while true; do\n    echo loop\n  done\n}");
        let has_loop_back = cfg
            .blocks()
            .iter()
            .any(|b| b.successors.iter().any(|&(_, k)| k == EdgeKind::LoopBack));
        assert!(has_loop_back, "while loop should have a LoopBack edge");
    }

    #[test]
    fn case_with_default_no_false_edge() {
        let cfg = build_func_cfg(
            "foo() {\n  case \"$1\" in\n    a) echo a ;;\n    *) echo default ;;\n  esac\n}",
        );
        // With a default arm, there should be no ConditionalFalse from the case block
        let entry = cfg.entry;
        let succs = successor_kinds(&cfg, entry);
        assert!(
            !succs.contains(&EdgeKind::ConditionalFalse),
            "case with default should not have ConditionalFalse edge"
        );
    }

    #[test]
    fn case_without_default_has_false_edge() {
        let cfg = build_func_cfg(
            "foo() {\n  case \"$1\" in\n    a) echo a ;;\n    b) echo b ;;\n  esac\n}",
        );
        let entry = cfg.entry;
        let succs = successor_kinds(&cfg, entry);
        assert!(
            succs.contains(&EdgeKind::ConditionalFalse),
            "case without default should have ConditionalFalse edge for no-match path"
        );
    }

    #[test]
    fn list_and_operator() {
        let cfg = build_script_cfg("cd /tmp && echo ok");
        // entry has cd, ConditionalTrue → block with echo, ConditionalFalse → join
        let entry = cfg.entry;
        let succs = successor_kinds(&cfg, entry);
        assert!(succs.contains(&EdgeKind::ConditionalTrue));
        assert!(succs.contains(&EdgeKind::ConditionalFalse));
    }

    #[test]
    fn list_or_operator() {
        let cfg = build_script_cfg("cd /tmp || exit 1");
        let entry = cfg.entry;
        let succs = successor_kinds(&cfg, entry);
        // || : ConditionalFalse → next cmd, ConditionalTrue → join (skip)
        assert!(succs.contains(&EdgeKind::ConditionalTrue));
        assert!(succs.contains(&EdgeKind::ConditionalFalse));
    }

    #[test]
    fn reverse_postorder_visits_entry_first() {
        let cfg = build_script_cfg("x=1\necho \"$x\"");
        let rpo = cfg.reverse_postorder();
        assert_eq!(rpo[0], cfg.entry);
    }

    #[test]
    fn reverse_postorder_visits_all_reachable() {
        let cfg = build_func_cfg("foo() {\n  if true; then\n    echo a\n  else\n    echo b\n  fi\n}");
        let rpo = cfg.reverse_postorder();
        // Should visit all blocks (entry, then, else, join, exit)
        assert!(rpo.len() >= 5);
    }

    #[test]
    fn nested_if_in_loop() {
        let cfg = build_func_cfg(
            "foo() {\n  for f in *.txt; do\n    if [[ -f \"$f\" ]]; then\n      echo found\n    fi\n  done\n}",
        );
        // Should have both LoopBack and ConditionalTrue/False edges
        let has_loop_back = cfg
            .blocks()
            .iter()
            .any(|b| b.successors.iter().any(|&(_, k)| k == EdgeKind::LoopBack));
        let has_conditional = cfg
            .blocks()
            .iter()
            .any(|b| b.successors.iter().any(|&(_, k)| k == EdgeKind::ConditionalTrue));
        assert!(has_loop_back);
        assert!(has_conditional);
    }
}
