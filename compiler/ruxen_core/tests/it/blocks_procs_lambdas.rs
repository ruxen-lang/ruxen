//! Blocks, procs and lambdas (plan: blocks-procs-lambdas, card c1).
//!
//! `syntax_` pins: every closure spelling parses to its kind (Block, Proc,
//! Lambda) through the real lexer, parser and resolver; `Proc[(A) -> R]`
//! resolves to the same type as `any Fn[Fn(A) -> R]`; `ruxen fmt` keeps each
//! spelling and prints a bare closure value as a `->` lambda.

use ruxen_core::driver;
use ruxen_core::formatter;
use ruxen_core::hir::nodes::{HirExpr, HirExprKind, HirItem};
use ruxen_core::hir::visit::{self as hir_visit, Visit as HirVisit};
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::ast::{ClosureKind, ClosureSpelling, Expr, ExprKind, Program};
use ruxen_core::parser::visit::{self as ast_visit, Visit as AstVisit};
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

pub(crate) fn parse(source: &str) -> Result<Program, Vec<String>> {
    let tokens = Lexer::new(source).tokenize().expect("lex");
    Parser::new(tokens)
        .parse()
        .map_err(|d| d.iter().map(|e| e.to_string()).collect())
}

/// Every closure literal in the program, in source order.
fn ast_closures(program: &Program) -> Vec<(ClosureSpelling, ClosureKind, Vec<String>)> {
    struct Collect(Vec<(ClosureSpelling, ClosureKind, Vec<String>)>);
    impl AstVisit for Collect {
        fn visit_expr(&mut self, e: &Expr) {
            if let ExprKind::Closure(c) = &e.kind {
                let names = c.params.iter().map(|p| p.name.clone()).collect();
                self.0.push((c.spelling, c.kind, names));
            }
            ast_visit::walk_expr(self, e);
        }
    }
    let mut v = Collect(Vec::new());
    for item in &program.items {
        v.visit_item(item);
    }
    v.0
}

/// The HIR closure kinds inside the user function `name`, in source order.
fn hir_closure_kinds(source: &str, name: &str) -> Vec<ClosureKind> {
    struct Collect(Vec<ClosureKind>);
    impl HirVisit for Collect {
        fn visit_expr(&mut self, e: &HirExpr) {
            if let HirExprKind::Closure { kind, .. } = &e.kind {
                self.0.push(*kind);
            }
            hir_visit::walk_expr(self, e);
        }
    }
    let result = typeck::type_check(&parse(source).expect("parse"));
    let mut v = Collect(Vec::new());
    for item in &result.program.items {
        if let HirItem::Function(f) = item {
            if f.name == name {
                v.visit_func(f);
            }
        }
    }
    v.0
}

const ALL_SPELLINGS: &str = r#"def twice(f: Fn(Int) -> Int) -> Int
  f.(f.(1))
end

a = ->(x: Int) { x + 1 }
b = ->(x: Int) do
  x + 2
end
c = lambda { |x: Int| x * 2 }
d = proc { |x: Int| x - 1 }
e = Proc.new { |x: Int| x }
g = { |x: Int| x }
h = -> { 1 }
puts twice { |x| x + 1 }
"#;

#[test]
fn syntax_each_spelling_parses_to_its_kind() {
    use ClosureKind::{Block, Lambda, Proc};
    use ClosureSpelling as S;
    let program = parse(ALL_SPELLINGS).expect("parse");
    let x = || vec!["x".to_string()];
    assert_eq!(
        ast_closures(&program),
        vec![
            (S::Stabby, Lambda, x()),
            (S::Stabby, Lambda, x()),
            (S::Lambda, Lambda, x()),
            (S::Proc, Proc, x()),
            (S::ProcNew, Proc, x()),
            (S::Bare, Lambda, x()),
            (S::Stabby, Lambda, vec![]),
            (S::Bare, Block, x()),
        ]
    );
    assert_eq!(
        hir_closure_kinds(ALL_SPELLINGS, "main"),
        vec![Lambda, Lambda, Lambda, Proc, Proc, Lambda, Lambda, Block]
    );
}

