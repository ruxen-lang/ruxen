//! Ruby-style `alias new_name old_name` resolution.
//!
//! A pure resolver synonym — both names resolve to ONE function body, with no
//! delegating thunk and no extra codegen. See `docs/decisions/alias-keyword.md`.
//!
//! Two scopes:
//!   * **Free-fn alias** (`alias foo bar` at top level / module) → value-scope
//!     rebind: `scopes.insert(foo → bar's DefId)`. Bound in Pass-1b so call
//!     sites resolve during Pass-2 body resolution (`bind_free_fn_alias`).
//!   * **Method alias** (`alias member? include?` in a type body) → recorded in
//!     `Resolver::method_aliases[type → {alias → canonical}]`, threaded to
//!     typeck + MIR which rewrite the alias name to the canonical at lookup /
//!     mangle time (`record_method_alias`).
//!
//! Diagnostics: E1120 unknown target, E1121 cycle, E1122 collision/self-alias,
//! E1123 operator-spelled without a surface dispatch path (ADR D6).

use std::collections::HashMap;

use crate::diagnostics::Diagnostic;
use crate::parser::ast;
use crate::resolve::Resolver;
use crate::resolve::symbols::DefKind;

/// An operator-symbol method name (`+`, `<<`, `[]`, `[]=`, `-@`, …).
fn is_operator_name(name: &str) -> bool {
    matches!(
        name,
        "+" | "-"
            | "*"
            | "/"
            | "%"
            | "&"
            | "|"
            | "^"
            | "<<"
            | ">>"
            | "!"
            | "[]"
            | "[]="
            | "+@"
            | "-@"
    )
}

/// Operator names whose expression forms route through a synthetic method call
/// in typeck and MIR. The method call consumes the same canonical-name map as a
/// normal call, so these operators can be aliases without a second dispatch
/// mechanism. Unary `+@` has no surface expression in Ruxen and remains E1123.
fn supports_operator_alias(name: &str) -> bool {
    matches!(
        name,
        "+" | "-" | "*" | "/" | "%" | "&" | "|" | "^" | "<<" | ">>" | "!" | "[]" | "[]=" | "-@"
    )
}

impl Resolver {
    /// Bind a top-level / module-scope `alias new old` free-function synonym.
    /// `old`'s DefId must already be registered (Pass-1b ran first). On success
    /// `new` is inserted into the current value scope pointing at the same
    /// DefId — one body, two names. Errors: E1123 (operator in free-fn scope), E1122
    /// (self-alias / collision), E1120 (unknown target).
    pub(super) fn bind_free_fn_alias(&mut self, a: &ast::AliasDef) {
        // Operators are methods, not free functions; no top-level operator
        // expression can dispatch through a free-function alias.
        if is_operator_name(&a.new_name) || is_operator_name(&a.old_name) {
            self.diagnostics.push(Diagnostic::error_with_code(
                format!(
                    "operator aliases require method scope (`alias {} {}`); \
                     free-function aliases cannot dispatch operators",
                    a.new_name, a.old_name
                ),
                a.span.clone(),
                "E1123",
            ));
            return;
        }
        // Self-alias is the degenerate collision (ADR D5 → E1122).
        if a.new_name == a.old_name {
            self.diagnostics.push(Diagnostic::error_with_code(
                format!(
                    "`alias {} {}` aliases a name to itself",
                    a.new_name, a.old_name
                ),
                a.span.clone(),
                "E1122",
            ));
            return;
        }
        // The new name must not already be a callable in this scope (ADR D5).
        if let Some(existing) = self.scopes.lookup(&a.new_name) {
            if self
                .symbols
                .get(existing)
                .map(|d| {
                    matches!(
                        d.kind,
                        DefKind::Function { .. }
                            | DefKind::Method { .. }
                            | DefKind::OverloadSet { .. }
                    )
                })
                .unwrap_or(false)
            {
                self.diagnostics.push(Diagnostic::error_with_code(
                    format!(
                        "alias `{}` collides with an existing definition of that name",
                        a.new_name
                    ),
                    a.span.clone(),
                    "E1122",
                ));
                return;
            }
        }
        // The target must resolve to a callable (function / overload set).
        match self.scopes.lookup(&a.old_name) {
            Some(target) if self.is_callable_def(target) => {
                self.scopes.insert(a.new_name.clone(), target);
            }
            _ => {
                self.diagnostics.push(Diagnostic::error_with_code(
                    format!(
                        "alias target `{}` is not a known function in scope",
                        a.old_name
                    ),
                    a.span.clone(),
                    "E1120",
                ));
            }
        }
    }

