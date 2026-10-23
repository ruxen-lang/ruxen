//! Field and element drop glue: which fields an object owns, the per-type
//! field glue (`__ruxen_drop_fields_<T>`), element descriptors
//! (`RUXEN_ELEM_*`) and element glue.

use super::*;

/// Name prefix of the synthesised per-object-type field drop glue
/// (`__ruxen_drop_fields_User`, `__ruxen_drop_fields_Box__mono__String`),
/// see [`Lowerer::synthesize_field_drop_glue`]. Prefixed rather than
/// `<Class>_drop_fields` so a user method named `drop_fields` cannot be
/// mistaken for it. Only [`field_drop_glue_name`] spells it; a pass that
/// needs to recognise a glue call asks [`is_field_glue_call`], which reads
/// the calling body's record, never the name.
pub(super) const FIELD_DROP_GLUE_PREFIX: &str = "__ruxen_drop_fields_";

/// True when a field of type `ty` owns the value stored in it, so the
/// object frees it when it dies or when the field is overwritten: a
/// `String`, `Array`, `Hash` or `Set`, a tuple (a heap box that owns its
/// elements; a Copy one gets its own box per owner, `is_boxed_copy`), an
/// instance of a class, a boxed Copy struct (each owner holds its own box,
/// `drops::copies`), or a closure ([`is_owned_closure`]). Every other field
/// (scalars, references, other struct and enum values, other `any Mixin`
/// objects, an unsubstituted type parameter) is not freed.
pub(in crate::mir::lower) fn field_owns_value(ty: &Ty, symbols: &SymbolTable) -> bool {
    use crate::resolve::symbols::DefKind;
    match ty {
        Ty::String | Ty::Array(_) | Ty::Map(_, _) | Ty::Set(_) | Ty::Tuple(_) => true,
        _ if is_owned_closure(ty) => true,
        Ty::Struct { .. } => super::copies::is_boxed_copy(ty, symbols),
        Ty::Class { name, .. } => symbols
            .type_def(name)
            .is_some_and(|def| matches!(def.kind, DefKind::Class { .. })),
        _ => false,
    }
}

/// The `(slot, type)` of every field an instance of `ty` owns (see
/// [`field_owns_value`]), parent-first and shifted past the `class_info`
/// header: the layout `alloc_size` budgets and every `GetField` reads. The
/// class's generic parameters are replaced by `ty`'s arguments. A tuple's
/// slots are its elements, in order. Empty for any other type and for a
/// class that does not own its fields ([`SymbolTable::class_owns_fields`]).
pub(in crate::mir::lower) fn owned_field_slots(ty: &Ty, symbols: &SymbolTable) -> Vec<(usize, Ty)> {
    use crate::resolve::symbols::DefKind;
    if let Ty::Tuple(elems) = ty {
        return elems
            .iter()
            .enumerate()
            .filter(|(_, elem)| field_owns_value(elem, symbols))
            .map(|(slot, elem)| (slot, elem.clone()))
            .collect();
    }
    let Ty::Class { name, generic_args } = ty else {
        return Vec::new();
    };
    if !symbols.class_owns_fields(name) {
        return Vec::new();
    }
    let Some(DefKind::Class { info }) = symbols.type_def(name).map(|def| &def.kind) else {
        return Vec::new();
    };
    let subst: HashMap<String, Ty> = info
        .generic_params
        .iter()
        .map(|param| param.name.clone())
        .zip(generic_args.iter().cloned())
        .collect();
    let mut chain = vec![info];
    let mut seen = HashSet::new();
    while let Some(parent) = chain.last().and_then(|info| info.parent) {
        if !seen.insert(parent) {
            break;
        }
        match symbols.get(parent).map(|def| &def.kind) {
            Some(DefKind::Class { info }) => chain.push(info),
            _ => break,
        }
    }
    chain.reverse();
    let mut slot = crate::mir::lower::type_helpers::class_field_index_shift_in(name, symbols);
    let mut owned = Vec::new();
    for info in chain {
        for &field in &info.fields {
            let Some(DefKind::Field { ty, .. }) = symbols.get(field).map(|def| &def.kind) else {
                continue;
            };
            let ty = ty.subst_params(&subst);
            if field_owns_value(&ty, symbols) {
                owned.push((slot, ty));
            }
            slot += 1;
        }
    }
    owned
}

