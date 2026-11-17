//! Canonical structural helpers on `Ty`: the exhaustive `map_inner` fold
//! primitive and the queries built on it (`any`, `subst_params`,
//! `peel_refs`, `callable_signature`/`is_callable`, `builtin_head`/
//! `from_builtin_head`, `method_home`, `is_string_form`).

use super::*;

impl Ty {
    /// Apply `f` to every directly-nested `Ty` child, rebuilding `self` with the
    /// results. EXHAUSTIVE: every variant is matched explicitly — there is no
    /// `_ =>` arm, so adding a new `Ty` variant is a compile error here until it
    /// is handled. This is the single structural-recursion primitive for `Ty`;
    /// folds like `subst_params` are written as `match { interesting arms; other =>
    /// other.map_inner(f) }`.
    pub fn map_inner(self, f: &mut impl FnMut(&Ty) -> Ty) -> Ty {
        match self {
            // Leaves — no nested Ty. A `TypeParam`'s bounds are part of the
            // parameter's declaration, not a child of this use, so a fold that
            // replaces the parameter never needs to reach into them.
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
            | Ty::ConstArg(_)
            | Ty::Infer(_)
            | Ty::TypeParam { .. }
            | Ty::RawPtrVoid
            | Ty::RawPtrMutVoid
            | Ty::Error => self,

            // Single-child wrappers.
            Ty::Array(t) => Ty::Array(Box::new(f(&t))),
            Ty::Set(t) => Ty::Set(Box::new(f(&t))),
            Ty::Option(t) => Ty::Option(Box::new(f(&t))),
            Ty::Ref(t) => Ty::Ref(Box::new(f(&t))),
            Ty::RefMut(t) => Ty::RefMut(Box::new(f(&t))),
            Ty::RawPtr(t) => Ty::RawPtr(Box::new(f(&t))),
            Ty::RawPtrMut(t) => Ty::RawPtrMut(Box::new(f(&t))),

            // Named-lifetime refs (bug #3: these were skipped by subst_ty).
            Ty::RefLifetime(l, t) => Ty::RefLifetime(l, Box::new(f(&t))),
            Ty::RefMutLifetime(l, t) => Ty::RefMutLifetime(l, Box::new(f(&t))),

            // Two-child.
            Ty::Map(k, v) => Ty::Map(Box::new(f(&k)), Box::new(f(&v))),
            Ty::Result(ok, err) => Ty::Result(Box::new(f(&ok)), Box::new(f(&err))),

            // Const-sized array: fold the element, keep the const expr.
            Ty::FixedArray(t, n) => Ty::FixedArray(Box::new(f(&t)), n),

            // Sequences of children.
            Ty::Tuple(ts) => Ty::Tuple(ts.iter().map(&mut *f).collect()),

            // Nominal types with generic args.
            Ty::Class { name, generic_args } => Ty::Class {
                name,
                generic_args: generic_args.iter().map(&mut *f).collect(),
            },
            Ty::Struct { name, generic_args } => Ty::Struct {
                name,
                generic_args: generic_args.iter().map(&mut *f).collect(),
            },
            Ty::Enum { name, generic_args } => Ty::Enum {
                name,
                generic_args: generic_args.iter().map(&mut *f).collect(),
            },

            // Function types.
            Ty::Fn { params, ret } => Ty::Fn {
                params: params.iter().map(&mut *f).collect(),
                ret: Box::new(f(&ret)),
            },
            Ty::FnMut { params, ret } => Ty::FnMut {
                params: params.iter().map(&mut *f).collect(),
                ret: Box::new(f(&ret)),
            },
            Ty::FnOnce { params, ret } => Ty::FnOnce {
                params: params.iter().map(&mut *f).collect(),
                ret: Box::new(f(&ret)),
            },

            // Mixin bounds: fold each bound's generic args, so the `T`/`U` in
            // `any Fn[Fn(T) -> U]` is reached like any other child.
            Ty::SomeMixin(bounds) => Ty::SomeMixin(map_bound_args(bounds, f)),
            Ty::AnyMixin(bounds) => Ty::AnyMixin(map_bound_args(bounds, f)),

            // Transparent / opaque wrappers — fold the inner.
            Ty::Alias { name, target } => Ty::Alias {
                name,
                target: Box::new(f(&target)),
            },
            Ty::Newtype { name, inner } => Ty::Newtype {
                name,
                inner: Box::new(f(&inner)),
            },
        }
    }

    /// Peel every reference layer (`&`, `&mut`, `&'a`, `&'a mut`) off `self`,
    /// returning the first non-reference inner type. The single canonical
    /// reference-peeler — replaces the several partial hand-rolled peels.
    pub fn peel_refs(&self) -> &Ty {
        match self {
            Ty::Ref(t) | Ty::RefMut(t) | Ty::RefLifetime(_, t) | Ty::RefMutLifetime(_, t) => {
                t.peel_refs()
            }
            other => other,
        }
    }

    /// Whether a parameter of this type takes a BLOCK: a `Fn[...]` (`Ty::Fn`,
    /// incl. `&block` and the implicit-yield `__block`), behind any references.
    /// Such a parameter only calls its closure, so the closure borrows its
    /// captures and the caller keeps and frees the closure pair. A
    /// `Proc[...]` / `any Fn[...]` parameter (`Ty::AnyMixin`) may store it
    /// instead. The one predicate typeck (trailing-block kinds), the borrow
    /// checker (E1018, argument moves) and MIR (`runtime_abi`) all ask.
    pub fn is_borrowed_block(&self) -> bool {
        matches!(self.peel_refs(), Ty::Fn { .. })
    }

    /// The type a type parameter bound to `self` stands for in a
    /// specialized body. A type parameter's value may be stored (a field
    /// typed `T`), so a closure bound to one is a stored proc
    /// (`Proc[...]`, which the core library declares as `any Fn[...]`), never
    /// a borrowed block (`Fn[...]`, which only a declared block parameter
    /// is). Every other type is itself.
    pub fn as_type_argument(&self) -> Ty {
        match self {
            Ty::Fn { .. } => Ty::proc_of(self.clone()),
            other => other.clone(),
        }
    }