    fn is_callable_def(&self, def_id: crate::hir::nodes::DefId) -> bool {
        self.symbols
            .get(def_id)
            .map(|d| {
                matches!(
                    d.kind,
                    DefKind::Function { .. } | DefKind::OverloadSet { .. }
                )
            })
            .unwrap_or(false)
    }

    /// Record method `alias new old` synonyms for a type body. `type_key` is the
    /// home type's (possibly module-qualified) name. `method_names` is the set
    /// of canonical method names visible on the type (in-body `def`s + included
    /// mixin methods, ADR D7). Flattens alias-of-alias to the root (ADR D4) and
    /// detects cycles (E1121). Errors: E1123 (unsupported operator), E1122
    /// (self/collision), E1120 (unknown target).
    pub(super) fn record_method_aliases(
        &mut self,
        type_key: &str,
        aliases: &[ast::AliasDef],
        method_names: &[String],
    ) {
        if aliases.is_empty() {
            return;
        }
        // Local view of the aliases declared in THIS body, so alias-of-alias can
        // be flattened to a canonical method name in a settling fixpoint.
        let mut local: HashMap<String, String> = HashMap::new();
        for a in aliases {
            if (is_operator_name(&a.new_name) && !supports_operator_alias(&a.new_name))
                || (is_operator_name(&a.old_name) && !supports_operator_alias(&a.old_name))
            {
                self.diagnostics.push(Diagnostic::error_with_code(
                    format!(
                        "operator alias has no supported expression dispatch path (`alias {} {}`)",
                        a.new_name, a.old_name
                    ),
                    a.span.clone(),
                    "E1123",
                ));
                continue;
            }
            if a.new_name == a.old_name {
                self.diagnostics.push(Diagnostic::error_with_code(
                    format!(
                        "`alias {} {}` aliases a name to itself",
                        a.new_name, a.old_name
                    ),
                    a.span.clone(),
                    "E1122",
                ));
                continue;
            }
            // The new name must not shadow a real method on the type (D5).
            if method_names.iter().any(|m| m == &a.new_name) {
                self.diagnostics.push(Diagnostic::error_with_code(
                    format!(
                        "alias `{}` collides with an existing method on `{}`",
                        a.new_name, type_key
                    ),
                    a.span.clone(),
                    "E1122",
                ));
                continue;
            }
            local.insert(a.new_name.clone(), a.old_name.clone());
        }

        // Flatten every alias to a canonical method name. An `old` that is
        // itself a local alias is followed transitively; a real method name
        // terminates the walk (D4). A walk that neither terminates at a real
        // method nor at a known alias is either an unknown target (E1120) or,
        // if it loops back through `local`, a cycle (E1121).
        let entry = self.method_aliases.entry(type_key.to_string()).or_default();
        for a in aliases {
            let Some(start) = local.get(&a.new_name) else {
                continue; // skipped above (unsupported operator / self / collision)
            };
            let mut current = start.clone();
            let mut seen = vec![a.new_name.clone()];
            let canonical = loop {
                if method_names.iter().any(|m| m == &current) {
                    break Some(current.clone());
                }
                if seen.contains(&current) {
                    // Looped without reaching a real method → cycle.
                    self.diagnostics.push(Diagnostic::error_with_code(
                        format!(
                            "alias `{}` on `{}` forms a cycle and resolves to no method",
                            a.new_name, type_key
                        ),
                        a.span.clone(),
                        "E1121",
                    ));
                    break None;
                }
                seen.push(current.clone());
                match local.get(&current) {
                    Some(next) => current = next.clone(),
                    None => {
                        // Not a method and not a local alias → unknown target.
                        self.diagnostics.push(Diagnostic::error_with_code(
                            format!(
                                "alias target `{}` is not a method on `{}`",
                                a.old_name, type_key
                            ),
                            a.span.clone(),
                            "E1120",
                        ));
                        break None;
                    }
                }
            };
            if let Some(canon) = canonical {
                entry.insert(a.new_name.clone(), canon);
            }
        }
    }
}