/// The field drop glue an instance of `ty` needs before its storage is
/// freed, or `None` when it owns no field. Named per object type: a generic
/// class instance gets its own glue over its substituted field types, a
/// tuple one per element types.
pub(in crate::mir::lower) fn field_drop_glue_name(
    ty: &Ty,
    symbols: &SymbolTable,
) -> Option<String> {
    if owned_field_slots(ty, symbols).is_empty() {
        return None;
    }
    Some(format!("{FIELD_DROP_GLUE_PREFIX}{}", object_glue_key(ty)?))
}

/// The per-type part of a glue name for a class, struct or tuple `ty`.
fn object_glue_key(ty: &Ty) -> Option<String> {
    Some(match ty {
        Ty::Class { name, generic_args } | Ty::Struct { name, generic_args }
            if generic_args.is_empty() =>
        {
            name.replace('.', "_")
        }
        Ty::Class { name, generic_args } | Ty::Struct { name, generic_args } => {
            crate::mir::lower::instances::mono_base(name, generic_args)
        }
        // Spelled from the full type, borrows included: `(&String, Int)`
        // owns nothing that `(String, Int)` owns, so their glue differs.
        Ty::Tuple(_) => format!(
            "Tuple_{}",
            crate::mir::lower::instances::symbol_token(&ty.to_string())
        ),
        _ => return None,
    })
}

/// Whether a call to `callee` in `func` is field drop glue
/// (`MirFunction::field_glue_calls`, recorded where the call was emitted):
/// the glue only reads its object, ahead of the object's own free.
pub(in crate::mir::lower) fn is_field_glue_call(func: &MirFunction, callee: &str) -> bool {
    func.field_glue_calls.contains_key(callee)
}

/// Wire values of the element-descriptor tags. Must stay in step with the
/// `RUXEN_ELEM_*` constants in `library/std/core/runtime/runtime.h`.
pub(super) const ELEM_POD: i64 = 0;
pub(super) const ELEM_CSTR: i64 = 1;
pub(super) const ELEM_VEC: i64 = 3;
pub(super) const ELEM_HASH: i64 = 4;
pub(super) const ELEM_SET: i64 = 5;
pub(super) const ELEM_OBJ: i64 = 6;
pub(super) const ELEM_CLOSURE: i64 = 7;
pub(super) const ELEM_BORROW: i64 = 8;

/// A descriptor is at most 16 four-bit tags (one `i64`).
pub(super) const ELEM_DESC_MAX_TAGS: usize = 16;

/// How many tags may precede an OBJ tag. The glue address fills the word
/// above the OBJ tag, so it is shifted left four bits per preceding tag plus
/// four for OBJ itself; a 48-bit code address still fits after two.
pub(super) const ELEM_OBJ_MAX_PREFIX_TAGS: usize = 2;

/// Name prefix of the synthesised per-type element glue functions
/// (`__ruxen_elem_glue_Point`), see [`Lowerer::synthesize_elem_glue`].
pub(in crate::mir::lower) const ELEM_GLUE_PREFIX: &str = "__ruxen_elem_glue_";

/// What one collection slot owns, as built by [`elem_desc`].
#[derive(Debug, Clone, PartialEq)]
pub(in crate::mir::lower) enum ElemDesc {
    /// A descriptor word known at compile time.
    Word(i64),
    /// A stream ending in OBJ: `tags` holds the `prefix_tags` preceding tags
    /// and the OBJ tag, and the address of the function `glue` is stored
    /// above them at run time.
    Glue {
        tags: i64,
        prefix_tags: usize,
        glue: String,
        /// The type of the object the glue frees and copies.
        object: Ty,
    },
}

