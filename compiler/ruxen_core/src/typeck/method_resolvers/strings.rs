//! TIER 3 — string-shape structural resolvers.
//!
//! `Ty::String` / `Ty::Str` method typing + the `ParseIntError` /
//! `ParseFloatError` `.message()` accessors, carved verbatim out of the
//! legacy match. Pure arms (use `eng` only via static helpers).

use crate::hir::types::Ty;

use super::resolver::MethodResolver;
use super::InferenceEngine;

pub(super) fn resolvers() -> Vec<MethodResolver> {
    vec![MethodResolver {
        matches: |ty, _method| match ty {
            Ty::String | Ty::Str => true,
            Ty::Class { name, .. } => name == "ParseIntError" || name == "ParseFloatError",
            _ => false,
        },
        resolve: |_eng, ty, method, _args, _span| match (ty, method) {
            // String methods
            (Ty::String, "clone") => Some(Ty::String),
            (Ty::String, "size") => Some(Ty::USize),
            (Ty::String, "empty?") => Some(Ty::Bool),
            (Ty::String, "push_str") => Some(Ty::Unit),
            (Ty::String, "trim") => Some(Ty::Str),
            (Ty::String, "to_lower") => Some(Ty::String),
            (Ty::String, "to_upper") => Some(Ty::String),
            (Ty::String, "chars") => Some(Ty::Array(Box::new(Ty::Char))),
            // Phase 2 stdlib batch 2 (#02): split returns owned Vec[String]
            // (per the v1 rule: iterator producers return Vec, not lazy
            // SplitIter, until prompt 05 ships the lazy iterator story).
            (Ty::String, "split") => Some(Ty::Array(Box::new(Ty::String))),
            (Ty::String, "push") => Some(Ty::Unit),
            (Ty::String, "as_str") => Some(Ty::Str),
            (Ty::String, "from") => Some(Ty::String),
            (Ty::String, "include?") => Some(Ty::Bool),
            (Ty::String, "starts_with") => Some(Ty::Bool),
            (Ty::String, "ends_with") => Some(Ty::Bool),
            (Ty::String, "repeat") => Some(Ty::String),
            (Ty::String, "lines") => Some(Ty::Array(Box::new(Ty::String))),
            (Ty::String, "replace") => Some(Ty::String),
            // Phase 2 stdlib (#02).
            (Ty::String, "new") => Some(Ty::String),
            (Ty::String, "with_capacity") => Some(Ty::String),
            (Ty::String, "to_string") => Some(Ty::String),
            (Ty::String, "bytes") => Some(Ty::Array(Box::new(Ty::UInt8))),
            (Ty::String, "trim_start") => Some(Ty::Str),
            (Ty::String, "trim_end") => Some(Ty::Str),
            (Ty::String, "find") => Some(Ty::Option(Box::new(Ty::USize))),
            (Ty::String, "splitn") => Some(Ty::Array(Box::new(Ty::String))),
            (Ty::String, "clear") => Some(Ty::Unit),
            (Ty::String, "truncate") => Some(Ty::Unit),
            (Ty::String, "insert") => Some(Ty::Unit),
            (Ty::String, "insert_str") => Some(Ty::Unit),
            (Ty::String, "remove") => Some(Ty::Char),
            (Ty::String, "parse_int") => Some(Ty::Result(
                Box::new(Ty::Int),
                Box::new(InferenceEngine::class_ty("ParseIntError", vec![])),
            )),
            (Ty::String, "parse_float") => Some(Ty::Result(
                Box::new(Ty::Float),
                Box::new(InferenceEngine::class_ty("ParseFloatError", vec![])),
            )),
            (Ty::String, "into_bytes") => Some(Ty::Array(Box::new(Ty::UInt8))),
            (Ty::Str, "size") => Some(Ty::USize),
            (Ty::Str, "empty?") => Some(Ty::Bool),
            (Ty::Str, "trim") => Some(Ty::Str),
            (Ty::Str, "to_lower") => Some(Ty::Str),
            (Ty::Str, "to_upper") => Some(Ty::Str),
            (Ty::Str, "chars") => Some(Ty::Array(Box::new(Ty::Char))),
            // String#split returns Array<String> in Ruby — always. Both
            // owned-`String` and borrowed-`&str` receivers should produce
            // the same surface type. The historical `SplitIter` class
            // shape on the `&str` arm was a Rust-style lazy iterator that
            // didn't expose `.get(i)` / `.len()`, leaving callers stuck
            // (every multipart/header parser hits this). Unifying to
            // Array<String> matches Ruby and removes the footgun. Pin:
            // `docs/rondo_v1_blockers.md` B13.
            (Ty::Str, "split") => Some(Ty::Array(Box::new(Ty::String))),
            (Ty::Str, "parse_uint") => Some(Ty::Result(Box::new(Ty::USize), Box::new(Ty::Error))),
            (Ty::Str, "as_str") => Some(Ty::Str),
            (Ty::Str, "include?") => Some(Ty::Bool),
            (Ty::Str, "starts_with") => Some(Ty::Bool),
            (Ty::Str, "ends_with") => Some(Ty::Bool),
            (Ty::Str, "lines") => Some(Ty::Array(Box::new(Ty::String))),
            (Ty::Str, "replace") => Some(Ty::String),
            (Ty::Str, "to_string") => Some(Ty::String),
            (Ty::Str, "bytes") => Some(Ty::Array(Box::new(Ty::UInt8))),
            (Ty::Str, "trim_start") => Some(Ty::Str),
            (Ty::Str, "trim_end") => Some(Ty::Str),
            (Ty::Str, "find") => Some(Ty::Option(Box::new(Ty::USize))),
            (Ty::Str, "splitn") => Some(Ty::Array(Box::new(Ty::String))),
            (Ty::Str, "parse_int") => Some(Ty::Result(
                Box::new(Ty::Int),
                Box::new(InferenceEngine::class_ty("ParseIntError", vec![])),
            )),
            (Ty::Str, "parse_float") => Some(Ty::Result(
                Box::new(Ty::Float),
                Box::new(InferenceEngine::class_ty("ParseFloatError", vec![])),
            )),
            // ParseIntError / ParseFloatError accessors.
            (Ty::Class { name, .. }, "message")
                if name == "ParseIntError" || name == "ParseFloatError" =>
            {
                Some(Ty::String)
            }
            (Ty::String, "from_iter") => Some(Ty::String),
            // `to_s` on String/Str both yield a `String`.
            (Ty::String, "to_s") => Some(Ty::String),
            (Ty::Str, "to_s") => Some(Ty::String),
            // Within-namespace fallthrough (not a cross-cutting catch-all).
            _ => None,
        },
    }]
}