#[test]
fn syntax_stabby_lambda_rejects_pipe_params() {
    let errors = parse("f = ->(x: Int) { |y| y }\n").unwrap_err();
    assert!(
        errors.iter().any(|e| e.contains("`->` lambda")),
        "expected the stabby-params error, got {errors:?}"
    );
}

#[test]
fn syntax_proc_type_resolves_like_any_fn() {
    let source = r#"def keep(p: Proc[(Int) -> Int], q: Proc[() -> nil]) -> Int
  1
end

def keep_any(p: any Fn[Fn(Int) -> Int], q: any Fn[Fn() -> nil]) -> Int
  1
end
"#;
    let result = typeck::type_check(&parse(source).expect("parse"));
    let param_tys = |name: &str| {
        result
            .program
            .items
            .iter()
            .find_map(|i| match i {
                HirItem::Function(f) if f.name == name => {
                    Some(f.params.iter().map(|p| p.ty.clone()).collect::<Vec<_>>())
                }
                _ => None,
            })
            .expect("function")
    };
    let procs = param_tys("keep");
    assert!(
        procs
            .iter()
            .all(|t| matches!(t, ruxen_core::hir::types::Ty::AnyMixin(_))),
        "Proc[...] must be an `any Fn` type, got {procs:?}"
    );
    assert_eq!(procs, param_tys("keep_any"));
}

/// `Proc` is the core library's `type Proc[F] = any Fn[F]`, and the bare
/// signature `(A) -> R` is a generic argument of any named type, not a
/// `Proc` special case: a user alias spells and formats it the same way,
/// and a parenthesised list without `->` stays a tuple.
#[test]
fn syntax_signature_generic_arg_works_for_any_alias() {
    let source = r#"type Callback[A] = Proc[(A) -> nil]

type Twins[T] = Array[(T, T)]

def keep(cb: Callback[Int], pairs: Twins[Int]) -> Int
  1
end

def keep_any(cb: any Fn[Fn(Int) -> nil], pairs: Array[(Int, Int)]) -> Int
  1
end
"#;
    let result = typeck::type_check(&parse(source).expect("parse"));
    let errors: Vec<_> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "typeck: {errors:?}");
    let param_tys = |name: &str| {
        result
            .program
            .items
            .iter()
            .find_map(|i| match i {
                HirItem::Function(f) if f.name == name => {
                    Some(f.params.iter().map(|p| p.ty.clone()).collect::<Vec<_>>())
                }
                _ => None,
            })
            .expect("function")
    };
    assert_eq!(param_tys("keep"), param_tys("keep_any"));
    assert_eq!(fmt(source), source);
}

fn fmt(source: &str) -> String {
    let r = formatter::format(source);
    assert!(r.errors.is_empty(), "format errored: {:?}", r.errors);
    let again = formatter::format(&r.output);
    assert_eq!(r.output, again.output, "fmt is not idempotent");
    r.output
}

#[test]
fn syntax_fmt_keeps_each_written_spelling() {
    let source = r#"def keep(p: Proc[(Int) -> Int]) -> Int
  p.(1)
end

a = ->(x: Int) { x + 1 }
b = ->(x: Int) do
  x + 2
end
h = -> { 1 }
c = lambda { |x: Int| x * 2 }
d = proc { |x: Int| x - 1 }
e = Proc.new { |x: Int| x }
m = move ->(x: Int) { x }
[1].each { |x| puts x }
[1].each do |x|
  puts x
end
"#;
    assert_eq!(fmt(source), source);
}

