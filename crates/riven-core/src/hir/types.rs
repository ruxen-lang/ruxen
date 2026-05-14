//! Type representation for the Riven type system.
//!
//! Every type in Riven is represented as a `Ty`. During type inference,
//! unknown types use `Ty::Infer(TypeId)` which gets resolved through
//! unification. After type checking, all `Infer` types must be resolved
//! to concrete types.

use std::collections::HashSet;
use std::fmt;

use crate::resolve::symbols::SymbolTable;

/// Unique identifier for type variables during inference.
pub type TypeId = u32;

/// Tier-2 const generics (T2.02): a compile-time integer expression
/// that may appear in a type-level position (array size, generic-arg
/// slot of a const parameter).
///
/// Stage 4 ships the data layout; the evaluator and arithmetic
/// operators are stage 8.  For now the only producers are
/// `Lit` (from integer literals at use sites) and `Param` (from a
/// const-generic parameter reference inside a body).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ConstExpr {
    /// A concrete integer value.
    Lit(u64),
    /// A reference to an in-scope const generic parameter.
    Param(String),
    /// Arithmetic on other const expressions.  Stage 8 wiring.
    Op(Box<ConstExpr>, ConstOp, Box<ConstExpr>),
    /// Recovery placeholder so resolve doesn't crash on malformed
    /// const-arg positions.
    Error,
}

impl ConstExpr {
    /// Return the literal value if this expression is a bare `Lit`.
    pub fn as_lit(&self) -> Option<u64> {
        match self {
            ConstExpr::Lit(n) => Some(*n),
            _ => None,
        }
    }

    /// T2.02 S8.S4: rewrite to a canonical normal form by applying
    /// arithmetic identity rules.  Two `ConstExpr` values that
    /// denote the same compile-time integer for *every* binding
    /// should produce the same normal form, so that
    /// `Ty::ConstArg(N + 0)` and `Ty::ConstArg(N)` compare equal
    /// through derived `PartialEq`.
    ///
    /// Rules applied bottom-up (recursive normalization of children
    /// before the parent rewrite):
    ///
    /// - `Lit(a) ⊙ Lit(b)` collapses to a single `Lit(c)` whenever
    ///   the eval succeeds with an empty binding map.  Overflow /
    ///   div-zero leave the `Op` shape intact so a later E0703 pass
    ///   can surface the failure with the original spans.
    /// - `x + 0` and `0 + x` → `x`.
    /// - `x - 0` → `x`.  (`0 - x` is not rewritten — it would
    ///   require negation, which `u64` doesn't have.)
    /// - `x * 1` and `1 * x` → `x`.
    /// - `x * 0` and `0 * x` → `0`.
    /// - `x / 1` → `x`.
    ///
    /// Not handled (intentional spec limitation §B8): distributive
    /// rewrites like `N * (M + 1)` vs. `N*M + N`, commutative
    /// reordering of mixed `Param`/`Lit`, associative reassociation.
    /// These cases produce distinct normal forms in v1 even though
    /// they denote the same value; the v2 plan is to surface
    /// `E-CONST-NORMAL-FORM` at the kind-check when two
    /// instantiations differ only by a form the rewriter can't
    /// canonicalise.
    pub fn normal_form(self) -> ConstExpr {
        match self {
            ConstExpr::Op(a, op, b) => {
                let a = a.normal_form();
                let b = b.normal_form();
                // Constant fold first — if both sides are literals
                // and eval succeeds, replace with a single Lit.
                if let (ConstExpr::Lit(_), ConstExpr::Lit(_)) = (&a, &b) {
                    let empty = std::collections::HashMap::new();
                    let folded = ConstExpr::Op(Box::new(a.clone()), op, Box::new(b.clone()));
                    if let Ok(v) = folded.eval(&empty) {
                        return ConstExpr::Lit(v);
                    }
                    // Overflow / div-zero: keep the Op shape so the
                    // downstream E0703 wiring still has the spans.
                    return folded;
                }
                match op {
                    ConstOp::Add => {
                        if matches!(b, ConstExpr::Lit(0)) {
                            return a;
                        }
                        if matches!(a, ConstExpr::Lit(0)) {
                            return b;
                        }
                    }
                    ConstOp::Sub => {
                        if matches!(b, ConstExpr::Lit(0)) {
                            return a;
                        }
                    }
                    ConstOp::Mul => {
                        if matches!(b, ConstExpr::Lit(1)) {
                            return a;
                        }
                        if matches!(a, ConstExpr::Lit(1)) {
                            return b;
                        }
                        if matches!(b, ConstExpr::Lit(0)) || matches!(a, ConstExpr::Lit(0)) {
                            return ConstExpr::Lit(0);
                        }
                    }
                    ConstOp::Div => {
                        if matches!(b, ConstExpr::Lit(1)) {
                            return a;
                        }
                    }
                }
                ConstExpr::Op(Box::new(a), op, Box::new(b))
            }
            other => other,
        }
    }