    /// The stored-closure type `Proc[fn_ty]`, which the core library
    /// declares as `type Proc[F] = any Fn[F]`: the one constructor of an
    /// `Fn`-family `any` bound.
    pub fn proc_of(fn_ty: Ty) -> Ty {
        Ty::AnyMixin(vec![MixinRef {
            name: "Fn".into(),
            generic_args: vec![fn_ty],
        }])
    }

    /// True when this is, behind any references, a `Proc[...]`
    /// (`any Fn[...]`): a closure that owns its captures, so it can be
    /// stored and shared (`Closure#clone`, its `.rx` method home in
    /// `std/core`). A `Fn[...]` value may be a block that borrows its
    /// captures and has no `clone`.
    pub fn is_proc(&self) -> bool {
        matches!(self.peel_refs(), Ty::AnyMixin(bounds)
            if bounds.iter().any(MixinRef::is_fn_family))
    }

    /// Substitute every `TypeParam` named in `subst` with its bound type,
    /// recursing through every nested child via `map_inner` (references of
    /// every kind, collections, `Fn` types, aliases and mixin bounds). A
    /// parameter missing from `subst` is left in place. The one type-parameter
    /// substitution shared by typeck and MIR.
    pub fn subst_params(&self, subst: &HashMap<std::string::String, Ty>) -> Ty {
        match self {
            Ty::TypeParam { name, .. } => subst.get(name).cloned().unwrap_or_else(|| self.clone()),
            other => other
                .clone()
                .map_inner(&mut |child| child.subst_params(subst)),
        }
    }

    /// True when `pred` holds for `self` or for any type nested inside it,
    /// searched with the same children `map_inner` folds.
    pub fn any(&self, pred: &mut impl FnMut(&Ty) -> bool) -> bool {
        self.any_within(pred, &mut |_| true)
    }

    /// True when `self` is made of more than `limit` type nodes (itself and
    /// every type nested in it). Stops counting at `limit + 1`, so it is
    /// cheap on a type of any size. The one size bound for checks that must
    /// stop a type from growing without end (instance arguments, E0746, and
    /// the derive check).
    pub fn exceeds_nodes(&self, limit: usize) -> bool {
        let mut seen = 0usize;
        self.any(&mut |_| {
            seen += 1;
            seen > limit
        })
    }

