//! The instance engine: one worklist that specializes, per concrete
//! instantiation, every generic body whose lowering depends on its type
//! parameters.
//!
//! Inside a generic body a value of a type parameter `T` has no concrete
//! type, so MIR's type-directed lowerings (`==`, `"#{x}"`, a method call on
//! the value, a derived `clone`, a compiler-supplied `ElemDesc[T]`) have
//! nothing to dispatch on. The engine lowers such a body once per concrete
//! `T` a program reaches, with `T` substituted, so the existing concrete
//! paths fire unchanged.
//!
//! ── Templates ──
//! A template is a generic body family with the type parameters its
//! instances bind ([`Template::params`], in key order):
//!   * a user generic class (no `lib` methods): every method it defines or
//!     includes, plus its derived bodies, per class instantiation;
//!   * one body method of a lib shell (a generic class with `lib` methods:
//!     `Array[T]#join`, `MutexGuard[T]#get`), per receiver instantiation;
//!   * a generic free function;
//!   * a method with generic parameters of its own, per receiver and own
//!     arguments.
//!
//! ── The one specialization predicate ──
//! A candidate is a template when its lowering depends on its parameters:
//! one is mixin-bounded (a `where` bound included), a body dispatches on a
//! bare parameter value (interpolation, method receiver) or on an operand
//! mentioning one, a class field mentions one (its derived bodies), a body
//! calls a `lib` method whose `ElemDesc` / `KeyDesc` the compiler fills from
//! the receiver, or a body reaches another template with arguments that mention
//! its parameters. The last rule is a fixpoint over the candidates, so a
//! generic body that only forwards to a template is a template too.
//!
//! ── The worklist ──
//! Seeds are every concrete site in the program. Each new instance is keyed
//! by its injective symbol (`names::mono_base`), its bodies are substituted
//! once, and their sites feed the worklist in turn, so every instance any
//! concrete body reaches exists, through any chain of templates. An
//! instance whose arguments nest deeper than [`MAX_INSTANCE_DEPTH`] is error
//! E0746 (an expanding instantiation such as `Nest[T]` building
//! `Nest[Array[T]]` would otherwise never end).
//!
//! ── The opaque rule ──
//! Every template's own body is still lowered once, generic, and recorded
//! in `Instances::opaque_bodies`. Concrete call sites redirect to the
//! instances, so an opaque body is reached only from another opaque body;
//! `prune_unmonomorphised_bodies` removes the unreached ones and reports one
//! a live function still reaches.

mod names;
mod redirect;
mod sites;

pub(super) use names::{
    abstract_callee, mono_base, strip_mono_suffix, symbol_token, ABSTRACT_CALLEE_PREFIX,
};
pub(super) use sites::unify_collect_type_params;

use super::derive::DeriveField;
use super::*;
use crate::hir::visit::{walk_item, Visit};
use sites::{
    is_bare_type_param, is_fully_concrete, mentions_type_param, type_depth, walk_expr_sites,
    walk_func_sites, Site,
};

/// The deepest type nesting an instance argument may reach (error E0746).
const MAX_INSTANCE_DEPTH: usize = 32;

/// A generic body family the engine instantiates (see the module docs).
#[derive(Clone)]
enum Template {
    Class(HirClassDef),
    ShellMethod {
        class: String,
        params: Vec<String>,
        def: HirFuncDef,
    },
    Function(HirFuncDef),
    Method {
        /// The owner's symbol prefix (`Frame`, `Mod_Frame`).
        owner: String,
        /// The owner's type with its own generic params (`Box[T]`).
        owner_ty: Ty,
        owner_params: Vec<String>,
        def: HirFuncDef,
    },
}

impl Template {
    /// The type parameters an instance binds, in key order: the owner's,
    /// then the method's own.
    fn params(&self) -> Vec<String> {
        match self {
            Template::Class(c) => names_of(&c.generic_params),
            Template::ShellMethod { params, .. } => params.clone(),
            Template::Function(def) => names_of(&def.generic_params),
            Template::Method {
                owner_params, def, ..
            } => owner_params
                .iter()
                .cloned()
                .chain(own_params(def, owner_params))
                .collect(),
        }
    }

