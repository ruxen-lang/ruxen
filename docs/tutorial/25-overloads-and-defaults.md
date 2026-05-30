# Function Overloads and Optional Arguments

Sometimes you want two functions with the same name that take different argument shapes — one for integers, one for strings, say. Sometimes you want one function with an optional parameter that has a sensible default. Ruxen supports both: **overloads** (multiple definitions of the same name distinguished by parameter types) and **defaults** (trailing parameters with `= value`). They compose freely, work inside classes and modules, and follow simple rules at the call site.

If you've used Python, `def greet(name, prefix="Hello")` should look familiar. The overload side is closer to C++ or Java — same name, different signatures, picked by the compiler at the call site.

---

## 1. Your first default argument

Save as `greet.rx`:

```ruxen
def join_label(name: String, suffix: String = "!") -> String
  "#{name}#{suffix}"
end

def main
  puts join_label("ruxen")
  puts join_label("ruxen", "?")
end
```

Run:

```bash
ruxen run greet.rx
```

Output:

```
ruxen!
ruxen?
```

The `= "!"` after `suffix: String` gives that parameter a default. Callers may pass it or skip it.

Rule: defaults apply only to the **tail** of the parameter list. Once one parameter has a default, every parameter after it must also have one.

## 2. Overloading on argument type

Define the same name multiple times with different parameter types. The compiler picks the matching body based on the static types at the call site:

```ruxen
def classify(value: Int)    -> String { "int" }
def classify(value: String) -> String { "string" }
def classify(value: Bool)   -> String { "bool" }

def main
  puts classify(7)
  puts classify("seven")
  puts classify(true)
end
```

Output:

```
int
string
bool
```

A few rules:

- **No two overloads can have the same parameter shape.** Two `classify(value: Int)` definitions are a conflict.
- **Arity counts.** `classify(a: Int)` and `classify(a: Int, b: Int)` are different signatures and coexist fine.
- **No implicit conversion.** A call `classify(1u8)` (a `UInt8`) will not silently fall through to the `Int` overload — it'd be an error if no `UInt8` overload existed.

Return types can differ across overloads.

## 3. Methods overload the same way

Method overloads work exactly like top-level overloads, and they mix freely with default arguments:

```ruxen
class Picker
  def init end

  def mark(value: String, suffix: String = "!") -> String
    "#{value}#{suffix}"
  end

  def pick(value: Int)    -> String { "int" }
  def pick(value: String) -> String { "string" }
end

def main
  let p = Picker.new
  puts p.mark("ruxen")
  puts p.mark("ruxen", "?")
  puts p.pick(7)
  puts p.pick("seven")
end
```

Output:

```
ruxen!
ruxen?
int
string
```

## 4. Overloads across inheritance

A subclass can add new overloads of an inherited name without losing the parent's:

```ruxen
class ParentPicker
  def init end
  def pick(value: Int)    -> String { "parent-int" }
  def pick(value: String) -> String { "parent-string" }
end

class ChildPicker < ParentPicker
  def init
    super()
  end

  def pick(value: Bool) -> String { "child-bool" }
end

def main
  let c = ChildPicker.new
  puts c.pick(7)
  puts c.pick("seven")
  puts c.pick(true)
end
```

Output:

```
parent-int
parent-string
child-bool
```

The dispatcher checks the receiver class first, then walks up the parent chain. A child *can* shadow a parent overload by redefining the exact same shape.

## 5. Overloads inside modules

Module-scoped functions overload by the same rules:

```ruxen
module Tools
  def pick(value: Int)    -> String { "module-int" }
  def pick(value: String) -> String { "module-string" }
end

def main
  puts Tools.pick(7)
  puts Tools.pick("seven")
end
```

A class nested inside a module has its own overload set:

```ruxen
module Tools
  class Picker
    def init end
    def pick(value: Int)    -> String { "module-class-int" }
    def pick(value: String) -> String { "module-class-string" }
  end
end
```

## 6. Mixin contracts can require multiple overloads

A mixin's required method list can name several overloads with the same name. Including types must satisfy *every* listed overload:

```ruxen
mixin Picker
  def pick(value: Int)    -> String
  def pick(value: String) -> String
end

class Item
  def init end

  include Picker

  def pick(value: Int)    -> String { "mixin-int" }
  def pick(value: String) -> String { "mixin-string" }
end

def show[T: Picker](item: &T)
  puts item.pick(7)
  puts item.pick("seven")
end

def main
  let item = Item.new
  show(&item)
end
```

A generic constrained by `T: Picker` can call both overloads — the mixin bound provides both signatures.

## 7. How the compiler picks an overload

When you write `f(args)`:

1. **Filter by arity.** A parameter with a default counts as optional.
2. **Match by exact parameter types.** Among the surviving candidates, pick the one whose parameter types exactly match the argument types.
3. **If multiple candidates match equally, report an ambiguity error.** Disambiguate by adding a type annotation at the argument site, or by removing one of the overloads.

There's no implicit numeric coercion in overload resolution — `Int` and `Int8` are different and don't auto-convert at the call site.

## 8. Common mistakes

- **Putting a default *before* a non-default parameter.** `def f(a: Int = 0, b: Int)` is rejected — defaults must be trailing. The fix is to reorder so `b` comes first.
- **Two overloads with the same shape.** `def pick(a: Int)` and another `def pick(a: Int)` is a duplicate-definition error. Different names or different shapes.
- **Expecting `Int` to auto-fall-through to a `UInt8` overload.** It won't. Either add an `Int` overload or cast at the call site (`1 as UInt8`).
- **Calling an overloaded method through `any`-style dispatch.** Vtable dispatch (`&any Picker`) picks one specific overload — usually the one whose types are most general. For multi-overload mixins, prefer the static `[T: Picker]` form.

> **Try it:** add `def classify(value: Float) -> String { "float" }` to the example in section 2 and call `classify(1.5)`. Then try `classify(1)` — does it route to the int or the float version?

---

## Recap

- **Defaults** — trailing parameters can have `= value`, callers may skip them.
- **Overloads** — same name, different parameter shapes; compiler picks at the call site.
- Methods, module functions, and inherited methods all overload the same way.
- Mixin contracts can require multiple overloads of one name.
- No implicit numeric coercion in overload resolution — use explicit casts.

**Next:** [Chapter 26 — JSON](26-json.md).
