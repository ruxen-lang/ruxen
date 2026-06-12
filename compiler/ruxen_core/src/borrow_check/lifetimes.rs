use crate::borrow_check::regions::ScopeId;
use crate::hir::nodes::DefId;
use crate::hir::types::Ty;
use crate::lexer::token::Span;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelfRefMode {
    Shared,
    Mutable,
}

#[derive(Debug)]
pub struct LifetimeError {
    pub kind: LifetimeErrorKind,
    pub span: Span,
}

#[derive(Debug)]
pub enum LifetimeErrorKind {
    AmbiguousOutputLifetime { input_ref_count: usize },
    ReturnRefToLocal { local_def: DefId },
    BorrowOutlivesOwner { borrow_def: DefId, owner_def: DefId },
}

impl LifetimeErrorKind {
    pub fn input_count(&self) -> usize {
        match self {
            LifetimeErrorKind::AmbiguousOutputLifetime { input_ref_count } => *input_ref_count,
            _ => 0,
        }
    }
}

pub struct LifetimeChecker {
    function_locals: Vec<(DefId, ScopeId)>,
}

impl Default for LifetimeChecker {
    fn default() -> Self {
        Self::new()
    }
}

impl LifetimeChecker {
    pub fn new() -> Self {
        Self {
            function_locals: Vec::new(),
        }
    }

    pub fn register_local(&mut self, def_id: DefId, scope: ScopeId) {
        self.function_locals.push((def_id, scope));
    }

    pub fn clear_locals(&mut self) {
        self.function_locals.clear();
    }

    /// Check lifetime elision rules for a function signature.
    /// Returns Ok if elision resolves the output lifetime, Err if annotation needed.
    pub fn check_elision(
        params: &[Ty],
        return_ty: &Ty,
        self_mode: Option<SelfRefMode>,
    ) -> Result<(), LifetimeError> {
        // If return type contains no references, always OK
        if !Self::contains_ref(return_ty) {
            return Ok(());
        }

        // Rule 3: method with &self → output gets self's lifetime
        if self_mode.is_some() {
            return Ok(());
        }

        // Count input reference parameters
        let ref_count = params.iter().filter(|p| Self::contains_ref(p)).count();

        // Rule 2: exactly one input reference → output gets that lifetime
        if ref_count == 1 {
            return Ok(());
        }

        // No input refs but output is a ref — must be static or error
        if ref_count == 0 {
            return Err(LifetimeError {
                kind: LifetimeErrorKind::AmbiguousOutputLifetime { input_ref_count: 0 },
                span: Span::new(0, 0, 0, 0),
            });
        }

        // Multiple input refs and no &self — ambiguous
        Err(LifetimeError {
            kind: LifetimeErrorKind::AmbiguousOutputLifetime {
                input_ref_count: ref_count,
            },
            span: Span::new(0, 0, 0, 0),
        })
    }

    /// Check if a function returns a reference to a local variable.
    pub fn check_return_ref(
        &self,
        local_def: DefId,
        local_scope: ScopeId,
        func_scope: ScopeId,
        span: Span,
    ) -> Result<(), LifetimeError> {
        if local_scope != func_scope {
            return Err(LifetimeError {
                kind: LifetimeErrorKind::ReturnRefToLocal { local_def },
                span,
            });
        }
        Ok(())
    }

    /// Check if a borrow outlives its owner.
    pub fn check_outlives(
        &self,
        borrow_def: DefId,
        owner_def: DefId,
        owner_scope: ScopeId,
        borrow_scope: ScopeId,
        span: Span,
    ) -> Result<(), LifetimeError> {
        if owner_scope != borrow_scope && borrow_scope.0 < owner_scope.0 {
            return Err(LifetimeError {
                kind: LifetimeErrorKind::BorrowOutlivesOwner {
                    borrow_def,
                    owner_def,
                },
                span,
            });
        }
        Ok(())
    }

    /// Check if a type contains any reference.
    fn contains_ref(ty: &Ty) -> bool {
        match ty {
            Ty::Ref(_) | Ty::RefMut(_) | Ty::RefLifetime(_, _) | Ty::RefMutLifetime(_, _) => true,
            Ty::Tuple(elems) => elems.iter().any(Self::contains_ref),
            Ty::Option(inner) | Ty::Array(inner) => Self::contains_ref(inner),
            Ty::Result(a, b) => Self::contains_ref(a) || Self::contains_ref(b),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hir::types::Ty;
    use crate::lexer::token::Span;

    fn span() -> Span {
        Span::new(0, 1, 1, 1)
    }

    #[test]
    fn elision_rule_2_single_input_ref() {
        let params = vec![Ty::Ref(Box::new(Ty::String))];
        let ret = Ty::Ref(Box::new(Ty::String));
        let result = LifetimeChecker::check_elision(&params, &ret, None);
        assert!(
            result.is_ok(),
            "single ref input → output inherits its lifetime"
        );
    }

    #[test]
    fn elision_rule_3_method_self_ref() {
        let params = vec![];
        let ret = Ty::Ref(Box::new(Ty::String));
        let self_mode = Some(SelfRefMode::Shared);
        let result = LifetimeChecker::check_elision(&params, &ret, self_mode);
        assert!(result.is_ok(), "&self method → output gets self's lifetime");
    }

    #[test]
    fn elision_fails_ambiguous_two_refs() {
        let params = vec![Ty::Ref(Box::new(Ty::String)), Ty::Ref(Box::new(Ty::String))];
        let ret = Ty::Ref(Box::new(Ty::String));
        let result = LifetimeChecker::check_elision(&params, &ret, None);
        assert!(
            result.is_err(),
            "ambiguous lifetimes should require annotation"
        );
    }

    #[test]
    fn no_ref_output_always_ok() {
        let params = vec![Ty::Ref(Box::new(Ty::String)), Ty::Ref(Box::new(Ty::String))];
        let ret = Ty::Int;
        let result = LifetimeChecker::check_elision(&params, &ret, None);
        assert!(
            result.is_ok(),
            "non-ref return type never needs lifetime annotation"
        );
    }

    #[test]
    fn return_ref_to_local_detected() {
        let checker = LifetimeChecker::new();
        let func_scope = ScopeId(0);
        let local_scope = ScopeId(1);
        let result = checker.check_return_ref(42, local_scope, func_scope, span());
        assert!(result.is_err(), "returning reference to local should fail");
    }
}