    /// T2.02 S7/S8: evaluate the const expression against a binding map.
    ///
    /// - `Lit(n)`   → `Ok(n)`.
    /// - `Param(n)` → `Ok(value)` if `n` is bound; `Err(Unresolved)`
    ///                 otherwise (the caller may treat this as an
    ///                 "unresolved param" condition rather than an
    ///                 error, depending on context).
    /// - `Op(a, ⊙, b)` → recurse on both sides, then apply checked
    ///                 `u64` arithmetic.  Wrap-around / borrow surface
    ///                 as `Err(Overflow)`; `_/0` as
    ///                 `Err(DivisionByZero)`.
    /// - `Error`    → `Err(Malformed)` — propagated from a parser
    ///                 recovery path; should not reach a code-gen
    ///                 layout call.
    pub fn eval(
        &self,
        bindings: &std::collections::HashMap<String, u64>,
    ) -> Result<u64, ConstEvalError> {
        match self {
            ConstExpr::Lit(n) => Ok(*n),
            ConstExpr::Param(name) => bindings
                .get(name)
                .copied()
                .ok_or_else(|| ConstEvalError::Unresolved(name.clone())),
            ConstExpr::Op(a, op, b) => {
                let av = a.eval(bindings)?;
                let bv = b.eval(bindings)?;
                match op {
                    ConstOp::Add => av.checked_add(bv).ok_or(ConstEvalError::Overflow),
                    // `u64` has no negatives — borrow below zero surfaces as Overflow,
                    // matching the spec's single "E-CONST-OVERFLOW" diagnostic slot.
                    ConstOp::Sub => av.checked_sub(bv).ok_or(ConstEvalError::Overflow),
                    ConstOp::Mul => av.checked_mul(bv).ok_or(ConstEvalError::Overflow),
                    ConstOp::Div => {
                        if bv == 0 {
                            Err(ConstEvalError::DivisionByZero)
                        } else {
                            Ok(av / bv)
                        }
                    }
                }
            }
            ConstExpr::Error => Err(ConstEvalError::Malformed),
        }
    }
}

/// Errors returned by `ConstExpr::eval`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstEvalError {
    /// A `Param(name)` couldn't be resolved against the bindings.
    Unresolved(String),
    /// Reserved for future eval modes that may temporarily lack support
    /// (e.g. `where`-clause predicate eval before S9 lands).  No live
    /// emit site post-S8.
    NotImplemented,
    /// Parser recovery produced a `ConstExpr::Error`.
    Malformed,
    /// Checked arithmetic overflowed (or `u64` borrow went below zero).
    Overflow,
    /// `_ / 0` evaluated at monomorphization (E-CONST-DIV-ZERO).
    DivisionByZero,
}

impl fmt::Display for ConstExpr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConstExpr::Lit(n) => write!(f, "{}", n),
            ConstExpr::Param(name) => write!(f, "{}", name),
            ConstExpr::Op(a, op, b) => write!(f, "{} {} {}", a, op, b),
            ConstExpr::Error => write!(f, "<const-error>"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ConstOp {
    Add,
    Sub,
    Mul,
    Div,
}

impl fmt::Display for ConstOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConstOp::Add => write!(f, "+"),
            ConstOp::Sub => write!(f, "-"),
            ConstOp::Mul => write!(f, "*"),
            ConstOp::Div => write!(f, "/"),
        }
    }
}

/// A reference to a trait, optionally with generic arguments.
#[derive(Debug, Clone, PartialEq)]
pub struct TraitRef {
    pub name: String,
    pub generic_args: Vec<Ty>,
}

impl fmt::Display for TraitRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)?;
        if !self.generic_args.is_empty() {
            write!(f, "[")?;
            for (i, arg) in self.generic_args.iter().enumerate() {
                if i > 0 {
                    write!(f, ", ")?;
                }
                write!(f, "{}", arg)?;
            }
            write!(f, "]")?;
        }
        Ok(())
    }
}

