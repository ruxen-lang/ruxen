//! TypeContext — the typing environment for type checking.
//!
//! Manages type variable allocation, substitution maps, and provides
//! the interface for querying resolved types during and after inference.

use std::collections::HashMap;

use crate::hir::types::{Ty, TypeId};

/// The central typing environment used throughout type inference.
///
/// TypeContext allocates fresh type variables, stores bindings from
/// inference variables to concrete types, and resolves chains of
/// type variable substitutions.
#[derive(Debug)]
pub struct TypeContext {
    /// Next available type variable ID.
    next_type_id: TypeId,

    /// Substitution map: type variable → type it's bound to.
    /// After unification, this maps each `Infer(id)` to its resolved type.
    substitutions: HashMap<TypeId, Ty>,
}

impl TypeContext {
    pub fn new() -> Self {
        Self {
            next_type_id: 0,
            substitutions: HashMap::new(),
        }
    }

    /// Allocate a fresh type variable.
    pub fn fresh_type_var(&mut self) -> Ty {
        let id = self.next_type_id;
        self.next_type_id += 1;
        Ty::Infer(id)
    }

    /// Bind a type variable to a concrete type.
    /// Returns an error if this would create a cycle.
    pub fn bind(&mut self, id: TypeId, ty: Ty) -> Result<(), String> {
        // Occurs check: prevent infinite types
        if self.occurs_in(id, &ty) {
            return Err(format!("infinite type: ?T{} occurs in {}", id, ty));
        }
        self.substitutions.insert(id, ty);
        Ok(())
    }

    /// Check if type variable `id` occurs anywhere in `ty`.
    /// Prevents infinite types like `T = Vec[T]`.
    fn occurs_in(&self, id: TypeId, ty: &Ty) -> bool {
        match ty {
            Ty::Infer(other_id) => {
                if *other_id == id {
                    return true;
                }
                // Follow substitution chain
                if let Some(bound) = self.substitutions.get(other_id) {
                    return self.occurs_in(id, bound);
                }
                false
            }
            Ty::Tuple(elems) => elems.iter().any(|e| self.occurs_in(id, e)),
            Ty::Array(elem, _) => self.occurs_in(id, elem),
            Ty::Vec(elem) | Ty::Set(elem) | Ty::Option(elem) => self.occurs_in(id, elem),
            Ty::HashMap(k, v) | Ty::Result(k, v) => self.occurs_in(id, k) || self.occurs_in(id, v),
            Ty::Ref(inner) | Ty::RefMut(inner) => self.occurs_in(id, inner),
            Ty::RefLifetime(_, inner) | Ty::RefMutLifetime(_, inner) => self.occurs_in(id, inner),
            Ty::Class { generic_args, .. }
            | Ty::Struct { generic_args, .. }
            | Ty::Enum { generic_args, .. } => generic_args.iter().any(|a| self.occurs_in(id, a)),
            Ty::Fn { params, ret } | Ty::FnMut { params, ret } | Ty::FnOnce { params, ret } => {
                params.iter().any(|p| self.occurs_in(id, p)) || self.occurs_in(id, ret)
            }
            Ty::Alias { target, .. } => self.occurs_in(id, target),
            Ty::Newtype { inner, .. } => self.occurs_in(id, inner),
            // Primitives and other non-composite types
            _ => false,
        }
    }

