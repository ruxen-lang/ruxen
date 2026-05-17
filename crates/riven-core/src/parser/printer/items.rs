//! Pretty-printing for top-level items, modules, classes, structs,
//! enums, mixins, impls, functions, `use` declarations, type aliases,
//! newtypes, consts, and field declarations.

use super::super::ast::*;
use super::format::*;
use super::PrettyPrinter;

impl PrettyPrinter {
    // ── top-level items ─────────────────────────────────────────────

    pub(super) fn print_top_level_item(&mut self, item: &TopLevelItem) {
        match item {
            TopLevelItem::Module(m) => self.print_module(m),
            TopLevelItem::Class(c) => self.print_class(c),
            TopLevelItem::Struct(s) => self.print_struct(s),
            TopLevelItem::Enum(e) => self.print_enum(e),
            TopLevelItem::Mixin(t) => self.print_trait(t),
            TopLevelItem::Impl(i) => self.print_impl(i),
            TopLevelItem::Function(f) => self.print_func(f),
            TopLevelItem::Use(u) => self.print_use(u),
            TopLevelItem::TypeAlias(ta) => self.print_type_alias(ta),
            TopLevelItem::Newtype(nt) => self.print_newtype(nt),
            TopLevelItem::Const(c) => self.print_const(c),
            TopLevelItem::Lib(l) => {
                self.line(&format!("lib {} ({} functions)", l.name, l.functions.len()));
            }
            TopLevelItem::Extern(e) => {
                self.line(&format!(
                    "extern \"{}\" ({} functions)",
                    e.abi,
                    e.functions.len()
                ));
            }
        }
    }

    // ── module ──────────────────────────────────────────────────────

    fn print_module(&mut self, m: &ModuleDef) {
        self.line(&format!("Module {}", m.name));
        self.indent();
        for item in &m.items {
            self.print_top_level_item(item);
        }
        self.dedent();
    }

    // ── class ───────────────────────────────────────────────────────

    fn print_class(&mut self, c: &ClassDef) {
        let generics = format_opt_generic_params(&c.generic_params);
        let parent = c
            .parent
            .as_ref()
            .map(|p| format!(" < {}", format_type_path(p)))
            .unwrap_or_default();
        self.line(&format!("Class {}{}{}", c.name, generics, parent));
        self.indent();
        for f in &c.fields {
            self.print_field_decl(f);
        }
        for m in &c.methods {
            self.print_func(m);
        }
        for imp in &c.inner_impls {
            self.print_inner_impl(imp);
        }
        self.dedent();
    }

    // ── struct ──────────────────────────────────────────────────────

    fn print_struct(&mut self, s: &StructDef) {
        let generics = format_opt_generic_params(&s.generic_params);
        let derives = if s.derive_traits.is_empty() {
            String::new()
        } else {
            format!(" include({})", s.derive_traits.join(", "))
        };
        self.line(&format!("Struct {}{}{}", s.name, generics, derives));
        self.indent();
        for f in &s.fields {
            self.print_field_decl(f);
        }
        for m in &s.methods {
            self.print_func(m);
        }
        for imp in &s.inner_impls {
            self.print_inner_impl(imp);
        }
        self.dedent();
    }

    // ── enum ────────────────────────────────────────────────────────

    fn print_enum(&mut self, e: &EnumDef) {
        let generics = format_opt_generic_params(&e.generic_params);
        self.line(&format!("Enum {}{}", e.name, generics));
        self.indent();
        for v in &e.variants {
            self.print_variant(v);
        }
        for m in &e.methods {
            self.print_func(m);
        }
        for imp in &e.inner_impls {
            self.print_inner_impl(imp);
        }
        self.dedent();
    }

    fn print_variant(&mut self, v: &Variant) {
        match &v.fields {
            VariantKind::Unit => {
                self.line(&format!("Variant {}", v.name));
            }
            VariantKind::Tuple(fields) => {
                let types: Vec<String> = fields.iter().map(|f| format_type(&f.type_expr)).collect();
                self.line(&format!("Variant {}({})", v.name, types.join(", ")));
            }
            VariantKind::Struct(fields) => {
                self.line(&format!("Variant {} {{", v.name));
                self.indent();
                for f in fields {
                    let name = f.name.as_deref().unwrap_or("_");
                    self.line(&format!("{}: {}", name, format_type(&f.type_expr)));
                }
                self.dedent();
                self.line("}");
            }
        }
    }

    // ── trait ────────────────────────────────────────────────────────

    fn print_trait(&mut self, t: &MixinDef) {
        let generics = format_opt_generic_params(&t.generic_params);
        let supers = if t.super_traits.is_empty() {
            String::new()
        } else {
            let names: Vec<String> = t
                .super_traits
                .iter()
                .map(|b| format_type_path(&b.path))
                .collect();
            format!(": {}", names.join(" + "))
        };
        self.line(&format!("Mixin {}{}{}", t.name, generics, supers));
        self.indent();
        for item in &t.items {
            self.print_trait_item(item);
        }
        self.dedent();
    }

