//! Symbol table — stores all definitions in the program.
//!
//! Every named entity (variable, function, class, field, etc.) gets a unique
//! DefId and a corresponding Definition entry in the symbol table.

use crate::hir::nodes::{DefId, HirSelfMode};
use crate::hir::types::{MixinRef, Ty};
use crate::lexer::token::Span;
use crate::parser::ast::Visibility;
use std::collections::HashMap;
use std::sync::OnceLock;

/// The signature of a function or method.
#[derive(Debug, Clone)]
pub struct FnSignature {
    pub self_mode: Option<HirSelfMode>,
    pub is_class_method: bool,
    pub is_async: bool,
    pub generic_params: Vec<GenericParamInfo>,
    pub params: Vec<ParamInfo>,
    pub return_ty: Ty,
    /// #06.8 Phase 2: the C-symbol alias declared via
    /// `lib "X" def name as "<c-symbol>"(...) end`. `Some(c)` means the
    /// linker will resolve this function to symbol `c`; `None` means the
    /// Ruxen name *is* the linked C symbol (the historical default for
    /// `lib` blocks). Non-FFI functions always carry `None`.
    pub c_symbol: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GenericParamInfo {
    pub name: String,
    pub bounds: Vec<MixinRef>,
    /// Tier-2 const generics (T2.02 S5): tracks whether this slot
    /// holds a type parameter or a const parameter.  Default `Type`
    /// keeps every pre-S5 construction site backwards-compatible.
    pub kind: GenericParamKind,
}

/// Tier-2 const generics: kind of a declared generic parameter.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum GenericParamKind {
    #[default]
    Type,
    Const {
        ty: Ty,
    },
}

impl GenericParamInfo {
    /// Backwards-compatible constructor for type generic params.
    pub fn type_param(name: String, bounds: Vec<MixinRef>) -> Self {
        Self {
            name,
            bounds,
            kind: GenericParamKind::Type,
        }
    }

    /// Stage 5: constructor for a const generic parameter.
    pub fn const_param(name: String, ty: Ty) -> Self {
        Self {
            name,
            bounds: vec![],
            kind: GenericParamKind::Const { ty },
        }
    }
}

#[derive(Debug, Clone)]
pub struct ParamInfo {
    pub name: String,
    pub ty: Ty,
    pub auto_assign: bool,
    pub default: Option<crate::parser::ast::Expr>,
}

/// The `Send`/`Sync` markers a class, struct or enum declares
/// (`docs/specs/types/send_sync_enforcement.spec.md` B10 to B12).
///
/// `include Send` (and `unsafe include Send`) sets `manual_*`;
/// `include !Send` (`exclude Send`) sets `opt_out_*`. Both start false: v1
/// has no auto-derive for user types to suppress.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AutoTraitFlags {
    pub opt_out_send: bool,
    pub opt_out_sync: bool,
    pub manual_send: bool,
    pub manual_sync: bool,
}

impl AutoTraitFlags {
    /// Record one `include`/`exclude` of `trait_name`. Names other than
    /// `Send` and `Sync` are ignored.
    pub fn record(&mut self, trait_name: &str, negative: bool) {
        let (opt_out, manual) = match trait_name {
            "Send" => (&mut self.opt_out_send, &mut self.manual_send),
            "Sync" => (&mut self.opt_out_sync, &mut self.manual_sync),
            _ => return,
        };
        // The last declaration wins, so `exclude Send` then `include Send`
        // (or the reverse) never leaves both flags set.
        *opt_out = negative;
        *manual = !negative;
    }

    /// Whether the type explicitly includes `Send` (`is_send`) or `Sync`.
    pub fn manual(&self, is_send: bool) -> bool {
        if is_send {
            self.manual_send
        } else {
            self.manual_sync
        }
    }

    /// Whether the type explicitly excludes `Send` (`is_send`) or `Sync`.
    pub fn opted_out(&self, is_send: bool) -> bool {
        if is_send {
            self.opt_out_send
        } else {
            self.opt_out_sync
        }
    }
}

/// What a compiler-supplied `lib` parameter carries: its type argument's
/// element descriptor (`ElemDesc[T]`, how a slot of it is copied and freed)
/// or key descriptor (`KeyDesc[K]`, the same plus how it hashes and compares
/// as a table key). Declared in `.rx` by `layout elem_desc` / `layout key_desc`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Supplied {
    ElemDesc,
    KeyDesc,
}