    /// [`Ty::any`], searching the children of only those types `descend`
    /// accepts (`pred` still sees every type it reaches).
    pub fn any_within(
        &self,
        pred: &mut impl FnMut(&Ty) -> bool,
        descend: &mut impl FnMut(&Ty) -> bool,
    ) -> bool {
        if pred(self) {
            return true;
        }
        if !descend(self) {
            return false;
        }
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
            | Ty::ConstArg(_)
            | Ty::Infer(_)
            | Ty::TypeParam { .. }
            | Ty::RawPtrVoid
            | Ty::RawPtrMutVoid
            | Ty::Error => false,
            Ty::Array(t)
            | Ty::Set(t)
            | Ty::Option(t)
            | Ty::Ref(t)
            | Ty::RefMut(t)
            | Ty::RawPtr(t)
            | Ty::RawPtrMut(t)
            | Ty::RefLifetime(_, t)
            | Ty::RefMutLifetime(_, t)
            | Ty::FixedArray(t, _)
            | Ty::Alias { target: t, .. }
            | Ty::Newtype { inner: t, .. } => t.any_within(pred, descend),
            Ty::Map(a, b) | Ty::Result(a, b) => {
                a.any_within(pred, descend) || b.any_within(pred, descend)
            }
            Ty::Tuple(ts)
            | Ty::Class {
                generic_args: ts, ..
            }
            | Ty::Struct {
                generic_args: ts, ..
            }
            | Ty::Enum {
                generic_args: ts, ..
            } => ts.iter().any(|t| t.any_within(pred, descend)),
            Ty::Fn { params, ret } | Ty::FnMut { params, ret } | Ty::FnOnce { params, ret } => {
                params.iter().any(|t| t.any_within(pred, descend)) || ret.any_within(pred, descend)
            }
            Ty::SomeMixin(bounds) | Ty::AnyMixin(bounds) => bounds
                .iter()
                .any(|b| b.generic_args.iter().any(|t| t.any_within(pred, descend))),
        }
    }

    /// The `(params, ret)` of a callable type: a bare `Fn`/`FnMut`/`FnOnce`,
    /// or the `any Fn[Fn(T) -> U]` / `some Fn[…]` mixin spelling, whose
    /// function type rides in the first `Fn`-family bound's first generic
    /// arg. Does not look through references; callers peel first.
    pub fn callable_signature(&self) -> Option<(&[Ty], &Ty)> {
        let fn_ty = match self {
            Ty::SomeMixin(bounds) | Ty::AnyMixin(bounds) => {
                bounds.iter().find_map(MixinRef::callable_ty)?
            }
            other => other,
        };
        match fn_ty {
            Ty::Fn { params, ret } | Ty::FnMut { params, ret } | Ty::FnOnce { params, ret } => {
                Some((params.as_slice(), ret.as_ref()))
            }
            _ => None,
        }
    }

    /// The written spelling of a callable type, looking through references:
    /// `Fn(Int) -> nil` for a block type and `Proc[(Int) -> nil]` for a
    /// stored closure (`Proc[...]`, `any Fn[...]`), with a unit spelled
    /// `nil`. `None` for a non-callable type.
    pub fn callable_spelling(&self) -> Option<std::string::String> {
        let signature = self.signature_spelling()?;
        Some(if self.is_borrowed_block() {
            format!("Fn{signature}")
        } else {
            format!("Proc[{signature}]")
        })
    }

    /// The `Proc[(A, B) -> R]` spelling of a callable type's signature, the
    /// storable parameter type for it (`nil` for a unit). `None` for a
    /// non-callable type.
    pub fn proc_spelling(&self) -> Option<std::string::String> {
        Some(format!("Proc[{}]", self.signature_spelling()?))
    }

    /// `(A, B) -> R` for a callable type, looking through references, with a
    /// unit spelled `nil`.
    fn signature_spelling(&self) -> Option<std::string::String> {
        let (params, ret) = self.peel_refs().callable_signature()?;
        let surface = |t: &Ty| match t {
            Ty::Unit => "nil".to_string(),
            other => other.to_string(),
        };
        let params: Vec<std::string::String> = params.iter().map(surface).collect();
        Some(format!("({}) -> {}", params.join(", "), surface(ret)))
    }

    /// Whether a value of this type may hold a reference, counting those its
    /// type does not show: a reference, a callable (what it captures), a
    /// class, struct or enum (its fields; a builtin head spelled as its class
    /// shows its type arguments), a `some`/`any` mixin value (its
    /// implementor's fields), or a type parameter `param` accepts (an
    /// instance may bind it to a reference). The one test for what a value
    /// may carry; elision reads only written references
    /// ([`Ty::holds_reference`]).
    pub fn may_hold_reference(&self, param: &mut impl FnMut(&str) -> bool) -> bool {
        self.any_within(
            &mut |t: &Ty| match t {
                Ty::TypeParam { name, .. } => param(name),
                other => other.hides_contents() || other.is_ref(),
            },
            &mut |_| true,
        )
    }

    /// Whether a value of this type may hold what its type does not show: a
    /// class, struct or enum (its fields; a builtin head spelled as its
    /// class, `Hash[K, V]` as the receiver of a `.rx` shell method, shows
    /// its type arguments), a `some`/`any` mixin value (its implementor's
    /// fields) or a callable (what it captures). The one list
    /// [`Ty::may_hold_reference`], [`Ty::may_hold_reference_to`] and
    /// [`result_carries`] read.
    pub fn hides_contents(&self) -> bool {
        match self {
            Ty::Class { .. } => self.builtin_head().is_none(),
            Ty::Struct { .. }
            | Ty::Enum { .. }
            | Ty::SomeMixin(_)
            | Ty::AnyMixin(_)
            | Ty::Fn { .. }
            | Ty::FnMut { .. }
            | Ty::FnOnce { .. } => true,
            _ => false,
        }
    }

    /// Whether a value of this type may hold a reference to one of
    /// `pointees` (as Rust's struct can hold a reference only through a
    /// field whose lifetime it names): a reference to such a type (a type
    /// parameter pointee on either side matches any), a callable, mixin
    /// value, struct or enum (what it holds is not shown), or a user class
    /// `class` says may through its fields. A bare type parameter holds
    /// none; a class's type arguments are searched.
    pub fn may_hold_reference_to(
        &self,
        pointees: &[Ty],
        class: &mut impl FnMut(&str) -> bool,
    ) -> bool {
        let generic = |ty: &Ty| matches!(ty, Ty::TypeParam { .. });
        self.any_within(
            &mut |t: &Ty| match t {
                Ty::TypeParam { .. } => false,
                Ty::Class { name, .. } => t.hides_contents() && class(name),
                other if other.hides_contents() => true,
                other => other.deref_ty().is_some_and(|inner| {
                    generic(inner) || pointees.iter().any(|p| p == inner || generic(p))
                }),
            },
            &mut |_| true,
        )
    }

    /// The types the references inside this type point at.
    pub fn pointees(&self) -> Vec<Ty> {
        let mut found = Vec::new();
        self.any(&mut |t: &Ty| {
            found.extend(t.deref_ty().cloned());
            false
        });
        found
    }

    /// Whether this is a callable shape, looking through references: a bare
    /// `Fn`/`FnMut`/`FnOnce`, or a `some`/`any` mixin with an `Fn`-family
    /// bound (with or without a spelled-out signature).
    pub fn is_callable(&self) -> bool {
        match self.peel_refs() {
            Ty::Fn { .. } | Ty::FnMut { .. } | Ty::FnOnce { .. } => true,
            Ty::SomeMixin(bounds) | Ty::AnyMixin(bounds) => {
                bounds.iter().any(MixinRef::is_fn_family)
            }
            _ => false,
        }
    }

    /// The `.rx` class shell that homes a builtin head's methods, with the
    /// head's generic args, looking through references. Both spellings of a
    /// builtin head answer the same: the structural `Ty::Array(T)` and the
    /// nominal `Ty::Class { name: "Array", generic_args: [T] }` that `self`
    /// carries inside the shell's own body. `None` for every other type,
    /// including a non-generic `Class { "Array" }`.
    pub fn builtin_head(&self) -> Option<(&'static str, Vec<Ty>)> {
        match self.peel_refs() {
            Ty::Array(e) => Some(("Array", vec![(**e).clone()])),
            Ty::Set(e) => Some(("Set", vec![(**e).clone()])),
            Ty::Map(k, v) => Some(("Hash", vec![(**k).clone(), (**v).clone()])),
            Ty::Option(t) => Some(("Option", vec![(**t).clone()])),
            Ty::Result(o, e) => Some(("Result", vec![(**o).clone(), (**e).clone()])),
            Ty::Class { name, generic_args } if !generic_args.is_empty() => BUILTIN_HEAD_HOMES
                .iter()
                .find(|home| *home == name)
                .map(|home| (*home, generic_args.clone())),
            _ => None,
        }
    }

    /// Inverse of `builtin_head`: the structural type for a `(shell name,
    /// generic args)` pair. `None` on an arity mismatch or a name that is not
    /// a builtin head.
    pub fn from_builtin_head(home: &str, args: &[Ty]) -> Option<Ty> {
        match (home, args) {
            ("Array", [e]) => Some(Ty::Array(Box::new(e.clone()))),
            ("Set", [e]) => Some(Ty::Set(Box::new(e.clone()))),
            ("Hash", [k, v]) => Some(Ty::Map(Box::new(k.clone()), Box::new(v.clone()))),
            ("Option", [t]) => Some(Ty::Option(Box::new(t.clone()))),
            ("Result", [o, e]) => Some(Ty::Result(Box::new(o.clone()), Box::new(e.clone()))),
            _ => None,
        }
    }

    /// The name of the class whose methods this type dispatches to, looking
    /// through references: the bare shell name of a builtin head (`Array`,
    /// `Set`, `Hash`, `Option`, `Result`), the name of a class, struct or
    /// enum, `closure_shell` for a stored closure value (`Proc[...]`, `any
    /// Fn[...]`: the class the core library declares `layout closure`,
    /// `SymbolTable::closure_shell_name`), and otherwise the type's display
    /// text (`Int`, `Char`, …).
    pub fn method_home(&self, closure_shell: Option<&str>) -> std::string::String {
        let peeled = self.peel_refs();
        if let Some(shell) = closure_shell.filter(|_| peeled.is_proc()) {
            return shell.to_string();
        }
        match peeled {
            Ty::Array(_) => "Array".to_string(),
            Ty::Set(_) => "Set".to_string(),
            Ty::Map(_, _) => "Hash".to_string(),
            Ty::Option(_) => "Option".to_string(),
            Ty::Result(_, _) => "Result".to_string(),
            Ty::Class { name, .. } | Ty::Struct { name, .. } | Ty::Enum { name, .. } => {
                name.clone()
            }
            other => other.to_string(),
        }
    }

    /// Whether this is a String, looking through references (`&String`,
    /// `&var String`). The one String-form test shared by MIR and both
    /// codegen backends.
    pub fn is_string_form(&self) -> bool {
        match self.peel_refs() {
            Ty::String => true,
            Ty::Class { name, .. } => name == "String",
            _ => false,
        }
    }

    /// Returns true if this is a scalar primitive: a numeric type, `Bool` or
    /// `Char`. These are the fixed-width register values; `Unit`, `Never`
    /// and `String` are not scalars.
    pub fn is_scalar_primitive(&self) -> bool {
        self.is_numeric() || matches!(self, Ty::Bool | Ty::Char)
    }

    /// Whether a `&var` of a value of this type is the ADDRESS of the
    /// caller's slot (a String or a scalar) rather than a copy of its handle
    /// word (classes, structs, enums, arrays, hashes, closures, type
    /// parameters). References are looked through. The one rule shared by
    /// borrow check and MIR lowering (`mir::refmut_abi`).
    pub fn refmut_is_slot_address(&self) -> bool {
        let pointee = self.peel_refs();
        pointee.is_string_form() || pointee.is_scalar_primitive()
    }

    /// Match `self`, a declared type naming type parameters, against the
    /// `actual` type at a use, recording `param → actual` into `out` for
    /// every parameter `is_param` accepts. The one type-parameter binder
    /// shared by typeck and MIR instance selection.
    ///
    /// - A bare parameter binds the actual type as it is, references
    ///   included: `T` against `&String` is `T = &String`, so an instance
    ///   over borrowed values is never confused with one over owned values.
    ///   `bindable` filters what may bind (typeck skips unresolved variables,
    ///   MIR takes only fully concrete types). The first binding wins.
    /// - Reference layers match structurally, of any kind: `&T` against
    ///   `&String` or `&var String` is `T = String`. A reference formal
    ///   against an owned actual (a value passed where the signature
    ///   borrows) matches the pointee, and a non-reference formal against a
    ///   reference actual (an auto-dereferenced receiver) matches through it.
    /// - Every generic head matches its own head, nominal types by name and
    ///   arity, callable types (`Fn(...)`, `any Fn[...]`) by parameters and
    ///   return. A shape mismatch binds nothing.
    pub fn bind_type_params(
        &self,
        actual: &Ty,
        is_param: &dyn Fn(&str) -> bool,
        bindable: &dyn Fn(&Ty) -> bool,
        out: &mut HashMap<std::string::String, Ty>,
    ) {
        let mut bind = |formal: &Ty, actual: &Ty| {
            formal.bind_type_params(actual, is_param, bindable, &mut *out)
        };
        if let Ty::TypeParam { name, .. } = self {
            if is_param(name) && bindable(actual) {
                out.entry(name.clone()).or_insert_with(|| actual.clone());
            }
            return;
        }
        match (self.deref_ty(), actual.deref_ty()) {
            (Some(formal), Some(actual)) => return bind(formal, actual),
            (Some(formal), None) => return bind(formal, actual),
            (None, Some(actual)) => return bind(self, actual),
            (None, None) => {}
        }
        if let (Some((fparams, fret)), Some((aparams, aret))) =
            (self.callable_signature(), actual.callable_signature())
        {
            if fparams.len() == aparams.len() {
                for (f, a) in fparams.iter().zip(aparams) {
                    bind(f, a);
                }
            }
            return bind(fret, aret);
        }
        match (self, actual) {
            (Ty::Array(f), Ty::Array(a))
            | (Ty::Set(f), Ty::Set(a))
            | (Ty::Option(f), Ty::Option(a))
            | (Ty::FixedArray(f, _), Ty::FixedArray(a, _)) => bind(f, a),
            (Ty::Map(fk, fv), Ty::Map(ak, av)) | (Ty::Result(fk, fv), Ty::Result(ak, av)) => {
                bind(fk, ak);
                bind(fv, av);
            }
            (Ty::Tuple(fs), Ty::Tuple(as_)) if fs.len() == as_.len() => {
                for (f, a) in fs.iter().zip(as_) {
                    bind(f, a);
                }
            }
            (
                Ty::Class {
                    name: fname,
                    generic_args: fargs,
                },
                Ty::Class {
                    name: aname,
                    generic_args: aargs,
                },
            )
            | (
                Ty::Struct {
                    name: fname,
                    generic_args: fargs,
                },
                Ty::Struct {
                    name: aname,
                    generic_args: aargs,
                },
            )
            | (
                Ty::Enum {
                    name: fname,
                    generic_args: fargs,
                },
                Ty::Enum {
                    name: aname,
                    generic_args: aargs,
                },
            ) if fname == aname && fargs.len() == aargs.len() => {
                for (f, a) in fargs.iter().zip(aargs) {
                    bind(f, a);
                }
            }
            _ => {}
        }
    }
}