/// What one collection slot of type `ty` owns, as a `RUXEN_ELEM_*`
/// descriptor: a pre-order stream of 4-bit tags, low nibble first.
///
/// The ONE builder of element descriptors. Drop glue frees a slot per this
/// word and `ruxen_elem_clone` (the derived-collection helpers, rewritten in
/// `key_desc.rs`) copies a slot per the same word, so copy depth and drop
/// depth cannot drift apart. A type that does not fit is described to the
/// deepest level that fits, with POD below it: such a value leaks its
/// deepest elements rather than risking a free of something the copy
/// aliased.
pub(in crate::mir::lower) fn elem_desc(ty: &Ty, symbols: &SymbolTable) -> ElemDesc {
    for depth in (0..=ELEM_DESC_MAX_TAGS).rev() {
        let mut tags = Vec::new();
        let glue = push_elem_tags(ty, depth, symbols, &mut tags);
        if tags.len() > ELEM_DESC_MAX_TAGS {
            continue;
        }
        let word = tags
            .iter()
            .enumerate()
            .fold(0, |desc, (i, tag)| desc | (tag << (4 * i)));
        match glue {
            Some((glue, object)) if tags.len() <= ELEM_OBJ_MAX_PREFIX_TAGS + 1 => {
                return ElemDesc::Glue {
                    tags: word,
                    prefix_tags: tags.len() - 1,
                    glue,
                    object,
                }
            }
            None => return ElemDesc::Word(word),
            _ => {}
        }
    }
    ElemDesc::Word(ELEM_POD)
}

/// The key descriptor of `ty` (`KeyDesc[K]`, runtime.h): how a table copies,
/// frees, hashes and compares a key of this type. Its element descriptor,
/// with a borrowed key described as BORROW followed by what it borrows (a
/// `&String` key owns nothing but hashes by content). A borrow whose
/// referent does not fit is a plain BORROW, which hashes by the word.
pub(in crate::mir::lower) fn key_desc(ty: &Ty, symbols: &SymbolTable) -> ElemDesc {
    match ty {
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => match key_desc(inner, symbols) {
            ElemDesc::Word(word) if word >> 60 == 0 => ElemDesc::Word(ELEM_BORROW | (word << 4)),
            ElemDesc::Glue {
                tags,
                prefix_tags,
                glue,
                object,
            } if prefix_tags < ELEM_OBJ_MAX_PREFIX_TAGS => ElemDesc::Glue {
                tags: ELEM_BORROW | (tags << 4),
                prefix_tags: prefix_tags + 1,
                glue,
                object,
            },
            _ => ElemDesc::Word(ELEM_BORROW),
        },
        _ => elem_desc(ty, symbols),
    }
}

/// Append the tags for `ty` to `out`, and return the glue function (and the
/// object type it serves) when the stream ends in an OBJ tag.
pub(super) fn push_elem_tags(
    ty: &Ty,
    depth: usize,
    symbols: &SymbolTable,
    out: &mut Vec<i64>,
) -> Option<(String, Ty)> {
    match ty {
        Ty::String => {
            out.push(ELEM_CSTR);
            None
        }
        Ty::Array(elem) if depth > 0 => {
            out.push(ELEM_VEC);
            push_elem_tags(elem, depth - 1, symbols, out)
        }
        Ty::Map(key, val) if depth > 0 => {
            out.push(ELEM_HASH);
            out.push(key_elem_desc(key));
            push_elem_tags(val, depth - 1, symbols, out)
        }
        Ty::Set(elem) if depth > 0 => {
            out.push(ELEM_SET);
            out.push(key_elem_desc(elem));
            None
        }
        _ if is_owned_closure(ty) => {
            out.push(ELEM_CLOSURE);
            None
        }
        _ => {
            let glue = obj_glue_name(ty, symbols);
            out.push(if glue.is_some() { ELEM_OBJ } else { ELEM_POD });
            glue.map(|glue| (glue, ty.clone()))
        }
    }
}