impl Supplied {
    /// The descriptor class layout directive `class` declares, if any.
    pub fn of_class(class: &crate::parser::ast::ClassDef) -> Option<Self> {
        if class.has_layout("elem_desc") {
            Some(Supplied::ElemDesc)
        } else if class.has_layout("key_desc") {
            Some(Supplied::KeyDesc)
        } else {
            None
        }
    }
}

/// Information about a class definition.
#[derive(Debug, Clone)]
pub struct ClassInfo {
    pub generic_params: Vec<GenericParamInfo>,
    pub parent: Option<DefId>,
    pub fields: Vec<DefId>,
    pub methods: Vec<DefId>,
    pub auto_traits: AutoTraitFlags,
    /// T2.02 S9: where-clause const predicates (`where N > 0`,
    /// `where N + M == 8`).  Evaluated at instantiation time
    /// against the binding map; failing predicates emit E0706.
    /// Default empty for backwards compatibility.
    pub const_predicates: Vec<HirConstPredicate>,
    /// #06.8 T0c: set when the class body carried a
    /// `layout flat_heap_struct` directive. Marker only in Wave 1 —
    /// the link-time layout-mismatch check (E0724) is reserved but
    /// not yet emitted. Will be consumed by the runtime-ABI pin
    /// check once a real stdlib class is migrated to user-source
    /// `.rx` and gates a `ruxen_<class>_layout_check` symbol.
    pub flat_heap_struct: bool,
    /// Set when the class body carried a `layout closure` directive: its
    /// values are closure pairs, so it is the method home of every stored
    /// closure value (`Proc[...]`, `any Fn[...]`). The core library's
    /// `class Closure` declares it; [`SymbolTable::closure_shell`] reads it.
    pub closure_shell: bool,
    /// Set by a `layout elem_desc` or `layout key_desc` directive: a `lib`
    /// parameter of this class's type (`ElemDesc[T]`, `KeyDesc[K]` in
    /// std/core) is a descriptor of its type argument, filled in by the
    /// compiler, never by a caller.
    pub supplied: Option<Supplied>,
    /// Mixin vtables Phase B-1: DefIds of mixins this class
    /// `include`s that are marked `dispatch runtime`. Populated by
    /// `resolve_class_def` after looking up each include's mixin
    /// def. Consumed by Phase B-2/B-3 (vtable + class_info struct
    /// emission) and Phase B-4/B-5 (header allocation + init-time
    /// write). Spec: docs/specs/types/mixin_vtables.spec.md.
    ///
    /// Empty for any class that doesn't include a `dispatch runtime`
    /// mixin — those classes keep their existing flat layout with no
    /// class_info_ptr header.
    pub runtime_dispatch_includes: Vec<DefId>,
}

/// Information about a struct definition.
#[derive(Debug, Clone)]
pub struct StructInfo {
    pub generic_params: Vec<GenericParamInfo>,
    pub fields: Vec<DefId>,
    pub layout: Vec<String>,
    pub auto_traits: AutoTraitFlags,
    /// T2.02 S9: see `ClassInfo::const_predicates`.
    pub const_predicates: Vec<HirConstPredicate>,
}