/// The core type representation for Riven.
#[derive(Debug, Clone, PartialEq)]
pub enum Ty {
    // Primitives
    Int,
    Int8,
    Int16,
    Int32,
    Int64,
    UInt,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    ISize,
    USize,
    Float,
    Float32,
    Float64,
    Bool,
    Char,
    /// `()` — the unit type
    Unit,
    /// `!` — the never/bottom type (subtype of everything)
    Never,

    // String types
    /// Owned `String` (heap-allocated, growable)
    String,
    /// `&str` — borrowed string slice
    Str,

    // Composite types
    /// `(T, U, V)` — fixed-size heterogeneous tuple
    Tuple(Vec<Ty>),
    /// `[T; N]` — fixed-size array (N is a `ConstExpr`; was `usize`
    /// before T2.02 stage 4).
    Array(Box<Ty>, ConstExpr),
    /// Tier-2 const generics S6: a const-argument slot inside a
    /// `Ty::Class { generic_args }` / `Ty::Struct { generic_args }` /
    /// `Ty::Enum { generic_args }` list.  Distinguishes
    /// `Vector[Int, 3]` from `Vector[Int, 4]` at the type level so
    /// unification rejects assignment between distinct
    /// instantiations and monomorphization (S7+) can key on the
    /// pair `(type-args, const-args)`.
    ConstArg(ConstExpr),
    /// `Vec[T]` — dynamic, heap-allocated
    Vec(Box<Ty>),
    /// `HashMap[K, V]` — key-value map
    HashMap(Box<Ty>, Box<Ty>),
    /// `Set[T]`
    Set(Box<Ty>),

    // Option and Result
    /// `Option[T]`
    Option(Box<Ty>),
    /// `Result[T, E]`
    Result(Box<Ty>, Box<Ty>),

    // References
    /// `&T` — immutable borrow
    Ref(Box<Ty>),
    /// `&mut T` — mutable borrow
    RefMut(Box<Ty>),
    /// `&'a T` — immutable borrow with explicit lifetime
    RefLifetime(std::string::String, Box<Ty>),
    /// `&'a mut T` — mutable borrow with explicit lifetime
    RefMutLifetime(std::string::String, Box<Ty>),

    // User-defined types
    Class {
        name: std::string::String,
        generic_args: Vec<Ty>,
    },
    Struct {
        name: std::string::String,
        generic_args: Vec<Ty>,
    },
    Enum {
        name: std::string::String,
        generic_args: Vec<Ty>,
    },

    // Trait-related
    /// `impl Trait` — static dispatch, structural satisfaction OK
    ImplTrait(Vec<TraitRef>),
    /// `dyn Trait` — dynamic dispatch, requires explicit impl
    DynTrait(Vec<TraitRef>),

    // Function types
    Fn {
        params: Vec<Ty>,
        ret: Box<Ty>,
    },
    FnMut {
        params: Vec<Ty>,
        ret: Box<Ty>,
    },
    FnOnce {
        params: Vec<Ty>,
        ret: Box<Ty>,
    },

    /// Unknown type to be resolved during inference
    Infer(TypeId),

    /// Generic type parameter: `T`, `T: Bound`
    TypeParam {
        name: std::string::String,
        bounds: Vec<TraitRef>,
    },

    /// Type alias target (transparent)
    Alias {
        name: std::string::String,
        target: Box<Ty>,
    },

    /// Newtype wrapper (opaque)
    Newtype {
        name: std::string::String,
        inner: Box<Ty>,
    },

    /// Raw immutable pointer: `*T` (C's `const T*`)
    RawPtr(Box<Ty>),

    /// Raw mutable pointer: `*mut T` (C's `T*`)
    RawPtrMut(Box<Ty>),

    /// Opaque void pointer: `*Void` (C's `const void*`)
    RawPtrVoid,

    /// Opaque mutable void pointer: `*mut Void` (C's `void*`)
    RawPtrMutVoid,

    /// Placeholder for error recovery — allows type checking to continue
    Error,
}

/// Metadata about a type's properties.
#[derive(Debug, Clone, PartialEq)]
pub struct TypeInfo {
    pub is_copy: bool,
    pub is_drop: bool,
    pub size: Option<usize>,
    pub alignment: Option<usize>,
}

/// Whether a value is copied or moved on assignment/passing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MoveSemantics {
    Copy,
    Move,
}