/// The element glue for a slot holding an instance of `ty`, or `None` when
/// such a slot stays POD (the container never frees it). A non-generic class
/// or struct qualifies when it has a `clone` (derived or declared): a
/// derived-collection helper must be able to copy what the drop will free.
/// A tuple qualifies when every element it owns has a descriptor, which is
/// how its glue frees and copies that element. Generic classes stay POD
/// because their methods are per-instantiation symbols; enums because their
/// payloads have no drop rule yet (`docs/specs/ownership/drop.spec.md`, out
/// of scope).
pub(super) fn obj_glue_name(ty: &Ty, symbols: &SymbolTable) -> Option<String> {
    if let Ty::Tuple(_) = ty {
        let described = owned_field_slots(ty, symbols)
            .iter()
            .all(|(_, elem)| !matches!(elem_desc(elem, symbols), ElemDesc::Word(ELEM_POD)));
        if !described {
            return None;
        }
        return Some(format!("{ELEM_GLUE_PREFIX}{}", object_glue_key(ty)?));
    }
    let name = match ty {
        Ty::Class { name, generic_args } | Ty::Struct { name, generic_args }
            if generic_args.is_empty() && !name.contains('.') =>
        {
            name
        }
        _ => return None,
    };
    crate::implicit_includes::derive_applies(ty, symbols, "Clone")
        .then(|| format!("{ELEM_GLUE_PREFIX}{name}"))
}

/// Record in `func` the element glue `desc` names (see
/// `MirFunction::elem_glue_types`); every emitter of a descriptor calls this.
pub(in crate::mir::lower) fn record_elem_glue(func: &mut MirFunction, desc: &ElemDesc) {
    if let ElemDesc::Glue { glue, object, .. } = desc {
        func.elem_glue_types.insert(glue.clone(), object.clone());
    }
}

/// Emit the instructions that compute `desc` into `out` and return the
/// value to pass. A `Glue` descriptor ORs the glue function's address,
/// shifted above the tag stream, into the tags; a `Word` is a literal.
pub(in crate::mir::lower) fn elem_desc_value(
    desc: ElemDesc,
    new_temp: &mut dyn FnMut() -> LocalId,
    out: &mut Vec<MirInst>,
) -> MirValue {
    match desc {
        ElemDesc::Word(word) => MirValue::Literal(Literal::Int(word)),
        ElemDesc::Glue {
            tags,
            prefix_tags,
            glue,
            ..
        } => {
            let addr = new_temp();
            out.push(MirInst::FuncAddr {
                dest: addr,
                func_name: glue,
            });
            let shifted = new_temp();
            out.push(MirInst::BinOp {
                dest: shifted,
                op: BinOp::Shl,
                lhs: MirValue::Use(addr),
                rhs: MirValue::Literal(Literal::Int(4 * (prefix_tags as i64 + 1))),
            });
            let word = new_temp();
            out.push(MirInst::BinOp {
                dest: word,
                op: BinOp::BitOr,
                lhs: MirValue::Use(shifted),
                rhs: MirValue::Literal(Literal::Int(tags)),
            });
            MirValue::Use(word)
        }
    }
}