    /// The source bodies of this template.
    fn defs(&self) -> Vec<&HirFuncDef> {
        match self {
            Template::Class(c) => class_methods(c).collect(),
            Template::ShellMethod { def, .. }
            | Template::Function(def)
            | Template::Method { def, .. } => vec![def],
        }
    }

    /// Whether one of [`Template::params`] carries a mixin bound (a method's
    /// `where` bound on its owner's parameter included).
    fn bounded(&self) -> bool {
        let params = self.params();
        let bounded = |g: &HirGenericParam| !g.bounds.is_empty() && params.contains(&g.name);
        let class_bound =
            matches!(self, Template::Class(c) if c.generic_params.iter().any(bounded));
        class_bound
            || self
                .defs()
                .iter()
                .any(|def| def.generic_params.iter().any(bounded))
    }

    fn name(&self) -> &str {
        match self {
            Template::Class(c) => &c.name,
            Template::ShellMethod { def, .. }
            | Template::Function(def)
            | Template::Method { def, .. } => &def.name,
        }
    }
}

fn names_of(params: &[HirGenericParam]) -> Vec<String> {
    params.iter().map(|g| g.name.clone()).collect()
}

/// A method's own generic parameters: those its owner does not declare
/// (a `where` predicate on the owner's parameter adds an entry that is not).
fn own_params(def: &HirFuncDef, owner_params: &[String]) -> Vec<String> {
    def.generic_params
        .iter()
        .filter(|g| !owner_params.contains(&g.name))
        .map(|g| g.name.clone())
        .collect()
}

/// A class's methods and the methods of its inline impl blocks.
pub(super) fn class_methods(c: &HirClassDef) -> impl Iterator<Item = &HirFuncDef> {
    c.methods.iter().chain(
        c.impl_blocks
            .iter()
            .flat_map(|b| b.items.iter())
            .filter_map(|i| match i {
                HirImplItem::Method(m) => Some(m),
                _ => None,
            }),
    )
}

/// One concrete instantiation of a template.
struct Instance {
    template: usize,
    /// The instance symbol: a class's base (`Box__mono__Int`), else the
    /// emitted function's name.
    symbol: String,
    /// The `self` type the bodies are lowered with.
    self_ty: Option<Ty>,
    /// The substituted bodies, with the symbol each is lowered under.
    bodies: Vec<(String, HirFuncDef)>,
    /// A class instance's derive fields, substituted.
    fields: Vec<DeriveField>,
}

/// The engine's state (see the module docs).
#[derive(Default)]
pub(super) struct Instances {
    templates: Vec<Template>,
    classes: HashMap<String, usize>,
    shell_methods: HashMap<String, Vec<usize>>,
    functions: HashMap<String, usize>,
    methods: HashMap<DefId, usize>,
    /// The DefIds of shell-method and method templates.
    template_defs: HashSet<DefId>,
    /// Generic classes with `lib` methods (`Array`, `MutexGuard`).
    shells: HashSet<String>,
    /// The instance symbols the engine emits; a call site is redirected
    /// only to one of these.
    emitted: HashSet<String>,
    found: Vec<Instance>,
    /// Real body methods of lib shells (and the mixin defaults they
    /// include), by generic-stripped mangled name (`Array_map`): a call site
    /// mangling `Array[Int]_map` with no instance reaches the shared body
    /// through `resolve_ffi_alias_callee`.
    pub(super) lib_body_methods: HashSet<String>,
    /// The opaque (generic) bodies lowered for templates; live only when a
    /// reachable function calls them (`prune_unmonomorphised_bodies`).
    pub(super) opaque_bodies: HashSet<String>,
}