/// HIR-level lowered where-clause const predicate.
///
/// The parser's `ast::ConstPredicate` holds a raw `Expr` tree
/// (full comparison + arithmetic + identifier surface).  At resolve
/// time we lower the recognised shape into this compact form so
/// evaluation at instantiation is a straight binary comparison over
/// two `ConstExpr` sub-trees.
///
/// Unsupported shapes lower to a sentinel that evaluates to false
/// at every instantiation; users see E0706 with the unsupported
/// form's span.
#[derive(Debug, Clone, PartialEq)]
pub struct HirConstPredicate {
    pub lhs: crate::hir::types::ConstExpr,
    pub op: ConstPredOp,
    pub rhs: crate::hir::types::ConstExpr,
    pub span: crate::lexer::token::Span,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstPredOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// Information about an enum definition.
#[derive(Debug, Clone)]
pub struct EnumInfo {
    pub generic_params: Vec<GenericParamInfo>,
    pub variants: Vec<DefId>,
    pub auto_traits: AutoTraitFlags,
    /// T2.02 S9: see `ClassInfo::const_predicates`.
    pub const_predicates: Vec<HirConstPredicate>,
}

/// Information about a trait definition.
#[derive(Debug, Clone)]
pub struct MixinInfo {
    pub generic_params: Vec<GenericParamInfo>,
    pub super_traits: Vec<MixinRef>,
    pub required_methods: Vec<String>,
    pub default_methods: Vec<String>,
    pub assoc_types: Vec<String>,
    /// Mixin vtables spec §B1 — `Static` for ordinary mixins
    /// (default), `Runtime` for `mixin Foo dispatch runtime`. Typeck
    /// keys off this flag when validating `&Mixin` / `&var Mixin`
    /// references (E1118) and runtime-dispatch satisfaction (E1117).
    pub dispatch_mode: crate::parser::ast::DispatchMode,
}

/// The kind of definition — what this name refers to.
#[derive(Debug, Clone)]
pub enum DefKind {
    Variable {
        mutable: bool,
        ty: Ty,
    },
    Function {
        signature: FnSignature,
    },
    OverloadSet {
        candidates: Vec<DefId>,
    },
    Class {
        info: ClassInfo,
    },
    Struct {
        info: StructInfo,
    },
    Enum {
        info: EnumInfo,
    },
    EnumVariant {
        parent: DefId,
        variant_idx: usize,
        kind: VariantDefKind,
    },
    Trait {
        info: MixinInfo,
    },
    TypeAlias {
        target: Ty,
        /// The alias's own generic parameters (`type Proc[F] = any Fn[F]`),
        /// in order; `target` names them as `Ty::TypeParam`s, which a use
        /// (`Proc[(Int) -> nil]`) replaces by its arguments
        /// (`Resolver::instantiate_alias`). Empty for a plain alias.
        params: Vec<String>,
    },
    Newtype {
        inner: Ty,
    },
    TypeParam {
        bounds: Vec<MixinRef>,
    },
    /// Tier-2 const generics (T2.02 stage 3).
    ///
    /// Registered when the resolver encounters
    /// `GenericParam::Const { name, ty, .. }`.  `ty` is the resolved
    /// `Ty` of the annotation (typically `Ty::USize` / `Ty::Int` /
    /// `Ty::Bool`).  Stage 5 will use this to validate const-arg
    /// types at use sites and emit E0701 when the wrong primitive
    /// is passed.
    ConstParam {
        ty: Ty,
    },
    Module {
        items: Vec<DefId>,
    },
    Field {
        parent: DefId,
        ty: Ty,
        index: usize,
    },
    Method {
        parent: DefId,
        signature: FnSignature,
    },
    Const {
        ty: Ty,
    },
    /// A parameter in a function or closure
    Param {
        ty: Ty,
        auto_assign: bool,
    },
    /// Self reference inside a class/impl
    SelfValue {
        ty: Ty,
    },
}

impl DefKind {
    /// The signature of a `Function` or `Method`, `None` for every other kind.
    pub fn signature(&self) -> Option<&FnSignature> {
        match self {
            DefKind::Function { signature } | DefKind::Method { signature, .. } => Some(signature),
            _ => None,
        }
    }

    /// The declared generic params of a nominal type (`Class`, `Struct` or
    /// `Enum`), `None` for every other kind.
    pub fn generic_params(&self) -> Option<&[GenericParamInfo]> {
        match self {
            DefKind::Class { info } => Some(&info.generic_params),
            DefKind::Struct { info } => Some(&info.generic_params),
            DefKind::Enum { info } => Some(&info.generic_params),
            _ => None,
        }
    }

    /// The `Send`/`Sync` markers of a nominal type (`Class`, `Struct` or
    /// `Enum`), `None` for every other kind.
    pub fn auto_traits(&self) -> Option<&AutoTraitFlags> {
        match self {
            DefKind::Class { info } => Some(&info.auto_traits),
            DefKind::Struct { info } => Some(&info.auto_traits),
            DefKind::Enum { info } => Some(&info.auto_traits),
            _ => None,
        }
    }

