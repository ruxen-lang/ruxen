//! Fields own their values, reads borrow (plan: fields, card f1; TODO.md
//! Decisions "Fields"). Reading a non-Copy field is a `&T`; that borrow
//! cannot flow into an owned `T` slot (E0744, fix: `.clone`), keeps its
//! object borrowed while it lives (E1009), and storing into a field moves
//! the value in (E1001 on reuse). Diagnostics-only pass: the programs are
//! type- and borrow-checked through the real lexer, parser and typeck, not
//! run, so inline sources are appropriate here. The loan check runs on MIR,
//! so its case runs the whole front end (`driver::check`).

use ruxen_core::borrow_check;
use ruxen_core::diagnostics::DiagnosticLevel;
use ruxen_core::driver;
use ruxen_core::lexer::Lexer;
use ruxen_core::parser::Parser;
use ruxen_core::typeck;

/// Every error as `(code, line)`, typeck first, then borrow check.
fn errors(source: &str) -> Vec<(String, u32)> {
    let tokens = Lexer::new(source).tokenize().expect("lex");
    let program = Parser::new(tokens).parse().expect("parse");
    let result = typeck::type_check(&program);
    let mut out: Vec<(String, u32)> = result
        .diagnostics
        .iter()
        .filter(|d| d.level == DiagnosticLevel::Error)
        .map(|d| {
            (
                d.code.clone().unwrap_or_else(|| d.message.clone()),
                d.span.line,
            )
        })
        .collect();
    out.extend(
        borrow_check::borrow_check(&result.program, &result.symbols)
            .iter()
            .map(|e| (e.code.code_str().to_string(), e.primary.span.line)),
    );
    out
}

const USER: &str = "\
class User
  name: String
  age: Int
  def init(@name: String, @age: Int)
  end
end
";