impl<'a> Lowerer<'a> {
    /// Define every element glue function an OBJ descriptor names
    /// (`FuncAddr __ruxen_elem_glue_<Type>`, see [`elem_desc`]). Runs once
    /// over the whole program, after every body (and every drop) is lowered.
    ///
    /// `glue(op, slot, other)` mirrors a scope-exit drop of a `<Type>` local
    /// for `op == RUXEN_ELEM_GLUE_DROP` (the type's `def drop`, then
    /// `ruxen_dealloc` unless that drop releases the storage itself; for a
    /// tuple, its field glue and `ruxen_dealloc`), returns an owned copy
    /// for `op == RUXEN_ELEM_GLUE_CLONE` (`<Type>_clone(slot)`; for a tuple,
    /// a new box of element copies), and hashes `slot` or compares it with
    /// `other` as a table key for `RUXEN_ELEM_GLUE_HASH` / `_EQ`
    /// ([`Lowerer::build_key_glue_arms`]). The glue wanted and the type each one
    /// serves come from `MirFunction::elem_glue_types`; a glue body names
    /// further glue, so the caller repeats until nothing is added.
    pub(in crate::mir::lower) fn synthesize_elem_glue(
        &self,
        mir: &mut MirProgram,
    ) -> Result<(), String> {
        let defined: HashSet<String> = mir.functions.iter().map(|f| f.name.clone()).collect();
        let wanted: std::collections::BTreeMap<String, Ty> = mir
            .functions
            .iter()
            .flat_map(|f| f.elem_glue_types.iter())
            .filter(|(glue, _)| !defined.contains(*glue))
            .map(|(glue, ty)| (glue.clone(), ty.clone()))
            .collect();
        for (glue_name, object_ty) in wanted {
            let glue = self.build_elem_glue(&glue_name, &object_ty, &defined)?;
            mir.functions.push(glue);
        }
        Ok(())
    }

    fn build_elem_glue(
        &self,
        glue_name: &str,
        object_ty: &Ty,
        defined: &HashSet<String>,
    ) -> Result<MirFunction, String> {
        let mut glue = MirFunction::new(glue_name, Ty::Int);
        let op = glue.new_local("op", Ty::Int, false);
        let slot = glue.new_local("slot", Ty::Int, false);
        let other = glue.new_local("other", Ty::Int, false);
        glue.params = vec![op, slot, other];

        // `op` is RUXEN_ELEM_GLUE_DROP (0), _CLONE (1), _HASH (2) or _EQ (3).
        let drop_block = glue.new_block();
        let clone_block = glue.new_block();
        let hash_block = glue.new_block();
        let eq_block = glue.new_block();
        let entry = glue.entry_block;
        let mut test = entry;
        for (code, then_block) in [(0, drop_block), (1, clone_block), (2, hash_block)] {
            let is_op = glue.new_temp(Ty::Bool);
            glue.blocks[test].instructions.push(MirInst::Compare {
                dest: is_op,
                op: CmpOp::Eq,
                lhs: MirValue::Use(op),
                rhs: MirValue::Literal(Literal::Int(code)),
            });
            let else_block = if code == 2 {
                eq_block
            } else {
                glue.new_block()
            };
            glue.blocks[test].terminator = Terminator::Branch {
                cond: MirValue::Use(is_op),
                then_block,
                else_block,
            };
            test = else_block;
        }
        self.build_key_glue_arms(&mut glue, object_ty, slot, other, hash_block, eq_block);
        let object = glue.new_temp(object_ty.clone());
        glue.blocks[entry].instructions.push(MirInst::Assign {
            dest: object,
            value: MirValue::Use(slot),
        });

        let user_drop = self.user_drop_callee(object_ty);
        let free = owned_value_drop(&mut glue, self.symbols, user_drop, object_ty, object);
        glue.blocks[drop_block].instructions.extend(free);
        let none = glue.new_temp(Ty::Int);
        glue.blocks[drop_block].instructions.push(MirInst::Assign {
            dest: none,
            value: MirValue::Literal(Literal::Int(0)),
        });
        glue.blocks[drop_block].terminator = Terminator::Return(Some(MirValue::Use(none)));

        let copy = match object_ty {
            Ty::Tuple(_) => self.synthesize_clone_field(&mut glue, clone_block, object, object_ty),
            Ty::Class { name, .. } | Ty::Struct { name, .. } => {
                let clone_mangled = format!("{name}_clone");
                let clone_callee = self.resolve_ffi_alias_callee(clone_mangled.clone());
                if clone_callee == clone_mangled && !defined.contains(&clone_callee) {
                    return Err(format!(
                        "element glue for `{name}` needs `{clone_mangled}`, which was not lowered"
                    ));
                }
                let copy = glue.new_temp(Ty::Int);
                glue.blocks[clone_block].instructions.push(MirInst::Call {
                    dest: Some(copy),
                    callee: clone_callee,
                    args: vec![MirValue::Use(slot)],
                });
                copy
            }
            _ => unreachable!("`obj_glue_name` admits only a class, struct or tuple"),
        };
        glue.blocks[clone_block].terminator = Terminator::Return(Some(MirValue::Use(copy)));
        Ok(glue)
    }

