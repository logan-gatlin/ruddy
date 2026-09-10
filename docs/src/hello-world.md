---
doc: true
---

# Hello World!

This walkthrough creates a simple program that prints `Hello, world!` using Ruddy.
The [download page](download.md) covers installation of Ruddy and Node.js.
The `ruddy` and `node` commands must be available in the terminal.

## Create a project

The following commands create a new `hello` directory and enter it:

```sh
ruddy new hello
cd hello
```

The directory must not already exist.
All remaining commands run from inside `hello`.
The project contains these files:

| File | Purpose |
| --- | --- |
| `Ruddy.toml` | Project configuration |
| `src/main.rud` | The program entry point file |
| `.gitignore` | Excludes generated files in `build/` from Git |

The command also initializes a Git repository.
The generated configuration selects an executable with `src/main.rud` as its root source file and JavaScript as its output target.
No configuration changes are needed.

## Write the greeting

Edit `src/main.rud` so it contains this program:

```ruddy
let main = fn _ => println "Hello, world!"
```

The `main` function is called with a [unit](dictionary.md#unit) parameter when the program first starts.
This function defines its argument as `_`, meaning it is ignored.
The [standard library](dictionary.md#standard-library) provides `println`, which writes a string followed by a newline and returns `()`.

Functions can may have [effects](dictionary.md#effect).


## Build and run

To builds the program without running it:

```sh
ruddy build
```

To build and run the program using `node`:

```sh
ruddy run
```

The program prints:

```text
Hello, world!
```

## Try FizzBuzz

The project can also print FizzBuzz for the natural numbers from 1 through 100.
Replace `src/main.rud` with this program:

```ruddy
let divisible_by = fn number divisor => nat::is_zero (nat::remainder number divisor)

let fizzbuzz = fn number =>
  if divisible_by number 15n then "FizzBuzz"
  else if divisible_by number 3n then "Fizz"
  else if divisible_by number 5n then "Buzz"
  else str::from_nat number
  end

let count = fn number => do
  _ = println (fizzbuzz number)
  return if nat::less_than number 100n then count (nat::add number 1n) else () end
end

let main = fn _ => count 1n
```

The suffix `n` marks each number as a [natural number literal](grammar.md#literals).
The `divisible_by` function uses `nat::remainder` and `nat::is_zero` to test whether a number divides evenly.
The [conditional expression](grammar.md#blocks-and-choices) checks 15 first so numbers divisible by both 3 and 5 produce `FizzBuzz`.
The `count` function prints one result and calls itself with the next number until it reaches 100.
The `main` function starts the count at 1.
Running `ruddy run` now begins with:

```text
1
2
Fizz
4
Buzz
Fizz
7
8
Fizz
Buzz
11
Fizz
13
14
FizzBuzz
```
