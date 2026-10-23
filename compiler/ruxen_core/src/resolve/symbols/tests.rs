use super::*;
use crate::lexer::token::Span;

fn dummy_span() -> Span {
    Span::new(0, 0, 1, 1)
}

fn var_kind(ty: Ty) -> DefKind {
    DefKind::Variable { mutable: false, ty }
}

fn fn_kind(ret: Ty) -> DefKind {
    DefKind::Function {
        signature: FnSignature {
            self_mode: None,
            is_class_method: false,
            is_async: false,
            generic_params: Vec::new(),
            params: Vec::new(),
            return_ty: ret,
            c_symbol: None,
        },
    }
}

#[test]
fn fieldless_struct_is_effectively_copy() {
    let mut symbols = SymbolTable::new();
    symbols.define(
        "Point".to_string(),
        DefKind::Struct {
            info: StructInfo {
                generic_params: Vec::new(),
                fields: Vec::new(),
                layout: Vec::new(),
                auto_traits: AutoTraitFlags::default(),
                const_predicates: vec![],
            },
        },
        Visibility::Public,
        dummy_span(),
    );

    let point = Ty::Struct {
        name: "Point".to_string(),
        generic_args: Vec::new(),
    };

    assert!(ty_is_effectively_copy(&point, &symbols));
    assert!(ty_is_effectively_copy(
        &Ty::Tuple(vec![point.clone(), Ty::Int]),
        &symbols
    ));
    assert!(!ty_is_effectively_copy(
        &Ty::Array(Box::new(point)),
        &symbols
    ));
}

fn class_kind() -> DefKind {
    DefKind::Class {
        info: ClassInfo {
            generic_params: Vec::new(),
            parent: None,
            fields: Vec::new(),
            methods: Vec::new(),
            auto_traits: AutoTraitFlags::default(),
            const_predicates: vec![],
            flat_heap_struct: false,
            closure_shell: false,
            supplied: None,
            runtime_dispatch_includes: Vec::new(),
        },
    }
}

#[test]
fn new_table_is_empty() {
    let table = SymbolTable::new();
    assert_eq!(table.len(), 0);
    assert!(table.is_empty());
}

#[test]
fn default_matches_new() {
    let table = SymbolTable::default();
    assert!(table.is_empty());
}

#[test]
fn define_returns_sequential_defids_starting_at_zero() {
    let mut table = SymbolTable::new();
    let a = table.define(
        "a".to_string(),
        var_kind(Ty::Int),
        Visibility::Private,
        dummy_span(),
    );
    let b = table.define(
        "b".to_string(),
        var_kind(Ty::Bool),
        Visibility::Private,
        dummy_span(),
    );
    let c = table.define(
        "c".to_string(),
        var_kind(Ty::Unit),
        Visibility::Private,
        dummy_span(),
    );
    assert_eq!(a, 0);
    assert_eq!(b, 1);
    assert_eq!(c, 2);
    assert_eq!(table.len(), 3);
    assert!(!table.is_empty());
}

#[test]
fn get_returns_definition_by_defid() {
    let mut table = SymbolTable::new();
    let id = table.define(
        "foo".to_string(),
        var_kind(Ty::Int),
        Visibility::Public,
        dummy_span(),
    );
    let def = table.get(id).expect("definition should be present");
    assert_eq!(def.id, id);
    assert_eq!(def.name, "foo");
    assert_eq!(def.visibility, Visibility::Public);
}

#[test]
fn get_returns_none_for_unknown_defid() {
    let table = SymbolTable::new();
    assert!(table.get(0).is_none());
    assert!(table.get(999).is_none());
}

#[test]
fn distinguishes_function_class_and_variable_kinds() {
    let mut table = SymbolTable::new();
    let v = table.define(
        "x".to_string(),
        var_kind(Ty::Int),
        Visibility::Private,
        dummy_span(),
    );
    let f = table.define(
        "do_stuff".to_string(),
        fn_kind(Ty::Unit),
        Visibility::Public,
        dummy_span(),
    );
    let c = table.define(
        "Widget".to_string(),
        class_kind(),
        Visibility::Public,
        dummy_span(),
    );

    assert!(matches!(
        table.get(v).unwrap().kind,
        DefKind::Variable { .. }
    ));
    assert!(matches!(
        table.get(f).unwrap().kind,
        DefKind::Function { .. }
    ));
    assert!(matches!(table.get(c).unwrap().kind, DefKind::Class { .. }));
}