    /// The HASH and EQ arms of `object_ty`'s element glue: a type that
    /// derives `Hashable` and `PartialEq` hashes and compares through the
    /// methods those derives define (`hash_code`, `eq`), any other by its
    /// identity (the word), as Ruby's default `Object#hash` does.
    fn build_key_glue_arms(
        &self,
        glue: &mut MirFunction,
        object_ty: &Ty,
        slot: LocalId,
        other: LocalId,
        hash_block: BlockId,
        eq_block: BlockId,
    ) {
        let derived = |trait_name: &str| {
            crate::mir::lower::type_helpers::derived_nominal(object_ty, self.symbols, trait_name)
                .filter(|name| !name.contains('.'))
                .zip(self.symbols.derived_method(trait_name))
                .map(|(name, method)| format!("{name}_{method}"))
        };
        let hash = derived(crate::implicit_includes::HASHABLE);
        let eq = derived("PartialEq");
        let (hash_value, eq_value) = match hash.zip(eq) {
            Some((hash_fn, eq_fn)) => {
                let h = glue.new_temp(Ty::Int);
                glue.blocks[hash_block].instructions.push(MirInst::Call {
                    dest: Some(h),
                    callee: hash_fn,
                    args: vec![MirValue::Use(slot)],
                });
                let e = glue.new_temp(Ty::Bool);
                glue.blocks[eq_block].instructions.push(MirInst::Call {
                    dest: Some(e),
                    callee: eq_fn,
                    args: vec![MirValue::Use(slot), MirValue::Use(other)],
                });
                (h, e)
            }
            None => {
                let e = glue.new_temp(Ty::Bool);
                glue.blocks[eq_block].instructions.push(MirInst::Compare {
                    dest: e,
                    op: CmpOp::Eq,
                    lhs: MirValue::Use(slot),
                    rhs: MirValue::Use(other),
                });
                (slot, e)
            }
        };
        glue.blocks[hash_block].terminator = Terminator::Return(Some(MirValue::Use(hash_value)));
        let word = glue.new_temp(Ty::Int);
        glue.blocks[eq_block].instructions.push(MirInst::Assign {
            dest: word,
            value: MirValue::Use(eq_value),
        });
        glue.blocks[eq_block].terminator = Terminator::Return(Some(MirValue::Use(word)));
    }

    /// The user `def drop` an instance of `ty` runs before its fields and
    /// storage are freed (the callee scope-exit drop calls), and whether
    /// that callee releases the storage itself
    /// (`def consume drop`, `SymbolTable::drop_consumes_self`). `None`
    /// without a `def drop`.
    pub(in crate::mir::lower) fn user_drop_callee(&self, ty: &Ty) -> Option<(String, bool)> {
        let Ty::Class { name, .. } = ty else {
            return None;
        };
        if !is_drop_class(self.symbols, &self.user_drop_classes, name) {
            return None;
        }
        let callee = self.resolve_ffi_alias_callee(format!("{name}_drop"));
        let frees_storage = self.symbols.drop_consumes_self(name);
        Some((callee, frees_storage))
    }

