//! Type layout computation — size, alignment, field offsets.
//!
//! Every Riven type has a well-defined in-memory layout:
//! - `size`: number of bytes occupied
//! - `alignment`: required byte-boundary alignment
//! - `field_offsets`: byte offset of each field (for composite types)

use crate::hir::types::Ty;
use crate::resolve::symbols::{DefKind, SymbolTable, VariantDefKind};

/// Memory layout information for a single type.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeLayout {
    /// Total size in bytes (including any trailing padding).
    pub size: usize,
    /// Required alignment in bytes (always a power of two).
    pub alignment: usize,
    /// Byte offset of each field within the type (for structs/tuples/enums).
    pub field_offsets: Vec<usize>,
}

impl TypeLayout {
    /// Convenience constructor for primitive/opaque types with no fields.
    fn primitive(size: usize, alignment: usize) -> Self {
        TypeLayout {
            size,
            alignment,
            field_offsets: vec![],
        }
    }
}

/// Round `offset` up to the nearest multiple of `alignment`.
///
/// `alignment` must be a power of two.
pub fn align_up(offset: usize, alignment: usize) -> usize {
    debug_assert!(
        alignment.is_power_of_two(),
        "alignment must be a power of two"
    );
    if alignment == 0 {
        return offset;
    }
    (offset + alignment - 1) & !(alignment - 1)
}

/// Lay out a sequence of fields sequentially, inserting alignment padding between
/// them and adding trailing padding so the total size is a multiple of the
/// struct's own alignment (= max field alignment).
///
/// Returns a `TypeLayout` whose `field_offsets` has one entry per input field.
pub fn layout_struct_fields(fields: &[TypeLayout]) -> TypeLayout {
    if fields.is_empty() {
        return TypeLayout::primitive(0, 1);
    }

    let mut offset = 0usize;
    let mut max_align = 1usize;
    let mut offsets = Vec::with_capacity(fields.len());

    for field in fields {
        // Align current offset to this field's alignment requirement.
        offset = align_up(offset, field.alignment);
        offsets.push(offset);
        offset += field.size;
        if field.alignment > max_align {
            max_align = field.alignment;
        }
    }

    // Trailing padding: total size must be a multiple of the struct's alignment.
    let total_size = align_up(offset, max_align);

    TypeLayout {
        size: total_size,
        alignment: max_align,
        field_offsets: offsets,
    }
}

/// Compute C-compatible struct layout.
///
/// Fields are in declaration order with C alignment rules. This is
/// identical to `layout_struct_fields` but is separate to make the
/// intent explicit for `@[repr(C)]` structs.
pub fn c_struct_layout(fields: &[TypeLayout]) -> TypeLayout {
    layout_struct_fields(fields)
}

/// Compute packed struct layout (`@[repr(C, packed)]`).
///
/// All alignment padding is removed — each field immediately follows
/// the previous one. The struct's own alignment is 1.
pub fn packed_struct_layout(fields: &[TypeLayout]) -> TypeLayout {
    if fields.is_empty() {
        return TypeLayout::primitive(0, 1);
    }

    let mut offset = 0usize;
    let mut offsets = Vec::with_capacity(fields.len());

    for field in fields {
        offsets.push(offset);
        offset += field.size;
    }

    TypeLayout {
        size: offset,
        alignment: 1,
        field_offsets: offsets,
    }
}

/// Compute transparent struct layout (`@[repr(transparent)]`).
///
/// The struct must have exactly one non-zero-sized field. The layout
/// is identical to that field's layout.
pub fn transparent_struct_layout(fields: &[TypeLayout]) -> Result<TypeLayout, &'static str> {
    let non_zero: Vec<_> = fields.iter().filter(|f| f.size > 0).collect();
    if non_zero.len() != 1 {
        return Err("repr(transparent) requires exactly one non-zero-sized field");
    }
    Ok(non_zero[0].clone())
}

/// Lay out a tagged union (Option, Result, or Enum).
///
/// Layout: `[tag: u32 (4 bytes)] [padding] [payload: max(all variants)]`
///
/// The tag is always 4 bytes. The payload starts at the first offset that
/// satisfies the payload's alignment. Total size is rounded up to the
/// overall alignment.
fn layout_tagged_union(payload_sizes: &[usize], payload_aligns: &[usize]) -> TypeLayout {
    const TAG_SIZE: usize = 4;

    // Compute the maximum payload size and alignment across all variants.
    let max_payload_size = payload_sizes.iter().copied().max().unwrap_or(0);
    let max_payload_align = payload_aligns.iter().copied().max().unwrap_or(1);

    // Overall alignment = max(tag_align=4, payload_align)
    let overall_align = max_payload_align.max(TAG_SIZE);

    // Payload starts after the tag, aligned to the payload's own alignment.
    let payload_offset = align_up(TAG_SIZE, max_payload_align);

    // Total size before trailing padding.
    let raw_size = payload_offset + max_payload_size;

    // Round up to the overall alignment.
    let total_size = align_up(raw_size, overall_align);

    // We report field_offsets as [tag_offset=0, payload_offset].
    TypeLayout {
        size: total_size,
        alignment: overall_align,
        field_offsets: vec![0, payload_offset],
    }
}

