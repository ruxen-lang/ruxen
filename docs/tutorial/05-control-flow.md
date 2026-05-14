# Control Flow

## If / Elsif / Else

`if` is an expression — it returns a value:

```riven
let result = if x > 0
  "positive"
elsif x < 0
  "negative"
else
  "zero"
end
```

Note: Riven uses `elsif`, not `else if` or `elif`.

## Match

Pattern matching is exhaustive — the compiler ensures all cases are covered:

```riven
match value
  0     -> "zero"
  1     -> "one"
  n     -> "other: #{n}"
end
```

### Enum Matching

```riven
match status
  Status.Pending            -> handle_pending()
  Status.InProgress(who)    -> puts "Assigned: #{who}"
  Status.Completed(date)    -> puts "Done: #{date}"
  Status.Cancelled(reason)  -> puts "Cancelled: #{reason}"
end
```

### Match with Guards

```riven
match score
  n if n >= 90 -> "A"
  n if n >= 80 -> "B"
  n if n >= 70 -> "C"
  _            -> "F"
end
```

### Or Patterns

```riven
match day
  "Saturday" | "Sunday" -> "weekend"
  _                     -> "weekday"
end
```

### Destructuring

```riven
match point
  (0, 0)    -> "origin"
  (x, 0)    -> "on x-axis at #{x}"
  (0, y)    -> "on y-axis at #{y}"
  (x, y)    -> "at (#{x}, #{y})"
end
```

## If-Let / While-Let

Combine pattern matching with control flow:

```riven
if let Some(user) = find_user(42)
  puts user.name
end

while let Some(item) = queue.pop
  process(item)
end
```

## While Loops

```riven
var i = 0
while i < 10
  puts i
  i += 1
end
```

## For Loops

Iterate over anything iterable:

```riven
for item in collection
  puts item
end

for i in 0..10
  puts i            # 0 through 9
end

for i in 0..=10
  puts i            # 0 through 10 (inclusive)
end
```

## Loop (Infinite)

```riven
loop
  let input = read_line()
  if input == "quit"
    break
  end
  process(input)
end
```

## Break and Continue

```riven
for n in 0..100
  if n % 2 == 0
    continue          # skip even numbers
  end
  if n > 50
    break             # stop at 50
  end
  puts n
end
```

## Blocks as Expressions

Every block in Riven is an expression. The last value is the result:

```riven
let value = do
  let x = compute()
  let y = transform(x)
  x + y
end
```