impl Instances {
    fn index(&mut self) {
        self.classes.clear();
        self.shell_methods.clear();
        self.functions.clear();
        self.methods.clear();
        self.template_defs.clear();
        for (i, t) in self.templates.iter().enumerate() {
            match t {
                Template::Class(c) => {
                    self.classes.insert(c.name.clone(), i);
                }
                Template::ShellMethod { class, def, .. } => {
                    self.shell_methods.entry(class.clone()).or_default().push(i);
                    self.template_defs.insert(def.def_id);
                }
                Template::Function(def) => {
                    self.functions.insert(def.name.clone(), i);
                }
                Template::Method { def, .. } => {
                    self.methods.insert(def.def_id, i);
                    self.template_defs.insert(def.def_id);
                }
            }
        }
    }
}

impl<'a> Lowerer<'a> {
    /// Find every template and every instance the program reaches (module
    /// docs). Runs before `lower_items`: call-site redirects read the
    /// emitted set while bodies are lowered.
    pub(super) fn collect_instances(&mut self, program: &HirProgram) -> Result<(), String> {
        self.instances.shells = program
            .ffi_libs
            .iter()
            .flat_map(|lib| &lib.functions)
            .filter_map(|f| f.parent_type.clone())
            .collect();
        self.collect_lib_body_methods(&program.items);

        let mut candidates = Vec::new();
        self.collect_candidates(&program.items, &mut Vec::new(), &mut candidates);
        self.instances.templates = candidates;
        self.instances.index();
        let eligible = self.eligible_templates(program);
        let templates = std::mem::take(&mut self.instances.templates);
        self.instances.templates = templates
            .into_iter()
            .zip(eligible)
            .filter_map(|(t, keep)| keep.then_some(t))
            .collect();
        self.instances.index();
        if self.instances.templates.is_empty() {
            return Ok(());
        }

        let mut pending = Vec::new();
        struct Seeds<'s, F> {
            f: &'s mut F,
        }
        impl<F: FnMut(Site<'_>)> Visit for Seeds<'_, F> {
            fn visit_item(&mut self, item: &HirItem) {
                if let HirItem::Class(HirClassDef { fields, .. })
                | HirItem::Struct(HirStructDef { fields, .. }) = item
                {
                    for field in fields {
                        (self.f)(Site::Type(&field.ty));
                    }
                }
                if let HirItem::Const(c) = item {
                    (self.f)(Site::Type(&c.ty));
                }
                walk_item(self, item);
            }
            fn visit_func(&mut self, func: &HirFuncDef) {
                walk_func_sites(func, self.f);
            }
            // `walk_item` reaches `visit_expr` only for a const initializer.
            fn visit_expr(&mut self, expr: &HirExpr) {
                walk_expr_sites(expr, self.f);
            }
        }
        let mut seed = |site: Site<'_>| self.discover(site, &mut pending);
        let mut seeds = Seeds { f: &mut seed };
        for item in &program.items {
            seeds.visit_item(item);
        }

        let mut next = 0;
        loop {
            for (template, args) in std::mem::take(&mut pending) {
                self.record_instance(template, args)?;
            }
            let Some(inst) = self.instances.found.get(next) else {
                break;
            };
            for (_, def) in &inst.bodies {
                walk_func_sites(def, &mut |site| self.discover(site, &mut pending));
            }
            for field in &inst.fields {
                self.discover(Site::Type(&field.ty), &mut pending);
            }
            next += 1;
        }
        Ok(())
    }