    /// Resolve a type by following substitution chains.
    /// Returns the most concrete type available.
    pub fn resolve(&self, ty: &Ty) -> Ty {
        match ty {
            Ty::Infer(id) => {
                if let Some(bound) = self.substitutions.get(id) {
                    self.resolve(bound)
                } else {
                    ty.clone()
                }
            }
            Ty::Tuple(elems) => Ty::Tuple(elems.iter().map(|e| self.resolve(e)).collect()),
            Ty::Array(elem, size) => Ty::Array(Box::new(self.resolve(elem)), *size),
            Ty::Vec(elem) => Ty::Vec(Box::new(self.resolve(elem))),
            Ty::HashMap(k, v) => Ty::HashMap(Box::new(self.resolve(k)), Box::new(self.resolve(v))),
            Ty::Set(elem) => Ty::Set(Box::new(self.resolve(elem))),
            Ty::Option(inner) => Ty::Option(Box::new(self.resolve(inner))),
            Ty::Result(ok, err) => {
                Ty::Result(Box::new(self.resolve(ok)), Box::new(self.resolve(err)))
            }
            Ty::Ref(inner) => Ty::Ref(Box::new(self.resolve(inner))),
            Ty::RefMut(inner) => Ty::RefMut(Box::new(self.resolve(inner))),
            Ty::RefLifetime(lt, inner) => {
                Ty::RefLifetime(lt.clone(), Box::new(self.resolve(inner)))
            }
            Ty::RefMutLifetime(lt, inner) => {
                Ty::RefMutLifetime(lt.clone(), Box::new(self.resolve(inner)))
            }
            Ty::Class { name, generic_args } => Ty::Class {
                name: name.clone(),
                generic_args: generic_args.iter().map(|a| self.resolve(a)).collect(),
            },
            Ty::Struct { name, generic_args } => Ty::Struct {
                name: name.clone(),
                generic_args: generic_args.iter().map(|a| self.resolve(a)).collect(),
            },
            Ty::Enum { name, generic_args } => Ty::Enum {
                name: name.clone(),
                generic_args: generic_args.iter().map(|a| self.resolve(a)).collect(),
            },
            Ty::Fn { params, ret } => Ty::Fn {
                params: params.iter().map(|p| self.resolve(p)).collect(),
                ret: Box::new(self.resolve(ret)),
            },
            Ty::FnMut { params, ret } => Ty::FnMut {
                params: params.iter().map(|p| self.resolve(p)).collect(),
                ret: Box::new(self.resolve(ret)),
            },
            Ty::FnOnce { params, ret } => Ty::FnOnce {
                params: params.iter().map(|p| self.resolve(p)).collect(),
                ret: Box::new(self.resolve(ret)),
            },
            Ty::Alias { target, .. } => self.resolve(target),
            // Everything else: return as-is
            other => other.clone(),
        }
    }

    /// Check if all inference variables in a type are resolved.
    pub fn is_fully_resolved(&self, ty: &Ty) -> bool {
        match &self.resolve(ty) {
            Ty::Infer(_) => false,
            Ty::Tuple(elems) => elems.iter().all(|e| self.is_fully_resolved(e)),
            Ty::Array(elem, _) => self.is_fully_resolved(elem),
            Ty::Vec(elem) | Ty::Set(elem) | Ty::Option(elem) => self.is_fully_resolved(elem),
            Ty::HashMap(k, v) | Ty::Result(k, v) => {
                self.is_fully_resolved(k) && self.is_fully_resolved(v)
            }
            Ty::Ref(inner) | Ty::RefMut(inner) => self.is_fully_resolved(inner),
            Ty::RefLifetime(_, inner) | Ty::RefMutLifetime(_, inner) => {
                self.is_fully_resolved(inner)
            }
            Ty::Class { generic_args, .. }
            | Ty::Struct { generic_args, .. }
            | Ty::Enum { generic_args, .. } => {
                generic_args.iter().all(|a| self.is_fully_resolved(a))
            }
            Ty::Fn { params, ret } | Ty::FnMut { params, ret } | Ty::FnOnce { params, ret } => {
                params.iter().all(|p| self.is_fully_resolved(p)) && self.is_fully_resolved(ret)
            }
            _ => true,
        }
    }

    /// Get the current number of allocated type variables.
    pub fn type_var_count(&self) -> u32 {
        self.next_type_id
    }

    /// Look up a substitution directly.
    pub fn get_binding(&self, id: TypeId) -> Option<&Ty> {
        self.substitutions.get(&id)
    }
}

impl Default for TypeContext {
    fn default() -> Self {
        Self::new()
    }
}