    /// Define every field drop glue a call names (`__ruxen_drop_fields_<T>`,
    /// see [`heap_free_call`]) and no function defines yet. A glue drops
    /// nested objects, which can name further glue, so the caller repeats
    /// until nothing is added.
    ///
    /// `glue(self)` does nothing for NULL; otherwise it frees each owned
    /// field ([`owned_field_slots`]) in reverse slot order, skipping a NULL
    /// field (one never stored): a nested object runs its `def drop`, then its
    /// own field glue and storage free, exactly as a scope-exit drop of a
    /// local of that type would.
    pub(in crate::mir::lower) fn synthesize_field_drop_glue(
        &self,
        mir: &mut MirProgram,
    ) -> Result<(), String> {
        let defined: HashSet<String> = mir
            .functions
            .iter()
            .filter(|f| f.role == MirFnRole::FieldGlue)
            .map(|f| f.name.clone())
            .collect();
        let wanted: std::collections::BTreeMap<String, Ty> = mir
            .functions
            .iter()
            .flat_map(|f| f.field_glue_calls.iter())
            .filter(|(callee, _)| !defined.contains(*callee))
            .map(|(callee, ty)| (callee.clone(), ty.clone()))
            .collect();
        for (name, object_ty) in wanted {
            mir.functions
                .push(self.build_field_drop_glue(&name, &object_ty));
        }
        Ok(())
    }

    fn build_field_drop_glue(&self, name: &str, object_ty: &Ty) -> MirFunction {
        let mut glue = MirFunction::new(name, Ty::Unit);
        glue.role = MirFnRole::FieldGlue;
        let object = glue.new_local("self", object_ty.clone(), false);
        glue.params = vec![object];
        let entry = glue.entry_block;
        let done = glue.new_block();
        let mut current = glue.new_block();
        let is_null = glue.new_temp(Ty::Bool);
        glue.blocks[entry].instructions.push(MirInst::Compare {
            dest: is_null,
            op: CmpOp::Eq,
            lhs: MirValue::Use(object),
            rhs: MirValue::Literal(Literal::Int(0)),
        });
        glue.blocks[entry].terminator = Terminator::Branch {
            cond: MirValue::Use(is_null),
            then_block: done,
            else_block: current,
        };
        for (slot, field_ty) in owned_field_slots(object_ty, self.symbols).into_iter().rev() {
            let value = glue.new_temp(field_ty.clone());
            glue.blocks[current].instructions.push(MirInst::GetField {
                dest: value,
                base: object,
                field_index: slot,
            });
            let live = glue.new_temp(Ty::Bool);
            glue.blocks[current].instructions.push(MirInst::Compare {
                dest: live,
                op: CmpOp::NotEq,
                lhs: MirValue::Use(value),
                rhs: MirValue::Literal(Literal::Int(0)),
            });
            let drop_block = glue.new_block();
            let next = glue.new_block();
            glue.blocks[current].terminator = Terminator::Branch {
                cond: MirValue::Use(live),
                then_block: drop_block,
                else_block: next,
            };
            let user_drop = self.user_drop_callee(&field_ty);
            let free = owned_value_drop(&mut glue, self.symbols, user_drop, &field_ty, value);
            glue.blocks[drop_block].instructions.extend(free);
            glue.blocks[drop_block].terminator = Terminator::Goto(next);
            current = next;
        }
        glue.blocks[current].terminator = Terminator::Goto(done);
        glue.blocks[done].terminator = Terminator::Return(None);
        glue
    }
}