/// Resolve a user-defined class or struct from the symbol table and compute
/// its layout based on the declared field types (in field-index order).
fn layout_user_type(name: &str, symbols: &SymbolTable) -> TypeLayout {
    // Find the class or struct definition by name.
    let def = symbols.iter().find(|d| {
        d.name == name && matches!(d.kind, DefKind::Class { .. } | DefKind::Struct { .. })
    });

    let field_def_ids: Vec<u32> = match def {
        Some(d) => match &d.kind {
            DefKind::Class { info } => info.fields.clone(),
            DefKind::Struct { info } => info.fields.clone(),
            _ => return TypeLayout::primitive(0, 1),
        },
        None => return TypeLayout::primitive(0, 1),
    };

    // Collect field layouts, sorted by their declared index.
    let mut indexed_fields: Vec<(usize, TypeLayout)> = field_def_ids
        .iter()
        .filter_map(|&fid| {
            let fdef = symbols.get(fid)?;
            if let DefKind::Field { ty, index, .. } = &fdef.kind {
                Some((*index, layout_of(ty, symbols)))
            } else {
                None
            }
        })
        .collect();

    indexed_fields.sort_by_key(|(idx, _)| *idx);
    let field_layouts: Vec<TypeLayout> = indexed_fields.into_iter().map(|(_, l)| l).collect();
    layout_struct_fields(&field_layouts)
}

/// Resolve a user-defined enum from the symbol table and compute its layout
/// as a tagged union over all variant payloads.
fn layout_user_enum(name: &str, symbols: &SymbolTable) -> TypeLayout {
    let def = symbols
        .iter()
        .find(|d| d.name == name && matches!(d.kind, DefKind::Enum { .. }));

    let variant_def_ids: Vec<u32> = match def {
        Some(d) => match &d.kind {
            DefKind::Enum { info } => info.variants.clone(),
            _ => return TypeLayout::primitive(4, 4), // just the tag
        },
        None => return TypeLayout::primitive(4, 4),
    };

    let mut payload_sizes = Vec::with_capacity(variant_def_ids.len());
    let mut payload_aligns = Vec::with_capacity(variant_def_ids.len());

    for vid in &variant_def_ids {
        if let Some(vdef) = symbols.get(*vid) {
            if let DefKind::EnumVariant { kind, .. } = &vdef.kind {
                match kind {
                    VariantDefKind::Unit => {
                        payload_sizes.push(0);
                        payload_aligns.push(1);
                    }
                    VariantDefKind::Tuple(tys) => {
                        // Lay out tuple payload.
                        let field_layouts: Vec<TypeLayout> =
                            tys.iter().map(|t| layout_of(t, symbols)).collect();
                        let pl = layout_struct_fields(&field_layouts);
                        payload_sizes.push(pl.size);
                        payload_aligns.push(pl.alignment);
                    }
                    VariantDefKind::Struct(fields) => {
                        let field_layouts: Vec<TypeLayout> =
                            fields.iter().map(|(_, t)| layout_of(t, symbols)).collect();
                        let pl = layout_struct_fields(&field_layouts);
                        payload_sizes.push(pl.size);
                        payload_aligns.push(pl.alignment);
                    }
                }
            }
        }
    }

    if payload_sizes.is_empty() {
        // Enum with no variants — degenerate (never-constructable).
        return TypeLayout::primitive(4, 4);
    }

    layout_tagged_union(&payload_sizes, &payload_aligns)
}