#[test]
fn span_is_preserved_on_definitions() {
    let mut table = SymbolTable::new();
    let span = Span::new(10, 20, 4, 7);
    let id = table.define(
        "here".to_string(),
        var_kind(Ty::Int),
        Visibility::Private,
        span.clone(),
    );
    let def = table.get(id).unwrap();
    assert_eq!(def.span, span);
    assert_eq!(def.span.line, 4);
    assert_eq!(def.span.column, 7);
}

#[test]
fn duplicate_names_allocate_distinct_defids() {
    // The symbol table itself does not deduplicate — it's the scope
    // layer that handles shadowing. Two defines with the same name must
    // get distinct DefIds.
    let mut table = SymbolTable::new();
    let a = table.define(
        "same".to_string(),
        var_kind(Ty::Int),
        Visibility::Private,
        dummy_span(),
    );
    let b = table.define(
        "same".to_string(),
        var_kind(Ty::Bool),
        Visibility::Private,
        dummy_span(),
    );
    assert_ne!(a, b);
    assert_eq!(table.len(), 2);
}

#[test]
fn iter_yields_definitions_in_insertion_order() {
    let mut table = SymbolTable::new();
    table.define(
        "a".to_string(),
        var_kind(Ty::Int),
        Visibility::Private,
        dummy_span(),
    );
    table.define(
        "b".to_string(),
        var_kind(Ty::Bool),
        Visibility::Private,
        dummy_span(),
    );
    table.define(
        "c".to_string(),
        var_kind(Ty::Char),
        Visibility::Private,
        dummy_span(),
    );
    let names: Vec<_> = table.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, vec!["a", "b", "c"]);
}

#[test]
fn update_ty_changes_variable_type() {
    let mut table = SymbolTable::new();
    let id = table.define(
        "x".to_string(),
        var_kind(Ty::Int),
        Visibility::Private,
        dummy_span(),
    );
    table.update_ty(id, Ty::Bool);
    let def = table.get(id).unwrap();
    if let DefKind::Variable { ty, .. } = &def.kind {
        assert_eq!(*ty, Ty::Bool);
    } else {
        panic!("expected DefKind::Variable");
    }
}

#[test]
fn update_ty_is_noop_for_class_definitions() {
    let mut table = SymbolTable::new();
    let id = table.define(
        "C".to_string(),
        class_kind(),
        Visibility::Public,
        dummy_span(),
    );
    // Class is not one of the variants update_ty touches; it must be a no-op.
    table.update_ty(id, Ty::Int);
    assert!(matches!(table.get(id).unwrap().kind, DefKind::Class { .. }));
}

#[test]
fn def_ty_returns_type_for_variable() {
    let mut table = SymbolTable::new();
    let id = table.define(
        "x".to_string(),
        var_kind(Ty::Int),
        Visibility::Private,
        dummy_span(),
    );
    assert_eq!(table.def_ty(id), Some(Ty::Int));
}

#[test]
fn def_ty_returns_fn_type_for_function() {
    let mut table = SymbolTable::new();
    let id = table.define(
        "f".to_string(),
        fn_kind(Ty::Bool),
        Visibility::Public,
        dummy_span(),
    );
    match table.def_ty(id) {
        Some(Ty::Fn { params, ret }) => {
            assert!(params.is_empty());
            assert_eq!(*ret, Ty::Bool);
        }
        other => panic!("expected Ty::Fn, got {:?}", other),
    }
}

#[test]
fn def_ty_returns_none_for_class() {
    let mut table = SymbolTable::new();
    let id = table.define(
        "C".to_string(),
        class_kind(),
        Visibility::Public,
        dummy_span(),
    );
    assert_eq!(table.def_ty(id), None);
}

