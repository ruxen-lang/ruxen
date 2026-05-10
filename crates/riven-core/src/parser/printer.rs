//! Debug pretty-printer for the Riven AST.
//!
//! Dumps any AST node into readable, indented text output. Expressions are
//! shown in abbreviated form to keep output manageable.

use super::ast::*;
use crate::lexer::token::StringPart;

// ─── PrettyPrinter ──────────────────────────────────────────────────

pub struct PrettyPrinter {
    indent: usize,
    output: String,
}

impl Default for PrettyPrinter {
    fn default() -> Self {
        Self::new()
    }
}

impl PrettyPrinter {
    pub fn new() -> Self {
        Self {
            indent: 0,
            output: String::new(),
        }
    }

    pub fn print_program(mut self, program: &Program) -> String {
        self.line("Program");
        self.indent();
        for item in &program.items {
            self.print_top_level_item(item);
        }
        self.dedent();
        self.output
    }

    // ── helpers ──────────────────────────────────────────────────────

    fn line(&mut self, text: &str) {
        for _ in 0..self.indent {
            self.output.push_str("  ");
        }
        self.output.push_str(text);
        self.output.push('\n');
    }

    fn indent(&mut self) {
        self.indent += 1;
    }

    fn dedent(&mut self) {
        self.indent = self.indent.saturating_sub(1);
    }

    // ── top-level items ─────────────────────────────────────────────