#[test]
fn syntax_fmt_prints_bare_closure_values_as_stabby() {
    let source = r#"g = { |x: Int| x }
z = { || 1 }
m = move { |x: Int| x }
w = do |x: Int|
  x
end
run({ |x| x })
"#;
    let expected = r#"g = ->(x: Int) { x }
z = ->() { 1 }
m = move ->(x: Int) { x }
w = ->(x: Int) do
  x
end
run(->(x) { x })
"#;
    let out = fmt(source);
    assert_eq!(out, expected);
    let kinds: Vec<_> = ast_closures(&parse(&out).expect("reparse"))
        .into_iter()
        .map(|(s, k, _)| (s, k))
        .collect();
    assert!(
        kinds
            .iter()
            .all(|&(s, k)| s == ClosureSpelling::Stabby && k == ClosureKind::Lambda),
        "reparsed closures must be stabby lambdas, got {kinds:?}"
    );
}

/// Run the front end on `source` (`driver::check`), asserting it
/// type-checks and lowers cleanly, and return the borrow error codes in
/// order: the borrow check's, then the MIR loan check's.
pub(crate) fn borrow_codes(source: &str) -> Vec<String> {
    let frontend = driver::check(&parse(source).expect("parse"), &driver::Options::default());
    let errors: Vec<_> = frontend
        .diagnostics()
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .collect();
    assert!(errors.is_empty(), "typecheck errors: {errors:?}");
    assert!(
        frontend.lowering_error.is_none(),
        "{}",
        frontend.render(None)
    );
    frontend
        .borrow_errors
        .iter()
        .map(|e| e.code.code_str().to_string())
        .collect()
}

const BLOCK_TARGETS: &str = r#"def each_one(&block: Fn[(Int) -> nil])
  yield 1
end

def keep(p: Proc[(Int) -> nil])
  p.(1)
end
"#;

#[test]
fn semantics_trailing_block_kind_follows_its_param_type() {
    use ClosureKind::{Block, Proc};
    let source = format!(
        "{BLOCK_TARGETS}
each_one {{ |x| puts x }}
keep {{ |x| puts x }}
each_one do |x|
  puts x
end
"
    );
    assert_eq!(hir_closure_kinds(&source, "main"), vec![Block, Proc, Block]);
}

/// A block bound to a mixin default method takes its kind from the
/// default's parameter type (typeck records the default in `resolved`).
#[test]
fn semantics_mixin_default_block_kind_follows_its_param_type() {
    use ClosureKind::{Block, Proc};
    let source = r#"mixin Visit
  def visit(f: Fn(Int) -> nil) -> nil
    f.(1)
  end
  def keep(p: Proc[(Int) -> nil]) -> nil
    p.(1)
  end
end

class Bag
  include Visit
  def init
  end
end

b = Bag.new
b.visit { |x| puts x }
b.keep { |x| puts x }
"#;
    assert_eq!(hir_closure_kinds(source, "main"), vec![Block, Proc]);
}

#[test]
fn semantics_blocks_borrow_their_captures() {
    let source = format!(
        "{BLOCK_TARGETS}
s = \"hi\"
each_one {{ |x| puts s }}
each_one do |x|
  puts s
end
puts s
"
    );
    assert_eq!(borrow_codes(&source), Vec::<String>::new());
}

#[test]
fn semantics_procs_and_lambdas_move_their_captures() {
    let reuse_after = |closure: &str| {
        format!(
            "{BLOCK_TARGETS}
s = \"hi\"
{closure}
puts s
"
        )
    };
    for closure in [
        "f = -> { s.size }",
        "f = lambda { s.size }",
        "f = proc { s.size }",
        "f = Proc.new { s.size }",
        "keep { |x| puts s }",
    ] {
        assert_eq!(
            borrow_codes(&reuse_after(closure)),
            vec!["E1001"],
            "`{closure}` must move `s`"
        );
    }
    let second_capture = "s = \"hi\"
p = proc { s.size }
q = -> { s.size }
";
    assert_eq!(borrow_codes(second_capture), vec!["E1001"]);
}