    /// Every generic body that could need instances, in program order.
    fn collect_candidates(
        &self,
        items: &[HirItem],
        path: &mut Vec<String>,
        out: &mut Vec<Template>,
    ) {
        let qualify =
            |path: &[String], name: &str| Self::symbol_name(&Self::qualified_item_name(path, name));
        for item in items {
            match item {
                HirItem::Class(c) => {
                    let params = names_of(&c.generic_params);
                    let owner_ty = Ty::Class {
                        name: Self::qualified_item_name(path, &c.name),
                        generic_args: type_params(&c.generic_params),
                    };
                    method_templates(&qualify(path, &c.name), owner_ty, &params, &c.methods, out);
                    if c.generic_params.is_empty() {
                        continue;
                    }
                    if self.instances.shells.contains(&c.name) {
                        for def in class_methods(c) {
                            if own_params(def, &params).is_empty() {
                                out.push(Template::ShellMethod {
                                    class: c.name.clone(),
                                    params: params.clone(),
                                    def: def.clone(),
                                });
                            }
                        }
                    } else if class_methods(c).next().is_some() {
                        out.push(Template::Class(c.clone()));
                    }
                }
                HirItem::Struct(s) => {
                    let owner_ty = Ty::Struct {
                        name: Self::qualified_item_name(path, &s.name),
                        generic_args: type_params(&s.generic_params),
                    };
                    let params = names_of(&s.generic_params);
                    method_templates(&qualify(path, &s.name), owner_ty, &params, &s.methods, out);
                }
                HirItem::Enum(e) => {
                    let owner_ty = Ty::Enum {
                        name: Self::qualified_item_name(path, &e.name),
                        generic_args: type_params(&e.generic_params),
                    };
                    let params = names_of(&e.generic_params);
                    method_templates(&qualify(path, &e.name), owner_ty, &params, &e.methods, out);
                }
                HirItem::Function(f) if !f.generic_params.is_empty() => {
                    out.push(Template::Function(f.clone()));
                }
                HirItem::Module(m) => {
                    path.push(m.name.clone());
                    self.collect_candidates(&m.items, path, out);
                    path.pop();
                }
                HirItem::Function(_)
                | HirItem::Impl(_)
                | HirItem::Const(_)
                | HirItem::Mixin(_)
                | HirItem::TypeAlias(_)
                | HirItem::Newtype(_) => {}
            }
        }
    }

    /// The specialization predicate (module docs) over the indexed
    /// candidates: one flag per candidate.
    fn eligible_templates(&self, program: &HirProgram) -> Vec<bool> {
        let supplied_methods: HashSet<&str> = program
            .ffi_libs
            .iter()
            .flat_map(|lib| &lib.functions)
            .filter(|f| {
                f.param_types
                    .iter()
                    .any(|ty| self.symbols.supplied_arg(ty).is_some())
            })
            .map(|f| f.ruxen_name.as_str())
            .collect();
        let templates = &self.instances.templates;
        let mut direct = vec![false; templates.len()];
        let mut refs: Vec<Vec<usize>> = vec![Vec::new(); templates.len()];
        for (i, template) in templates.iter().enumerate() {
            let params = template.params();
            let mut dependent = template.bounded()
                || matches!(template, Template::Class(c)
                    if c.fields.iter().any(|f| mentions_type_param(&f.ty, &params)));
            let mut reached = Vec::new();
            for def in template.defs() {
                walk_func_sites(def, &mut |site| match site {
                    Site::Display(ty) => dependent |= is_bare_type_param(ty, &params),
                    Site::Operand(ty) => dependent |= mentions_type_param(ty, &params),
                    Site::Type(ty) => {
                        ty.any(&mut |t| {
                            if let Ty::Class { name, generic_args } = t {
                                if let Some(&j) = self.instances.classes.get(name) {
                                    if generic_args.iter().any(|a| mentions_type_param(a, &params))
                                    {
                                        reached.push(j);
                                    }
                                }
                            }
                            false
                        });
                    }
                    Site::Receiver(recv, name) => {
                        dependent |= is_bare_type_param(recv, &params);
                        let Some((class, args)) = self.shell_head(recv) else {
                            return;
                        };
                        if !args.iter().any(|a| mentions_type_param(a, &params)) {
                            return;
                        }
                        let canonical = self.symbols.canonical_method_name(&class, name);
                        dependent |=
                            supplied_methods.contains(format!("{class}_{canonical}").as_str());
                        reached.extend(self.shell_methods_named(&class, name));
                    }
                    Site::Function(callee, args) => {
                        if let Some(&j) = self.instances.functions.get(callee) {
                            if args.iter().any(|a| mentions_type_param(&a.ty, &params)) {
                                reached.push(j);
                            }
                        }
                    }
                    Site::Method(method, recv, args, block) => {
                        if let Some(&j) = self.instances.methods.get(&method) {
                            if mentions_type_param(recv, &params)
                                || args
                                    .iter()
                                    .chain(block)
                                    .any(|a| mentions_type_param(&a.ty, &params))
                            {
                                reached.push(j);
                            }
                        }
                    }
                });
            }
            direct[i] = dependent;
            refs[i] = reached;
        }
        let mut eligible = direct;
        loop {
            let mut grew = false;
            for i in 0..eligible.len() {
                if !eligible[i] && refs[i].iter().any(|&j| eligible[j]) {
                    eligible[i] = true;
                    grew = true;
                }
            }
            if !grew {
                return eligible;
            }
        }
    }