/// The most type nodes ([`Ty::exceeds_nodes`]) a type argument the compiler
/// builds by itself may hold: past it, an instance argument is error E0746
/// and the derive check stops looking. A real program's types stay far
/// below; only a type that grows on every step (`Nest[(T, T)]` built from
/// `Nest[T]`) reaches it, and it does so within a few steps however the
/// type grows, where a depth bound lets a doubling type reach `2^depth`
/// nodes first.
pub const MAX_TYPE_NODES: usize = 256;

/// What a call's result may borrow, read off the callee's DECLARED
/// signature by [`result_source`]: Rust's lifetime elision with Ruby's
/// surface, so no lifetime is ever spelled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResultSource {
    /// The declared result holds no reference (`-> Option[V]`, `-> Int`):
    /// it borrows nothing of the arguments. A reference a type argument
    /// brings in (`V = &String`) is one an argument already held, so the
    /// result keeps that argument's loans and takes none.
    Nothing,
    /// The declared result holds a reference and borrows from the
    /// arguments at these MIR slots (receiver first). A named lifetime in
    /// the result (`def longest[a](x: &a String, y: &a String) -> &a
    /// String`) names every parameter whose reference carries it; an
    /// unnamed one is elided to the receiver when there is one, else the
    /// one reference parameter.
    Slots(Vec<usize>),
    /// The declared result holds a reference but there is no receiver and
    /// no reference parameter, or no declaration: a caller assumes it may
    /// point into every argument.
    Unknown,
    /// The declared result holds a reference, there is no receiver and
    /// several reference parameters (these MIR slots), and the result's
    /// reference names no lifetime: which one it borrows from is
    /// ambiguous, error E0748 (Rust's E0106). The fix is a method of the
    /// value it borrows from, an owned result, or a named lifetime.
    Ambiguous(Vec<usize>),
}