impl Ty {
    /// Returns true if this type has Copy semantics.
    ///
    /// Copy types: all integers, floats, Bool, Char, Unit, references (&T, &str),
    /// ranges, and tuples where all elements are Copy.
    pub fn is_copy(&self) -> bool {
        match self {
            // Primitives are always Copy
            Ty::Int
            | Ty::Int8
            | Ty::Int16
            | Ty::Int32
            | Ty::Int64
            | Ty::UInt
            | Ty::UInt8
            | Ty::UInt16
            | Ty::UInt32
            | Ty::UInt64
            | Ty::ISize
            | Ty::USize
            | Ty::Float
            | Ty::Float32
            | Ty::Float64
            | Ty::Bool
            | Ty::Char
            | Ty::Unit => true,

            // Never is Copy (vacuously — you can never have a Never value)
            Ty::Never => true,

            // Immutable references are Copy
            Ty::Ref(_) | Ty::RefLifetime(_, _) => true,
            // &str is Copy (it's a borrowed reference)
            Ty::Str => true,

            // Tuples are Copy if all elements are Copy
            Ty::Tuple(elems) => elems.iter().all(|e| e.is_copy()),

            // Arrays are Copy if element type is Copy
            Ty::Array(elem, _) => elem.is_copy(),

            // Raw pointers are Copy (like in Rust)
            Ty::RawPtr(_) | Ty::RawPtrMut(_) | Ty::RawPtrVoid | Ty::RawPtrMutVoid => true,

            // Error type is treated as Copy for error recovery
            Ty::Error => true,

            // Everything else is Move
            _ => false,
        }
    }

    /// Returns true if this type should be treated as Copy after consulting
    /// symbol-table metadata for nominal user-defined types.
    pub fn is_copy_with(&self, symbols: &SymbolTable) -> bool {
        match self {
            _ if self.is_copy() => true,
            Ty::Tuple(elems) => elems.iter().all(|elem| elem.is_copy_with(symbols)),
            Ty::Array(elem, _) => elem.is_copy_with(symbols),
            Ty::Alias { target, .. } => target.is_copy_with(symbols),
            Ty::Newtype { inner, .. } => inner.is_copy_with(symbols),
            Ty::Struct { .. } | Ty::Class { .. } | Ty::Enum { .. } => {
                crate::resolve::symbols::ty_has_derive_trait(self, symbols, "Copy")
            }
            _ => false,
        }
    }