#[test]
fn semantics_storing_a_block_is_e1018() {
    for body in [
        "b = block",
        "keep(block)",
        "f = ->(x: Int) { block.(x) }",
        "xs.push(block)",
        "return block",
    ] {
        let source = format!(
            "{BLOCK_TARGETS}
def store(xs: &var Array[Fn(Int) -> nil], &block: Fn[(Int) -> nil])
  {body}
end
"
        );
        assert_eq!(
            borrow_codes(&source),
            vec!["E1018"],
            "`{body}` stores the block"
        );
    }
}

#[test]
fn semantics_calling_testing_and_forwarding_a_block_is_allowed() {
    let source = format!(
        "{BLOCK_TARGETS}
def apply(f: Fn(Int) -> nil)
  f.(2)
end

def use_block(&block: Fn[(Int) -> nil])
  if block_given?
    block.(1)
    yield 2
    each_one(&block)
    apply(block)
    apply(block)
    each_one {{ |x| block.(x) }}
  end
end
"
    );
    assert_eq!(borrow_codes(&source), Vec::<String>::new());
}

/// Forwarding `&block` to a mixin default reads the default typeck settled
/// the call against (recorded in `resolved`), not the first method of that
/// name anywhere: `Other#visit` stores its argument, so a name-only lookup
/// would report a false E1018. Forwarding to a mixin default that keeps the
/// block (`Proc[...]`) still stores it. (`Kernel` defaults: see
/// `semantics_forwarding_to_kernel_tap_reads_the_recorded_default`.)
#[test]
fn semantics_forwarding_to_mixin_defaults_reads_the_settled_default() {
    let source = |body: &str| {
        format!(
            r#"class Other
  def init
  end
  def visit(p: Proc[(Int) -> nil]) -> nil
    p.(0)
  end
end

mixin Visit
  def visit(f: Fn(Int) -> nil) -> nil
    f.(1)
  end
  def keep(p: Proc[(Int) -> nil]) -> nil
    p.(1)
  end
end

class Bag
  include Visit
  def init
  end
end

def forward(b: &Bag, &block: Fn[(Int) -> nil])
  {body}
end
"#
        )
    };
    assert_eq!(
        borrow_codes(&source("b.visit(&block)")),
        Vec::<String>::new()
    );
    assert_eq!(borrow_codes(&source("b.keep(&block)")), vec!["E1018"]);
    assert_eq!(
        borrow_codes(&source("b.visit(block)")),
        Vec::<String>::new()
    );
    assert_eq!(borrow_codes(&source("b.keep(block)")), vec!["E1018"]);
}

/// A trailing block on a receiver typed by a mixin-bound type parameter
/// binds to the bound's declared signature: a `Fn[...]` param keeps it a
/// borrowing Block, so the capture stays usable after the call.
#[test]
fn semantics_bounded_type_param_block_reads_the_bound_signature() {
    let source = r##"mixin Walk
  def walk(f: Fn(Int) -> nil) -> nil
    f.(1)
  end
end

def drive[T: Walk](x: &T)
  s = "gen"
  x.walk { |n| puts "#{s}#{n}" }
  puts s
end
"##;
    assert_eq!(hir_closure_kinds(source, "drive"), vec![ClosureKind::Block]);
    assert_eq!(borrow_codes(source), Vec::<String>::new());
}

/// A closure's own `Fn[...]` parameter is a block inside its body: it may be
/// called, but keeping it stores it.
#[test]
fn semantics_closure_fn_param_is_a_block() {
    let source = |body: &str| {
        format!(
            "run = ->(f: Fn(Int) -> nil) {{ {body} }}
"
        )
    };
    assert_eq!(borrow_codes(&source("f.(1)")), Vec::<String>::new());
    assert_eq!(borrow_codes(&source("saved = f")), vec!["E1018"]);
}

/// A trailing block on a call whose signature typeck cannot find (a method
/// of an unconstrained type parameter) becomes an owning Proc: owning can
/// never free a capture twice.
#[test]
fn semantics_unknown_signature_block_is_proc() {
    let source = r#"def drive[T](x: T)
  x.run { |n| puts n }
end
"#;
    assert_eq!(hir_closure_kinds(source, "drive"), vec![ClosureKind::Proc]);
}