/// The one elision rule: what a callee declared with `params` (excluding
/// the receiver; `has_receiver` says whether one comes first) and result
/// `ret` may borrow. A `&block` / `Fn` parameter is not a reference
/// parameter: a block is only called, never borrowed from.
pub fn result_source(has_receiver: bool, params: &[Ty], ret: &Ty) -> ResultSource {
    if !ret.holds_reference() {
        return ResultSource::Nothing;
    }
    // Named lifetimes: the result borrows from every parameter whose
    // reference carries one of them (a receiver cannot name one).
    let lifetimes = |ty: &Ty| {
        let mut names = Vec::new();
        ty.any(&mut |t: &Ty| {
            if let Ty::RefLifetime(name, _) | Ty::RefMutLifetime(name, _) = t {
                names.push(name.clone());
            }
            false
        });
        names
    };
    let result_names = lifetimes(ret);
    if !result_names.is_empty() {
        let first_param = usize::from(has_receiver);
        let slots = params
            .iter()
            .enumerate()
            .filter(|(_, ty)| lifetimes(ty).iter().any(|n| result_names.contains(n)))
            .map(|(index, _)| first_param + index)
            .collect();
        return ResultSource::Slots(slots);
    }
    if has_receiver {
        return ResultSource::Slots(vec![0]);
    }
    let refs: Vec<usize> = params
        .iter()
        .enumerate()
        .filter(|(_, ty)| ty.is_ref() && !ty.peel_refs().is_callable())
        .map(|(index, _)| index)
        .collect();
    match refs.as_slice() {
        [] => ResultSource::Unknown,
        [slot] => ResultSource::Slots(vec![*slot]),
        _ => ResultSource::Ambiguous(refs),
    }
}

/// What a callee's DECLARED signature says about its result: what it
/// points into ([`result_source`], the elision rule) and whose existing
/// references it may carry ([`result_carries`]). Rust keeps the two apart:
/// `h.remove(&key)` points into nothing of `key` and carries the table's
/// references, so changing `key` afterwards is fine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeclaredResult {
    pub source: ResultSource,
    /// The MIR slots (receiver first) whose references the result may hold.
    pub carries: Vec<usize>,
    /// The `(from, into)` slots where the callee, when it may write
    /// `into`, may keep the references `from` passes ([`declared_keeps`]).
    pub keeps: Vec<(usize, usize)>,
}

impl DeclaredResult {
    /// A body with no declared signature. A reader treats an `Unknown`
    /// source as pointing into and carrying every argument, so `carries`
    /// stays empty here.
    pub fn unknown() -> Self {
        DeclaredResult {
            source: ResultSource::Unknown,
            carries: Vec::new(),
            keeps: Vec::new(),
        }
    }
}

/// [`result_source`], [`result_carries`] and [`declared_keeps`] of one
/// declaration; `receiver` is the receiver's declared type when there is
/// one, and `class` answers whether a user class's fields may hold a
/// reference to one of the given pointees
/// (`SymbolTable::fields_may_hold_reference_to`).
pub fn declared_result(
    receiver: Option<&Ty>,
    params: &[Ty],
    ret: &Ty,
    class: &mut impl FnMut(&str, &[Ty]) -> bool,
) -> DeclaredResult {
    DeclaredResult {
        source: result_source(receiver.is_some(), params, ret),
        carries: result_carries(receiver, params, ret),
        keeps: declared_keeps(receiver, params, class),
    }
}

/// The `(from, into)` slot pairs (receiver first) where a callee writing
/// slot `into` may keep the references slot `from` passes: `into`'s
/// declared type, below its own top reference, may hold a reference to
/// what `from` points at ([`Ty::may_hold_reference_to`]; a type parameter
/// never: a `Hash[K, V]` holds `K`s, not a `&K` passed to `remove`; a user
/// class when `class` says its fields may). `def init(@r: &String)` keeps
/// `r` in a class with a `&String` field; a class holding a `&var Canvas`
/// keeps no `&String`. Rust's `fn set(&mut self, r: &'a T)` likewise keeps
/// `r` only through a field of lifetime `'a`.
pub fn declared_keeps(
    receiver: Option<&Ty>,
    params: &[Ty],
    class: &mut impl FnMut(&str, &[Ty]) -> bool,
) -> Vec<(usize, usize)> {
    let slots: Vec<&Ty> = receiver.into_iter().chain(params).collect();
    let mut keeps = Vec::new();
    for (from, from_ty) in slots.iter().enumerate() {
        let pointees = from_ty.pointees();
        if pointees.is_empty() {
            continue;
        }
        for (into, into_ty) in slots.iter().enumerate() {
            let container = into_ty.deref_ty().unwrap_or(into_ty);
            if from != into
                && container.may_hold_reference_to(&pointees, &mut |name| class(name, &pointees))
            {
                keeps.push((from, into));
            }
        }
    }
    keeps
}