    /// Returns true if this type is Send without consulting nominal field
    /// metadata from the symbol table.
    pub fn is_send(&self) -> bool {
        match self {
            Ty::Int
            | Ty::Int8
            | Ty::Int16
            | Ty::Int32
            | Ty::Int64
            | Ty::UInt
            | Ty::UInt8
            | Ty::UInt16
            | Ty::UInt32
            | Ty::UInt64
            | Ty::ISize
            | Ty::USize
            | Ty::Float
            | Ty::Float32
            | Ty::Float64
            | Ty::Bool
            | Ty::Char
            | Ty::Unit
            | Ty::Never
            | Ty::String
            | Ty::Str => true,
            Ty::Ref(inner) | Ty::RefLifetime(_, inner) => inner.is_sync(),
            Ty::RefMut(inner) | Ty::RefMutLifetime(_, inner) => inner.is_send(),
            Ty::Tuple(elems) => elems.iter().all(|elem| elem.is_send()),
            Ty::Array(elem, _) | Ty::Vec(elem) | Ty::Set(elem) | Ty::Option(elem) => elem.is_send(),
            Ty::HashMap(key, value) | Ty::Result(key, value) => key.is_send() && value.is_send(),
            Ty::RawPtr(_) | Ty::RawPtrMut(_) | Ty::RawPtrVoid | Ty::RawPtrMutVoid => false,
            Ty::ImplTrait(bounds) | Ty::DynTrait(bounds) => has_trait_bound(bounds, "Send"),
            Ty::Fn { .. } | Ty::FnMut { .. } | Ty::FnOnce { .. } => true,
            Ty::TypeParam { bounds, .. } => has_trait_bound(bounds, "Send"),
            Ty::Alias { target, .. } => target.is_send(),
            Ty::Newtype { inner, .. } => inner.is_send(),
            Ty::Error => true,
            Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. } | Ty::Infer(_) => false,
            // Const args are a type-level marker, not a real type — they
            // never block thread-safety classification of their parent.
            Ty::ConstArg(_) => true,
        }
    }

    /// Returns true if this type is Send after consulting nominal field
    /// metadata from the symbol table.
    pub fn is_send_with(&self, symbols: &SymbolTable) -> bool {
        let mut visiting = HashSet::new();
        is_send_with_inner(self, symbols, &mut visiting)
    }

    /// Returns true if this type is Sync without consulting nominal field
    /// metadata from the symbol table.
    pub fn is_sync(&self) -> bool {
        match self {
            Ty::Int
            | Ty::Int8
            | Ty::Int16
            | Ty::Int32
            | Ty::Int64
            | Ty::UInt
            | Ty::UInt8
            | Ty::UInt16
            | Ty::UInt32
            | Ty::UInt64
            | Ty::ISize
            | Ty::USize
            | Ty::Float
            | Ty::Float32
            | Ty::Float64
            | Ty::Bool
            | Ty::Char
            | Ty::Unit
            | Ty::Never
            | Ty::String
            | Ty::Str => true,
            Ty::Ref(inner)
            | Ty::RefMut(inner)
            | Ty::RefLifetime(_, inner)
            | Ty::RefMutLifetime(_, inner) => inner.is_sync(),
            Ty::Tuple(elems) => elems.iter().all(|elem| elem.is_sync()),
            Ty::Array(elem, _) | Ty::Vec(elem) | Ty::Set(elem) | Ty::Option(elem) => elem.is_sync(),
            Ty::HashMap(key, value) | Ty::Result(key, value) => key.is_sync() && value.is_sync(),
            Ty::RawPtr(_) | Ty::RawPtrMut(_) | Ty::RawPtrVoid | Ty::RawPtrMutVoid => false,
            Ty::ImplTrait(bounds) | Ty::DynTrait(bounds) => has_trait_bound(bounds, "Sync"),
            Ty::Fn { .. } | Ty::FnMut { .. } | Ty::FnOnce { .. } => true,
            Ty::TypeParam { bounds, .. } => has_trait_bound(bounds, "Sync"),
            Ty::Alias { target, .. } => target.is_sync(),
            Ty::Newtype { inner, .. } => inner.is_sync(),
            Ty::Error => true,
            Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. } | Ty::Infer(_) => false,
            Ty::ConstArg(_) => true,
        }
    }

    /// Returns true if this type is Sync after consulting nominal field
    /// metadata from the symbol table.
    pub fn is_sync_with(&self, symbols: &SymbolTable) -> bool {
        let mut visiting = HashSet::new();
        is_sync_with_inner(self, symbols, &mut visiting)
    }

    /// Returns true if this type has Move semantics.
    pub fn is_move(&self) -> bool {
        !self.is_copy()
    }

    /// Returns the move semantics for this type.
    pub fn move_semantics(&self) -> MoveSemantics {
        if self.is_copy() {
            MoveSemantics::Copy
        } else {
            MoveSemantics::Move
        }
    }

    /// Returns true if this is any kind of reference.
    pub fn is_ref(&self) -> bool {
        matches!(
            self,
            Ty::Ref(_) | Ty::RefMut(_) | Ty::RefLifetime(_, _) | Ty::RefMutLifetime(_, _) | Ty::Str
        )
    }

    /// Returns true if this is a mutable reference.
    pub fn is_mut_ref(&self) -> bool {
        matches!(self, Ty::RefMut(_) | Ty::RefMutLifetime(_, _))
    }

    /// Returns true if this is an immutable reference.
    pub fn is_immut_ref(&self) -> bool {
        matches!(self, Ty::Ref(_) | Ty::RefLifetime(_, _) | Ty::Str)
    }

    /// Returns the inner type if this is a reference, otherwise None.
    pub fn deref_ty(&self) -> Option<&Ty> {
        match self {
            Ty::Ref(inner) | Ty::RefMut(inner) => Some(inner),
            Ty::RefLifetime(_, inner) | Ty::RefMutLifetime(_, inner) => Some(inner),
            _ => None,
        }
    }

    /// Returns true if this is an unresolved inference variable.
    pub fn is_infer(&self) -> bool {
        matches!(self, Ty::Infer(_))
    }

    /// Returns true if this is the error sentinel type.
    pub fn is_error(&self) -> bool {
        matches!(self, Ty::Error)
    }

    /// Returns true if this is the Never (bottom) type.
    pub fn is_never(&self) -> bool {
        matches!(self, Ty::Never)
    }

    /// Returns true if this is a numeric type (integer or float).
    pub fn is_numeric(&self) -> bool {
        self.is_integer() || self.is_float()
    }

    /// Returns true if this is any integer type.
    pub fn is_integer(&self) -> bool {
        matches!(
            self,
            Ty::Int
                | Ty::Int8
                | Ty::Int16
                | Ty::Int32
                | Ty::Int64
                | Ty::UInt
                | Ty::UInt8
                | Ty::UInt16
                | Ty::UInt32
                | Ty::UInt64
                | Ty::ISize
                | Ty::USize
        )
    }

    /// Returns true if this is any float type.
    pub fn is_float(&self) -> bool {
        matches!(self, Ty::Float | Ty::Float32 | Ty::Float64)
    }

    /// Returns true if this is a signed integer type.
    pub fn is_signed_integer(&self) -> bool {
        matches!(
            self,
            Ty::Int | Ty::Int8 | Ty::Int16 | Ty::Int32 | Ty::Int64 | Ty::ISize
        )
    }

    /// Returns true if this is an unsigned integer type.
    pub fn is_unsigned_integer(&self) -> bool {
        matches!(
            self,
            Ty::UInt | Ty::UInt8 | Ty::UInt16 | Ty::UInt32 | Ty::UInt64 | Ty::USize
        )
    }

    /// Returns the bit width of a numeric type, or None.
    pub fn bit_width(&self) -> Option<u32> {
        match self {
            Ty::Int8 | Ty::UInt8 => Some(8),
            Ty::Int16 | Ty::UInt16 => Some(16),
            Ty::Int32 | Ty::UInt32 | Ty::Float32 => Some(32),
            Ty::Int64 | Ty::UInt64 | Ty::Float64 => Some(64),
            Ty::Int | Ty::UInt | Ty::Float => Some(64), // defaults
            Ty::ISize | Ty::USize => Some(64),          // assume 64-bit platform
            _ => None,
        }
    }

    /// Returns true if this is an Option type.
    pub fn is_option(&self) -> bool {
        matches!(self, Ty::Option(_))
    }

    /// Returns true if this is a Result type.
    pub fn is_result(&self) -> bool {
        matches!(self, Ty::Result(_, _))
    }

    /// Returns the user-visible name of this type.
    pub fn type_name(&self) -> std::string::String {
        format!("{}", self)
    }
}

