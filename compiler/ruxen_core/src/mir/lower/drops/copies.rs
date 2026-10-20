//! Heap-boxed Copy values: a Copy tuple, struct or class is copied as a
//! value in the language but lives in a heap box, so every owner it gains
//! gets its own box. Without this, `b = a` or `arr.push(a)` on a Copy value
//! left two owners of one box and both freed it.

use super::*;

/// True when `ty` is a Copy type stored in a heap box its owner frees: a
/// tuple, struct or class whose fields are all Copy. Enums are left out:
/// their drop never frees a payload, so a shared box is never freed twice.
pub(super) fn is_boxed_copy(ty: &Ty, symbols: &SymbolTable) -> bool {
    matches!(ty, Ty::Tuple(_) | Ty::Struct { .. } | Ty::Class { .. })
        && ty_is_effectively_copy(ty, symbols)
}

impl<'a> Lowerer<'a> {
    /// Give a boxed Copy value ([`is_boxed_copy`]) its own box at every place
    /// it gains an owner: a `Copy` into another local, a `SetField` into an
    /// object or tuple (not into an enum, whose payload is never freed; a
    /// struct-typed field is never freed either, so its copy leaks rather
    /// than dangles), an argument a C callee takes by value ([`FfiByValue`]), and a
    /// returned value that is not this frame's own box (a parameter, a field
    /// or element read): the caller owns what it is returned. A fresh
    /// temporary consumed right there (built by an `Alloc` and read nowhere
    /// else) moves in as it is, and a borrow (a `Ref`, as a block's capture
    /// slot holds) is stored as it is. A closure's captures keep the
    /// ownership lowering gave them (`synthesize_closure_drop` decides which
    /// slots the closure frees and which the body moves out): a store into a
    /// captures box (`View::Closure`) and a closure body's return of a value
    /// it read from its captures are left alone. So a boxed Copy value never escapes as an
    /// alias: a callee keeps no argument, and a container's elements are
    /// never shared. Runs on every lowered body before the call summaries
    /// and drop elaboration read it.
    pub(in crate::mir::lower) fn duplicate_boxed_copies(
        &self,
        func: &mut MirFunction,
        ffi_by_value: &FfiByValue,
        lowered: &HashSet<String>,
    ) {
        let reads = value_reads(func);
        let allocated: HashSet<LocalId> = func
            .blocks
            .iter()
            .flat_map(|b| b.instructions.iter())
            .filter_map(|inst| match inst {
                MirInst::Alloc { dest, .. } => Some(*dest),
                _ => None,
            })
            .collect();
        // A `Ref` result is a borrow (a block's capture slot is filled
        // through one): storing it makes no owner.
        let borrowed: HashSet<LocalId> = func
            .blocks
            .iter()
            .flat_map(|b| b.instructions.iter())
            .filter_map(|inst| match inst {
                MirInst::Ref { dest, .. } => Some(*dest),
                _ => None,
            })
            .collect();
        let captures_boxes: HashSet<LocalId> = func
            .views
            .values()
            .filter_map(|view| match view {
                Some(View::Closure {
                    captures: Some(captures),
                    ..
                }) => Some(*captures),
                _ => None,
            })
            .collect();
        let consumed_fresh = |func: &MirFunction, local: LocalId| {
            borrowed.contains(&local)
                || func.locals[local as usize].kind == crate::mir::nodes::LocalKind::Temp
                    && allocated.contains(&local)
                    && reads.get(&local).copied().unwrap_or(0) == 1
        };
        for block in 0..func.blocks.len() {
            let instructions = std::mem::take(&mut func.blocks[block].instructions);
            let spans = std::mem::take(&mut func.blocks[block].inst_spans);
            let mut out = Vec::with_capacity(instructions.len());
            let mut out_spans = Vec::with_capacity(spans.len());
            for (index, mut inst) in instructions.into_iter().enumerate() {
                let span = spans.get(index).cloned().flatten();
                let mut copies = Vec::new();
                match &mut inst {
                    MirInst::Copy { dest, src } => {
                        let ty = func.locals[*src as usize].ty.clone();
                        if is_boxed_copy(&ty, self.symbols) {
                            self.emit_box_copy(func, &ty, *src, *dest, &mut copies);
                            for copy in copies {
                                out.push(copy);
                                out_spans.push(span.clone());
                            }
                            continue;
                        }
                    }
                    MirInst::SetField {
                        base,
                        value: MirValue::Use(value),
                        ..
                    } => {
                        let owner = &func.locals[*base as usize].ty;
                        let into_enum =
                            matches!(owner, Ty::Enum { .. } | Ty::Option(_) | Ty::Result(..))
                                || captures_boxes.contains(base);
                        if !into_enum {
                            if let Some(copy) =
                                self.owned_box_copy(func, *value, &consumed_fresh, &mut copies)
                            {
                                *value = copy;
                            }
                        }
                    }
                    MirInst::Call { callee, args, .. } => {
                        for (slot, arg) in args.iter_mut().enumerate() {
                            let MirValue::Use(value) = arg else { continue };
                            if !ffi_by_value.takes_by_value(callee, slot) {
                                continue;
                            }
                            if let Some(copy) =
                                self.owned_box_copy(func, *value, &consumed_fresh, &mut copies)
                            {
                                *value = copy;
                            }
                        }
                    }
                    _ => {}
                }
                for copy in copies {
                    out.push(copy);
                    out_spans.push(span.clone());
                }
                out.push(inst);
                out_spans.push(span);
            }
            func.blocks[block].instructions = out;
            func.blocks[block].inst_spans = out_spans;
        }
        let owned = own_boxes(func, lowered);
        let from_captures = capture_reads(func);
        for block in 0..func.blocks.len() {
            let Terminator::Return(Some(MirValue::Use(value))) = func.blocks[block].terminator
            else {
                continue;
            };
            let ty = func.locals[value as usize].ty.clone();
            if !is_boxed_copy(&ty, self.symbols)
                || owned.contains(&value)
                || from_captures.contains(&value)
            {
                continue;
            }
            let copy = func.new_temp(ty.clone());
            let mut copies = Vec::new();
            self.emit_box_copy(func, &ty, value, copy, &mut copies);
            for inst in copies {
                func.blocks[block].push_inst(inst, None);
            }
            func.blocks[block].terminator = Terminator::Return(Some(MirValue::Use(copy)));
        }
    }