/// The slots (receiver first) whose references a result declared `ret`
/// may carry: none when `ret` can hold no reference, else each slot whose
/// declared type below its own top reference may hold one
/// ([`Ty::may_hold_reference`]), counting a type parameter only when it
/// also occurs in `ret`. `get(key: &K) -> Option[&V]` carries the receiver
/// (`V`), not the key.
pub fn result_carries(receiver: Option<&Ty>, params: &[Ty], ret: &Ty) -> Vec<usize> {
    if !ret.may_hold_reference(&mut |_| true) {
        return Vec::new();
    }
    let mut in_result = HashSet::new();
    ret.any(&mut |t: &Ty| {
        if let Ty::TypeParam { name, .. } = t {
            in_result.insert(name.clone());
        }
        false
    });
    // A callable's captures reach the result only through what it returns
    // or a result that can store a callable (Rust: `fn tap<F: Fn(&Self)>(self,
    // f: F) -> Self` names none of `F`'s lifetimes); a bare type parameter
    // gets only the references of the slots that name it.
    let stores_callable = ret.any_within(&mut |t: &Ty| t.hides_contents(), &mut |_| true);
    receiver
        .into_iter()
        .chain(params)
        .enumerate()
        .filter(|(_, ty)| {
            let ty = ty.deref_ty().unwrap_or(ty);
            let mut named = |name: &str| in_result.contains(name);
            match ty.callable_signature() {
                Some((_, returns)) if !stores_callable => returns.may_hold_reference(&mut named),
                _ => ty.may_hold_reference(&mut named),
            }
        })
        .map(|(slot, _)| slot)
        .collect()
}

/// Class shells whose instances are represented by a structural `Ty` head
/// instead of `Ty::Class`. Kept in sync with `Ty::builtin_head` by the unit
/// test `builtin_head_homes_cover_every_head`.
pub const BUILTIN_HEAD_HOMES: &[&str] = &["Array", "Set", "Hash", "Option", "Result"];

fn map_bound_args(bounds: Vec<MixinRef>, f: &mut impl FnMut(&Ty) -> Ty) -> Vec<MixinRef> {
    bounds
        .into_iter()
        .map(|b| MixinRef {
            name: b.name,
            generic_args: b.generic_args.iter().map(&mut *f).collect(),
        })
        .collect()
}

#[cfg(test)]
mod map_inner_tests {
    use super::*;

    // A fold that renames every TypeParam "T" to Int, recursing everywhere.
    fn t_to_int(ty: &Ty) -> Ty {
        match ty {
            Ty::TypeParam { name, .. } if name == "T" => Ty::Int,
            other => other.clone().map_inner(&mut |c| t_to_int(c)),
        }
    }

    #[test]
    fn substitutes_through_explicit_lifetime_refs() {
        // &'a T  — the variant subst_ty historically MISSED (bug #3)
        let ty = Ty::RefLifetime(
            "a".into(),
            Box::new(Ty::TypeParam {
                name: "T".into(),
                bounds: vec![],
            }),
        );
        assert_eq!(
            t_to_int(&ty),
            Ty::RefLifetime("a".into(), Box::new(Ty::Int))
        );
    }

    #[test]
    fn substitutes_through_map_and_set() {
        // Map[T, T] and Set[T] — also historically missed by subst_ty.
        let tp = || Ty::TypeParam {
            name: "T".into(),
            bounds: vec![],
        };
        let m = Ty::Map(Box::new(tp()), Box::new(tp()));
        assert_eq!(t_to_int(&m), Ty::Map(Box::new(Ty::Int), Box::new(Ty::Int)));
        let s = Ty::Set(Box::new(tp()));
        assert_eq!(t_to_int(&s), Ty::Set(Box::new(Ty::Int)));
    }

    #[test]
    fn leaves_unrelated_leaves_untouched() {
        assert_eq!(t_to_int(&Ty::Int), Ty::Int);
        assert_eq!(t_to_int(&Ty::String), Ty::String);
    }
}

#[cfg(test)]
mod ty_helper_tests {
    use super::*;

    #[test]
    fn result_source_follows_rust_elision() {
        let r = |t: Ty| Ty::Ref(Box::new(t));
        let k = Ty::TypeParam {
            name: "K".to_string(),
            bounds: vec![],
        };
        let keys = Ty::Array(Box::new(r(k.clone())));
        // A receiver is the source of a result holding a reference.
        assert_eq!(
            result_source(true, &[], &keys),
            ResultSource::Slots(vec![0])
        );
        // A declared result with no reference borrows nothing, even when an
        // instance's `V` is a reference.
        assert_eq!(
            result_source(true, &[r(k.clone())], &Ty::Option(Box::new(k.clone()))),
            ResultSource::Nothing
        );
        // Without a receiver: the one reference parameter (a block is
        // none); several are ambiguous.
        let block = r(Ty::Fn {
            params: vec![],
            ret: Box::new(Ty::Unit),
        });
        assert_eq!(
            result_source(false, &[Ty::Int, r(Ty::String), block], &r(Ty::String)),
            ResultSource::Slots(vec![1])
        );
        assert_eq!(
            result_source(false, &[r(Ty::String), r(Ty::String)], &r(Ty::String)),
            ResultSource::Ambiguous(vec![0, 1])
        );
        assert_eq!(
            result_source(false, &[Ty::Int], &r(Ty::String)),
            ResultSource::Unknown
        );
        // A named lifetime picks every parameter carrying it.
        let named = |t: Ty| Ty::RefLifetime("a".to_string(), Box::new(t));
        assert_eq!(
            result_source(
                false,
                &[named(Ty::String), r(Ty::String), named(Ty::String)],
                &named(Ty::String)
            ),
            ResultSource::Slots(vec![0, 2])
        );
    }