fn has_trait_bound(bounds: &[TraitRef], trait_name: &str) -> bool {
    bounds.iter().any(|bound| bound.name == trait_name)
}

fn is_send_with_inner(ty: &Ty, symbols: &SymbolTable, visiting: &mut HashSet<u32>) -> bool {
    match ty {
        Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. } => {
            nominal_members_are_thread_safe(ty, symbols, visiting, is_send_with_inner, true)
        }
        Ty::Ref(inner) | Ty::RefLifetime(_, inner) => is_sync_with_inner(inner, symbols, visiting),
        Ty::RefMut(inner) | Ty::RefMutLifetime(_, inner) => {
            is_send_with_inner(inner, symbols, visiting)
        }
        Ty::Tuple(elems) => elems
            .iter()
            .all(|elem| is_send_with_inner(elem, symbols, visiting)),
        Ty::Array(elem, _) | Ty::Vec(elem) | Ty::Set(elem) | Ty::Option(elem) => {
            is_send_with_inner(elem, symbols, visiting)
        }
        Ty::HashMap(key, value) | Ty::Result(key, value) => {
            is_send_with_inner(key, symbols, visiting)
                && is_send_with_inner(value, symbols, visiting)
        }
        Ty::Alias { target, .. } => is_send_with_inner(target, symbols, visiting),
        Ty::Newtype { inner, .. } => is_send_with_inner(inner, symbols, visiting),
        _ => ty.is_send(),
    }
}

fn is_sync_with_inner(ty: &Ty, symbols: &SymbolTable, visiting: &mut HashSet<u32>) -> bool {
    match ty {
        Ty::Class { .. } | Ty::Struct { .. } | Ty::Enum { .. } => {
            nominal_members_are_thread_safe(ty, symbols, visiting, is_sync_with_inner, false)
        }
        Ty::Ref(inner)
        | Ty::RefMut(inner)
        | Ty::RefLifetime(_, inner)
        | Ty::RefMutLifetime(_, inner) => is_sync_with_inner(inner, symbols, visiting),
        Ty::Tuple(elems) => elems
            .iter()
            .all(|elem| is_sync_with_inner(elem, symbols, visiting)),
        Ty::Array(elem, _) | Ty::Vec(elem) | Ty::Set(elem) | Ty::Option(elem) => {
            is_sync_with_inner(elem, symbols, visiting)
        }
        Ty::HashMap(key, value) | Ty::Result(key, value) => {
            is_sync_with_inner(key, symbols, visiting)
                && is_sync_with_inner(value, symbols, visiting)
        }
        Ty::Alias { target, .. } => is_sync_with_inner(target, symbols, visiting),
        Ty::Newtype { inner, .. } => is_sync_with_inner(inner, symbols, visiting),
        _ => ty.is_sync(),
    }
}