    /// Mutable form of [`Self::auto_traits`].
    pub fn auto_traits_mut(&mut self) -> Option<&mut AutoTraitFlags> {
        match self {
            DefKind::Class { info } => Some(&mut info.auto_traits),
            DefKind::Struct { info } => Some(&mut info.auto_traits),
            DefKind::Enum { info } => Some(&mut info.auto_traits),
            _ => None,
        }
    }
}

/// Kind of enum variant (for construction checking).
#[derive(Debug, Clone)]
pub enum VariantDefKind {
    Unit,
    Tuple(Vec<Ty>),
    Struct(Vec<(String, Ty)>),
}

/// A single definition in the symbol table.
#[derive(Debug, Clone)]
pub struct Definition {
    pub id: DefId,
    pub name: String,
    pub kind: DefKind,
    pub visibility: Visibility,
    pub span: Span,
}

/// The name and parent index over a [`SymbolTable`]. Every list is in
/// definition (DefId) order, so "the first match in the index" is the same
/// definition a front-to-back scan of the table would find.
#[derive(Debug, Default)]
struct SymbolIndex {
    /// Definition name → every DefId carrying it.
    by_name: HashMap<String, Vec<DefId>>,
    /// MIR symbol spelling (`.` → `_`) → every DefId whose name contains a
    /// `.` and mangles to it. Names without a `.` are only in `by_name`.
    by_symbol: HashMap<String, Vec<DefId>>,
    /// `base` → every DefId whose name starts with `base__overload` (the
    /// resolver's overload renaming), keyed by every such `base`.
    overloads: HashMap<String, Vec<DefId>>,
    /// Parent DefId → its `Method`, `Field` and `EnumVariant` children.
    children: HashMap<DefId, Vec<DefId>>,
    /// Every `Class` definition.
    classes: Vec<DefId>,
    /// The class declared `layout closure` (`ClassInfo::closure_shell`).
    closure_shell: Option<DefId>,
    /// The classes declared `layout elem_desc` / `layout key_desc`
    /// (`ClassInfo::supplied`).
    supplied_shells: Vec<(DefId, Supplied)>,
}

impl SymbolIndex {
    fn build(definitions: &[Definition]) -> Self {
        let mut index = Self::default();
        for def in definitions {
            index.insert(def);
        }
        index
    }

    fn insert(&mut self, def: &Definition) {
        self.by_name
            .entry(def.name.clone())
            .or_default()
            .push(def.id);
        if def.name.contains('.') {
            self.by_symbol
                .entry(def.name.replace('.', "_"))
                .or_default()
                .push(def.id);
        }
        for (pos, _) in def.name.match_indices(OVERLOAD_MARK) {
            self.overloads
                .entry(def.name[..pos].to_string())
                .or_default()
                .push(def.id);
        }
        match &def.kind {
            DefKind::Method { parent, .. }
            | DefKind::Field { parent, .. }
            | DefKind::EnumVariant { parent, .. } => {
                self.children.entry(*parent).or_default().push(def.id);
            }
            DefKind::Class { info } => {
                self.classes.push(def.id);
                if info.closure_shell {
                    self.closure_shell = Some(def.id);
                }
                if let Some(supplied) = info.supplied {
                    self.supplied_shells.push((def.id, supplied));
                }
            }
            DefKind::Variable { .. }
            | DefKind::Function { .. }
            | DefKind::OverloadSet { .. }
            | DefKind::Struct { .. }
            | DefKind::Enum { .. }
            | DefKind::Trait { .. }
            | DefKind::TypeAlias { .. }
            | DefKind::Newtype { .. }
            | DefKind::TypeParam { .. }
            | DefKind::ConstParam { .. }
            | DefKind::Module { .. }
            | DefKind::Const { .. }
            | DefKind::Param { .. }
            | DefKind::SelfValue { .. } => {}
        }
    }
}

/// The infix the resolver renames an overload with: `name__overload<N>`.
const OVERLOAD_MARK: &str = "__overload";

/// True for a `Method` named `method` or `method__overload<N>`.
fn is_method_named(def: &Definition, method: &str) -> bool {
    matches!(def.kind, DefKind::Method { .. })
        && (def.name == method
            || def
                .name
                .strip_prefix(method)
                .is_some_and(|rest| rest.starts_with(OVERLOAD_MARK)))
}

/// True for a `Class`, `Struct` or `Enum` definition: a nominal type with
/// fields or variants and methods.
fn is_nominal(def: &Definition) -> bool {
    matches!(
        def.kind,
        DefKind::Class { .. } | DefKind::Struct { .. } | DefKind::Enum { .. }
    )
}

/// The symbol table: stores all definitions indexed by DefId, plus a name and
/// parent index (see [`SymbolTable::named`]).
///
/// The index is maintained by `define` and dropped by `get_mut`, because a
/// caller holding `&mut Definition` may rename it (overload mangling, module
/// qualification) or change its kind (a builtin anchor becoming a `Class`).
/// The next query rebuilds it. `update_ty` changes only types, which the
/// index does not key on, so it keeps the index.
#[derive(Debug)]
pub struct SymbolTable {
    definitions: Vec<Definition>,
    index: OnceLock<SymbolIndex>,
    /// Ruby `alias new old` method synonyms (docs/decisions/alias-keyword.md):
    /// home-type name → `{alias → canonical}`. Populated by the resolver,
    /// consumed by MIR's `select_method_symbol_name` to rewrite an alias method
    /// name to its canonical before symbol mangling (so `set.member?(x)` emits
    /// `Set_include?`, not a bodiless `Set_member?`). A pure synonym — no extra
    /// method body exists.
    method_aliases: std::collections::HashMap<String, std::collections::HashMap<String, String>>,
    /// The name a diagnostic shows for a definition the resolver
    /// synthesized under an internal name (the block slot `__block` reads
    /// `block`). See [`SymbolTable::display_name`].
    display_names: std::collections::HashMap<DefId, String>,
}

impl SymbolTable {
    pub fn new() -> Self {
        Self {
            definitions: Vec::new(),
            index: OnceLock::new(),
            method_aliases: std::collections::HashMap::new(),
            display_names: std::collections::HashMap::new(),
        }
    }

