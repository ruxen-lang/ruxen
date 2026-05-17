use super::*;

impl<'a> Lowerer<'a> {
    /// Phase 2 #06.D2.S1: synthesize a `Display::fmt` MIR function for each
    /// primitive type that participates in string interpolation. Each emitted
    /// function has signature `(self: Prim, fmt: &mut Formatter) -> Unit` and
    /// delegates to the existing `riven_<prim>_to_string` runtime helper
    /// (except `String_fmt`, which writes `self` directly). These functions
    /// are emitted unconditionally at program-lowering time and serve as the
    /// canonical target once `lower_interpolation` is rewired in Stage 3.
    pub(super) fn synthesize_primitive_fmt_displays(&self) -> Vec<MirFunction> {
        let formatter_ty = Ty::RefMut(Box::new(Ty::Class {
            name: "Formatter".to_string(),
            generic_args: vec![],
        }));

        // (fn_name, self_ty, primitive kind — controls how the value is
        // converted to a string before write_str).  Kind drives which
        // runtime helper is invoked and whether precision is read from
        // the Formatter (Float / String only — Char / Int / Bool ignore
        // precision per Rust semantics).
        enum Kind {
            Char,
            Int,
            Float,
            Bool,
            String_,
        }
        let specs: &[(&str, Ty, Kind)] = &[
            ("Char_fmt", Ty::Char, Kind::Char),
            ("Int_fmt", Ty::Int, Kind::Int),
            ("Float_fmt", Ty::Float, Kind::Float),
            ("Bool_fmt", Ty::Bool, Kind::Bool),
            ("String_fmt", Ty::String, Kind::String_),
        ];

        let mut out = Vec::with_capacity(specs.len());
        for (name, self_ty, kind) in specs {
            let mut mir_fn = MirFunction::new(*name, Ty::Unit);
            let self_local = mir_fn.new_local("self", self_ty.clone(), false);
            mir_fn.params.push(self_local);
            let fmt_local = mir_fn.new_local("fmt", formatter_ty.clone(), true);
            mir_fn.params.push(fmt_local);

            let entry = mir_fn.entry_block;

            // Phase 2 #06.D4: Float and String consult the formatter's
            // precision slot; Char / Int / Bool do not.
            let str_local = match kind {
                Kind::Char => {
                    let dest = mir_fn.new_temp(Ty::String);
                    mir_fn.blocks[entry].instructions.push(MirInst::Call {
                        dest: Some(dest),
                        callee: "riven_char_to_string".to_string(),
                        args: vec![MirValue::Use(self_local)],
                    });
                    dest
                }
                Kind::Int => {
                    let dest = mir_fn.new_temp(Ty::String);
                    mir_fn.blocks[entry].instructions.push(MirInst::Call {
                        dest: Some(dest),
                        callee: "riven_int_to_string".to_string(),
                        args: vec![MirValue::Use(self_local)],
                    });
                    dest
                }
                Kind::Bool => {
                    let dest = mir_fn.new_temp(Ty::String);
                    mir_fn.blocks[entry].instructions.push(MirInst::Call {
                        dest: Some(dest),
                        callee: "riven_bool_to_string".to_string(),
                        args: vec![MirValue::Use(self_local)],
                    });
                    dest
                }
                Kind::Float => {
                    // p = Formatter_precision(fmt)
                    let prec_local = mir_fn.new_temp(Ty::Int);
                    mir_fn.blocks[entry].instructions.push(MirInst::Call {
                        dest: Some(prec_local),
                        callee: "Formatter_precision".to_string(),
                        args: vec![MirValue::Use(fmt_local)],
                    });
                    // s = Float_to_string_prec(self, p)
                    let dest = mir_fn.new_temp(Ty::String);
                    mir_fn.blocks[entry].instructions.push(MirInst::Call {
                        dest: Some(dest),
                        callee: "Float_to_string_prec".to_string(),
                        args: vec![MirValue::Use(self_local), MirValue::Use(prec_local)],
                    });
                    dest
                }
                Kind::String_ => {
                    // p = Formatter_precision(fmt)
                    let prec_local = mir_fn.new_temp(Ty::Int);
                    mir_fn.blocks[entry].instructions.push(MirInst::Call {
                        dest: Some(prec_local),
                        callee: "Formatter_precision".to_string(),
                        args: vec![MirValue::Use(fmt_local)],
                    });
                    // s = String_truncate_chars(self, p)   (p == -1 → copy)
                    let dest = mir_fn.new_temp(Ty::String);
                    mir_fn.blocks[entry].instructions.push(MirInst::Call {
                        dest: Some(dest),
                        callee: "String_truncate_chars".to_string(),
                        args: vec![MirValue::Use(self_local), MirValue::Use(prec_local)],
                    });
                    dest
                }
            };

            // Formatter_write_str(fmt, str_value) — result discarded.
            mir_fn.blocks[entry].instructions.push(MirInst::Call {
                dest: None,
                callee: "Formatter_write_str".to_string(),
                args: vec![MirValue::Use(fmt_local), MirValue::Use(str_local)],
            });

            mir_fn.blocks[entry].terminator = Terminator::Return(None);
            out.push(mir_fn);
        }
        out
    }