    /// The carries half: the receiver and each parameter whose declared type
    /// below its top reference may hold a reference the result can hold.
    #[test]
    fn result_carries_reads_the_declared_types() {
        let r = |t: Ty| Ty::Ref(Box::new(t));
        let table = Ty::Map(Box::new(tp("K")), Box::new(tp("V")));
        let opt = |t: Ty| Ty::Option(Box::new(t));
        // `get(key: &K) -> Option[&V]` and `remove(key: &K) -> Option[V]`,
        // as a method and as a C declaration (the same receiver type):
        // the receiver only.
        for ret in [opt(r(tp("V"))), opt(tp("V"))] {
            let facts = declared_result(Some(&table), &[r(tp("K"))], &ret, &mut |_, _| true);
            assert_eq!(facts.carries, vec![0], "{ret:?}");
        }
        // `make(h: &var Hash[Int, &String]) -> Option[&String]`: slot 0.
        let held = Ty::RefMut(Box::new(Ty::Map(
            Box::new(Ty::Int),
            Box::new(r(Ty::String)),
        )));
        assert_eq!(result_carries(None, &[held], &opt(r(Ty::String))), vec![0]);
        // `pick(s: &String, xs: Array[&String]) -> &String`: `xs` only.
        let xs = Ty::Array(Box::new(r(Ty::String)));
        assert_eq!(
            result_carries(None, &[r(Ty::String), xs], &r(Ty::String)),
            vec![1]
        );
        // A user class, a `Proc` and a `some` mixin value may hold
        // references their types do not show.
        let class = Ty::Class {
            name: "W".into(),
            generic_args: vec![],
        };
        let some = Ty::SomeMixin(vec![MixinRef {
            name: "Show".into(),
            generic_args: vec![],
        }]);
        let proc_ty = any_fn(vec![], Ty::Int);
        assert_eq!(
            result_carries(None, &[r(class.clone()), proc_ty, some], &class),
            vec![0, 1, 2]
        );
        // A closure's captures reach the result only through what it
        // returns: `tap(f: Fn(&Self) -> nil) -> Self` carries the receiver,
        // `then[U](f: Fn(&Self) -> U) -> U` carries `f`.
        let self_ty = tp("Self");
        let block = |ret: Ty| Ty::Fn {
            params: vec![r(self_ty.clone())],
            ret: Box::new(ret),
        };
        assert_eq!(
            result_carries(Some(&self_ty), &[block(Ty::Unit)], &self_ty),
            vec![0]
        );
        assert_eq!(
            result_carries(Some(&self_ty), &[block(tp("U"))], &tp("U")),
            vec![1]
        );
        // A result that can hold no reference carries nothing.
        assert!(result_carries(Some(&table), &[], &Ty::Int).is_empty());
        // What a written slot may keep: a class receiver whose fields may
        // hold a `&String` and a `&var Array[&String]` keep a `&String`;
        // a class whose fields cannot keeps nothing, and a `Hash[K, V]`
        // never keeps a `&K` (it holds `K`s).
        let arr = Ty::RefMut(Box::new(Ty::Array(Box::new(r(Ty::String)))));
        assert_eq!(
            declared_keeps(Some(&class), &[r(Ty::String), arr.clone()], &mut |_, _| {
                true
            }),
            vec![(1, 0), (1, 2), (2, 0)]
        );
        assert_eq!(
            declared_keeps(Some(&class), &[r(Ty::String), arr], &mut |_, _| false),
            vec![(1, 2)]
        );
        assert!(declared_keeps(Some(&table), &[r(tp("K"))], &mut |_, _| true).is_empty());
        let shell = Ty::Class {
            name: "Hash".into(),
            generic_args: vec![tp("K"), tp("V")],
        };
        assert!(declared_keeps(Some(&shell), &[r(tp("K"))], &mut |_, _| true).is_empty());
    }

    fn tp(name: &str) -> Ty {
        Ty::TypeParam {
            name: name.into(),
            bounds: vec![],
        }
    }

    fn any_fn(params: Vec<Ty>, ret: Ty) -> Ty {
        Ty::proc_of(Ty::Fn {
            params,
            ret: Box::new(ret),
        })
    }

    fn subst(pairs: &[(&str, Ty)]) -> HashMap<String, Ty> {
        pairs
            .iter()
            .map(|(n, t)| (n.to_string(), t.clone()))
            .collect()
    }

    #[test]
    fn subst_params_reaches_every_nested_position() {
        let m = subst(&[("T", Ty::Int), ("U", Ty::Bool)]);
        // `&'a T`, `Map[String, T]`, `Fn(T) -> U` and a class's generic args:
        // the positions the three retired substitution folds each missed.
        let cases = [
            (
                Ty::RefLifetime("a".into(), Box::new(tp("T"))),
                Ty::RefLifetime("a".into(), Box::new(Ty::Int)),
            ),
            (
                Ty::Map(Box::new(Ty::String), Box::new(tp("T"))),
                Ty::Map(Box::new(Ty::String), Box::new(Ty::Int)),
            ),
            (
                Ty::Fn {
                    params: vec![tp("T")],
                    ret: Box::new(tp("U")),
                },
                Ty::Fn {
                    params: vec![Ty::Int],
                    ret: Box::new(Ty::Bool),
                },
            ),
            (
                Ty::Class {
                    name: "MutexGuard".into(),
                    generic_args: vec![tp("T")],
                },
                Ty::Class {
                    name: "MutexGuard".into(),
                    generic_args: vec![Ty::Int],
                },
            ),
        ];
        for (input, expected) in cases {
            assert_eq!(input.subst_params(&m), expected, "for {input:?}");
        }
    }

    #[test]
    fn subst_params_reaches_into_mixin_bounds() {
        // The `.rx` closure combinators take `any Fn[Fn(T) -> U]`; the bound's
        // inner function type must be substituted like any other child.
        let got = any_fn(vec![tp("T")], tp("U")).subst_params(&subst(&[("T", Ty::Int)]));
        assert_eq!(got, any_fn(vec![Ty::Int], tp("U")));
    }

    #[test]
    fn any_searches_function_and_mixin_children() {
        let is_t = &mut |t: &Ty| matches!(t, Ty::TypeParam { name, .. } if name == "T");
        assert!(any_fn(vec![Ty::Int], tp("T")).any(is_t));
        assert!(Ty::Option(Box::new(Ty::Array(Box::new(tp("T"))))).any(is_t));
        assert!(!Ty::Map(Box::new(Ty::String), Box::new(Ty::Int)).any(is_t));
    }

    #[test]
    fn holds_reference_does_not_search_a_callable_signature() {
        let takes_ref = any_fn(vec![Ty::Ref(Box::new(Ty::String))], Ty::Int);
        assert!(!takes_ref.holds_reference());
        assert!(!Ty::Array(Box::new(takes_ref)).holds_reference());
        assert!(Ty::Array(Box::new(Ty::Ref(Box::new(Ty::String)))).holds_reference());
    }