/// The error codes typeck reports for `source`, in order.
pub(crate) fn typeck_codes(source: &str) -> Vec<String> {
    typeck::type_check(&parse(source).expect("parse"))
        .diagnostics
        .iter()
        .filter(|d| d.level == ruxen_core::diagnostics::DiagnosticLevel::Error)
        .map(|d| {
            d.code
                .clone()
                .unwrap_or_else(|| format!("<no code>: {}", d.message))
        })
        .collect()
}

/// The parameter types of the closures inside the user function `name`.
fn hir_closure_param_tys(source: &str, name: &str) -> Vec<Vec<ruxen_core::hir::types::Ty>> {
    struct Collect(Vec<Vec<ruxen_core::hir::types::Ty>>);
    impl HirVisit for Collect {
        fn visit_expr(&mut self, e: &HirExpr) {
            if let HirExprKind::Closure { params, .. } = &e.kind {
                self.0.push(params.iter().map(|p| p.ty.clone()).collect());
            }
            hir_visit::walk_expr(self, e);
        }
    }
    let result = typeck::type_check(&parse(source).expect("parse"));
    let mut v = Collect(Vec::new());
    for item in &result.program.items {
        if let HirItem::Function(f) = item {
            if f.name == name {
                v.visit_func(f);
            }
        }
    }
    v.0
}

/// A trailing block on a free-function call takes its parameter types from
/// the function's `Fn[...]` parameter BEFORE its body is inferred: `puts(n)`
/// must not pin an unannotated `n` to `String` (that passed the Int 1 as a
/// string pointer and segfaulted).
#[test]
fn semantics_free_fn_block_params_take_the_signature_types() {
    use ruxen_core::hir::types::Ty;
    let source = r#"def inner(&block: Fn[(Int) -> nil])
  yield 1
end

def outer(&block: Fn[(Int) -> nil])
  inner(&block)
end

outer { |n| puts n }
outer do |n|
  puts n
end
"#;
    assert_eq!(typeck_codes(source), Vec::<String>::new());
    assert_eq!(
        hir_closure_param_tys(source, "main"),
        vec![vec![Ty::Int], vec![Ty::Int]]
    );
}

/// An argument that cannot be a callable of a closure parameter's `Fn`
/// signature is E0743, for free functions, `.rx` methods on builtin heads and
/// a closure whose declared parameter types differ.
#[test]
fn semantics_non_callable_arg_to_fn_param_is_e0743() {
    for (body, defs) in [
        ("run(5)", "def run(f: Fn(Int) -> nil)\n  f.(1)\nend\n"),
        ("[1, 2].each(5)", ""),
        ("[1, 2].to_set.each(5)", ""),
        ("h = {\"a\" => 1}\nh.each(5)", ""),
        (
            "run(->(s: String) { puts s })",
            "def run(f: Fn(Int) -> nil)\n  f.(1)\nend\n",
        ),
        (
            "f = ->(s: String) { puts s }\nrun(f)",
            "def run(f: Fn(Int) -> nil)\n  f.(1)\nend\n",
        ),
        ("[1, 2].each(->(s: String) { puts s })", ""),
    ] {
        let source = format!("{defs}\n{body}\n");
        assert_eq!(typeck_codes(&source), vec!["E0743"], "`{body}`");
    }
}

/// Callables that do fit a closure parameter stay accepted: a matching
/// lambda, a forwarded block, a stored `Proc[...]`, and blocks whose
/// parameters are inferred from the signature.
#[test]
fn semantics_fitting_callables_to_fn_params_are_accepted() {
    let source = r#"def run(f: Fn(Int) -> nil)
  f.(1)
end

def keep(p: Proc[(Int) -> nil])
  p.(1)
end

def fwd(&block: Fn[(Int) -> nil])
  run(block)
  [1].each(&block)
end

run(->(n: Int) { puts n })
f = ->(n: Int) { puts n }
run(f)
run { |n| puts n }
keep(->(n: Int) { puts n })
[1, 2].each { |n| puts n }
[1, 2].to_set.each { |n| puts n }
h = {"a" => 1}
h.each { |kv| puts kv.0 }
fwd { |n| puts n }
"#;
    assert_eq!(typeck_codes(source), Vec::<String>::new());
}