    /// Synthesize the body of `{StructName}_to_debug(self) -> String` for a
    /// struct that declares `derive Debug`. Output shape:
    /// `Name { field1: <fmt(field1)>, field2: <fmt(field2)>, ... }`.
    /// v1 limitation: only primitive field types are formatted faithfully;
    /// other struct fields with `derive Debug` recurse; everything else
    /// renders as `<...>` so the formatter never panics.
    pub(super) fn synthesize_struct_to_debug(&self, s: &HirStructDef) -> MirFunction {
        let fn_name = format!("{}_to_debug", s.name);
        let self_ty = Ty::Struct {
            name: s.name.clone(),
            generic_args: vec![],
        };

        let mut mir_fn = MirFunction::new(&fn_name, Ty::String);
        let self_local = mir_fn.new_local("self", self_ty, false);
        mir_fn.params.push(self_local);

        let entry = mir_fn.entry_block;

        let leading = if s.fields.is_empty() {
            format!("{} {{}}", s.name)
        } else {
            format!("{} {{ ", s.name)
        };

        let leading_local = mir_fn.new_temp(Ty::String);
        mir_fn.blocks[entry]
            .instructions
            .push(MirInst::StringLiteral {
                dest: leading_local,
                value: leading,
            });
        let mut acc = leading_local;

        for (idx, field) in s.fields.iter().enumerate() {
            if idx > 0 {
                let sep = mir_fn.new_temp(Ty::String);
                mir_fn.blocks[entry]
                    .instructions
                    .push(MirInst::StringLiteral {
                        dest: sep,
                        value: ", ".to_string(),
                    });
                let next = mir_fn.new_temp(Ty::String);
                mir_fn.blocks[entry].instructions.push(MirInst::Call {
                    dest: Some(next),
                    callee: "riven_string_concat".to_string(),
                    args: vec![MirValue::Use(acc), MirValue::Use(sep)],
                });
                acc = next;
            }

            let label = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[entry]
                .instructions
                .push(MirInst::StringLiteral {
                    dest: label,
                    value: format!("{}: ", field.name),
                });
            let after_label = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[entry].instructions.push(MirInst::Call {
                dest: Some(after_label),
                callee: "riven_string_concat".to_string(),
                args: vec![MirValue::Use(acc), MirValue::Use(label)],
            });
            acc = after_label;

            let field_local = mir_fn.new_temp(field.ty.clone());
            mir_fn.blocks[entry].instructions.push(MirInst::GetField {
                dest: field_local,
                base: self_local,
                field_index: idx,
            });

            let field_str = if field.ty == Ty::Char {
                let dest = mir_fn.new_temp(Ty::String);
                mir_fn.blocks[entry].instructions.push(MirInst::Call {
                    dest: Some(dest),
                    callee: "riven_char_to_string".to_string(),
                    args: vec![MirValue::Use(field_local)],
                });
                dest
            } else if field.ty.is_integer() {
                let dest = mir_fn.new_temp(Ty::String);
                mir_fn.blocks[entry].instructions.push(MirInst::Call {
                    dest: Some(dest),
                    callee: "riven_int_to_string".to_string(),
                    args: vec![MirValue::Use(field_local)],
                });
                dest
            } else if field.ty.is_float() {
                let dest = mir_fn.new_temp(Ty::String);
                mir_fn.blocks[entry].instructions.push(MirInst::Call {
                    dest: Some(dest),
                    callee: "riven_float_to_string".to_string(),
                    args: vec![MirValue::Use(field_local)],
                });
                dest
            } else if field.ty == Ty::Bool {
                let dest = mir_fn.new_temp(Ty::String);
                mir_fn.blocks[entry].instructions.push(MirInst::Call {
                    dest: Some(dest),
                    callee: "riven_bool_to_string".to_string(),
                    args: vec![MirValue::Use(field_local)],
                });
                dest
            } else if matches!(field.ty, Ty::String | Ty::Str) {
                field_local
            } else if let Some(inner_struct_name) = self.struct_with_derive_debug(&field.ty) {
                let dest = mir_fn.new_temp(Ty::String);
                mir_fn.blocks[entry].instructions.push(MirInst::Call {
                    dest: Some(dest),
                    callee: format!("{}_to_debug", inner_struct_name),
                    args: vec![MirValue::Use(field_local)],
                });
                dest
            } else {
                let dest = mir_fn.new_temp(Ty::String);
                mir_fn.blocks[entry]
                    .instructions
                    .push(MirInst::StringLiteral {
                        dest,
                        value: "<...>".to_string(),
                    });
                dest
            };

            let next = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[entry].instructions.push(MirInst::Call {
                dest: Some(next),
                callee: "riven_string_concat".to_string(),
                args: vec![MirValue::Use(acc), MirValue::Use(field_str)],
            });
            acc = next;
        }

        if !s.fields.is_empty() {
            let trailing = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[entry]
                .instructions
                .push(MirInst::StringLiteral {
                    dest: trailing,
                    value: " }".to_string(),
                });
            let next = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[entry].instructions.push(MirInst::Call {
                dest: Some(next),
                callee: "riven_string_concat".to_string(),
                args: vec![MirValue::Use(acc), MirValue::Use(trailing)],
            });
            acc = next;
        }

        mir_fn.blocks[entry].terminator = Terminator::Return(Some(MirValue::Use(acc)));
        mir_fn
    }

    pub(super) fn synthesize_struct_eq(&self, s: &HirStructDef) -> MirFunction {
        let fn_name = format!("{}_eq", s.name);
        let self_ty = Ty::Struct {
            name: s.name.clone(),
            generic_args: vec![],
        };

        let mut mir_fn = MirFunction::new(&fn_name, Ty::Bool);
        let self_local = mir_fn.new_local("self", self_ty.clone(), false);
        let other_local = mir_fn.new_local("other", Ty::Ref(Box::new(self_ty)), false);
        mir_fn.params.push(self_local);
        mir_fn.params.push(other_local);

        let entry = mir_fn.entry_block;
        let mut acc = mir_fn.new_temp(Ty::Bool);
        mir_fn.blocks[entry].instructions.push(MirInst::Assign {
            dest: acc,
            value: MirValue::Literal(Literal::Bool(true)),
        });

        for (idx, field) in s.fields.iter().enumerate() {
            let lhs = mir_fn.new_temp(field.ty.clone());
            mir_fn.blocks[entry].instructions.push(MirInst::GetField {
                dest: lhs,
                base: self_local,
                field_index: idx,
            });
            let rhs = mir_fn.new_temp(field.ty.clone());
            mir_fn.blocks[entry].instructions.push(MirInst::GetField {
                dest: rhs,
                base: other_local,
                field_index: idx,
            });

            let field_eq =
                if let Some(inner_name) = self.struct_with_derive_trait(&field.ty, "PartialEq") {
                    let dest = mir_fn.new_temp(Ty::Bool);
                    mir_fn.blocks[entry].instructions.push(MirInst::Call {
                        dest: Some(dest),
                        callee: format!("{}_eq", inner_name),
                        args: vec![MirValue::Use(lhs), MirValue::Use(rhs)],
                    });
                    dest
                } else if matches!(field.ty, Ty::String | Ty::Str) {
                    let dest = mir_fn.new_temp(Ty::Bool);
                    mir_fn.blocks[entry].instructions.push(MirInst::Call {
                        dest: Some(dest),
                        callee: "riven_string_eq".to_string(),
                        args: vec![MirValue::Use(lhs), MirValue::Use(rhs)],
                    });
                    dest
                } else {
                    let dest = mir_fn.new_temp(Ty::Bool);
                    mir_fn.blocks[entry].instructions.push(MirInst::Compare {
                        dest,
                        op: CmpOp::Eq,
                        lhs: MirValue::Use(lhs),
                        rhs: MirValue::Use(rhs),
                    });
                    dest
                };

            let next = mir_fn.new_temp(Ty::Bool);
            mir_fn.blocks[entry].instructions.push(MirInst::BinOp {
                dest: next,
                op: BinOp::And,
                lhs: MirValue::Use(acc),
                rhs: MirValue::Use(field_eq),
            });
            acc = next;
        }

        mir_fn.blocks[entry].terminator = Terminator::Return(Some(MirValue::Use(acc)));
        mir_fn
    }