fn nominal_members_are_thread_safe(
    ty: &Ty,
    symbols: &SymbolTable,
    visiting: &mut HashSet<u32>,
    recurse: fn(&Ty, &SymbolTable, &mut HashSet<u32>) -> bool,
    is_send: bool,
) -> bool {
    let Some(def) = nominal_definition(ty, symbols) else {
        return false;
    };
    if nominal_type_has_manual_auto_trait(def, is_send) {
        return true;
    }
    if nominal_type_has_negative_auto_trait(def, is_send) {
        return false;
    }
    if !visiting.insert(def.id) {
        return true;
    }

    let result = match &def.kind {
        crate::resolve::symbols::DefKind::Class { info } => info.fields.iter().all(|field_id| {
            symbols
                .def_ty(*field_id)
                .is_some_and(|field_ty| recurse(&field_ty, symbols, visiting))
        }),
        crate::resolve::symbols::DefKind::Struct { info } => info.fields.iter().all(|field_id| {
            symbols
                .def_ty(*field_id)
                .is_some_and(|field_ty| recurse(&field_ty, symbols, visiting))
        }),
        crate::resolve::symbols::DefKind::Enum { info } => info.variants.iter().all(|variant_id| {
            match symbols.get(*variant_id).map(|variant| &variant.kind) {
                Some(crate::resolve::symbols::DefKind::EnumVariant { kind, .. }) => match kind {
                    crate::resolve::symbols::VariantDefKind::Unit => true,
                    crate::resolve::symbols::VariantDefKind::Tuple(fields) => fields
                        .iter()
                        .all(|field_ty| recurse(field_ty, symbols, visiting)),
                    crate::resolve::symbols::VariantDefKind::Struct(fields) => fields
                        .iter()
                        .all(|(_, field_ty)| recurse(field_ty, symbols, visiting)),
                },
                _ => false,
            }
        }),
        _ => false,
    };

    visiting.remove(&def.id);
    result
}

fn nominal_type_has_manual_auto_trait(
    def: &crate::resolve::symbols::Definition,
    is_send: bool,
) -> bool {
    match &def.kind {
        crate::resolve::symbols::DefKind::Class { info } => {
            if is_send {
                info.manual_send
            } else {
                info.manual_sync
            }
        }
        crate::resolve::symbols::DefKind::Struct { info } => {
            if is_send {
                info.manual_send
            } else {
                info.manual_sync
            }
        }
        crate::resolve::symbols::DefKind::Enum { info } => {
            if is_send {
                info.manual_send
            } else {
                info.manual_sync
            }
        }
        _ => false,
    }
}

fn nominal_type_has_negative_auto_trait(
    def: &crate::resolve::symbols::Definition,
    is_send: bool,
) -> bool {
    match &def.kind {
        crate::resolve::symbols::DefKind::Class { info } => {
            if is_send {
                info.opt_out_send
            } else {
                info.opt_out_sync
            }
        }
        crate::resolve::symbols::DefKind::Struct { info } => {
            if is_send {
                info.opt_out_send
            } else {
                info.opt_out_sync
            }
        }
        crate::resolve::symbols::DefKind::Enum { info } => {
            if is_send {
                info.opt_out_send
            } else {
                info.opt_out_sync
            }
        }
        _ => false,
    }
}