/// A closure nested in an owning lambda propagates its captures to the
/// lambda: the lambda owns `s` (a move), so reusing `s` afterwards is E1001.
/// The nested block only borrows it.
#[test]
fn semantics_nested_closure_captures_propagate_to_the_owner() {
    let source = |tail: &str| {
        format!(
            "s = \"xyz\"
outer = -> do
  [1].each {{ |i| puts s }}
  s.size
end
puts outer.()
{tail}
"
        )
    };
    assert_eq!(borrow_codes(&source("puts s")), vec!["E1001"]);
    assert_eq!(borrow_codes(&source("nil")), Vec::<String>::new());
}

/// `b.tap(&block)` forwards to the universal `Kernel#tap` default, which only
/// calls its `Fn(&Self)` argument. A competing user `tap` that stores a
/// `Proc[...]` must not govern the call: typeck records the `Kernel` default
/// in `resolved` (asserted directly), and the borrow checker reads that
/// record, so forwarding is not E1018. Forwarding to `Other#tap` itself still
/// stores the block.
#[test]
fn semantics_forwarding_to_kernel_tap_reads_the_recorded_default() {
    let source = |body: &str| {
        format!(
            r#"class Other
  def init
  end
  def tap(p: Proc[(&Bag) -> nil]) -> nil
    nil
  end
end

class Bag
  def init
  end
end

def forward(b: &Bag, o: &Other, &block: Fn[(&Bag) -> nil])
  {body}
  nil
end
"#
        )
    };
    let tap_parent = |src: &str| {
        struct Find(Vec<ruxen_core::hir::nodes::DefId>);
        impl HirVisit for Find {
            fn visit_expr(&mut self, e: &HirExpr) {
                if let HirExprKind::MethodCall {
                    method_name,
                    resolved,
                    ..
                } = &e.kind
                {
                    if method_name == "tap" {
                        self.0.push(*resolved);
                    }
                }
                hir_visit::walk_expr(self, e);
            }
        }
        let result = typeck::type_check(&parse(src).expect("parse"));
        let mut v = Find(Vec::new());
        for item in &result.program.items {
            if let HirItem::Function(f) = item {
                if f.name == "forward" {
                    v.visit_func(f);
                }
            }
        }
        v.0.iter()
            .map(|id| match result.symbols.get(*id).map(|d| &d.kind) {
                Some(ruxen_core::resolve::symbols::DefKind::Method { parent, .. }) => result
                    .symbols
                    .get(*parent)
                    .map(|p| p.name.clone())
                    .unwrap_or_default(),
                // A mixin default is a `Function` whose `Self` is bounded
                // by its mixin.
                Some(ruxen_core::resolve::symbols::DefKind::Function { signature }) => {
                    match &signature.return_ty {
                        ruxen_core::hir::types::Ty::TypeParam { name, bounds }
                            if name == "Self" =>
                        {
                            bounds
                                .iter()
                                .map(|b| b.name.clone())
                                .collect::<Vec<_>>()
                                .join("+")
                        }
                        other => format!("<not a mixin default: {other:?}>"),
                    }
                }
                other => format!("<not a method: {other:?}>"),
            })
            .collect::<Vec<_>>()
    };
    let to_bag = source("b.tap(&block)");
    assert_eq!(tap_parent(&to_bag), vec!["Kernel"]);
    assert_eq!(borrow_codes(&to_bag), Vec::<String>::new());
    let to_other = source("o.tap(block)");
    assert_eq!(tap_parent(&to_other), vec!["Other"]);
    assert_eq!(borrow_codes(&to_other), vec!["E1018"]);
}