    pub(super) fn synthesize_struct_hash_code(&self, s: &HirStructDef) -> MirFunction {
        let fn_name = format!("{}_hash_code", s.name);
        let self_ty = Ty::Struct {
            name: s.name.clone(),
            generic_args: vec![],
        };

        let mut mir_fn = MirFunction::new(&fn_name, Ty::Int);
        let self_local = mir_fn.new_local("self", self_ty, false);
        mir_fn.params.push(self_local);

        let entry = mir_fn.entry_block;
        let mut acc = mir_fn.new_temp(Ty::Int);
        mir_fn.blocks[entry].instructions.push(MirInst::Assign {
            dest: acc,
            value: MirValue::Literal(Literal::Int(1469598103934665603_i64)),
        });

        for (idx, field) in s.fields.iter().enumerate() {
            let field_local = mir_fn.new_temp(field.ty.clone());
            mir_fn.blocks[entry].instructions.push(MirInst::GetField {
                dest: field_local,
                base: self_local,
                field_index: idx,
            });

            let field_hash = if let Some(inner_name) = self
                .struct_with_derive_trait(&field.ty, "Hashable")
                .or_else(|| self.struct_with_derive_trait(&field.ty, "Hash"))
            {
                let dest = mir_fn.new_temp(Ty::Int);
                mir_fn.blocks[entry].instructions.push(MirInst::Call {
                    dest: Some(dest),
                    callee: format!("{}_hash_code", inner_name),
                    args: vec![MirValue::Use(field_local)],
                });
                dest
            } else if matches!(field.ty, Ty::String | Ty::Str) {
                let dest = mir_fn.new_temp(Ty::Int);
                mir_fn.blocks[entry].instructions.push(MirInst::Call {
                    dest: Some(dest),
                    callee: "riven_string_hash".to_string(),
                    args: vec![MirValue::Use(field_local)],
                });
                dest
            } else {
                field_local
            };

            let xored = mir_fn.new_temp(Ty::Int);
            mir_fn.blocks[entry].instructions.push(MirInst::BinOp {
                dest: xored,
                op: BinOp::BitXor,
                lhs: MirValue::Use(acc),
                rhs: MirValue::Use(field_hash),
            });
            let next = mir_fn.new_temp(Ty::Int);
            mir_fn.blocks[entry].instructions.push(MirInst::BinOp {
                dest: next,
                op: BinOp::Mul,
                lhs: MirValue::Use(xored),
                rhs: MirValue::Literal(Literal::Int(1099511628211_i64)),
            });
            acc = next;
        }

        mir_fn.blocks[entry].terminator = Terminator::Return(Some(MirValue::Use(acc)));
        mir_fn
    }

    pub(super) fn synthesize_struct_default(&self, s: &HirStructDef) -> MirFunction {
        let fn_name = format!("{}_default", s.name);
        let self_ty = Ty::Struct {
            name: s.name.clone(),
            generic_args: vec![],
        };

        let mut mir_fn = MirFunction::new(&fn_name, self_ty.clone());
        let entry = mir_fn.entry_block;
        let obj = mir_fn.new_temp(self_ty.clone());
        mir_fn.blocks[entry].instructions.push(MirInst::Alloc {
            dest: obj,
            ty: self_ty.clone(),
            size: self.alloc_size(&self_ty),
        });

        for (idx, field) in s.fields.iter().enumerate() {
            let value = self.synthesize_default_value(&mut mir_fn, entry, &field.ty);
            mir_fn.blocks[entry].instructions.push(MirInst::SetField {
                base: obj,
                field_index: idx,
                value: MirValue::Use(value),
            });
        }

        mir_fn.blocks[entry].terminator = Terminator::Return(Some(MirValue::Use(obj)));
        mir_fn
    }