    /// Show `id` as `name` in diagnostics (it was defined under an internal
    /// name lookups use).
    pub fn set_display_name(&mut self, id: DefId, name: impl Into<String>) {
        self.display_names.insert(id, name.into());
    }

    /// The name diagnostics show for `id`: its display name when the
    /// resolver gave it one, else its definition name.
    pub fn display_name(&self, id: DefId) -> Option<&str> {
        self.display_names
            .get(&id)
            .map(String::as_str)
            .or_else(|| self.get(id).map(|def| def.name.as_str()))
    }

    /// Record the resolver's method-alias map onto the symbol table so the MIR
    /// lowerer (which only holds `&SymbolTable`) can rewrite alias method names
    /// to their canonical at mangle time.
    pub fn set_method_aliases(
        &mut self,
        aliases: std::collections::HashMap<String, std::collections::HashMap<String, String>>,
    ) {
        self.method_aliases = aliases;
    }

    /// Resolve a method name on `type_name` through the alias map, returning the
    /// canonical method name if `method_name` is a registered alias, else the
    /// name unchanged (docs/decisions/alias-keyword.md). The lookup tries the
    /// full `type_name` first, then its generic-free base (`Array[Int]` →
    /// `Array`) — the alias map is keyed by the generic-free class name, while
    /// MIR call sites carry the suffixed receiver type.
    pub fn canonical_method_name<'a>(&'a self, type_name: &str, method_name: &'a str) -> &'a str {
        let base = type_name.split('[').next().unwrap_or(type_name);
        self.method_aliases
            .get(type_name)
            .or_else(|| self.method_aliases.get(base))
            .and_then(|m| m.get(method_name))
            .map(|s| s.as_str())
            .unwrap_or(method_name)
    }

    /// Allocate a new definition and return its DefId.
    pub fn define(
        &mut self,
        name: String,
        kind: DefKind,
        visibility: Visibility,
        span: Span,
    ) -> DefId {
        let id = self.definitions.len() as DefId;
        let def = Definition {
            id,
            name,
            kind,
            visibility,
            span,
        };
        if let Some(index) = self.index.get_mut() {
            index.insert(&def);
        }
        self.definitions.push(def);
        id
    }

    /// Look up a definition by DefId.
    pub fn get(&self, id: DefId) -> Option<&Definition> {
        self.definitions.get(id as usize)
    }

    /// Get a mutable reference to a definition. Drops the name and parent
    /// index, since the caller may rename the definition or change its kind.
    pub fn get_mut(&mut self, id: DefId) -> Option<&mut Definition> {
        self.index.take();
        self.definitions.get_mut(id as usize)
    }

    fn index(&self) -> &SymbolIndex {
        self.index
            .get_or_init(|| SymbolIndex::build(&self.definitions))
    }

    fn defs<'a>(
        &'a self,
        ids: Option<&'a Vec<DefId>>,
    ) -> impl Iterator<Item = &'a Definition> + 'a {
        ids.into_iter()
            .flatten()
            .map(|&id| &self.definitions[id as usize])
    }