    /// The shell-method templates of `class` a call to `name` reaches: the
    /// alias-canonical body and its overloads.
    fn shell_methods_named(&self, class: &str, name: &str) -> Vec<usize> {
        let canonical = self.symbols.canonical_method_name(class, name);
        let overload_prefix = format!("{canonical}__overload");
        self.instances
            .shell_methods
            .get(class)
            .into_iter()
            .flatten()
            .copied()
            .filter(|&j| {
                let name = self.instances.templates[j].name();
                name == canonical || name.starts_with(&overload_prefix)
            })
            .collect()
    }

    /// Record in `out` every instance a concrete `site` reaches.
    fn discover(&self, site: Site<'_>, out: &mut Vec<(usize, Vec<Ty>)>) {
        match site {
            Site::Type(ty) => {
                ty.any(&mut |t| {
                    if let Ty::Class { name, generic_args } = t {
                        if let Some(&j) = self.instances.classes.get(name) {
                            if !generic_args.is_empty()
                                && generic_args.iter().all(is_fully_concrete)
                            {
                                out.push((j, generic_args.clone()));
                            }
                        }
                    }
                    false
                });
            }
            Site::Receiver(recv, name) => {
                let Some((class, args)) = self.shell_head(recv) else {
                    return;
                };
                if args.iter().all(is_fully_concrete) {
                    for j in self.shell_methods_named(&class, name) {
                        out.push((j, args.clone()));
                    }
                }
            }
            Site::Method(method, recv, args, block) => {
                if let Some(&j) = self.instances.methods.get(&method) {
                    let arg_tys: Vec<Ty> = args.iter().chain(block).map(|a| a.ty.clone()).collect();
                    out.extend(self.method_instance_args(j, recv, &arg_tys).map(|a| (j, a)));
                }
            }
            Site::Function(callee, args) => {
                if let Some(&j) = self.instances.functions.get(callee) {
                    let arg_tys: Vec<Ty> = args.iter().map(|a| a.ty.clone()).collect();
                    out.extend(self.fn_instance_args(j, &arg_tys).map(|a| (j, a)));
                }
            }
            Site::Operand(_) | Site::Display(_) => {}
        }
    }

    /// The instance arguments of a call to method template `j` on `recv`
    /// with `arg_tys` (its trailing block last), when all are concrete.
    fn method_instance_args(&self, j: usize, recv: &Ty, arg_tys: &[Ty]) -> Option<Vec<Ty>> {
        let Template::Method {
            owner_params, def, ..
        } = &self.instances.templates[j]
        else {
            return None;
        };
        let mut args = Vec::new();
        if !owner_params.is_empty() {
            let recv_args = self.receiver_args(recv)?;
            if recv_args.len() != owner_params.len() {
                return None;
            }
            args.extend(recv_args);
        }
        args.extend(bind_from_args(
            def,
            &own_params(def, owner_params),
            arg_tys,
        )?);
        args.iter().all(is_fully_concrete).then_some(args)
    }

    /// The instance arguments of a call to function template `j`.
    fn fn_instance_args(&self, j: usize, arg_tys: &[Ty]) -> Option<Vec<Ty>> {
        let Template::Function(def) = &self.instances.templates[j] else {
            return None;
        };
        bind_from_args(def, &names_of(&def.generic_params), arg_tys)
    }