    pub(super) fn synthesize_struct_cmp(&self, s: &HirStructDef, partial: bool) -> MirFunction {
        let method_name = if partial { "partial_cmp" } else { "cmp" };
        let fn_name = format!("{}_{}", s.name, method_name);
        let self_ty = Ty::Struct {
            name: s.name.clone(),
            generic_args: vec![],
        };

        let mut mir_fn = MirFunction::new(&fn_name, Ty::Int);
        let self_local = mir_fn.new_local("self", self_ty.clone(), false);
        let other_local = mir_fn.new_local("other", Ty::Ref(Box::new(self_ty)), false);
        mir_fn.params.push(self_local);
        mir_fn.params.push(other_local);

        let mut current_block = mir_fn.entry_block;
        for (idx, field) in s.fields.iter().enumerate() {
            let lhs = mir_fn.new_temp(field.ty.clone());
            mir_fn.blocks[current_block]
                .instructions
                .push(MirInst::GetField {
                    dest: lhs,
                    base: self_local,
                    field_index: idx,
                });
            let rhs = mir_fn.new_temp(field.ty.clone());
            mir_fn.blocks[current_block]
                .instructions
                .push(MirInst::GetField {
                    dest: rhs,
                    base: other_local,
                    field_index: idx,
                });

            if let Some(inner_name) =
                self.struct_with_derive_trait(&field.ty, if partial { "PartialOrd" } else { "Ord" })
            {
                let cmp = mir_fn.new_temp(Ty::Int);
                mir_fn.blocks[current_block]
                    .instructions
                    .push(MirInst::Call {
                        dest: Some(cmp),
                        callee: format!("{}_{}", inner_name, method_name),
                        args: vec![MirValue::Use(lhs), MirValue::Use(rhs)],
                    });
                let is_eq = mir_fn.new_temp(Ty::Bool);
                mir_fn.blocks[current_block]
                    .instructions
                    .push(MirInst::Compare {
                        dest: is_eq,
                        op: CmpOp::Eq,
                        lhs: MirValue::Use(cmp),
                        rhs: MirValue::Literal(Literal::Int(0)),
                    });
                let next_block = mir_fn.new_block();
                let diff_block = mir_fn.new_block();
                mir_fn.blocks[current_block].terminator = Terminator::Branch {
                    cond: MirValue::Use(is_eq),
                    then_block: next_block,
                    else_block: diff_block,
                };
                mir_fn.blocks[diff_block].terminator = Terminator::Return(Some(MirValue::Use(cmp)));
                current_block = next_block;
                continue;
            }

            if matches!(field.ty, Ty::String | Ty::Str) {
                let cmp = mir_fn.new_temp(Ty::Int);
                mir_fn.blocks[current_block]
                    .instructions
                    .push(MirInst::Call {
                        dest: Some(cmp),
                        callee: "riven_string_cmp".to_string(),
                        args: vec![MirValue::Use(lhs), MirValue::Use(rhs)],
                    });
                let is_eq = mir_fn.new_temp(Ty::Bool);
                mir_fn.blocks[current_block]
                    .instructions
                    .push(MirInst::Compare {
                        dest: is_eq,
                        op: CmpOp::Eq,
                        lhs: MirValue::Use(cmp),
                        rhs: MirValue::Literal(Literal::Int(0)),
                    });
                let next_block = mir_fn.new_block();
                let diff_block = mir_fn.new_block();
                mir_fn.blocks[current_block].terminator = Terminator::Branch {
                    cond: MirValue::Use(is_eq),
                    then_block: next_block,
                    else_block: diff_block,
                };
                mir_fn.blocks[diff_block].terminator = Terminator::Return(Some(MirValue::Use(cmp)));
                current_block = next_block;
                continue;
            }

            let lt = mir_fn.new_temp(Ty::Bool);
            mir_fn.blocks[current_block]
                .instructions
                .push(MirInst::Compare {
                    dest: lt,
                    op: CmpOp::Lt,
                    lhs: MirValue::Use(lhs),
                    rhs: MirValue::Use(rhs),
                });
            let lt_block = mir_fn.new_block();
            let ge_block = mir_fn.new_block();
            mir_fn.blocks[current_block].terminator = Terminator::Branch {
                cond: MirValue::Use(lt),
                then_block: lt_block,
                else_block: ge_block,
            };
            mir_fn.blocks[lt_block].terminator =
                Terminator::Return(Some(MirValue::Literal(Literal::Int(-1))));

            let gt = mir_fn.new_temp(Ty::Bool);
            mir_fn.blocks[ge_block].instructions.push(MirInst::Compare {
                dest: gt,
                op: CmpOp::Gt,
                lhs: MirValue::Use(lhs),
                rhs: MirValue::Use(rhs),
            });
            let gt_block = mir_fn.new_block();
            let next_block = mir_fn.new_block();
            mir_fn.blocks[ge_block].terminator = Terminator::Branch {
                cond: MirValue::Use(gt),
                then_block: gt_block,
                else_block: next_block,
            };
            mir_fn.blocks[gt_block].terminator =
                Terminator::Return(Some(MirValue::Literal(Literal::Int(1))));
            current_block = next_block;
        }

        mir_fn.blocks[current_block].terminator =
            Terminator::Return(Some(MirValue::Literal(Literal::Int(0))));
        mir_fn
    }

