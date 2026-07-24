//! Node→bytecode emitter (extracted from mod.rs).
use super::*;

/// Lower a compiled `Node` body to a [`Chunk`], or `None` if it uses any node
/// outside Stage 1's vocabulary (`Call`/`SelfCall`/`MakeClosure`, or a `Const` with
/// a movable RUNTIME handle). `None` is always safe — the arm runs on `exec_node`.
pub(crate) fn compile_chunk(body: &Node) -> Option<Chunk> {
    let mut code = Vec::new();
    emit_node(body, &mut code)?;
    Some(Chunk { code })
}

/// Recursively emit `node` into `code`, leaving its value on the operand stack.
/// Returns `None` (aborting the whole chunk) on any unsupported node.
pub(crate) fn emit_node(node: &Node, code: &mut Vec<Inst>) -> Option<()> {
    match node {
        // A fresh `ConstVal` cloned from the node's (atoms inline; a movable RUNTIME
        // handle is re-encoded). Chunk handles are rewritten in place under a RUNTIME
        // compaction by `rewrite_chunk` (registered via the arm's `has_runtime_handles`).
        Node::Const(cv) => code.push(Inst::Const(ConstVal::new(cv.load()))),
        Node::Local(i) => code.push(Inst::Local(*i)),
        Node::Global(s) => code.push(Inst::Global(*s)),
        Node::GlobalIc { sym, site } => code.push(Inst::GlobalIc {
            sym: *sym,
            site: *site,
        }),
        Node::If(cond, then, els) => {
            emit_node(cond, code)?;
            let j_else = code.len();
            code.push(Inst::JumpIfFalse(0)); // backpatched
            emit_node(then, code)?;
            let j_end = code.len();
            code.push(Inst::Jump(0)); // backpatched
            let else_ip = code.len();
            emit_node(els, code)?;
            let end_ip = code.len();
            code[j_else] = Inst::JumpIfFalse(else_ip);
            code[j_end] = Inst::Jump(end_ip);
        }
        Node::Do(nodes) => {
            if nodes.is_empty() {
                code.push(Inst::Const(ConstVal::Atom(Value::nil())));
            } else {
                let last = nodes.len() - 1;
                for n in &nodes[..last] {
                    emit_node(n, code)?;
                    code.push(Inst::Pop); // evaluated for effect
                }
                emit_node(&nodes[last], code)?;
            }
        }
        Node::LetBind { binds, body } => {
            for (slot, rhs) in binds.iter() {
                emit_node(rhs, code)?;
                code.push(Inst::SetLocal(*slot));
            }
            emit_node(body, code)?;
        }
        Node::Vector(elems) => {
            for e in elems.iter() {
                emit_node(e, code)?;
            }
            code.push(Inst::MakeVector(elems.len()));
        }
        Node::Map(entries) => {
            for (k, v) in entries.iter() {
                emit_node(k, code)?;
                emit_node(v, code)?;
            }
            code.push(Inst::MakeMap(entries.len()));
        }
        Node::Prim1 {
            op,
            a,
            head,
            guard,
            pos,
        } => {
            emit_node(a, code)?;
            code.push(Inst::Prim1 {
                op: *op,
                head: *head,
                guard: AtomicU64::new(guard.load(Ordering::Relaxed)),
                pos: *pos,
            });
        }
        Node::Prim2 {
            op,
            a,
            b,
            map,
            head,
            guard,
            pos,
            broot: _,
        } => {
            // Snapshot the guard epoch; each push site creates its own AtomicU64
            // (AtomicU64 is not Copy so we can't reuse a single binding).
            let gv = guard.load(Ordering::Relaxed);
            // Fuse when operands are frame locals or integer literals: avoids
            // the 2 intermediate root-stack pushes the generic path needs.
            // Only integer constants are fused (keeps Prim2SlotInt below
            // MakeClosure's size, so the Inst enum doesn't grow).
            let fused = match (&**a, &**b) {
                (Node::Local(sa), Node::Local(sb)) => {
                    code.push(Inst::Prim2SlotSlot {
                        op: *op,
                        map: *map,
                        slot_a: *sa,
                        slot_b: *sb,
                        head: *head,
                        guard: AtomicU64::new(gv),
                        pos: *pos,
                    });
                    true
                }
                (Node::Local(sa), Node::Const(cv)) => {
                    if let ValueRef::Int(n) = cv.load().unpack() {
                        code.push(Inst::Prim2SlotInt {
                            op: *op,
                            map: *map,
                            slot_a: *sa,
                            int_b: n,
                            swapped: false,
                            head: *head,
                            guard: AtomicU64::new(gv),
                            pos: *pos,
                        });
                        true
                    } else {
                        false
                    }
                }
                (Node::Const(cv), Node::Local(sb)) => {
                    if let ValueRef::Int(n) = cv.load().unpack() {
                        // Slot goes to src[0], const to src[1] — invert the map. `swapped`
                        // so the dispatch fallback restores the original `(op Const Local)`
                        // order when it calls the user `head` (the inline path uses `map`).
                        let new_map = [1u8 - map[0], 1u8 - map[1]];
                        code.push(Inst::Prim2SlotInt {
                            op: *op,
                            map: new_map,
                            slot_a: *sb,
                            int_b: n,
                            swapped: true,
                            head: *head,
                            guard: AtomicU64::new(gv),
                            pos: *pos,
                        });
                        true
                    } else {
                        false
                    }
                }
                _ => false,
            };
            if !fused {
                emit_node(a, code)?;
                emit_node(b, code)?;
                code.push(Inst::Prim2 {
                    op: *op,
                    map: *map,
                    head: *head,
                    guard: AtomicU64::new(gv),
                    pos: *pos,
                });
            }
        }
        Node::Prim3 {
            op,
            a,
            b,
            c,
            head,
            guard,
            pos,
        } => {
            // No fused variants (one member, `table-put` — the operand-stack form is
            // already one inst); operands push in source order, the inst pops three.
            emit_node(a, code)?;
            emit_node(b, code)?;
            emit_node(c, code)?;
            code.push(Inst::Prim3 {
                op: *op,
                head: *head,
                guard: AtomicU64::new(guard.load(Ordering::Relaxed)),
                pos: *pos,
            });
        }
        Node::Call {
            callee,
            args,
            tail,
            pos,
            file: _,
            site,
        } => {
            // Callee first, then each arg (the order `exec_call` evaluates them). When
            // the head is a free global, carry its symbol + `site` so the call-site IC
            // can cache the resolved arm (Stage 5); the callee is still pushed and
            // resolved in-order, so the IC is a pure cache.
            let head = if let Node::Global(s) = &**callee {
                Some(*s)
            } else {
                None
            };
            // A free-global head is NOT staged: `Inst::Call` resolves it through the call IC
            // (or `env_get` on a miss), so there's no redundant head-`Global` push + per-call
            // `env_get`. A computed callee (head `None`) is staged below the args as before.
            if head.is_none() {
                emit_node(callee, code)?;
            }
            for a in args.iter() {
                emit_node(a, code)?;
            }
            code.push(Inst::Call {
                argc: args.len(),
                tail: *tail,
                pos: *pos,
                site: *site,
                head,
            });
        }
        Node::SelfCall { args, pos: _ } => {
            for a in args.iter() {
                emit_node(a, code)?;
            }
            code.push(Inst::SelfCall { argc: args.len() });
        }
        Node::MakeClosure {
            fn_rest,
            captures,
            self_name,
        } => {
            // Capture sources are leaf reads (an enclosing lexical → `Local`, or a
            // global → `Global`), so emitting them is safepoint-free; their values
            // land on the operand stack in `captures` order and `MakeClosure` binds
            // them to the matching names. A fresh `ConstVal` re-encodes `fn_rest`
            // (rewritten in place by `rewrite_chunk` under a compaction).
            for (_, src) in captures.iter() {
                emit_node(src, code)?;
            }
            let names: Box<[Symbol]> = captures.iter().map(|(name, _)| *name).collect();
            code.push(Inst::MakeClosure {
                fn_rest: ConstVal::new(fn_rest.load()),
                names,
                self_name: *self_name,
            });
        }
        Node::TryCatch {
            body,
            bind_slot,
            handler,
        } => {
            code.push(Inst::TryCatch {
                body: NodePtr(NonNull::from(body.as_ref())),
                bind_slot: *bind_slot,
                handler: NodePtr(NonNull::from(handler.as_ref())),
            });
        }
    }
    Some(())
}