    /// The generic args of a receiver: a builtin head's or a nominal type's.
    fn receiver_args(&self, recv: &Ty) -> Option<Vec<Ty>> {
        if let Some((_, args)) = recv.builtin_head() {
            return Some(args);
        }
        match recv.peel_refs() {
            Ty::Class { generic_args, .. }
            | Ty::Struct { generic_args, .. }
            | Ty::Enum { generic_args, .. } => Some(generic_args.clone()),
            _ => None,
        }
    }

    /// The lib shell class a receiver's methods live on, with its generic
    /// args: a builtin head (`Ty::Array(Int)` is `Array[Int]`) or an
    /// instance of a generic class with `lib` methods (`MutexGuard[String]`).
    fn shell_head(&self, ty: &Ty) -> Option<(String, Vec<Ty>)> {
        if let Some((class, args)) = ty.builtin_head() {
            return Some((class.to_string(), args));
        }
        match ty.peel_refs() {
            Ty::Class { name, generic_args }
                if !generic_args.is_empty() && self.instances.shells.contains(name) =>
            {
                Some((name.clone(), generic_args.clone()))
            }
            _ => None,
        }
    }

    /// The symbol of template `j` instantiated with `args`.
    fn instance_symbol(&self, j: usize, args: &[Ty]) -> String {
        match &self.instances.templates[j] {
            Template::Class(c) => mono_base(&c.name, args),
            Template::ShellMethod { class, def, .. } => {
                format!("{}_{}", mono_base(class, args), def.name)
            }
            Template::Function(def) => mono_base(&def.name, args),
            Template::Method {
                owner,
                owner_params,
                def,
                ..
            } => {
                let (owner_args, own) = args.split_at(owner_params.len());
                let owner_base = if owner_args.is_empty() {
                    owner.clone()
                } else {
                    mono_base(owner, owner_args)
                };
                mono_base(&format!("{owner_base}_{}", def.name), own)
            }
        }
    }