    /// Emit MIR that produces a deep clone of the value `src` of type
    /// `field_ty`, returning the local that holds the cloned value.
    /// Inserts instructions into `block`.
    ///
    /// Recipe:
    ///   * Copy types (primitives, references, function pointers,
    ///     `derive Copy` user types) → bitwise reuse of `src`.
    ///   * `String` / `Str`           → `riven_string_from(src)`.
    ///   * `Vec[_]`                   → `riven_vec_clone(src)`.
    ///   * `HashMap[_, _]`            → `riven_hash_clone(src)`.
    ///   * `Set[_]`                   → `riven_set_clone(src)`.
    ///   * Struct/Class/Enum that itself derives Clone → recursive
    ///     `<Type>_clone(src)`.
    ///   * Anything else falls back to a bitwise reuse — drop
    ///     elaboration in `implicit_includes/mod.rs::validate_clone_requirements`
    ///     ensures the fallback only triggers for types with E0610
    ///     already emitted, so the synthesised function still has a
    ///     compilable body for downstream codegen even though the
    ///     program will not link.
    pub(super) fn synthesize_clone_field(
        &self,
        mir_fn: &mut MirFunction,
        block: BlockId,
        src: LocalId,
        field_ty: &Ty,
    ) -> LocalId {
        if ty_is_effectively_copy(field_ty, self.symbols) {
            return src;
        }
        if matches!(field_ty, Ty::String | Ty::Str) {
            let dest = mir_fn.new_temp(field_ty.clone());
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: "riven_string_from".to_string(),
                args: vec![MirValue::Use(src)],
            });
            return dest;
        }
        if matches!(field_ty, Ty::Array(_)) {
            let dest = mir_fn.new_temp(field_ty.clone());
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: "riven_vec_clone".to_string(),
                args: vec![MirValue::Use(src)],
            });
            return dest;
        }
        if matches!(field_ty, Ty::Map(_, _)) {
            let dest = mir_fn.new_temp(field_ty.clone());
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: "riven_hash_clone".to_string(),
                args: vec![MirValue::Use(src)],
            });
            return dest;
        }
        if matches!(field_ty, Ty::Set(_)) {
            let dest = mir_fn.new_temp(field_ty.clone());
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: "riven_set_clone".to_string(),
                args: vec![MirValue::Use(src)],
            });
            return dest;
        }
        if let Some(inner_name) = self.user_type_with_derive_clone(field_ty) {
            let dest = mir_fn.new_temp(field_ty.clone());
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: format!("{}_clone", inner_name),
                args: vec![MirValue::Use(src)],
            });
            return dest;
        }
        // Fallback: bitwise reuse. The companion validator in
        // `implicit_includes/mod.rs` will already have surfaced E0610 for this
        // path, so the resulting MIR exists only to keep the rest of
        // codegen consistent during the same compilation unit.
        src
    }

    /// Look up a user-defined type by name and return the type name
    /// when the underlying definition (struct / class / enum) carries
    /// `derive Clone`.
    pub(super) fn user_type_with_derive_clone(&self, ty: &Ty) -> Option<String> {
        use crate::resolve::symbols::DefKind;
        let name = match ty {
            Ty::Struct { name, .. } | Ty::Class { name, .. } | Ty::Enum { name, .. } => {
                name.clone()
            }
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner) => return self.user_type_with_derive_clone(inner),
            Ty::Alias { target, .. } => return self.user_type_with_derive_clone(target),
            Ty::Newtype { inner, .. } => return self.user_type_with_derive_clone(inner),
            _ => return None,
        };
        for def in self.symbols.iter() {
            if def.name != name {
                continue;
            }
            let derives = match &def.kind {
                DefKind::Struct { info } => &info.derive_traits,
                DefKind::Class { info } => &info.derive_traits,
                DefKind::Enum { info } => &info.derive_traits,
                _ => continue,
            };
            if derives.iter().any(|t| t == "Clone") {
                return Some(name);
            }
        }
        None
    }

    /// Synthesise `{StructName}_clone(self) -> StructName` for a
    /// struct that declares `derive Clone`. The body allocates a fresh
    /// instance, clones each field according to
    /// [`Self::synthesize_clone_field`], and returns the new value.
    pub(super) fn synthesize_struct_clone(&self, s: &HirStructDef) -> MirFunction {
        let fn_name = format!("{}_clone", s.name);
        let self_ty = Ty::Struct {
            name: s.name.clone(),
            generic_args: vec![],
        };

        let mut mir_fn = MirFunction::new(&fn_name, self_ty.clone());
        let self_local = mir_fn.new_local("self", self_ty.clone(), false);
        mir_fn.params.push(self_local);
        let entry = mir_fn.entry_block;

        let dest = mir_fn.new_temp(self_ty.clone());
        mir_fn.blocks[entry].instructions.push(MirInst::Alloc {
            dest,
            ty: self_ty.clone(),
            size: self.alloc_size(&self_ty),
        });

        for (idx, field) in s.fields.iter().enumerate() {
            let field_local = mir_fn.new_temp(field.ty.clone());
            mir_fn.blocks[entry].instructions.push(MirInst::GetField {
                dest: field_local,
                base: self_local,
                field_index: idx,
            });
            let cloned = self.synthesize_clone_field(&mut mir_fn, entry, field_local, &field.ty);
            mir_fn.blocks[entry].instructions.push(MirInst::SetField {
                base: dest,
                field_index: idx,
                value: MirValue::Use(cloned),
            });
        }

        mir_fn.blocks[entry].terminator = Terminator::Return(Some(MirValue::Use(dest)));
        mir_fn
    }

    /// Synthesise `{ClassName}_clone(self) -> ClassName` for a class
    /// that declares `derive Clone`. Same body shape as the struct
    /// version; the storage layout (8-byte field slots) is identical
    /// at the MIR level.
    pub(super) fn synthesize_class_clone(&self, c: &HirClassDef) -> MirFunction {
        let fn_name = format!("{}_clone", c.name);
        let self_ty = Ty::Class {
            name: c.name.clone(),
            generic_args: vec![],
        };

        let mut mir_fn = MirFunction::new(&fn_name, self_ty.clone());
        let self_local = mir_fn.new_local("self", self_ty.clone(), false);
        mir_fn.params.push(self_local);
        let entry = mir_fn.entry_block;

        let dest = mir_fn.new_temp(self_ty.clone());
        mir_fn.blocks[entry].instructions.push(MirInst::Alloc {
            dest,
            ty: self_ty.clone(),
            size: self.alloc_size(&self_ty),
        });

        for (idx, field) in c.fields.iter().enumerate() {
            let field_local = mir_fn.new_temp(field.ty.clone());
            mir_fn.blocks[entry].instructions.push(MirInst::GetField {
                dest: field_local,
                base: self_local,
                field_index: idx,
            });
            let cloned = self.synthesize_clone_field(&mut mir_fn, entry, field_local, &field.ty);
            mir_fn.blocks[entry].instructions.push(MirInst::SetField {
                base: dest,
                field_index: idx,
                value: MirValue::Use(cloned),
            });
        }

        mir_fn.blocks[entry].terminator = Terminator::Return(Some(MirValue::Use(dest)));
        mir_fn
    }

    /// Synthesise `{EnumName}_clone(self) -> EnumName` for an enum
    /// that declares `derive Clone`. Lowering is a switch on the
    /// discriminant: each variant allocates a new enum, copies the
    /// tag, clones every payload field, and goto's a shared join
    /// block that returns the cloned value.
    pub(super) fn synthesize_enum_clone(&self, e: &HirEnumDef) -> MirFunction {
        let fn_name = format!("{}_clone", e.name);
        let self_ty = Ty::Enum {
            name: e.name.clone(),
            generic_args: vec![],
        };

        let mut mir_fn = MirFunction::new(&fn_name, self_ty.clone());
        let self_local = mir_fn.new_local("self", self_ty.clone(), false);
        mir_fn.params.push(self_local);

        let entry = mir_fn.entry_block;
        let result = mir_fn.new_temp(self_ty.clone());
        mir_fn.blocks[entry].instructions.push(MirInst::Alloc {
            dest: result,
            ty: self_ty.clone(),
            size: self.alloc_size(&self_ty),
        });

        let tag = mir_fn.new_temp(Ty::Int32);
        mir_fn.blocks[entry].instructions.push(MirInst::GetTag {
            dest: tag,
            src: self_local,
        });

        // One block per variant + a shared join block that holds the
        // single Return terminator. Variant 0 doubles as the Switch's
        // `otherwise` target so a malformed tag still reaches a real
        // arm rather than falling off the end.
        let join = mir_fn.new_block();
        let mut targets: Vec<(i64, BlockId)> = Vec::with_capacity(e.variants.len());
        let mut variant_blocks: Vec<BlockId> = Vec::with_capacity(e.variants.len());
        for _ in &e.variants {
            variant_blocks.push(mir_fn.new_block());
        }
        for (i, variant) in e.variants.iter().enumerate() {
            targets.push((variant.index as i64, variant_blocks[i]));
        }
        let otherwise = variant_blocks.first().copied().unwrap_or(join);

        mir_fn.blocks[entry].terminator = Terminator::Switch {
            value: MirValue::Use(tag),
            targets,
            otherwise,
        };

        for (i, variant) in e.variants.iter().enumerate() {
            let block = variant_blocks[i];
            mir_fn.blocks[block].instructions.push(MirInst::SetTag {
                dest: result,
                tag: variant.index as u32,
            });

            let payload_fields: &[HirVariantField] = match &variant.kind {
                HirVariantKind::Unit => &[],
                HirVariantKind::Tuple(fields) | HirVariantKind::Struct(fields) => fields,
            };

            if !payload_fields.is_empty() {
                let self_payload = mir_fn.new_temp(self_ty.clone());
                mir_fn.blocks[block].instructions.push(MirInst::GetPayload {
                    dest: self_payload,
                    src: self_local,
                    ty: self_ty.clone(),
                });
                let dest_payload = mir_fn.new_temp(self_ty.clone());
                mir_fn.blocks[block].instructions.push(MirInst::GetPayload {
                    dest: dest_payload,
                    src: result,
                    ty: self_ty.clone(),
                });
                for (idx, field) in payload_fields.iter().enumerate() {
                    let read = mir_fn.new_temp(field.ty.clone());
                    mir_fn.blocks[block].instructions.push(MirInst::GetField {
                        dest: read,
                        base: self_payload,
                        field_index: idx,
                    });
                    let cloned = self.synthesize_clone_field(&mut mir_fn, block, read, &field.ty);
                    mir_fn.blocks[block].instructions.push(MirInst::SetField {
                        base: dest_payload,
                        field_index: idx,
                        value: MirValue::Use(cloned),
                    });
                }
            }

            mir_fn.blocks[block].terminator = Terminator::Goto(join);
        }

        mir_fn.blocks[join].terminator = Terminator::Return(Some(MirValue::Use(result)));
        mir_fn
    }

    /// Phase 2 #06.C2: synthesize `{EnumName}_to_debug(self) -> String`
    /// for an enum that declares `derive Debug`. Output shape mirrors
    /// Rust's `Debug`:
    ///
    /// * `Unit` variants  → `"Variant"`
    /// * `Tuple(a, b)`    → `"Variant(<a>, <b>)"`
    /// * `Struct{x, y}`   → `"Variant { x: <x>, y: <y> }"`
    ///
    /// Field formatting mirrors `synthesize_struct_to_debug`: primitives
    /// use the `riven_*_to_string` runtime helpers, nested structs with
    /// `derive Debug` recurse, anything else renders as `<...>` so the
    /// formatter never panics.
    pub(super) fn synthesize_enum_to_debug(&self, e: &HirEnumDef) -> MirFunction {
        let fn_name = format!("{}_to_debug", e.name);
        let self_ty = Ty::Enum {
            name: e.name.clone(),
            generic_args: vec![],
        };

        let mut mir_fn = MirFunction::new(&fn_name, Ty::String);
        let self_local = mir_fn.new_local("self", self_ty.clone(), false);
        mir_fn.params.push(self_local);

        let entry = mir_fn.entry_block;
        let tag = mir_fn.new_temp(Ty::Int32);
        mir_fn.blocks[entry].instructions.push(MirInst::GetTag {
            dest: tag,
            src: self_local,
        });

        // One block per variant. Each block builds the variant's
        // debug string and terminates with its own `Return`, so we
        // don't need a join block. Variant 0 doubles as the Switch's
        // `otherwise` target to mirror `synthesize_enum_clone`.
        let mut variant_blocks: Vec<BlockId> = Vec::with_capacity(e.variants.len());
        for _ in &e.variants {
            variant_blocks.push(mir_fn.new_block());
        }
        let targets: Vec<(i64, BlockId)> = e
            .variants
            .iter()
            .enumerate()
            .map(|(i, v)| (v.index as i64, variant_blocks[i]))
            .collect();
        let otherwise = variant_blocks.first().copied().unwrap_or(entry);

        mir_fn.blocks[entry].terminator = Terminator::Switch {
            value: MirValue::Use(tag),
            targets,
            otherwise,
        };

        for (i, variant) in e.variants.iter().enumerate() {
            let block = variant_blocks[i];

            // Start with the variant name.
            let mut acc = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[block]
                .instructions
                .push(MirInst::StringLiteral {
                    dest: acc,
                    value: variant.name.clone(),
                });

            let payload_fields: &[HirVariantField] = match &variant.kind {
                HirVariantKind::Unit => &[],
                HirVariantKind::Tuple(fields) | HirVariantKind::Struct(fields) => fields,
            };

            if !payload_fields.is_empty() {
                let is_struct_variant = matches!(variant.kind, HirVariantKind::Struct(_));
                let open = if is_struct_variant { " { " } else { "(" };
                let close = if is_struct_variant { " }" } else { ")" };

                acc = self.concat_string_literal(&mut mir_fn, block, acc, open);

                let payload = mir_fn.new_temp(self_ty.clone());
                mir_fn.blocks[block].instructions.push(MirInst::GetPayload {
                    dest: payload,
                    src: self_local,
                    ty: self_ty.clone(),
                });

                for (idx, field) in payload_fields.iter().enumerate() {
                    if idx > 0 {
                        acc = self.concat_string_literal(&mut mir_fn, block, acc, ", ");
                    }
                    if is_struct_variant {
                        if let Some(name) = &field.name {
                            let label = format!("{}: ", name);
                            acc = self.concat_string_literal(&mut mir_fn, block, acc, &label);
                        }
                    }

                    let field_local = mir_fn.new_temp(field.ty.clone());
                    mir_fn.blocks[block].instructions.push(MirInst::GetField {
                        dest: field_local,
                        base: payload,
                        field_index: idx,
                    });

                    let field_str =
                        self.format_field_for_debug(&mut mir_fn, block, field_local, &field.ty);

                    let next = mir_fn.new_temp(Ty::String);
                    mir_fn.blocks[block].instructions.push(MirInst::Call {
                        dest: Some(next),
                        callee: "riven_string_concat".to_string(),
                        args: vec![MirValue::Use(acc), MirValue::Use(field_str)],
                    });
                    acc = next;
                }

                acc = self.concat_string_literal(&mut mir_fn, block, acc, close);
            }

            mir_fn.blocks[block].terminator = Terminator::Return(Some(MirValue::Use(acc)));
        }

        mir_fn
    }

    /// Append a literal `&str` to a String accumulator. Returns the
    /// new accumulator local. Helper for `synthesize_enum_to_debug`.
    pub(super) fn concat_string_literal(
        &self,
        mir_fn: &mut MirFunction,
        block: BlockId,
        acc: LocalId,
        text: &str,
    ) -> LocalId {
        let lit = mir_fn.new_temp(Ty::String);
        mir_fn.blocks[block]
            .instructions
            .push(MirInst::StringLiteral {
                dest: lit,
                value: text.to_string(),
            });
        let next = mir_fn.new_temp(Ty::String);
        mir_fn.blocks[block].instructions.push(MirInst::Call {
            dest: Some(next),
            callee: "riven_string_concat".to_string(),
            args: vec![MirValue::Use(acc), MirValue::Use(lit)],
        });
        next
    }

    /// Format a single field value for `_to_debug` output. Mirrors
    /// the per-field branch in `synthesize_struct_to_debug`. Phase D
    /// will replace this with a canonical `Display::fmt` dispatch.
    pub(super) fn format_field_for_debug(
        &self,
        mir_fn: &mut MirFunction,
        block: BlockId,
        field_local: LocalId,
        field_ty: &Ty,
    ) -> LocalId {
        if *field_ty == Ty::Char {
            let dest = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: "riven_char_to_string".to_string(),
                args: vec![MirValue::Use(field_local)],
            });
            return dest;
        }
        if field_ty.is_integer() {
            let dest = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: "riven_int_to_string".to_string(),
                args: vec![MirValue::Use(field_local)],
            });
            return dest;
        }
        if field_ty.is_float() {
            let dest = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: "riven_float_to_string".to_string(),
                args: vec![MirValue::Use(field_local)],
            });
            return dest;
        }
        if *field_ty == Ty::Bool {
            let dest = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: "riven_bool_to_string".to_string(),
                args: vec![MirValue::Use(field_local)],
            });
            return dest;
        }
        if matches!(field_ty, Ty::String | Ty::Str) {
            return field_local;
        }
        if let Some(inner_struct_name) = self.struct_with_derive_debug(field_ty) {
            let dest = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: format!("{}_to_debug", inner_struct_name),
                args: vec![MirValue::Use(field_local)],
            });
            return dest;
        }
        if let Some(inner_enum_name) = self.enum_with_derive_debug(field_ty) {
            let dest = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: format!("{}_to_debug", inner_enum_name),
                args: vec![MirValue::Use(field_local)],
            });
            return dest;
        }
        let dest = mir_fn.new_temp(Ty::String);
        mir_fn.blocks[block]
            .instructions
            .push(MirInst::StringLiteral {
                dest,
                value: "<...>".to_string(),
            });
        dest
    }

    pub(super) fn synthesize_default_value(
        &self,
        mir_fn: &mut MirFunction,
        block: BlockId,
        ty: &Ty,
    ) -> LocalId {
        if ty.is_integer() {
            let dest = mir_fn.new_temp(ty.clone());
            mir_fn.blocks[block].instructions.push(MirInst::Assign {
                dest,
                value: MirValue::Literal(Literal::Int(0)),
            });
            return dest;
        }
        if ty.is_float() {
            let dest = mir_fn.new_temp(ty.clone());
            mir_fn.blocks[block].instructions.push(MirInst::Assign {
                dest,
                value: MirValue::Literal(Literal::Float(0.0)),
            });
            return dest;
        }
        if *ty == Ty::Bool {
            let dest = mir_fn.new_temp(Ty::Bool);
            mir_fn.blocks[block].instructions.push(MirInst::Assign {
                dest,
                value: MirValue::Literal(Literal::Bool(false)),
            });
            return dest;
        }
        if *ty == Ty::Char {
            let dest = mir_fn.new_temp(Ty::Char);
            mir_fn.blocks[block].instructions.push(MirInst::Assign {
                dest,
                value: MirValue::Literal(Literal::Char('\0')),
            });
            return dest;
        }
        if matches!(ty, Ty::String) {
            let raw = mir_fn.new_temp(Ty::Str);
            mir_fn.blocks[block]
                .instructions
                .push(MirInst::StringLiteral {
                    dest: raw,
                    value: String::new(),
                });
            let dest = mir_fn.new_temp(Ty::String);
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: "riven_string_from".to_string(),
                args: vec![MirValue::Use(raw)],
            });
            return dest;
        }
        if matches!(ty, Ty::Str) {
            let dest = mir_fn.new_temp(Ty::Str);
            mir_fn.blocks[block]
                .instructions
                .push(MirInst::StringLiteral {
                    dest,
                    value: String::new(),
                });
            return dest;
        }
        if let Some(inner_name) = self.struct_with_derive_trait(ty, "Default") {
            let dest = mir_fn.new_temp(ty.clone());
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: format!("{}_default", inner_name),
                args: vec![],
            });
            return dest;
        }
        if matches!(ty, Ty::Array(_) | Ty::Map(_, _) | Ty::Set(_)) {
            let dest = mir_fn.new_temp(ty.clone());
            let type_name = type_name_from_ty(ty);
            let base = type_name.split('[').next().unwrap_or(type_name.as_str());
            mir_fn.blocks[block].instructions.push(MirInst::Call {
                dest: Some(dest),
                callee: format!("{}_new", base),
                args: vec![],
            });
            return dest;
        }
        if let Ty::Option(_) = ty {
            let dest = mir_fn.new_temp(ty.clone());
            mir_fn.blocks[block].instructions.push(MirInst::Alloc {
                dest,
                ty: ty.clone(),
                size: self.alloc_size(ty),
            });
            mir_fn.blocks[block]
                .instructions
                .push(MirInst::SetTag { dest, tag: 0 });
            return dest;
        }

        let dest = mir_fn.new_temp(ty.clone());
        mir_fn.blocks[block].instructions.push(MirInst::Assign {
            dest,
            value: MirValue::Literal(Literal::Int(0)),
        });
        dest
    }

    /// Lower `lhs == rhs` (or `lhs != rhs`) for a struct that derives
    /// `PartialEq`. Compares each field of the two structs in turn and
    /// returns the AND of all field equalities (for `Eq`) or its negation
    /// (for `NotEq`). Both sides must already have the struct shape
    /// described by `fields` (`(index, field_ty)` pairs).
    pub(super) fn lower_struct_partial_eq(
        &mut self,
        lhs: &HirExpr,
        rhs: &HirExpr,
        op: BinOp,
        fields: &[(usize, Ty)],
    ) -> Result<LocalId, String> {
        let lhs_local = self
            .lower_expr(lhs)?
            .ok_or_else(|| "lhs of struct == has no value".to_string())?;
        let rhs_local = self
            .lower_expr(rhs)?
            .ok_or_else(|| "rhs of struct == has no value".to_string())?;

        if fields.is_empty() {
            let dest = self.new_temp(Ty::Bool);
            self.emit(MirInst::Assign {
                dest,
                value: MirValue::Literal(Literal::Bool(matches!(op, BinOp::Eq))),
            });
            return Ok(dest);
        }

        let mut acc: Option<LocalId> = None;
        for (idx, field_ty) in fields {
            let lf = self.new_temp(field_ty.clone());
            self.emit(MirInst::GetField {
                dest: lf,
                base: lhs_local,
                field_index: *idx,
            });
            let rf = self.new_temp(field_ty.clone());
            self.emit(MirInst::GetField {
                dest: rf,
                base: rhs_local,
                field_index: *idx,
            });

            let field_eq = self.new_temp(Ty::Bool);
            self.emit(MirInst::Compare {
                dest: field_eq,
                op: CmpOp::Eq,
                lhs: MirValue::Use(lf),
                rhs: MirValue::Use(rf),
            });

            acc = Some(match acc {
                None => field_eq,
                Some(prev) => {
                    let combined = self.new_temp(Ty::Bool);
                    self.emit(MirInst::BinOp {
                        dest: combined,
                        op: BinOp::And,
                        lhs: MirValue::Use(prev),
                        rhs: MirValue::Use(field_eq),
                    });
                    combined
                }
            });
        }

        let eq_result = acc.expect("non-empty fields handled above");
        if matches!(op, BinOp::NotEq) {
            let negated = self.new_temp(Ty::Bool);
            self.emit(MirInst::Not {
                dest: negated,
                operand: MirValue::Use(eq_result),
            });
            Ok(negated)
        } else {
            Ok(eq_result)
        }
    }

    /// Lower `<` / `<=` / `>` / `>=` on a struct that derives `Ord`
    /// (or `PartialOrd`) by calling the synthesised
    /// `<Type>_cmp` / `<Type>_partial_cmp`, then comparing its
    /// `-1 / 0 / +1` result to `0` according to `op`.
    pub(super) fn lower_struct_ord(
        &mut self,
        lhs: &HirExpr,
        rhs: &HirExpr,
        op: BinOp,
        struct_name: &str,
        partial: bool,
    ) -> Result<LocalId, String> {
        let lhs_local = self
            .lower_expr(lhs)?
            .ok_or_else(|| "lhs of struct ordering has no value".to_string())?;
        let rhs_local = self
            .lower_expr(rhs)?
            .ok_or_else(|| "rhs of struct ordering has no value".to_string())?;

        let method_name = if partial { "partial_cmp" } else { "cmp" };
        let cmp = self.new_temp(Ty::Int);
        self.emit(MirInst::Call {
            dest: Some(cmp),
            callee: format!("{}_{}", struct_name, method_name),
            args: vec![MirValue::Use(lhs_local), MirValue::Use(rhs_local)],
        });

        let cmp_op = binop_to_cmpop(op);
        let dest = self.new_temp(Ty::Bool);
        self.emit(MirInst::Compare {
            dest,
            op: cmp_op,
            lhs: MirValue::Use(cmp),
            rhs: MirValue::Literal(Literal::Int(0)),
        });
        Ok(dest)
    }
}