#[test]
fn named_is_case_sensitive_and_follows_get_mut_renames() {
    let mut table = SymbolTable::new();
    let upper = table.define(
        "Foo".to_string(),
        var_kind(Ty::Int),
        Visibility::Private,
        dummy_span(),
    );
    let lower = table.define(
        "foo".to_string(),
        var_kind(Ty::Int),
        Visibility::Private,
        dummy_span(),
    );
    let ids = |table: &SymbolTable, name: &str| -> Vec<DefId> {
        table.named(name).map(|d| d.id).collect()
    };
    assert_eq!(ids(&table, "Foo"), vec![upper]);
    // Defined after the index was built: `define` extends it.
    let second = table.define(
        "foo".to_string(),
        var_kind(Ty::Int),
        Visibility::Private,
        dummy_span(),
    );
    assert_eq!(ids(&table, "foo"), vec![lower, second]);
    // Renamed through `get_mut` (overload mangling, module qualification).
    table.get_mut(lower).unwrap().name = "foo__overload1".to_string();
    assert_eq!(ids(&table, "foo"), vec![second]);
    assert_eq!(ids(&table, "foo__overload1"), vec![lower]);
    let overloads: Vec<DefId> = table.overloads_named("foo").map(|d| d.id).collect();
    assert_eq!(overloads, vec![lower, second]);
}

fn method_kind(parent: DefId) -> DefKind {
    let DefKind::Function { signature } = fn_kind(Ty::Unit) else {
        unreachable!("fn_kind builds a Function");
    };
    DefKind::Method { parent, signature }
}

#[test]
fn methods_of_is_overload_aware_and_scoped_to_the_parent() {
    let mut table = SymbolTable::new();
    let a = table.define(
        "A".to_string(),
        class_kind(),
        Visibility::Public,
        dummy_span(),
    );
    let b = table.define(
        "B".to_string(),
        class_kind(),
        Visibility::Public,
        dummy_span(),
    );
    let mut method = |name: &str, parent: DefId| {
        table.define(
            name.to_string(),
            method_kind(parent),
            Visibility::Public,
            dummy_span(),
        )
    };
    let a_foo = method("foo", a);
    method("foo", b);
    method("foobar", a);
    let a_foo_overload = method("foo__overload9", a);
    let of_a: Vec<DefId> = table.methods_of(a, "foo").map(|d| d.id).collect();
    assert_eq!(of_a, vec![a_foo, a_foo_overload]);
    let named_a: Vec<DefId> = table.methods_named("A", "foo").map(|d| d.id).collect();
    assert_eq!(named_a, of_a);
}

#[test]
fn nominal_skips_non_types_and_type_def_matches_the_mir_spelling() {
    let mut table = SymbolTable::new();
    table.define(
        "Socket".to_string(),
        var_kind(Ty::Int),
        Visibility::Private,
        dummy_span(),
    );
    let socket = table.define(
        "Socket".to_string(),
        class_kind(),
        Visibility::Public,
        dummy_span(),
    );
    let qualified = table.define(
        "Net.Socket".to_string(),
        class_kind(),
        Visibility::Public,
        dummy_span(),
    );
    assert_eq!(table.nominal("Socket").map(|d| d.id), Some(socket));
    assert!(table.nominal("Net_Socket").is_none());
    assert_eq!(table.type_def("Net_Socket").map(|d| d.id), Some(qualified));
    assert_eq!(table.type_def("Socket").map(|d| d.id), Some(socket));
}

#[test]
fn get_mut_allows_in_place_mutation() {
    let mut table = SymbolTable::new();
    let id = table.define(
        "x".to_string(),
        var_kind(Ty::Int),
        Visibility::Private,
        dummy_span(),
    );
    {
        let def = table.get_mut(id).unwrap();
        def.name = "renamed".to_string();
    }
    assert_eq!(table.get(id).unwrap().name, "renamed");
}

#[test]
fn visibility_is_preserved_per_definition() {
    let mut table = SymbolTable::new();
    let a = table.define(
        "a".to_string(),
        var_kind(Ty::Int),
        Visibility::Private,
        dummy_span(),
    );
    let b = table.define(
        "b".to_string(),
        var_kind(Ty::Int),
        Visibility::Public,
        dummy_span(),
    );
    let c = table.define(
        "c".to_string(),
        var_kind(Ty::Int),
        Visibility::Protected,
        dummy_span(),
    );
    assert_eq!(table.get(a).unwrap().visibility, Visibility::Private);
    assert_eq!(table.get(b).unwrap().visibility, Visibility::Public);
    assert_eq!(table.get(c).unwrap().visibility, Visibility::Protected);
}