    /// A new box copy of `value` when it is a boxed Copy value that is not a
    /// fresh temporary consumed here; its instructions go to `out`.
    fn owned_box_copy(
        &self,
        func: &mut MirFunction,
        value: LocalId,
        consumed_fresh: &dyn Fn(&MirFunction, LocalId) -> bool,
        out: &mut Vec<MirInst>,
    ) -> Option<LocalId> {
        let ty = func.locals[value as usize].ty.clone();
        if !is_boxed_copy(&ty, self.symbols) || consumed_fresh(func, value) {
            return None;
        }
        let copy = func.new_temp(ty.clone());
        self.emit_box_copy(func, &ty, value, copy, out);
        Some(copy)
    }

    /// Emit into `out` a new box `dest` holding a copy of the boxed Copy
    /// value `src` of type `ty`: every slot copied, and a tuple element that
    /// is itself a boxed Copy value copied into its own box.
    fn emit_box_copy(
        &self,
        func: &mut MirFunction,
        ty: &Ty,
        src: LocalId,
        dest: LocalId,
        out: &mut Vec<MirInst>,
    ) {
        let size = self.alloc_size(ty);
        out.push(MirInst::Alloc {
            dest,
            ty: ty.clone(),
            size,
        });
        let slots: Vec<Ty> = match ty {
            Ty::Tuple(elems) => elems.clone(),
            _ => vec![Ty::Int; size / 8],
        };
        for (index, slot_ty) in slots.iter().enumerate() {
            let value = func.new_temp(slot_ty.clone());
            out.push(MirInst::GetField {
                dest: value,
                base: src,
                field_index: index,
            });
            let stored = if matches!(ty, Ty::Tuple(_)) && is_boxed_copy(slot_ty, self.symbols) {
                let inner = func.new_temp(slot_ty.clone());
                self.emit_box_copy(func, slot_ty, value, inner, out);
                inner
            } else {
                value
            };
            out.push(MirInst::SetField {
                base: dest,
                field_index: index,
                value: MirValue::Use(stored),
            });
        }
    }
}