    /// Record the instance of template `j` with `args` unless it exists,
    /// substituting its bodies once.
    fn record_instance(&mut self, j: usize, args: Vec<Ty>) -> Result<(), String> {
        let template = &self.instances.templates[j];
        let params = template.params();
        if args.len() != params.len() {
            return Ok(());
        }
        if args.iter().map(type_depth).max().unwrap_or(0) > MAX_INSTANCE_DEPTH {
            let mut spelled = format!(
                "{}[{}]",
                template.name(),
                args.iter()
                    .map(|a| a.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            if spelled.len() > 80 {
                let cut = (0..=80)
                    .rev()
                    .find(|&i| spelled.is_char_boundary(i))
                    .unwrap_or(0);
                spelled.truncate(cut);
                spelled.push_str("...");
            }
            return Err(format!(
                "error[E0746]: the generic instantiation `{spelled}` nests deeper than \
                 {MAX_INSTANCE_DEPTH} levels: a generic body instantiates itself with an ever \
                 larger type"
            ));
        }
        let symbol = self.instance_symbol(j, &args);
        if !self.instances.emitted.insert(symbol.clone()) {
            return Ok(());
        }
        let subst: HashMap<String, Ty> = params.into_iter().zip(args.iter().cloned()).collect();
        let substituted = |def: &HirFuncDef, subst: &HashMap<String, Ty>| {
            let mut def = def.clone();
            trait_default::subst_type_params_in_func(&mut def, subst);
            def
        };
        let head = |class: &str, args: &[Ty]| {
            Ty::from_builtin_head(class, args).unwrap_or_else(|| Ty::Class {
                name: class.to_string(),
                generic_args: args.to_vec(),
            })
        };
        let template = self.instances.templates[j].clone();
        let instance = match &template {
            Template::Class(c) => {
                let mut bodies = Vec::new();
                for def in class_methods(c) {
                    if !self.instances.methods.contains_key(&def.def_id) {
                        bodies.push((format!("{symbol}_{}", def.name), substituted(def, &subst)));
                    }
                }
                let own: Vec<HirFuncDef> = class_methods(c).cloned().collect();
                for (default_fn, mixin_subst) in self.included_mixin_defaults(c, &own) {
                    // The mixin's own params are spelled in the class's
                    // params (`include Enumerable[T]`), and `Self` is the
                    // class's target type: both resolve through this
                    // instantiation's substitution.
                    let default_subst: HashMap<String, Ty> = mixin_subst
                        .iter()
                        .map(|(k, v)| (k.clone(), v.subst_params(&subst)))
                        .collect();
                    bodies.push((
                        format!("{symbol}_{}", default_fn.name),
                        substituted(&default_fn, &default_subst),
                    ));
                }
                let fields = self
                    .derive_fields_of_class(c)
                    .into_iter()
                    .map(|f| DeriveField {
                        ty: f.ty.subst_params(&subst),
                        ..f
                    })
                    .collect();
                Instance {
                    template: j,
                    symbol: symbol.clone(),
                    self_ty: Some(Ty::Class {
                        name: c.name.clone(),
                        generic_args: args.clone(),
                    }),
                    bodies,
                    fields,
                }
            }
            Template::ShellMethod { class, def, .. } => Instance {
                template: j,
                symbol: symbol.clone(),
                self_ty: Some(head(class, &args)),
                bodies: vec![(symbol.clone(), substituted(def, &subst))],
                fields: Vec::new(),
            },
            Template::Function(def) => {
                let mut def = substituted(def, &subst);
                def.name = symbol.clone();
                Instance {
                    template: j,
                    symbol: symbol.clone(),
                    self_ty: None,
                    bodies: vec![(symbol.clone(), def)],
                    fields: Vec::new(),
                }
            }
            Template::Method { owner_ty, def, .. } => {
                let self_ty = match owner_ty.subst_params(&subst) {
                    Ty::Class { name, generic_args } => head(&name, &generic_args),
                    other => other,
                };
                Instance {
                    template: j,
                    symbol: symbol.clone(),
                    self_ty: Some(self_ty),
                    bodies: vec![(symbol.clone(), substituted(def, &subst))],
                    fields: Vec::new(),
                }
            }
        };
        self.instances.found.push(instance);
        Ok(())
    }

    /// Lower every recorded instance: its bodies with `self` pinned to the
    /// instance's type, and a class instance's derived bodies. Runs after
    /// `lower_items`, so every opaque body is already in place.
    pub(super) fn emit_instances(&mut self, mir: &mut MirProgram) -> Result<(), String> {
        let found = std::mem::take(&mut self.instances.found);
        for inst in &found {
            let template = &self.instances.templates[inst.template];
            let class = match template {
                Template::Class(c) => Some(c.clone()),
                _ => None,
            };
            let is_function = matches!(template, Template::Function(_));
            for (symbol, def) in &inst.bodies {
                let lowered = if is_function {
                    self.lower_function(def)
                } else {
                    self.self_ty_override = inst.self_ty.clone();
                    let lowered = self.lower_method(symbol, def);
                    self.self_ty_override = None;
                    lowered
                };
                mir.functions.push(lowered?);
            }
            // A call on a class instance mangles to `<base>_clone` (and the
            // other derived methods) too, so the derived bodies are emitted
            // per instance, over the substituted field types.
            if let (Some(class), Some(instance_ty)) = (class, &inst.self_ty) {
                mir.functions.extend(self.synthesize_derives_as(
                    instance_ty,
                    &class.name,
                    &inst.symbol,
                    &inst.fields,
                ));
            }
        }
        Ok(())
    }

    /// The default methods `class` gets from each `include`d mixin and does
    /// not define itself (`own`), each with the map from the mixin's generic
    /// params, and `Self`, to the class-level types the `include` names.
    fn included_mixin_defaults(
        &self,
        class: &HirClassDef,
        own: &[HirFuncDef],
    ) -> Vec<(HirFuncDef, HashMap<String, Ty>)> {
        let mut out = Vec::new();
        for block in &class.impl_blocks {
            let Some(trait_ref) = &block.trait_ref else {
                continue;
            };
            if block.negative_trait {
                continue;
            }
            let Some(defaults) = self.trait_default_methods.get(&trait_ref.name) else {
                continue;
            };
            let mixin_params: Vec<String> = self
                .symbols
                .named(&trait_ref.name)
                .find_map(|def| match &def.kind {
                    crate::resolve::symbols::DefKind::Trait { info } => {
                        Some(info.generic_params.iter().map(|g| g.name.clone()).collect())
                    }
                    _ => None,
                })
                .unwrap_or_default();
            let mut subst: HashMap<String, Ty> = mixin_params
                .into_iter()
                .zip(trait_ref.generic_args.iter().cloned())
                .collect();
            subst.insert("Self".to_string(), block.target_ty.clone());
            let mut names: Vec<&String> = defaults.keys().collect();
            names.sort();
            for name in names {
                if own.iter().any(|m| &m.name == name) {
                    continue;
                }
                out.push((defaults[name].clone(), subst.clone()));
            }
        }
        out
    }

    /// Record the body methods of every lib shell (and the mixin defaults
    /// it includes and does not override) in `lib_body_methods`.
    fn collect_lib_body_methods(&mut self, items: &[HirItem]) {
        for item in items {
            match item {
                HirItem::Class(c)
                    if !c.generic_params.is_empty() && self.instances.shells.contains(&c.name) =>
                {
                    let class_cs = c.name.replace('.', "_");
                    let mut names: Vec<String> = c
                        .methods
                        .iter()
                        .map(|m| format!("{class_cs}_{}", m.name))
                        .collect();
                    let own: HashSet<&str> = c.methods.iter().map(|m| m.name.as_str()).collect();
                    for block in &c.impl_blocks {
                        let Some(trait_ref) = &block.trait_ref else {
                            continue;
                        };
                        if block.negative_trait {
                            continue;
                        }
                        let Some(defaults) = self.trait_default_methods.get(&trait_ref.name) else {
                            continue;
                        };
                        names.extend(
                            defaults
                                .keys()
                                .filter(|m| !own.contains(m.as_str()))
                                .map(|m| format!("{class_cs}_{m}")),
                        );
                    }
                    self.instances.lib_body_methods.extend(names);
                }
                HirItem::Module(m) => self.collect_lib_body_methods(&m.items),
                HirItem::Class(_)
                | HirItem::Function(_)
                | HirItem::Struct(_)
                | HirItem::Enum(_)
                | HirItem::Impl(_)
                | HirItem::Const(_)
                | HirItem::Mixin(_)
                | HirItem::TypeAlias(_)
                | HirItem::Newtype(_) => {}
            }
        }
    }
}

/// A method template candidate for each of `defs` with generic parameters
/// of its own.
fn method_templates(
    owner: &str,
    owner_ty: Ty,
    owner_params: &[String],
    defs: &[HirFuncDef],
    out: &mut Vec<Template>,
) {
    for def in defs {
        if !own_params(def, owner_params).is_empty() {
            out.push(Template::Method {
                owner: owner.to_string(),
                owner_ty: owner_ty.clone(),
                owner_params: owner_params.to_vec(),
                def: def.clone(),
            });
        }
    }
}

/// `T` as a type for each of `params` (an owner type spelled generically).
fn type_params(params: &[HirGenericParam]) -> Vec<Ty> {
    params
        .iter()
        .map(|g| Ty::TypeParam {
            name: g.name.clone(),
            bounds: vec![],
        })
        .collect()
}

/// Bind `params` of `def` from a call's argument types, when every one
/// resolves to a concrete type.
fn bind_from_args(def: &HirFuncDef, params: &[String], arg_tys: &[Ty]) -> Option<Vec<Ty>> {
    let mut subst = HashMap::new();
    for (param, actual) in def.params.iter().zip(arg_tys) {
        unify_collect_type_params(&param.ty, actual, &mut subst);
    }
    params
        .iter()
        .map(|name| subst.get(name).filter(|t| is_fully_concrete(t)).cloned())
        .collect()
}
