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