/// E1018's help spells the storable type for the block's own signature
/// (`Proc[(Int) -> nil]`, the spelling the parser accepts), not a
/// placeholder.
#[test]
fn semantics_e1018_help_names_the_blocks_proc_type() {
    let source = format!(
        "{BLOCK_TARGETS}
def store(&block: Fn[(Int, String) -> nil])
  b = block
end

def store_ret(f: Fn(Int) -> Int)
  g = f
end
"
    );
    let result = typeck::type_check(&parse(&source).expect("parse"));
    let helps: Vec<String> =
        ruxen_core::borrow_check::borrow_check(&result.program, &result.symbols)
            .iter()
            .map(|e| e.help.join(" "))
            .collect();
    assert_eq!(helps.len(), 2, "{helps:?}");
    assert!(
        helps[0].contains("`Proc[(Int, String) -> nil]`"),
        "help was {:?}",
        helps[0]
    );
    assert!(
        helps[1].contains("`Proc[(Int) -> Int]`"),
        "help was {:?}",
        helps[1]
    );
    // The spelled type parses as a parameter type.
    let reparse = "def keep(p: Proc[(Int, String) -> nil], q: Proc[(Int) -> Int])\n  nil\nend\n";
    assert!(parse(reparse).is_ok());
}

/// Forwarding a `Fn[...]` parameter to a callable VALUE reads the parameter
/// types of the callable's own signature, and forwarding to a mixin
/// requirement with no default body reads the requirement's declared
/// signature: neither stores the block. Forwarding to a `Proc[...]` slot of
/// either still stores it (E1018).
#[test]
fn semantics_forwarding_to_a_callable_or_bodiless_requirement_reads_its_signature() {
    let callable = |invoke_ty: &str| {
        format!(
            "def forward(invoke: {invoke_ty}, f: Fn(Int) -> nil) -> nil
  invoke.(f)
end
"
        )
    };
    assert_eq!(
        borrow_codes(&callable("Fn(Fn(Int) -> nil) -> nil")),
        Vec::<String>::new()
    );
    assert_eq!(
        borrow_codes(&callable("Fn(Proc[(Int) -> nil]) -> nil")),
        vec!["E1018"]
    );
    let requirement = |param_ty: &str| {
        format!(
            "mixin Walk
  def walk(f: {param_ty}) -> nil
end

def drive[T: Walk](x: &T, f: Fn(Int) -> nil) -> nil
  x.walk(f)
end
"
        )
    };
    assert_eq!(
        borrow_codes(&requirement("Fn(Int) -> nil")),
        Vec::<String>::new()
    );
    assert_eq!(
        borrow_codes(&requirement("Proc[(Int) -> nil]")),
        vec!["E1018"]
    );
    // Same-arity overloads: the borrow checker reads the overload typeck
    // selected by argument type, in either declaration order.
    let proc_sig = "  def walk(tag: Int, f: Proc[(Int) -> nil]) -> nil\n";
    let fn_sig = "  def walk(tag: String, f: Fn(Int) -> nil) -> nil\n";
    for sigs in [format!("{proc_sig}{fn_sig}"), format!("{fn_sig}{proc_sig}")] {
        let overloads = |tag: &str| {
            format!(
                "mixin Walk
{sigs}end

def drive[T: Walk](x: &T, f: Fn(Int) -> nil) -> nil
  x.walk({tag}, f)
end
"
            )
        };
        assert_eq!(
            borrow_codes(&overloads("\"s\"")),
            Vec::<String>::new(),
            "{sigs}"
        );
        assert_eq!(borrow_codes(&overloads("1")), vec!["E1018"], "{sigs}");
    }
}