    fn print_top_level_item(&mut self, item: &TopLevelItem) {
        match item {
            TopLevelItem::Module(m) => self.print_module(m),
            TopLevelItem::Class(c) => self.print_class(c),
            TopLevelItem::Struct(s) => self.print_struct(s),
            TopLevelItem::Enum(e) => self.print_enum(e),
            TopLevelItem::Trait(t) => self.print_trait(t),
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
            format!(" derive({})", s.derive_traits.join(", "))
        };
        self.line(&format!("Struct {}{}{}", s.name, generics, derives));
        self.indent();
        for f in &s.fields {
            self.print_field_decl(f);
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

    fn print_trait(&mut self, t: &TraitDef) {
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
        self.line(&format!("Trait {}{}{}", t.name, generics, supers));
        self.indent();
        for item in &t.items {
            self.print_trait_item(item);
        }
        self.dedent();
    }

    fn print_trait_item(&mut self, item: &TraitItem) {
        match item {
            TraitItem::AssocType { name, .. } => {
                self.line(&format!("type {}", name));
            }
            TraitItem::MethodSig(sig) => {
                self.line(&format!("sig {}", format_method_sig(sig)));
            }
            TraitItem::DefaultMethod(f) => {
                self.print_func(f);
            }
        }
    }

    // ── impl ────────────────────────────────────────────────────────

    fn print_impl(&mut self, imp: &ImplBlock) {
        let generics = format_opt_generic_params(&imp.generic_params);
        let header = match &imp.trait_name {
            Some(tr) => format!(
                "Impl{} {} for {}",
                generics,
                format_type_path(tr),
                format_type(&imp.target_type)
            ),
            None => format!("Impl{} {}", generics, format_type(&imp.target_type)),
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
        }
    }

    fn print_inner_impl(&mut self, imp: &InnerImpl) {
        self.line(&format!("impl {}", format_type_path(&imp.trait_name)));
        self.indent();
        for item in &imp.items {
            self.print_impl_item(item);
        }
        self.dedent();
    }

    // ── function ────────────────────────────────────────────────────

    fn print_func(&mut self, f: &FuncDef) {
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
        let path = u.path.join("::");
        match &u.kind {
            UseKind::Simple => self.line(&format!("Use {}", path)),
            UseKind::Alias(alias) => self.line(&format!("Use {} as {}", path, alias)),
            UseKind::Group(names) => self.line(&format!("Use {}::{{{}}}", path, names.join(", "))),
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

    // ── block & statements ──────────────────────────────────────────

    fn print_block(&mut self, block: &Block) {
        for stmt in &block.statements {
            self.print_statement(stmt);
        }
    }

    fn print_statement(&mut self, stmt: &Statement) {
        match stmt {
            Statement::Let(binding) => {
                let mutability = if binding.mutable { "mut " } else { "" };
                let pat = format_pattern(&binding.pattern);
                let ty = binding
                    .type_annotation
                    .as_ref()
                    .map(|t| format!(": {}", format_type(t)))
                    .unwrap_or_default();
                let val = binding
                    .value
                    .as_ref()
                    .map(|v| format!(" = {}", format_expr_short(v)))
                    .unwrap_or_default();
                self.line(&format!("let {}{}{}{}", mutability, pat, ty, val));
            }
            Statement::Expression(expr) => {
                self.print_expr(expr);
            }
        }
    }

    // ── expression (tree form for control flow, short for leaves) ──

    fn print_expr(&mut self, expr: &Expr) {
        match &expr.kind {
            ExprKind::If(if_expr) => {
                self.line(&format!("if {}", format_expr_short(&if_expr.condition)));
                self.indent();
                self.print_block(&if_expr.then_body);
                self.dedent();
                for elsif in &if_expr.elsif_clauses {
                    self.line(&format!("elsif {}", format_expr_short(&elsif.condition)));
                    self.indent();
                    self.print_block(&elsif.body);
                    self.dedent();
                }
                if let Some(else_body) = &if_expr.else_body {
                    self.line("else");
                    self.indent();
                    self.print_block(else_body);
                    self.dedent();
                }
            }
            ExprKind::IfLet(if_let) => {
                self.line(&format!(
                    "if let {} = {}",
                    format_pattern(&if_let.pattern),
                    format_expr_short(&if_let.value)
                ));
                self.indent();
                self.print_block(&if_let.then_body);
                self.dedent();
                if let Some(else_body) = &if_let.else_body {
                    self.line("else");
                    self.indent();
                    self.print_block(else_body);
                    self.dedent();
                }
            }
            ExprKind::Match(match_expr) => {
                self.line(&format!("match {}", format_expr_short(&match_expr.subject)));
                self.indent();
                for arm in &match_expr.arms {
                    let guard = arm
                        .guard
                        .as_ref()
                        .map(|g| format!(" if {}", format_expr_short(g)))
                        .unwrap_or_default();
                    self.line(&format!("{}{} =>", format_pattern(&arm.pattern), guard));
                    self.indent();
                    match &arm.body {
                        MatchArmBody::Expr(e) => {
                            self.line(&format_expr_short(e));
                        }
                        MatchArmBody::Block(b) => {
                            self.print_block(b);
                        }
                    }
                    self.dedent();
                }
                self.dedent();
            }
            ExprKind::While(w) => {
                self.line(&format!("while {}", format_expr_short(&w.condition)));
                self.indent();
                self.print_block(&w.body);
                self.dedent();
            }
            ExprKind::WhileLet(wl) => {
                self.line(&format!(
                    "while let {} = {}",
                    format_pattern(&wl.pattern),
                    format_expr_short(&wl.value)
                ));
                self.indent();
                self.print_block(&wl.body);
                self.dedent();
            }
            ExprKind::For(f) => {
                self.line(&format!(
                    "for {} in {}",
                    format_pattern(&f.pattern),
                    format_expr_short(&f.iterable)
                ));
                self.indent();
                self.print_block(&f.body);
                self.dedent();
            }
            ExprKind::Loop(l) => {
                self.line("loop");
                self.indent();
                self.print_block(&l.body);
                self.dedent();
            }
            ExprKind::Block(block) => {
                self.line("block");
                self.indent();
                self.print_block(block);
                self.dedent();
            }
            ExprKind::Closure(closure) => {
                let async_kw = if closure.is_async { "async " } else { "" };
                let mv = if closure.is_move { "move " } else { "" };
                let params: Vec<String> = closure
                    .params
                    .iter()
                    .map(|p| {
                        p.type_expr
                            .as_ref()
                            .map(|t| format!("{}: {}", p.name, format_type(t)))
                            .unwrap_or_else(|| p.name.clone())
                    })
                    .collect();
                self.line(&format!("{}{}|{}|", async_kw, mv, params.join(", ")));
                self.indent();
                match &closure.body {
                    ClosureBody::Expr(e) => {
                        self.line(&format_expr_short(e));
                    }
                    ClosureBody::Block(b) => {
                        self.print_block(b);
                    }
                }
                self.dedent();
            }
            // All other expressions: show abbreviated form on one line
            _ => {
                self.line(&format_expr_short(expr));
            }
        }
    }
}

// ─── Free-standing formatting helpers ───────────────────────────────

fn format_visibility(vis: Visibility) -> &'static str {
    match vis {
        Visibility::Private => "",
        Visibility::Public => "pub ",
        Visibility::Protected => "protected ",
    }
}

fn format_self_mode(m: SelfMode) -> &'static str {
    match m {
        SelfMode::Immutable => "self",
        SelfMode::Mutable => "mut self",
        SelfMode::Consuming => "own self",
    }
}

fn format_opt_generic_params(gp: &Option<GenericParams>) -> String {
    match gp {
        None => String::new(),
        Some(gp) => {
            let params: Vec<String> = gp
                .params
                .iter()
                .map(|p| match p {
                    GenericParam::Lifetime { name, .. } => format!("'{}", name),
                    GenericParam::Type { name, bounds, .. } => {
                        if bounds.is_empty() {
                            name.clone()
                        } else {
                            let bs: Vec<String> =
                                bounds.iter().map(|b| format_type_path(&b.path)).collect();
                            format!("{}: {}", name, bs.join(" + "))
                        }
                    }
                })
                .collect();
            format!("[{}]", params.join(", "))
        }
    }
}

fn format_where_clause(w: &WhereClause) -> String {
    let preds: Vec<String> = w
        .predicates
        .iter()
        .map(|p| {
            let bounds: Vec<String> = p.bounds.iter().map(|b| format_type_path(&b.path)).collect();
            format!("{}: {}", format_type(&p.type_expr), bounds.join(" + "))
        })
        .collect();
    format!("where {}", preds.join(", "))
}

fn format_method_sig(sig: &MethodSig) -> String {
    let generics = format_opt_generic_params(&sig.generic_params);
    let class_marker = if sig.is_class_method { "self." } else { "" };
    let self_mode = sig
        .self_mode
        .as_ref()
        .map(|m| format!("{}, ", format_self_mode(*m)))
        .unwrap_or_default();
    let params: Vec<String> = sig
        .params
        .iter()
        .map(|p| format!("{}: {}", p.name, format_type(&p.type_expr)))
        .collect();
    let ret = sig
        .return_type
        .as_ref()
        .map(|t| format!(" -> {}", format_type(t)))
        .unwrap_or_default();
    format!(
        "{}{}{}({}{}){}",
        class_marker,
        sig.name,
        generics,
        self_mode,
        params.join(", "),
        ret
    )
}

/// Format a type expression into a compact string.
pub fn format_type(t: &TypeExpr) -> String {
    match t {
        TypeExpr::Named(path) => format_type_path(path),
        TypeExpr::Reference {
            lifetime,
            mutable,
            inner,
            ..
        } => {
            let lt = lifetime
                .as_ref()
                .map(|l| format!("'{} ", l))
                .unwrap_or_default();
            let m = if *mutable { "mut " } else { "" };
            format!("&{}{}{}", lt, m, format_type(inner))
        }
        TypeExpr::Tuple { elements, .. } => {
            let elems: Vec<String> = elements.iter().map(format_type).collect();
            format!("({})", elems.join(", "))
        }
        TypeExpr::Array { element, size, .. } => match size {
            Some(sz) => format!("[{}; {}]", format_type(element), format_expr_short(sz)),
            None => format!("[{}]", format_type(element)),
        },
        TypeExpr::Function {
            params,
            return_type,
            ..
        } => {
            let ps: Vec<String> = params.iter().map(format_type).collect();
            format!("Fn({}) -> {}", ps.join(", "), format_type(return_type))
        }
        TypeExpr::ImplTrait { bounds, .. } => {
            let bs: Vec<String> = bounds.iter().map(|b| format_type_path(&b.path)).collect();
            format!("impl {}", bs.join(" + "))
        }
        TypeExpr::DynTrait { bounds, .. } => {
            let bs: Vec<String> = bounds.iter().map(|b| format_type_path(&b.path)).collect();
            format!("dyn {}", bs.join(" + "))
        }
        TypeExpr::Never { .. } => "!".to_string(),
        TypeExpr::Inferred { .. } => "_".to_string(),
        TypeExpr::RawPointer { mutable, inner, .. } => {
            if *mutable {
                format!("*mut {}", format_type(inner))
            } else {
                format!("*{}", format_type(inner))
            }
        }
    }
}

/// Format a type path like `std::collections::HashMap[K, V]`.
pub fn format_type_path(p: &TypePath) -> String {
    let base = p.segments.join("::");
    match &p.generic_args {
        None => base,
        Some(args) => {
            let a: Vec<String> = args.iter().map(format_type).collect();
            format!("{}[{}]", base, a.join(", "))
        }
    }
}

/// Format an expression in abbreviated (one-line) form.
pub fn format_expr_short(e: &Expr) -> String {
    match &e.kind {
        ExprKind::IntLiteral(v, suffix) => format_numeric(*v as f64, suffix),
        ExprKind::FloatLiteral(v, suffix) => format_numeric(*v, suffix),
        ExprKind::StringLiteral(s) => format!("\"{}\"", s),
        ExprKind::InterpolatedString(parts) => {
            let mut out = String::from("\"");
            for part in parts {
                match part {
                    StringPart::Literal(s) => out.push_str(s),
                    StringPart::Expr { spec, .. } => {
                        if spec.is_default() {
                            out.push_str("#{...}");
                        } else {
                            out.push_str("#{...:");
                            if let Some(c) = spec.fill {
                                out.push(c);
                            }
                            if let Some(c) = spec.align {
                                out.push(c);
                            }
                            if let Some(w) = spec.width {
                                out.push_str(&w.to_string());
                            }
                            if let Some(p) = spec.precision {
                                out.push('.');
                                out.push_str(&p.to_string());
                            }
                            if spec.debug {
                                out.push('?');
                            }
                            out.push('}');
                        }
                    }
                }
            }
            out.push('"');
            out
        }
        ExprKind::CharLiteral(c) => format!("'{}'", c),
        ExprKind::BoolLiteral(b) => b.to_string(),
        ExprKind::UnitLiteral => "()".to_string(),
        ExprKind::Identifier(name) => name.clone(),
        ExprKind::SelfRef => "self".to_string(),
        ExprKind::SelfType => "Self".to_string(),

        ExprKind::BinaryOp { left, op, right } => {
            format!(
                "({} {:?} {})",
                format_expr_short(left),
                op,
                format_expr_short(right)
            )
        }
        ExprKind::UnaryOp { op, operand } => {
            format!("({:?} {})", op, format_expr_short(operand))
        }

        ExprKind::Borrow(inner) => format!("&{}", format_expr_short(inner)),
        ExprKind::BorrowMut(inner) => format!("&mut {}", format_expr_short(inner)),

        ExprKind::FieldAccess { object, field } => {
            format!("{}.{}", format_expr_short(object), field)
        }
        ExprKind::MethodCall {
            object,
            method,
            generic_args,
            args,
            ..
        } => {
            let a: Vec<String> = args.iter().map(format_expr_short).collect();
            let generics = if generic_args.is_empty() {
                String::new()
            } else {
                let parts: Vec<String> = generic_args.iter().map(format_type).collect();
                format!("[{}]", parts.join(", "))
            };
            format!(
                "{}.{}{}({})",
                format_expr_short(object),
                method,
                generics,
                a.join(", ")
            )
        }
        ExprKind::SafeNav { object, field } => {
            format!("{}?.{}", format_expr_short(object), field)
        }
        ExprKind::SafeNavCall {
            object,
            method,
            args,
        } => {
            let a: Vec<String> = args.iter().map(format_expr_short).collect();
            format!(
                "{}?.{}({})",
                format_expr_short(object),
                method,
                a.join(", ")
            )
        }

        ExprKind::Call { callee, args, .. } => {
            let a: Vec<String> = args.iter().map(format_expr_short).collect();
            format!("{}({})", format_expr_short(callee), a.join(", "))
        }
        ExprKind::Index { object, index } => {
            format!(
                "{}[{}]",
                format_expr_short(object),
                format_expr_short(index)
            )
        }
        ExprKind::ClosureCall { callee, args } => {
            let a: Vec<String> = args.iter().map(format_expr_short).collect();
            format!("{}.call({})", format_expr_short(callee), a.join(", "))
        }

        ExprKind::Try(inner) => format!("{}?", format_expr_short(inner)),
        ExprKind::Await(inner) => format!("{}.await", format_expr_short(inner)),

        ExprKind::Assign { target, value } => {
            format!(
                "{} = {}",
                format_expr_short(target),
                format_expr_short(value)
            )
        }
        ExprKind::CompoundAssign { target, op, value } => {
            format!(
                "{} {:?}= {}",
                format_expr_short(target),
                op,
                format_expr_short(value)
            )
        }

        ExprKind::If(_) => "<if ...>".to_string(),
        ExprKind::IfLet(_) => "<if let ...>".to_string(),
        ExprKind::Match(_) => "<match ...>".to_string(),
        ExprKind::While(_) => "<while ...>".to_string(),
        ExprKind::WhileLet(_) => "<while let ...>".to_string(),
        ExprKind::For(_) => "<for ...>".to_string(),
        ExprKind::Loop(_) => "<loop ...>".to_string(),
        ExprKind::Block(_) => "<block>".to_string(),
        ExprKind::Closure(_) => "<closure>".to_string(),

        ExprKind::Range {
            start,
            end,
            inclusive,
        } => {
            let s = start
                .as_ref()
                .map(|e| format_expr_short(e))
                .unwrap_or_default();
            let e = end
                .as_ref()
                .map(|e| format_expr_short(e))
                .unwrap_or_default();
            let op = if *inclusive { "..=" } else { ".." };
            format!("{}{}{}", s, op, e)
        }

        ExprKind::ArrayLiteral(elems) => {
            if elems.len() <= 3 {
                let items: Vec<String> = elems.iter().map(format_expr_short).collect();
                format!("[{}]", items.join(", "))
            } else {
                format!("[...{} items]", elems.len())
            }
        }
        ExprKind::ArrayFill { value, count } => {
            format!(
                "[{}; {}]",
                format_expr_short(value),
                format_expr_short(count)
            )
        }
        ExprKind::TupleLiteral(elems) => {
            let items: Vec<String> = elems.iter().map(format_expr_short).collect();
            format!("({})", items.join(", "))
        }

        ExprKind::Return(val) => match val {
            Some(v) => format!("return {}", format_expr_short(v)),
            None => "return".to_string(),
        },
        ExprKind::Break(val) => match val {
            Some(v) => format!("break {}", format_expr_short(v)),
            None => "break".to_string(),
        },
        ExprKind::Continue => "continue".to_string(),

        ExprKind::Yield(exprs) => {
            let items: Vec<String> = exprs.iter().map(format_expr_short).collect();
            format!("yield {}", items.join(", "))
        }

        ExprKind::MacroCall { name, args, .. } => {
            let a: Vec<String> = args.iter().map(format_expr_short).collect();
            format!("{}!({})", name, a.join(", "))
        }

        ExprKind::Cast { expr, target_type } => {
            format!(
                "{} as {}",
                format_expr_short(expr),
                format_type(target_type)
            )
        }

        ExprKind::EnumVariant {
            type_path,
            variant,
            args,
        } => {
            let path = type_path.join("::");
            if args.is_empty() {
                format!("{}::{}", path, variant)
            } else {
                let a: Vec<String> = args
                    .iter()
                    .map(|fa| {
                        fa.name
                            .as_ref()
                            .map(|n| format!("{}: {}", n, format_expr_short(&fa.value)))
                            .unwrap_or_else(|| format_expr_short(&fa.value))
                    })
                    .collect();
                format!("{}::{}({})", path, variant, a.join(", "))
            }
        }

        ExprKind::UnsafeBlock(_) => "unsafe ... end".to_string(),
        ExprKind::NullLiteral => "null".to_string(),
    }
}

/// Format a pattern into a compact string.
pub fn format_pattern(p: &Pattern) -> String {
    match p {
        Pattern::Literal { expr, .. } => format_expr_short(expr),
        Pattern::Identifier { mutable, name, .. } => {
            if *mutable {
                format!("mut {}", name)
            } else {
                name.clone()
            }
        }
        Pattern::Wildcard { .. } => "_".to_string(),
        Pattern::Tuple { elements, .. } => {
            let elems: Vec<String> = elements.iter().map(format_pattern).collect();
            format!("({})", elems.join(", "))
        }
        Pattern::Enum {
            path,
            variant,
            fields,
            ..
        } => {
            let base = if path.is_empty() {
                variant.clone()
            } else {
                format!("{}::{}", path.join("::"), variant)
            };
            if fields.is_empty() {
                base
            } else {
                let fs: Vec<String> = fields.iter().map(format_pattern).collect();
                format!("{}({})", base, fs.join(", "))
            }
        }
        Pattern::Struct {
            path, fields, rest, ..
        } => {
            let base = path.join("::");
            let mut fs: Vec<String> = fields
                .iter()
                .map(|f| {
                    f.name
                        .as_ref()
                        .map(|n| format!("{}: {}", n, format_pattern(&f.pattern)))
                        .unwrap_or_else(|| format_pattern(&f.pattern))
                })
                .collect();
            if *rest {
                fs.push("..".to_string());
            }
            format!("{} {{ {} }}", base, fs.join(", "))
        }
        Pattern::Or { patterns, .. } => {
            let ps: Vec<String> = patterns.iter().map(format_pattern).collect();
            ps.join(" | ")
        }
        Pattern::Ref { mutable, name, .. } => {
            if *mutable {
                format!("ref mut {}", name)
            } else {
                format!("ref {}", name)
            }
        }
        Pattern::Rest { .. } => "..".to_string(),
    }
}

fn format_numeric(v: f64, suffix: &Option<crate::lexer::token::NumericSuffix>) -> String {
    let base = if v == (v as i64) as f64 {
        format!("{}", v as i64)
    } else {
        format!("{}", v)
    };
    match suffix {
        Some(s) => format!("{}{:?}", base, s),
        None => base,
    }
}