    fn print_trait_item(&mut self, item: &MixinItem) {
        match item {
            MixinItem::AssocType { name, .. } => {
                self.line(&format!("type {}", name));
            }
            MixinItem::MethodSig(sig) => {
                self.line(&format!("sig {}", format_method_sig(sig)));
            }
            MixinItem::DefaultMethod(f) => {
                self.print_func(f);
            }
        }
    }

    // ── impl ────────────────────────────────────────────────────────

    fn print_impl(&mut self, imp: &ImplBlock) {
        let generics = format_opt_generic_params(&imp.generic_params);
        let header = match &imp.trait_name {
            Some(tr) => format!(
                "Include{} {} in {}",
                generics,
                format_type_path(tr),
                format_type(&imp.target_type)
            ),
            None => format!("Include{} {}", generics, format_type(&imp.target_type)),
        };
        self.line(&header);
        self.indent();
        for item in &imp.items {
            self.print_impl_item(item);
        }
        self.dedent();
    }

    fn print_impl_item(&mut self, item: &ImplItem) {
        match item {
            ImplItem::AssocType {
                name, type_expr, ..
            } => {
                self.line(&format!("type {} = {}", name, format_type(type_expr)));
            }
            ImplItem::Method(f) => {
                self.print_func(f);
            }
            ImplItem::Include {
                is_unsafe,
                negative_trait,
                trait_name,
                ..
            } => {
                let prefix = if *is_unsafe {
                    "unsafe include "
                } else {
                    "include "
                };
                let bang = if *negative_trait { "!" } else { "" };
                self.line(&format!(
                    "{}{}{}",
                    prefix,
                    bang,
                    format_type_path(trait_name)
                ));
            }
        }
    }

    fn print_inner_impl(&mut self, imp: &InnerImpl) {
        // Debug dump uses the new vocabulary — the AST node is named
        // `InnerImpl` for historical reasons (§8) but the surface form is
        // an `include` directive carrying scattered methods.
        self.line(&format!("include {}", format_type_path(&imp.trait_name)));
        self.indent();
        for item in &imp.items {
            self.print_impl_item(item);
        }
        self.dedent();
    }

    // ── function ────────────────────────────────────────────────────

    pub(super) fn print_func(&mut self, f: &FuncDef) {
        let vis = format_visibility(f.visibility);
        let async_kw = if f.is_async { "async " } else { "" };
        let generics = format_opt_generic_params(&f.generic_params);
        let class_marker = if f.is_class_method { "self." } else { "" };
        let self_mode = f
            .self_mode
            .as_ref()
            .map(|m| format!("{}, ", format_self_mode(*m)))
            .unwrap_or_default();
        let params: Vec<String> = f
            .params
            .iter()
            .map(|p| {
                let auto = if p.auto_assign { "@" } else { "" };
                format!("{}{}: {}", auto, p.name, format_type(&p.type_expr))
            })
            .collect();
        let ret = f
            .return_type
            .as_ref()
            .map(|t| format!(" -> {}", format_type(t)))
            .unwrap_or_default();
        let where_cl = f
            .where_clause
            .as_ref()
            .map(|w| format!(" {}", format_where_clause(w)))
            .unwrap_or_default();
        self.line(&format!(
            "{}{}fn {}{}{}({}{}){}{}",
            vis,
            async_kw,
            class_marker,
            f.name,
            generics,
            self_mode,
            params.join(", "),
            ret,
            where_cl
        ));
        self.indent();
        self.print_block(&f.body);
        self.dedent();
    }

    // ── use ─────────────────────────────────────────────────────────

    fn print_use(&mut self, u: &UseDecl) {
        let path = u.path.join(".");
        match &u.kind {
            UseKind::Simple => self.line(&format!("Use {}", path)),
            UseKind::Alias(alias) => self.line(&format!("Use {} as {}", path, alias)),
            UseKind::Group(names) => self.line(&format!("Use {}.{{{}}}", path, names.join(", "))),
        }
    }

    // ── type alias & newtype ────────────────────────────────────────

    fn print_type_alias(&mut self, ta: &TypeAliasDef) {
        let generics = format_opt_generic_params(&ta.generic_params);
        self.line(&format!(
            "TypeAlias {}{} = {}",
            ta.name,
            generics,
            format_type(&ta.type_expr)
        ));
    }

    fn print_newtype(&mut self, nt: &NewtypeDef) {
        self.line(&format!(
            "Newtype {} = {}",
            nt.name,
            format_type(&nt.inner_type)
        ));
    }

    // ── const ───────────────────────────────────────────────────────

    fn print_const(&mut self, c: &ConstDef) {
        self.line(&format!(
            "Const {}: {} = {}",
            c.name,
            format_type(&c.type_expr),
            format_expr_short(&c.value)
        ));
    }

    // ── field declaration ───────────────────────────────────────────

    fn print_field_decl(&mut self, f: &FieldDecl) {
        let vis = format_visibility(f.visibility);
        self.line(&format!(
            "{}field {}: {}",
            vis,
            f.name,
            format_type(&f.type_expr)
        ));
    }
}