/// Compute the memory layout of any Riven type.
///
/// # Arguments
/// - `ty` — the type to compute the layout for
/// - `symbols` — the symbol table (used for user-defined types)
pub fn layout_of(ty: &Ty, symbols: &SymbolTable) -> TypeLayout {
    match ty {
        // ── Primitives ──────────────────────────────────────────────────────
        Ty::Bool | Ty::Int8 | Ty::UInt8 => TypeLayout::primitive(1, 1),
        Ty::Int16 | Ty::UInt16 => TypeLayout::primitive(2, 2),
        Ty::Int32 | Ty::UInt32 | Ty::Float32 | Ty::Char => TypeLayout::primitive(4, 4),
        Ty::Int
        | Ty::Int64
        | Ty::UInt
        | Ty::UInt64
        | Ty::ISize
        | Ty::USize
        | Ty::Float
        | Ty::Float64 => TypeLayout::primitive(8, 8),
        Ty::Unit | Ty::Never => TypeLayout::primitive(0, 1),

        // ── Strings ─────────────────────────────────────────────────────────
        // &str — fat pointer: (ptr, len) = 16 bytes, align 8
        Ty::Str => TypeLayout::primitive(16, 8),
        // String — (ptr, len, cap) = 24 bytes, align 8
        Ty::String => TypeLayout::primitive(24, 8),

        // ── References ──────────────────────────────────────────────────────
        // All thin reference/pointer types are one machine word.
        Ty::Ref(_) | Ty::RefMut(_) | Ty::RefLifetime(_, _) | Ty::RefMutLifetime(_, _) => {
            TypeLayout::primitive(8, 8)
        }

        // ── Raw Pointers ───────────────────────────────────────────────────
        // All raw pointer types are one machine word (8 bytes on 64-bit).
        Ty::RawPtr(_) | Ty::RawPtrMut(_) | Ty::RawPtrVoid | Ty::RawPtrMutVoid => {
            TypeLayout::primitive(8, 8)
        }

        // ── Collections ─────────────────────────────────────────────────────
        // Vec[T] — (ptr, len, cap) = 24 bytes, align 8
        Ty::Vec(_) => TypeLayout::primitive(24, 8),
        // HashMap[K,V] and Set[T] — 48 bytes (HashMap/HashSet header), align 8
        Ty::HashMap(_, _) | Ty::Set(_) => TypeLayout::primitive(48, 8),

        // ── Option[T] ───────────────────────────────────────────────────────
        Ty::Option(inner) => {
            let inner_layout = layout_of(inner, symbols);
            layout_tagged_union(&[inner_layout.size], &[inner_layout.alignment])
        }

        // ── Result[T, E] ────────────────────────────────────────────────────
        Ty::Result(ok, err) => {
            let ok_layout = layout_of(ok, symbols);
            let err_layout = layout_of(err, symbols);
            layout_tagged_union(
                &[ok_layout.size, err_layout.size],
                &[ok_layout.alignment, err_layout.alignment],
            )
        }

        // ── Tuple ───────────────────────────────────────────────────────────
        Ty::Tuple(elems) => {
            if elems.is_empty() {
                return TypeLayout::primitive(0, 1);
            }
            let field_layouts: Vec<TypeLayout> =
                elems.iter().map(|e| layout_of(e, symbols)).collect();
            layout_struct_fields(&field_layouts)
        }

        // ── Array ───────────────────────────────────────────────────────────
        Ty::Array(elem, n) => {
            let elem_layout = layout_of(elem, symbols);
            // T2.02 stage 4: `n` is now a `ConstExpr`.  Stage 7 will
            // pass full param bindings through; for now the only
            // reachable variant in codegen-time arrays is `Lit(N)`.
            // Unresolved `Param`s produce a zero-size layout, which
            // matches the pre-stage-4 fallback when the resolver
            // failed to read the literal.
            let count = n.as_lit().unwrap_or(0) as usize;
            TypeLayout {
                size: elem_layout.size * count,
                alignment: elem_layout.alignment,
                field_offsets: vec![],
            }
        }

        // ── User-defined types ───────────────────────────────────────────────
        Ty::Class { name, .. } | Ty::Struct { name, .. } => layout_user_type(name, symbols),
        Ty::Enum { name, .. } => layout_user_enum(name, symbols),

        // ── Transparent wrappers ────────────────────────────────────────────
        Ty::Alias { target, .. } => layout_of(target, symbols),
        Ty::Newtype { inner, .. } => layout_of(inner, symbols),

        // ── Function types ──────────────────────────────────────────────────
        // Fn, FnMut, FnOnce are represented as a single function pointer.
        Ty::Fn { .. } | Ty::FnMut { .. } | Ty::FnOnce { .. } => TypeLayout::primitive(8, 8),

        // ── Trait objects ───────────────────────────────────────────────────
        // dyn Trait = (data_ptr, vtable_ptr) = fat pointer, 16 bytes
        Ty::DynTrait(_) => TypeLayout::primitive(16, 8),

        // ── impl Trait (static dispatch) ────────────────────────────────────
        // The concrete type is erased here; we conservatively return pointer size.
        Ty::ImplTrait(_) => TypeLayout::primitive(8, 8),

        // ── Type parameters and inference variables ─────────────────────────
        // These should be monomorphised / resolved before layout is called.
        // Return a conservative placeholder (pointer-sized).
        Ty::TypeParam { .. } | Ty::Infer(_) => TypeLayout::primitive(8, 8),

        // ── Error sentinel ──────────────────────────────────────────────────
        Ty::Error => TypeLayout::primitive(0, 1),

        // ── Const-generic argument marker ───────────────────────────────────
        // `Ty::ConstArg` is a type-level marker inside a parent's
        // generic_args list, not a real value type.  It never reaches
        // layout for an actual variable; return zero-size.
        Ty::ConstArg(_) => TypeLayout::primitive(0, 1),
    }
}