/// The locals that hold a box this frame owns: every definition is an
/// `Alloc`, a call of a lowered body or a closure (each returns a box of its
/// own, see [`Lowerer::duplicate_boxed_copies`]), or a move or assignment
/// from such a local.
fn own_boxes(func: &MirFunction, lowered: &HashSet<String>) -> HashSet<LocalId> {
    let mut defs: HashMap<LocalId, Vec<Option<LocalId>>> = HashMap::new();
    for inst in func.blocks.iter().flat_map(|b| b.instructions.iter()) {
        let source = match inst {
            MirInst::Alloc { dest, .. } => Some((*dest, None)),
            MirInst::Call {
                dest: Some(dest),
                callee,
                ..
            } if lowered.contains(callee) => Some((*dest, None)),
            MirInst::CallIndirect {
                dest: Some(dest), ..
            } => Some((*dest, None)),
            MirInst::Assign {
                dest,
                value: MirValue::Use(src),
            }
            | MirInst::Move { dest, src } => Some((*dest, Some(*src))),
            _ => inst.dest().map(|dest| (dest, Some(dest))),
        };
        if let Some((dest, from)) = source {
            defs.entry(dest).or_default().push(from);
        }
    }
    let mut owned: HashSet<LocalId> = HashSet::new();
    loop {
        let before = owned.len();
        for (local, sources) in &defs {
            if sources
                .iter()
                .all(|source| source.is_none_or(|src| src != *local && owned.contains(&src)))
            {
                owned.insert(*local);
            }
        }
        if owned.len() == before {
            return owned;
        }
    }
}

/// In a closure body, the locals that hold what it read out of its captures
/// box (its first parameter), directly or through a cell; empty elsewhere.
fn capture_reads(func: &MirFunction) -> HashSet<LocalId> {
    let mut reads = HashSet::new();
    if func.role != MirFnRole::ClosureBody {
        return reads;
    }
    let Some(&captures) = func.params.first() else {
        return reads;
    };
    reads.insert(captures);
    loop {
        let before = reads.len();
        for inst in func.blocks.iter().flat_map(|b| b.instructions.iter()) {
            let (dest, src) = match inst {
                MirInst::GetField { dest, base, .. } => (*dest, *base),
                MirInst::Assign {
                    dest,
                    value: MirValue::Use(src),
                }
                | MirInst::Move { dest, src } => (*dest, *src),
                _ => continue,
            };
            if reads.contains(&src) {
                reads.insert(dest);
            }
        }
        if reads.len() == before {
            reads.remove(&captures);
            return reads;
        }
    }
}

/// How many instructions read each local as a value: every read except the
/// base of a `SetField` (filling a box is not a use of its value).
fn value_reads(func: &MirFunction) -> HashMap<LocalId, usize> {
    let mut reads = HashMap::new();
    for block in &func.blocks {
        for inst in &block.instructions {
            let base = match inst {
                MirInst::SetField { base, .. } => Some(*base),
                _ => None,
            };
            for local in inst.read_locals() {
                if Some(local) != base {
                    *reads.entry(local).or_insert(0) += 1;
                }
            }
        }
        if let Some(local) = block.terminator.read_local() {
            *reads.entry(local).or_insert(0) += 1;
        }
    }
    reads
}