/// The destruction of the owned, non-NULL value `value` of field type `ty`
/// (see [`field_owns_value`]): for a closure, its release through the pair's
/// own drop slot ([`closure_drop_call`]); otherwise `user_drop` (from
/// [`Lowerer::user_drop_callee`]) first, then, unless that drop released the
/// storage itself, the [`heap_free_call`], which for an object includes its
/// field glue. Temps are allocated in `func`.
pub(in crate::mir::lower) fn owned_value_drop(
    func: &mut MirFunction,
    symbols: &SymbolTable,
    user_drop: Option<(String, bool)>,
    ty: &Ty,
    value: LocalId,
) -> Vec<MirInst> {
    if is_owned_closure(ty) {
        return vec![closure_drop_call(value)];
    }
    let mut insts = Vec::new();
    let mut frees_storage = false;
    if let Some((callee, frees)) = user_drop {
        frees_storage = frees;
        insts.push(MirInst::Call {
            dest: None,
            callee,
            args: vec![MirValue::Use(value)],
        });
    }
    if !frees_storage {
        insts.extend(
            heap_free_call(func, symbols, ty, value).expect("an owned field value has a heap free"),
        );
    }
    insts
}

/// The key part of a `Hash` or `Set` descriptor stream (RUXEN_ELEM_HASH /
/// _SET): CSTR or POD only, so the value's descriptor that follows it is
/// found and fits. A table copies and frees its keys by its own installed
/// key descriptor (`key_desc`, filled from `KeyDesc[K]`), not by this.
pub(in crate::mir::lower) fn key_elem_desc(ty: &Ty) -> i64 {
    match ty {
        Ty::String => ELEM_CSTR,
        _ => ELEM_POD,
    }
}

#[cfg(test)]
mod elem_desc_tests {
    use super::*;

    fn array(elem: Ty) -> Ty {
        Ty::Array(Box::new(elem))
    }

    fn word(ty: &Ty) -> i64 {
        match elem_desc(ty, &SymbolTable::new()) {
            ElemDesc::Word(word) => word,
            glue => panic!("expected a compile-time word, got {glue:?}"),
        }
    }

    #[test]
    fn nested_types_encode_pre_order_low_nibble_first() {
        // Array[String]: VEC then CSTR.
        assert_eq!(word(&array(Ty::String)), 0x13);
        // Hash[String, Array[Int]]: HASH, key CSTR, value VEC then POD.
        let map = Ty::Map(Box::new(Ty::String), Box::new(array(Ty::Int)));
        assert_eq!(word(&map), 0x314);
        // A Set element is a key: never deeper than CSTR.
        assert_eq!(word(&Ty::Set(Box::new(array(Ty::String)))), 0x05);
    }

    #[test]
    fn a_type_deeper_than_sixteen_tags_is_cut_to_pod() {
        let mut deep = Ty::String;
        for _ in 0..20 {
            deep = array(deep);
        }
        // Fifteen VEC tags and a POD leaf: the deepest levels leak rather
        // than overflow the word.
        assert_eq!(word(&deep), 0x0333_3333_3333_3333);
    }

    #[test]
    fn a_glue_descriptor_ors_the_shifted_glue_address_into_its_tags() {
        let mut next = 10;
        let mut new_temp = || {
            next += 1;
            next
        };
        let mut out = Vec::new();
        let desc = ElemDesc::Glue {
            tags: 0x63,
            prefix_tags: 1,
            glue: format!("{ELEM_GLUE_PREFIX}Point"),
            object: Ty::Struct {
                name: "Point".to_string(),
                generic_args: vec![],
            },
        };
        let value = elem_desc_value(desc, &mut new_temp, &mut out);
        assert!(matches!(value, MirValue::Use(13)));
        assert!(matches!(
            &out[..],
            [
                MirInst::FuncAddr { dest: 11, func_name },
                MirInst::BinOp { dest: 12, op: BinOp::Shl, rhs: MirValue::Literal(Literal::Int(8)), .. },
                MirInst::BinOp { dest: 13, op: BinOp::BitOr, rhs: MirValue::Literal(Literal::Int(0x63)), .. },
            ] if func_name == "__ruxen_elem_glue_Point"
        ));
    }
}