/// A proc or lambda passed to a `Send`-bounded parameter owns its captures,
/// so it borrows no stack local (no E1013); the capture is moved (reuse is
/// E1001). A block borrows and stays E1013 (the mocked-HIR pin in
/// `borrow_check/tests.rs`).
#[test]
fn semantics_owning_closures_to_send_params_capture_by_move() {
    let source = |arg: &str, tail: &str| {
        format!(
            "def go(f: any Fn[Fn() -> Int] + Send) -> Int
  f.()
end

n = 41
s = \"str\"
puts go({arg})
{tail}
"
        )
    };
    for arg in [
        "-> { n + 1 }",
        "proc { n + 1 }",
        "-> { s.size }",
        "proc { s.size }",
    ] {
        assert_eq!(
            borrow_codes(&source(arg, "nil")),
            Vec::<String>::new(),
            "`{arg}`"
        );
    }
    assert_eq!(
        borrow_codes(&source("-> { s.size }", "puts s")),
        vec!["E1001"]
    );
    // An owning closure that captures a reference owns the reference, not
    // the pointee: the stack local can still die before a spawned thread
    // reads it.
    for arg in ["-> { r.size }", "proc { r.size }"] {
        assert_eq!(
            borrow_codes(&source(arg, "nil").replace("s = \"str\"\n", "s = \"str\"\nr = &s\n")),
            vec!["E1013"],
            "`{arg}`"
        );
    }
}

/// `Proc.new() { ... }` with empty parentheses is the same proc literal as
/// `Proc.new { ... }`; it is no call on an undefined `Proc` value.
#[test]
fn syntax_proc_new_with_empty_parens_is_the_proc_literal() {
    let source = "k = 7\nw = \"w-#{k}\"\n\
                  p = Proc.new() { |x: Int| (w.size as Int) + x }\n\
                  q = Proc.new() do |x: Int|\n  x\nend\nputs p.(1) + q.(1)\n";
    let program = parse(source).expect("parse");
    let x = || vec!["x".to_string()];
    assert_eq!(
        ast_closures(&program),
        vec![
            (ClosureSpelling::ProcNew, ClosureKind::Proc, x()),
            (ClosureSpelling::ProcNew, ClosureKind::Proc, x()),
        ]
    );
    assert_eq!(typeck_codes(source), Vec::<String>::new());
}

/// The method home of a stored closure is the class the core library
/// declares `layout closure` (I-19): `p.clone` resolves to a method of that
/// declared class, read off its declaration, never a class name spelled in
/// the compiler.
#[test]
fn semantics_a_proc_method_resolves_on_the_declared_closure_shell() {
    let source =
        "def share(p: Proc[() -> Int]) -> Int\n  q = p.clone\n  q.()\nend\nputs(share(-> { 7 }))\n";
    let result = typeck::type_check(&parse(source).expect("parse"));
    assert!(
        result
            .diagnostics
            .iter()
            .all(|d| d.level != ruxen_core::diagnostics::DiagnosticLevel::Error),
        "{:?}",
        result.diagnostics
    );
    let shell = result
        .symbols
        .closure_shell()
        .expect("std/core declares `layout closure`");
    let mut resolved = Vec::new();
    for item in &result.program.items[result.program.prelude_item_count..] {
        struct Calls<'v>(&'v mut Vec<ruxen_core::hir::nodes::DefId>);
        impl HirVisit for Calls<'_> {
            fn visit_expr(&mut self, expr: &HirExpr) {
                if let HirExprKind::MethodCall {
                    method_name,
                    resolved,
                    ..
                } = &expr.kind
                {
                    if method_name == "clone" {
                        self.0.push(*resolved);
                    }
                }
                hir_visit::walk_expr(self, expr);
            }
        }
        Calls(&mut resolved).visit_item(item);
    }
    let parent_of = |id| match result.symbols.get(id).map(|d| &d.kind) {
        Some(ruxen_core::resolve::symbols::DefKind::Method { parent, .. }) => Some(*parent),
        _ => None,
    };
    assert_eq!(resolved.len(), 1, "{resolved:?}");
    assert_eq!(parent_of(resolved[0]), Some(shell.id));
}