    #[test]
    fn callable_signature_reads_bare_and_mixin_spellings() {
        let bare = Ty::FnMut {
            params: vec![Ty::Int],
            ret: Box::new(Ty::Bool),
        };
        let mixin = any_fn(vec![Ty::Int], Ty::Bool);
        for ty in [&bare, &mixin] {
            let (params, ret) = ty.callable_signature().expect("callable");
            assert_eq!(params, [Ty::Int].as_slice());
            assert_eq!(ret, &Ty::Bool);
        }
        assert!(Ty::Int.callable_signature().is_none());
    }

    #[test]
    fn is_callable_accepts_an_unspelled_fn_bound_behind_a_reference() {
        let bound_only = Ty::Ref(Box::new(Ty::AnyMixin(vec![MixinRef {
            name: "FnOnce".into(),
            generic_args: vec![],
        }])));
        assert!(bound_only.is_callable());
        assert!(bound_only.peel_refs().callable_signature().is_none());
        assert!(!Ty::String.is_callable());
    }

    #[test]
    fn a_closure_bound_to_a_type_parameter_is_a_stored_proc() {
        let block = Ty::Fn {
            params: vec![Ty::Int],
            ret: Box::new(Ty::Unit),
        };
        let stored = block.as_type_argument();
        assert!(
            stored.is_proc() && !stored.is_borrowed_block(),
            "{stored:?}"
        );
        assert_eq!(stored, any_fn(vec![Ty::Int], Ty::Unit));
        assert_eq!(Ty::String.as_type_argument(), Ty::String);
    }

    #[test]
    fn method_home_names_the_dispatch_class() {
        let cases = [
            (
                Ty::Ref(Box::new(Ty::Map(Box::new(Ty::String), Box::new(Ty::Int)))),
                "Hash",
            ),
            (Ty::Option(Box::new(Ty::Int)), "Option"),
            (
                Ty::Struct {
                    name: "Pair".into(),
                    generic_args: vec![Ty::Int],
                },
                "Pair",
            ),
            (Ty::RefMut(Box::new(Ty::Char)), "Char"),
        ];
        for (ty, home) in cases {
            assert_eq!(ty.method_home(None), home, "for {ty:?}");
        }
        // A stored closure is homed on the class declared `layout closure`,
        // whatever its name: the home comes from the declaration.
        let proc = Ty::Ref(Box::new(any_fn(vec![Ty::Int], Ty::Unit)));
        assert_eq!(proc.method_home(Some("Shell")), "Shell");
        assert_ne!(proc.method_home(None), "Shell");
        // A borrowed block is no stored closure.
        let block = Ty::Fn {
            params: vec![],
            ret: Box::new(Ty::Unit),
        };
        assert_ne!(block.method_home(Some("Shell")), "Shell");
    }

    #[test]
    fn scalar_primitives_are_numbers_bool_and_char() {
        for ty in [Ty::Int8, Ty::Float32, Ty::USize, Ty::Bool, Ty::Char] {
            assert!(ty.is_scalar_primitive(), "{ty:?}");
        }
        for ty in [Ty::Unit, Ty::String, Ty::Never] {
            assert!(!ty.is_scalar_primitive(), "{ty:?}");
        }
    }

    /// One sample instance per builtin head. Pins `BUILTIN_HEAD_HOMES` (the
    /// class-eligibility list) against `Ty::builtin_head` (the Ty-variant
    /// map): adding a head to one without the other silently disables
    /// specialization for that head, which is a WRONG-ANSWER bug (the abstract
    /// body still lowers, it just mis-dispatches), not a link failure.
    fn samples() -> Vec<(Ty, &'static str)> {
        vec![
            (Ty::Array(Box::new(Ty::Int)), "Array"),
            (Ty::Set(Box::new(Ty::Int)), "Set"),
            (Ty::Map(Box::new(Ty::String), Box::new(Ty::Int)), "Hash"),
            (Ty::Option(Box::new(Ty::Int)), "Option"),
            (
                Ty::Result(Box::new(Ty::Int), Box::new(Ty::String)),
                "Result",
            ),
        ]
    }

    #[test]
    fn builtin_head_homes_cover_every_head() {
        let mut mapped: Vec<&str> = samples()
            .iter()
            .map(|(ty, expected)| {
                let (home, _) = Ty::builtin_head(ty)
                    .unwrap_or_else(|| panic!("no builtin head home for {ty:?}"));
                assert_eq!(home, *expected, "unexpected home for {ty:?}");
                home
            })
            .collect();
        mapped.sort_unstable();
        let mut listed: Vec<&str> = BUILTIN_HEAD_HOMES.to_vec();
        listed.sort_unstable();
        assert_eq!(
            mapped, listed,
            "BUILTIN_HEAD_HOMES and Ty::builtin_head have drifted apart"
        );
    }

    #[test]
    fn every_head_round_trips_through_its_inverse() {
        for (ty, _) in samples() {
            let (class, args) = Ty::builtin_head(&ty).expect("head");
            assert_eq!(
                Ty::from_builtin_head(class, &args).as_ref(),
                Some(&ty),
                "Ty::from_builtin_head is not the inverse of Ty::builtin_head for {ty:?}"
            );
        }
    }

    #[test]
    fn exceeds_nodes_counts_every_nested_type() {
        let pair = Ty::Tuple(vec![Ty::Int, Ty::Int]);
        assert!(!pair.exceeds_nodes(3));
        assert!(pair.exceeds_nodes(2));
        assert!(!Ty::Int.exceeds_nodes(1));
        assert!(Ty::Int.exceeds_nodes(0));
    }

    #[test]
    fn a_non_generic_class_is_not_a_builtin_head() {
        let plain = Ty::Class {
            name: "Array".to_string(),
            generic_args: vec![],
        };
        assert!(Ty::builtin_head(&plain).is_none());
        let user = Ty::Class {
            name: "Box".to_string(),
            generic_args: vec![Ty::Int],
        };
        assert!(Ty::builtin_head(&user).is_none());
    }
}