    /// Every definition named exactly `name`, in definition order.
    pub fn named<'a>(&'a self, name: &str) -> impl Iterator<Item = &'a Definition> + 'a {
        self.defs(self.index().by_name.get(name))
    }

    /// Every definition named `name` or `name__overload…` (the resolver's
    /// overload renaming), in definition order.
    pub fn overloads_named(&self, name: &str) -> impl Iterator<Item = &Definition> + '_ {
        let index = self.index();
        let mut ids: Vec<DefId> = index
            .by_name
            .get(name)
            .into_iter()
            .chain(index.overloads.get(name))
            .flatten()
            .copied()
            .collect();
        ids.sort_unstable();
        ids.into_iter().map(|id| &self.definitions[id as usize])
    }

    /// The first `Class`, `Struct` or `Enum` named exactly `name`.
    pub fn nominal(&self, name: &str) -> Option<&Definition> {
        self.named(name).find(|def| is_nominal(def))
    }

    /// The first `Class`, `Struct` or `Enum` whose name, or whose MIR symbol
    /// spelling (`.` → `_`, so `Net.Socket` is `Net_Socket`), is `symbol`.
    /// Used to split a mangled `Type_method` callee into its type.
    pub fn type_def(&self, symbol: &str) -> Option<&Definition> {
        let exact = self.nominal(symbol);
        let mangled = self
            .defs(self.index().by_symbol.get(symbol))
            .find(|def| is_nominal(def));
        match (exact, mangled) {
            (Some(a), Some(b)) => Some(if a.id < b.id { a } else { b }),
            (a, b) => a.or(b),
        }
    }

    /// True when the class `name` owns its fields: reading one borrows it,
    /// storing one frees the old value, and the object's death frees them.
    /// A synthesised async state machine (it carries a `__state` field) does
    /// not: its fields mirror the async body's parameters and locals, which
    /// `poll` moves out of without clearing the field, and its `__sub_i`
    /// sub-futures are dropped by its own synthesised `def drop`
    /// (`Lowerer::synthesize_future_state_machine_drop`).
    /// Whether the type named `name` declares `def consume drop`: its drop
    /// takes the whole value over and releases its storage itself (a C
    /// handle whose release is its refcount or handle free, declared
    /// `def consume drop as "ruxen_mutex_drop"`), so drop elaboration
    /// emits no storage free after calling it.
    pub fn drop_consumes_self(&self, name: &str) -> bool {
        self.type_def(name).is_some_and(|def| {
            self.methods_of(def.id, "drop").any(|m| {
                matches!(&m.kind, DefKind::Method { signature, .. }
                    if signature.self_mode == Some(crate::hir::nodes::HirSelfMode::Consuming))
            })
        })
    }

    /// The class declared `layout closure` (`class Closure` in std/core):
    /// the method home of every stored closure value. Read off the
    /// declaration; `None` when no such class is loaded.
    pub fn closure_shell(&self) -> Option<&Definition> {
        self.index()
            .closure_shell
            .map(|id| &self.definitions[id as usize])
    }

    /// The name of [`Self::closure_shell`], the home `Ty::method_home`
    /// gives a stored closure value.
    pub fn closure_shell_name(&self) -> Option<&str> {
        self.closure_shell().map(|def| def.name.as_str())
    }

    /// What `ty` carries and of which type when it is a class that declared
    /// `layout elem_desc` or `layout key_desc` (`ElemDesc[T]`, `KeyDesc[K]` in
    /// std/core): a `lib` parameter of that type is filled by the compiler.
    pub fn supplied_arg<'t>(&self, ty: &'t Ty) -> Option<(Supplied, &'t Ty)> {
        let Ty::Class { name, generic_args } = ty else {
            return None;
        };
        let (_, supplied) = self
            .index()
            .supplied_shells
            .iter()
            .find(|(id, _)| self.definitions[*id as usize].name == *name)?;
        Some((*supplied, generic_args.first()?))
    }

    /// The method a derive of the mixin `trait_name` defines: the one the
    /// mixin declares in `.rx` (`mixin Clone` / `def clone -> Self`),
    /// required first. `None` for a mixin that declares none (`Copy`, `Eq`)
    /// or is not loaded.
    pub fn derived_method(&self, trait_name: &str) -> Option<&str> {
        self.named(trait_name).find_map(|def| match &def.kind {
            DefKind::Trait { info } => info
                .required_methods
                .first()
                .or(info.default_methods.first())
                .map(String::as_str),
            _ => None,
        })
    }

    /// Whether the nominal type `type_name` defines the method a derive of
    /// `trait_name` would ([`Self::derived_method`]) itself, so no body is
    /// derived for it. The one rule every derive decision reads.
    pub fn declares_derived_method(&self, type_name: &str, trait_name: &str) -> bool {
        let Some(method) = self.derived_method(trait_name) else {
            return false;
        };
        self.nominal(type_name)
            .is_some_and(|def| self.methods_of(def.id, method).next().is_some())
    }

    pub fn class_owns_fields(&self, name: &str) -> bool {
        let Some(DefKind::Class { info }) = self.type_def(name).map(|def| &def.kind) else {
            return false;
        };
        !info
            .fields
            .iter()
            .any(|&field| self.get(field).is_some_and(|f| f.name == "__state"))
    }

    /// The `Method`, `Field` and `EnumVariant` definitions whose parent is
    /// `parent`, in definition order.
    pub fn children_of(&self, parent: DefId) -> impl Iterator<Item = &Definition> + '_ {
        self.defs(self.index().children.get(&parent))
    }

    /// The methods of `parent` named `method`, including the
    /// `method__overload<N>` siblings the resolver renames overloads to, in
    /// definition order.
    pub fn methods_of<'a>(
        &'a self,
        parent: DefId,
        method: &'a str,
    ) -> impl Iterator<Item = &'a Definition> + 'a {
        self.children_of(parent)
            .filter(move |def| is_method_named(def, method))
    }

    /// The methods named `method` (overload-aware, see
    /// [`Self::methods_of`]) of every definition named `owner`, in
    /// definition order.
    pub fn methods_named<'a>(
        &'a self,
        owner: &str,
        method: &str,
    ) -> std::vec::IntoIter<&'a Definition> {
        let mut found: Vec<&Definition> = Vec::new();
        for parent in self.named(owner) {
            found.extend(
                self.children_of(parent.id)
                    .filter(|def| is_method_named(def, method)),
            );
        }
        found.sort_by_key(|def| def.id);
        found.into_iter()
    }

    /// Every `Class` definition, in definition order.
    pub fn classes(&self) -> impl Iterator<Item = &Definition> + '_ {
        self.index()
            .classes
            .iter()
            .map(|&id| &self.definitions[id as usize])
    }

    /// Get the total number of definitions.
    pub fn len(&self) -> usize {
        self.definitions.len()
    }

    /// Check if the symbol table is empty.
    pub fn is_empty(&self) -> bool {
        self.definitions.is_empty()
    }

    /// Iterate over all definitions.
    pub fn iter(&self) -> impl Iterator<Item = &Definition> {
        self.definitions.iter()
    }

    /// Update the type of a variable/field/param definition.
    pub fn update_ty(&mut self, id: DefId, new_ty: Ty) {
        if let Some(def) = self.definitions.get_mut(id as usize) {
            match &mut def.kind {
                DefKind::Variable { ty, .. }
                | DefKind::Field { ty, .. }
                | DefKind::Param { ty, .. }
                | DefKind::SelfValue { ty }
                | DefKind::Const { ty }
                | DefKind::ConstParam { ty } => *ty = new_ty,
                _ => {}
            }
        }
    }

    /// Get the type associated with a definition, if applicable.
    pub fn def_ty(&self, id: DefId) -> Option<Ty> {
        self.get(id).and_then(|def| match &def.kind {
            DefKind::Variable { ty, .. }
            | DefKind::Field { ty, .. }
            | DefKind::Param { ty, .. }
            | DefKind::SelfValue { ty }
            | DefKind::Const { ty }
            | DefKind::ConstParam { ty } => Some(ty.clone()),
            DefKind::OverloadSet { candidates } => {
                candidates.first().and_then(|id| self.def_ty(*id))
            }
            DefKind::Function { signature } | DefKind::Method { signature, .. } => Some(Ty::Fn {
                params: signature.params.iter().map(|p| p.ty.clone()).collect(),
                ret: Box::new(if signature.is_async {
                    Ty::Class {
                        name: "Future".to_string(),
                        generic_args: vec![signature.return_ty.clone()],
                    }
                } else {
                    signature.return_ty.clone()
                }),
            }),
            _ => None,
        })
    }
}

impl Default for SymbolTable {
    fn default() -> Self {
        Self::new()
    }
}

mod derive_support;
pub use derive_support::{ty_has_derive_trait, ty_is_effectively_copy};

#[cfg(test)]
mod tests;