#[test]
fn borrowed_field_reads_are_usable_where_a_borrow_or_copy_fits() {
    let source = format!(
        "{USER}
class Named
  name: String
  def init(@name: String)
  end
  def name_ref -> &String
    @name
  end
  def name_owned -> String
    @name.clone
  end
end

def show(s: &String)
  puts s
end

def take(s: String)
  puts s
end

u = User.new(\"ada\", 36)
puts u.name
puts u.name.size
show(u.name)
take(u.name.clone)
n = u.age
r = u.name
puts r
w = u
puts w.name
puts n
"
    );
    assert_eq!(errors(&source), vec![]);
}

#[test]
fn borrowed_field_into_an_owned_slot_is_e0744() {
    let source = format!(
        "{USER}
class Box[T]
  v: T
  def init(@v: T)
  end
  def get -> T
    @v
  end
end

class Card
  title: String
  def init(@title: String)
  end
  def title -> String
    @title
  end
end

def take(s: String)
  puts s
end

def show(s: String)
  puts s
end

def show(n: Int, m: Int)
  puts n + m
end

# A typed local (`s: String = v`) is planned; top-level `let` does not parse.
def bind_owned(u: &User)
  let s: String = u.name
end

u = User.new(\"ada\", 36)
take(u.name)
bind_owned(u)
names = Array[String].new
names.push(u.name)
c = Card.new(u.name)
c.title = u.name
show(u.name)
"
    );
    // Line numbers count from the top of the combined source (USER is 6
    // lines, then a blank one).
    assert_eq!(
        errors(&source),
        vec![
            ("E0744".to_string(), 13), // generic getter `-> T` returns `@v: &T`
            ("E0744".to_string(), 22), // `-> String` getter returns `@title`
            ("E0744".to_string(), 40), // `let s: String` in `bind_owned`
            ("E0744".to_string(), 44), // free-fn by-value argument
            ("E0744".to_string(), 47), // method by-value argument (`push(T)`)
            ("E0744".to_string(), 48), // constructor by-value argument
            ("E0744".to_string(), 49), // field store
            ("E0742".to_string(), 50), // no overload takes `&String`
        ]
    );
}

/// Every reference form is a borrow for E0744, `&var` included: storing
/// `&var s` into an owned field made the field and `s` both free one String
/// (I-18, i3). A value whose type does not fit the assignment target is
/// reported, never dropped silently.
#[test]
fn a_mutable_borrow_into_an_owned_slot_is_e0744_and_mismatches_are_reported() {
    let source = format!(
        "{USER}
k = 3
u = User.new(\"old-#{{k}}\", 1)
s = \"shared-#{{k}}\"
u.name = &var s
u.name = &s
u.age = \"x\"
puts u.name
"
    );
    let found = errors(&source);
    assert_eq!(
        found.iter().filter(|(code, _)| code == "E0744").count(),
        2,
        "{found:?}"
    );
    assert!(found.iter().any(|(_, line)| *line == 13), "{found:?}");
    assert!(
        found.iter().all(|(_, line)| [11, 12, 13].contains(line)),
        "{found:?}"
    );
}

#[test]
fn storing_into_a_field_moves_the_value() {
    let source = "\
class Holder
  x: String
  def init(s: String)
    @x = s
    puts s
  end
end
";
    assert_eq!(errors(source), vec![("E1001".to_string(), 5)]);
}

#[test]
fn a_live_field_borrow_keeps_its_object_borrowed() {
    let source = format!(
        "{USER}
def finish(u: User)
  puts u.name
end

u = User.new(\"ada\", 36)
x = u.name
finish(u)
puts x
"
    );
    let frontend = driver::check(
        &driver::parse(&source).expect("parse"),
        &driver::Options::default(),
    );
    let loan_errors: Vec<_> = frontend
        .borrow_errors
        .iter()
        .map(|e| (e.code.code_str(), e.primary.span.line))
        .collect();
    assert_eq!(loan_errors, [("E1009", 14)], "{}", frontend.render(None));
}

/// A closure field is owned like any other (the object frees its pair), so
/// its read is a borrow: it may be called or lent to a block parameter, a
/// second owner takes `.clone` (a shared Proc), and moving it into an owned
/// slot is E0744. A `Fn[...]` block parameter has no `.clone`: it may borrow
/// its captures.
#[test]
fn a_closure_field_read_is_a_borrow_shared_by_clone() {
    let source = "\
class Holder
  cb: Proc[(Int) -> nil]
  def init(@cb: Proc[(Int) -> nil])
  end
end

def run(f: Fn(Int) -> nil) -> nil
  f.(1)
end

def keep(p: Proc[(Int) -> nil]) -> nil
  p.(2)
end

def copy_block(&block: Fn[(Int) -> nil]) -> nil
  c = block.clone
end

# A typed local (`stolen: Proc[...] = v`) is planned; top-level `let` does
# not parse.
def steal(h: &Holder)
  let stolen: Proc[(Int) -> nil] = h.cb
end

h = Holder.new(->(n: Int) { puts n })
h.cb.(3)
run(h.cb)
[5, 6].each(h.cb)
keep(h.cb.clone)
mine = h.cb.clone
keep(h.cb)
steal(h)
";
    let errs = errors(source);
    assert_eq!(
        errs.iter()
            .filter(|(code, _)| code == "E0744")
            .cloned()
            .collect::<Vec<_>>(),
        // `let stolen` in `steal`, then the by-value `keep(h.cb)`.
        vec![("E0744".to_string(), 22), ("E0744".to_string(), 31)],
        "all errors: {errs:?}"
    );
    assert!(
        errs.iter()
            .any(|(msg, line)| *line == 16 && msg.contains("no field or method `clone`")),
        "a block parameter must have no `clone`: {errs:?}"
    );
    assert!(
        errs.contains(&("E1018".to_string(), 16)),
        "the borrow checker still rejects any non-call use of a block: {errs:?}"
    );
    assert_eq!(errs.len(), 4, "unexpected errors: {errs:?}");
}

/// A tuple keeps owning its elements, so reading one is a borrow like a
/// field read: storing `cycle.0` into an owned field, or passing a tuple
/// that holds a borrow where an owned tuple is expected, is E0744 (the fix
/// is `.clone`); with `.clone` both compile.
#[test]
fn a_tuple_element_read_into_an_owned_slot_is_e0744() {
    let source = "class Session
  wire: String
  def init
    @wire = \"\"
  end
end

class User
  name: String
  def init(@name: String)
  end
end

def make(i: Int) -> (String, Int)
  (\"wire-#{i}\", i)
end

def take(p: (String, Int)) -> nil
  puts(p.0)
end

def run(k: Int) -> nil
  s = Session.new
  cycle = make(k)
  s.wire = cycle.0
  u = User.new(\"ann\")
  take((u.name, 1))
  s.wire = cycle.0.clone
  take((u.name.clone, 1))
end

run(1)
";
    let codes: Vec<String> = errors(source).into_iter().map(|(code, _)| code).collect();
    assert_eq!(codes, vec!["E0744", "E0744"], "{codes:?}");
}