fn nominal_definition<'a>(
    ty: &Ty,
    symbols: &'a SymbolTable,
) -> Option<&'a crate::resolve::symbols::Definition> {
    let name = match ty {
        Ty::Class { name, .. } | Ty::Struct { name, .. } | Ty::Enum { name, .. } => name,
        _ => return None,
    };

    symbols.iter().find(|def| {
        def.name == *name
            && matches!(
                def.kind,
                crate::resolve::symbols::DefKind::Class { .. }
                    | crate::resolve::symbols::DefKind::Struct { .. }
                    | crate::resolve::symbols::DefKind::Enum { .. }
            )
    })
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ty::Int => write!(f, "Int"),
            Ty::Int8 => write!(f, "Int8"),
            Ty::Int16 => write!(f, "Int16"),
            Ty::Int32 => write!(f, "Int32"),
            Ty::Int64 => write!(f, "Int64"),
            Ty::UInt => write!(f, "UInt"),
            Ty::UInt8 => write!(f, "UInt8"),
            Ty::UInt16 => write!(f, "UInt16"),
            Ty::UInt32 => write!(f, "UInt32"),
            Ty::UInt64 => write!(f, "UInt64"),
            Ty::ISize => write!(f, "ISize"),
            Ty::USize => write!(f, "USize"),
            Ty::Float => write!(f, "Float"),
            Ty::Float32 => write!(f, "Float32"),
            Ty::Float64 => write!(f, "Float64"),
            Ty::Bool => write!(f, "Bool"),
            Ty::Char => write!(f, "Char"),
            Ty::Unit => write!(f, "()"),
            Ty::Never => write!(f, "Never"),
            Ty::String => write!(f, "String"),
            Ty::Str => write!(f, "&str"),
            Ty::Tuple(elems) => {
                write!(f, "(")?;
                for (i, e) in elems.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", e)?;
                }
                if elems.len() == 1 {
                    write!(f, ",")?;
                }
                write!(f, ")")
            }
            Ty::Array(elem, size) => write!(f, "[{}; {}]", elem, size),
            Ty::Vec(elem) => write!(f, "Vec[{}]", elem),
            Ty::HashMap(k, v) => write!(f, "HashMap[{}, {}]", k, v),
            Ty::Set(elem) => write!(f, "Set[{}]", elem),
            Ty::Option(inner) => write!(f, "Option[{}]", inner),
            Ty::Result(ok, err) => write!(f, "Result[{}, {}]", ok, err),
            Ty::Ref(inner) => write!(f, "&{}", inner),
            Ty::RefMut(inner) => write!(f, "&mut {}", inner),
            Ty::RefLifetime(lt, inner) => write!(f, "&'{} {}", lt, inner),
            Ty::RefMutLifetime(lt, inner) => write!(f, "&'{} mut {}", lt, inner),
            Ty::Class { name, generic_args }
            | Ty::Struct { name, generic_args }
            | Ty::Enum { name, generic_args } => {
                write!(f, "{}", name)?;
                if !generic_args.is_empty() {
                    write!(f, "[")?;
                    for (i, a) in generic_args.iter().enumerate() {
                        if i > 0 {
                            write!(f, ", ")?;
                        }
                        write!(f, "{}", a)?;
                    }
                    write!(f, "]")?;
                }
                Ok(())
            }
            Ty::ImplTrait(bounds) => {
                write!(f, "impl ")?;
                for (i, b) in bounds.iter().enumerate() {
                    if i > 0 {
                        write!(f, " + ")?;
                    }
                    write!(f, "{}", b)?;
                }
                Ok(())
            }
            Ty::DynTrait(bounds) => {
                write!(f, "dyn ")?;
                for (i, b) in bounds.iter().enumerate() {
                    if i > 0 {
                        write!(f, " + ")?;
                    }
                    write!(f, "{}", b)?;
                }
                Ok(())
            }
            Ty::Fn { params, ret } => {
                write!(f, "Fn(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", p)?;
                }
                write!(f, ") -> {}", ret)
            }
            Ty::FnMut { params, ret } => {
                write!(f, "FnMut(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", p)?;
                }
                write!(f, ") -> {}", ret)
            }
            Ty::FnOnce { params, ret } => {
                write!(f, "FnOnce(")?;
                for (i, p) in params.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "{}", p)?;
                }
                write!(f, ") -> {}", ret)
            }
            Ty::Infer(id) => write!(f, "?T{}", id),
            Ty::TypeParam { name, bounds } => {
                write!(f, "{}", name)?;
                if !bounds.is_empty() {
                    write!(f, ": ")?;
                    for (i, b) in bounds.iter().enumerate() {
                        if i > 0 {
                            write!(f, " + ")?;
                        }
                        write!(f, "{}", b)?;
                    }
                }
                Ok(())
            }
            Ty::Alias { name, .. } => write!(f, "{}", name),
            Ty::Newtype { name, .. } => write!(f, "{}", name),
            Ty::RawPtr(inner) => write!(f, "*{}", inner),
            Ty::RawPtrMut(inner) => write!(f, "*mut {}", inner),
            Ty::RawPtrVoid => write!(f, "*Void"),
            Ty::RawPtrMutVoid => write!(f, "*mut Void"),
            Ty::Error => write!(f, "<error>"),
            Ty::ConstArg(e) => write!(f, "{}", e),
        }
    }
}
