# 3. The Rust code, file by file

This tour follows the code in the order it runs, from the moment Windows loads the DLL to the moment the last patch is
written. Each Rust feature is explained the first time it appears, so later sections assume the earlier ones.

Code excerpts are sometimes shortened. The real files are linked at each step, and reading the two side by side is a
good way to learn.

Two things to have open as you go:

- [The Rust Book](https://doc.rust-lang.org/book/), the official introduction. Chapter references below look like
  *(Book ch. 6)*.
- `cargo doc --open` in this project, which builds HTML documentation for the code and the `windows-sys` functions it
  calls.

---

## Step 0: the project file, `Cargo.toml`

[Cargo.toml](../Cargo.toml) tells **Cargo**, Rust's build tool and package manager, how to build the project.

```toml
[lib]
crate-type = ["cdylib"]
```

A **crate** is Rust's unit of compilation: a library or a program. `cdylib` means "a dynamic library with a C-style
interface", which on Windows means a `.dll`. Without this line Cargo would build a Rust-only library that other Rust
code could use, but that a game couldn't load.

```toml
[dependencies.windows-sys]
version = "0.59"
features = ["Win32_Foundation", "Win32_System_Memory", ...]
```

**Dependencies** are other crates this one uses, downloaded from [crates.io](https://crates.io). `windows-sys` is
Microsoft's official crate of raw declarations for the Windows API (`VirtualProtect`, `GetModuleHandleW`, …). The
Windows API is huge, so the crate is split into **features**, and only the parts listed get compiled. The exact
versions picked are recorded in `Cargo.lock`, so every build uses the same code.

```toml
[profile.release]
panic = "unwind"
lto = true
codegen-units = 1
strip = true
```

Settings for `cargo build --release`. `lto` and `codegen-units = 1` let the compiler optimize across the whole crate,
and `strip` removes debugging symbols to make the DLL smaller. `panic = "unwind"` (the default, written out to be
explicit) matters for a mod: a **panic** is Rust's reaction to a bug it can't recover from, like indexing past the end
of an array. With `unwind`, a panic ends only the thread it happens on, here our worker thread, instead of killing the
whole game.

Commands you'll use:

```sh
cargo build            # debug build, fast to compile  → target/debug/sekiro_camera_fixes.dll
cargo build --release  # optimized build               → target/release/sekiro_camera_fixes.dll
cargo test             # build and run all #[test] functions
cargo clippy           # extra lints: common mistakes and unidiomatic code
cargo fmt              # reformat all code in the standard style
```

---

## Step 1: modules, [src/lib.rs](../src/lib.rs)

```rust
#[macro_use]
mod logfile;
#[macro_use]
mod pattern;
mod config;
mod error;
mod game_data;
mod memory;
mod threads;
```

`lib.rs` is the **crate root**, where the compiler starts. Each `mod name;` pulls in the file `src/name.rs` as a
**module**, a namespace for related code *(Book ch. 7)*. Items in a module are private unless marked `pub`. That's
why you'll see `pub fn` in [memory.rs](../src/memory.rs) for functions `lib.rs` calls, and plain `fn` for helpers only
used inside the file.

`#[...]` is an **attribute**: an annotation that tells the compiler something about the next item.
`#[macro_use]` makes the macros defined in that module (`log!` and `pattern!`) usable in the modules declared after
it. Order matters here: `logfile` and `pattern` come first so `game_data` can use `pattern!`.

```rust
use std::path::{Path, PathBuf};
use windows_sys::Win32::System::Memory::VirtualProtect;
use config::Config;
```

`use` brings names into scope so you can write `Path` instead of `std::path::Path`. `std` is Rust's standard library.

---

## Step 2: the entry point, `DllMain`

```rust
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(
    module: HMODULE,
    reason: u32,
    _reserved: *mut c_void,
) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        unsafe { DisableThreadLibraryCalls(module) };
        let module = Module(module);
        let _ = thread::Builder::new()
            .name("SekiroCameraFixes".into())
            .spawn(move || run(module));
    }
    TRUE
}
```

There's a lot here. Piece by piece:

**`#[unsafe(no_mangle)]`**: normally the compiler renames functions internally ("mangling") so that two functions
called `new` in different modules don't clash. Windows looks for a function called exactly `DllMain`, so this turns
mangling off. It's wrapped in `unsafe(...)` because two exported functions with the same name could break the
program, and the compiler can't check that for you.

**`extern "system"`**: the **calling convention**, the low-level agreement about which registers carry the arguments
and the return value. Rust functions use Rust's own convention by default. Windows calls `DllMain` with its own, so
the function must declare it.

**`unsafe fn`**: see the box below.

**The types.** Windows types appear under their Windows names:

- `HMODULE`, a handle to a loaded module. In `windows-sys` it's a raw pointer.
- `u32`, an unsigned 32-bit integer. Rust integer types are named by sign and size: `u8`, `i32`, `u64`, and `usize`
  for pointer-sized.
- `*mut c_void`, a raw pointer to unknown data.
- `BOOL`, Windows' 32-bit boolean.

An argument named with a leading `_` is one we don't use. The underscore stops the compiler warning about it.

**The return value.** A function returns its last expression if it has no `;`. Here that's `TRUE`. There's no
`return` keyword because none is needed.

> ### What `unsafe` means
>
> Rust's compiler guarantees, for normal code, that you can't read freed memory, follow a dangling pointer, or have
> two threads race on the same data. Some things are outside what it can check: calling into Windows, using raw
> pointers, treating a random address as a slice of bytes. Those need an **`unsafe` block**:
>
> ```rust
> unsafe { DisableThreadLibraryCalls(module) };
> ```
>
> `unsafe` doesn't turn checks off. It means "I, the programmer, have checked the rules the compiler can't".
> The convention, followed throughout this project, is a `// SAFETY:` comment above each block saying *why* it's
> fine.
>
> An **`unsafe fn`** is one that is only safe to call when certain conditions hold. Its doc comment has a
> `# Safety` section listing them, and callers need an `unsafe` block. `DllMain` is `unsafe` because only the Windows
> loader may call it, with a valid module handle. *(Book ch. 20.1)*

**Starting the thread.** `thread::Builder::new().name(...).spawn(...)` is the **builder pattern**: create a
configuration object, adjust it with chained method calls, then finish with an action. `spawn` takes a **closure**,
an anonymous function, written `|args| body`. `move || run(module)` takes no arguments and *moves* `module` into the
closure, because the closure outlives `DllMain`'s local variables *(Book ch. 13.1)*.

`spawn` returns a `Result`, either success or an error *(Book ch. 9.2)*. `let _ = ...` deliberately ignores it:
if a thread can't be started there's nothing useful to do inside `DllMain`.

**`Module` and `Send`.** Why wrap `module` in `Module(module)` first?

```rust
struct Module(HMODULE);

// SAFETY: a module handle is a process-wide value, valid from any thread.
unsafe impl Send for Module {}
```

Rust only lets a value move to another thread if its type implements the **`Send`** trait, a marker meaning "safe to
send between threads". Raw pointers like `HMODULE` are *not* `Send` by default, because the compiler can't know what
they point at. A module handle is valid in every thread, so we wrap it in our own **tuple struct** (a struct with
unnamed fields, accessed as `.0`) and promise, with `unsafe impl`, that it is `Send`. This is the usual way to move a
raw handle into a thread *(Book ch. 16.4)*.

---

## Step 3: the worker thread starts, `run`

```rust
fn run(module: Module) {
    let dll_dir = module_path(module.0)
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_default();
    let mut log = LogFile::create(&dll_dir.join(LOG_FILE));
    log!(log, "SekiroCameraFixes {}", env!("CARGO_PKG_VERSION"));
```

**`Option`.** `module_path` returns `Option<PathBuf>`, which is either `Some(path)` or `None`. Rust has no `null`.
A value that might be missing is an `Option`, and the compiler makes you handle the `None` case *(Book ch. 6.1)*.

The chain handles it without any `if`s:

- `.and_then(|p| ...)` runs the closure only if there's a value. The closure itself returns an `Option`, because
  `parent()` can fail too.
- `.map(Path::to_path_buf)` converts the borrowed parent path into an owned `PathBuf`. Passing a function name like
  `Path::to_path_buf` works anywhere a closure is expected.
- `.unwrap_or_default()` gives the value, or the type's default (an empty path) if it was `None`.

**`let` and `let mut`.** Variables are immutable unless declared `mut`. `log` is `mut` because writing to it changes
it.

**`&`, borrowing.** `&dll_dir.join(LOG_FILE)` passes a *reference*: the function can look at the path but doesn't take
ownership of it. Ownership and borrowing are Rust's central idea *(Book ch. 4)*. In short, every value has exactly one
owner, and when the owner goes away, the value is cleaned up. `&x` lends it out read-only. `&mut x` lends it out with
permission to change it, and only one `&mut` can exist at a time.

**`env!("CARGO_PKG_VERSION")`** is a macro that reads the version from `Cargo.toml` at compile time.

```rust
    let exe_name = exe.file_name().unwrap_or_default();
    if !exe_name.eq_ignore_ascii_case("sekiro.exe") {
        log!(log, "host process is {exe_name:?}, not sekiro.exe; doing nothing");
        return;
    }
```

**Format strings.** `{exe_name:?}` inserts a variable into the text. Plain `{}` uses the type's user-facing
formatting (the `Display` trait), `{:?}` its debugging formatting (the `Debug` trait), `{:p}` prints a pointer,
`{:#x}` prints hex with `0x`, and `{:02X}` prints upper-case hex padded to 2 digits.

```rust
    let Some(image) = Image::main_module() else {
        log!(log, "could not read sekiro.exe image headers");
        return;
    };
```

**`let ... else`**: "if this matches `Some(image)`, bind `image` and carry on, otherwise run the `else` block, which
must leave the function". It's the tidy way to say "I need this value or I'm done" *(Book ch. 6.3)*.

---

## Step 4: logging, [src/logfile.rs](../src/logfile.rs)

```rust
pub struct LogFile {
    file: Option<File>,
    start: Instant,
}
```

A **struct** groups named fields *(Book ch. 5)*. `file` is an `Option<File>`: if the log file couldn't be created,
it's `None` and logging silently does nothing. A mod should never crash a game over a log file.

```rust
impl LogFile {
    pub fn create(path: &Path) -> Self {
        Self { file: File::create(path).ok(), start: Instant::now() }
    }

    pub fn line(&mut self, message: fmt::Arguments<'_>) {
        if let Some(file) = &mut self.file {
            let elapsed = self.start.elapsed().as_secs_f32();
            let _ = writeln!(file, "[{elapsed:7.2}s] {message}");
            let _ = file.flush();
        }
    }
}
```

**`impl`** attaches functions to a type. `create` has no `self`, so you call it as `LogFile::create(...)`, like a
constructor. `Self` means "the type we're implementing", here `LogFile`. `line` takes `&mut self`, so you call it on a
value, `log.line(...)`, and it may change the value.

**`File::create(path).ok()`**: `create` returns `Result<File, io::Error>`, and `.ok()` converts that to
`Option<File>`, dropping the error.

**`if let Some(file) = &mut self.file`** runs the block only when there is a file, and gives `file` a mutable
reference to it.

The log macro:

```rust
macro_rules! log {
    ($log:expr, $($arg:tt)*) => {
        $log.line(format_args!($($arg)*))
    };
}
```

**`macro_rules!`** defines a macro: code that writes code at compile time *(Book ch. 20.5)*. This one says: given an
expression (`$log:expr`), a comma, then any tokens (`$($arg:tt)*`), produce
`$log.line(format_args!(those tokens))`. So `log!(log, "x = {x}")` becomes `log.line(format_args!("x = {x}"))`.

`format_args!` is like `format!` but doesn't build a `String`. It packages the pieces into a `fmt::Arguments` value
that gets written directly into the file. This is how the standard library's own `println!` works.

---

## Step 5: reading settings, [src/config.rs](../src/config.rs)

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    pub disable_auto_rotate: bool,
    pub disable_lockon_reset: bool,
    pub controller: bool,
}
```

**`#[derive(...)]`** asks the compiler to write standard trait implementations automatically. A **trait** is a set of
methods a type promises to have, like an interface in other languages *(Book ch. 10.2)*:

- `Debug` allows `{:?}`, which is how the log prints `Config { disable_auto_rotate: true, … }`.
- `Clone` and `Copy` let a `Config` be duplicated freely, like a number, instead of moved.
- `PartialEq` and `Eq` allow `==`, which the tests use.

```rust
impl Default for Config {
    fn default() -> Self {
        Self { disable_auto_rotate: true, disable_lockon_reset: true, controller: false }
    }
}
```

Here the trait `Default` is implemented by hand, because the defaults aren't all `false`.

```rust
pub fn parse(text: &str) -> (Self, Vec<String>) {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut config = Self::default();
    let mut warnings = Vec::new();
    for (number, raw) in text.lines().enumerate() {
        let number = number + 1;
        let line = raw.split([';', '#']).next().unwrap_or_default().trim();
        if line.is_empty() || line.starts_with('[') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            warnings.push(format!("line {number}: expected `key = value`"));
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let setting = match key.as_str() {
            "disable_auto_rotate" => &mut config.disable_auto_rotate,
            "disable_lockon_reset" => &mut config.disable_lockon_reset,
            "controller" => &mut config.controller,
            _ => {
                warnings.push(format!("line {number}: unknown setting `{key}`"));
                continue;
            }
        };
        let value = value.trim();
        match parse_bool(value) {
            Some(value) => *setting = value,
            None => warnings.push(format!("line {number}: `{value}` is not true/false")),
        }
    }
    (config, warnings)
}
```

**`&str` vs `String`.** `&str` is a borrowed view into text owned by someone else. `String` owns its text and can
grow. `parse` only reads `text`, so it takes `&str`. The warnings are new text, so they're `String`s built with
`format!`.

**The first line.** Some editors start a UTF-8 file with an invisible "byte order mark", the character `U+FEFF`.
`trim()` doesn't remove it, so without `strip_prefix` it would end up glued to the first key. `strip_prefix` returns
`Some(rest)` if the text starts with it, and `unwrap_or(text)` falls back to the whole text otherwise.

**Iterators.** `text.lines()` yields each line, and `.enumerate()` pairs each with its index. `for (number, raw) in …`
**destructures** each pair into two variables *(Book ch. 13.2)*. `raw.split([';', '#']).next()` is the part of the
line before the first comment character, so `controller = true ; gamepad` still works.

**`match`** compares a value against patterns, top to bottom, and runs the first arm that matches. `_` matches
anything. The compiler checks that every possibility is covered *(Book ch. 6.2)*. A `match` is also an expression with
a value. Here each known key gives `&mut` (a **mutable reference**, a pointer you may write through) to the matching
field, and `*setting = value` writes through it. The `_` arm never produces a value: `continue` jumps straight to the
next line. Checking the key before the value means `bogus = maybe` is reported as an unknown setting, the more useful
of its two problems.

**Returning a tuple.** `(Self, Vec<String>)` returns two values at once. The caller unpacks them with
`let (config, warnings) = Config::parse(&text);`.

**Tests at the bottom of the file:**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_file_matches_defaults() {
        let (config, warnings) = Config::parse(DEFAULT_INI);
        assert_eq!(config, Config::default());
        assert!(warnings.is_empty(), "{warnings:?}");
    }
}
```

`#[cfg(test)]` compiles the module only for `cargo test`, so it isn't in the real DLL. `use super::*` imports
everything from the parent module, private items included. Each `#[test]` function is one test. It passes if it
returns normally and fails if it panics, which `assert!` and `assert_eq!` do when their check fails
*(Book ch. 11)*.

This particular test makes sure the default `.ini` file the mod writes says the same thing as `Config::default()`, so
the two can't drift apart.

---

## Step 6: deciding what to patch, `features`

Back in [lib.rs](../src/lib.rs):

```rust
struct Feature {
    name: &'static str,
    sites: Vec<Site>,
}

fn features(config: &Config) -> Vec<Feature> {
    let mut features = Vec::new();
    if config.disable_auto_rotate {
        let mut sites = vec![game_data::CAMADJUST_PITCH, game_data::CAMADJUST_YAW_Z];
        if !config.controller {
            sites.push(game_data::CAMADJUST_PITCH_XY);
        }
        sites.push(game_data::CAMADJUST_YAW_XY);
        features.push(Feature { name: "Disable camera auto rotate on movement", sites });
    }
    // ... same for the lock-on reset
    features
}
```

**`Vec<T>`** is a growable list of `T`. `vec![a, b]` creates one with initial items.

**`&'static str`** is a reference to text that lives for the whole program. A string literal like `"camera pitch"` is
baked into the DLL itself, so it never goes away. `'static` is a **lifetime**: a name for "how long a reference is
valid" *(Book ch. 10.3)*. Lifetimes are how the compiler proves a reference never outlives what it points to.
`'static` is the longest one.

**Field init shorthand.** `Feature { name: "…", sites }` is short for `sites: sites`.

---

## Step 7: what a patch site is, [src/game_data.rs](../src/game_data.rs)

```rust
pub enum Patch {
    Bytes(&'static [u8]),
    Cave { overwrite_len: usize, shellcode: &'static [u8] },
}

pub struct Site {
    pub name: &'static str,
    pub pattern: Pattern,
    pub offset: usize,
    pub expected: &'static [u8],
    pub patch: Patch,
}
```

**Enums with data.** A Rust `enum` is "one of these variants", and each variant can carry its own data
*(Book ch. 6.1)*. A `Patch` is *either* plain bytes *or* a cave with a length and some code. You can only reach the
data through a `match`, so you can't read a cave's `shellcode` from a `Bytes` patch by mistake.

**Slices.** `&'static [u8]` is a **slice**: a view of a run of bytes stored somewhere else, here baked into the DLL
*(Book ch. 4.3)*. A slice knows its own length.

```rust
pub const CAMRESET_LOCKON: Site = Site {
    name: "camera reset on lock-on",
    pattern: pattern!("C6 86 ?? ?? 00 00 ?? F3 0F 10 8E ?? ?? 00 00"),
    offset: 6,
    expected: &[0x01],
    patch: Patch::Bytes(&[0x00]),
};
```

**`const`** defines a value computed at compile time and inlined wherever it's used. These five consts are the whole
"database" of patches.

At the bottom of the file there's an unusual block:

```rust
const _: () = {
    let sites = [CAMADJUST_PITCH, CAMADJUST_YAW_Z, CAMADJUST_PITCH_XY, CAMADJUST_YAW_XY, CAMRESET_LOCKON];
    let mut i = 0;
    while i < sites.len() {
        let site = &sites[i];
        assert!(site.offset + site.expected.len() <= site.pattern.len());
        // ... more checks
        i += 1;
    }
};
```

This is a **compile-time assertion**. A `const` must be evaluated while compiling, so if any `assert!` inside fails,
*the build fails*. The `_` name and `()` type say we don't want a value, only the check. It verifies, for every site,
that the bytes checked before patching lie inside the pattern match and that every cave has room for a 5-byte jump.
(It uses `while` rather than `for` because `for` loops weren't allowed in compile-time code when this was written.)

---

## Step 8: byte patterns, [src/pattern.rs](../src/pattern.rs)

```rust
pub struct Pattern(&'static [Option<u8>]);
```

A pattern is a slice of `Option<u8>`: `Some(0xC6)` for a fixed byte, `None` for a `??` wildcard. Reusing `Option`
this way is common. It already means "a value or nothing".

### Parsing at compile time

The `pattern!` macro turns `"C6 86 ?? 00"` into `[Some(0xC6), Some(0x86), None, Some(0x00)]` *while compiling*:

```rust
macro_rules! pattern {
    ($text:literal) => {{
        const BYTES: [Option<u8>; $crate::pattern::token_count($text)] =
            match $crate::pattern::parse::<{ $crate::pattern::token_count($text) }>($text) {
                Some(bytes) => bytes,
                None => panic!(concat!("invalid byte pattern: ", $text)),
            };
        $crate::pattern::Pattern::new(&BYTES)
    }};
}
```

Several features work together here:

- **`const fn`.** `token_count` and `parse` are `const fn`: functions the compiler can run *during compilation*
  when given constant inputs. Compile-time code has some limits: when this was written it couldn't use `for` loops,
  the `?` operator or heap allocation. That's why `parse` is written with `while` loops and explicit `match`es.
- **Arrays and const generics.** `[Option<u8>; N]` is an **array**, a fixed-size list whose length `N` is part of its
  type. `parse::<N>` is **generic** over that length: `const N: usize` in its signature lets one function work for any
  length *(Book ch. 10.1)*. The macro computes `N` by counting tokens first.
- **A compile-time panic.** If the text is malformed, `parse` returns `None`, the macro hits `panic!`, and since that
  happens inside a `const`, it becomes a *compile error*. A typo in a pattern can never reach the game:

  ```text
  error[E0080]: evaluation panicked: invalid byte pattern: C6 86 ?? ?? 00 00 ?? F3 0F 10 8E ?? ?? 00 0G
  ```

- **`$crate`.** Inside a macro, `$crate::` refers to this crate, so the paths work wherever the macro is used.

### Searching

```rust
pub fn matches(self, haystack: &[u8]) -> impl Iterator<Item = usize> {
    let last_start = haystack.len().checked_sub(self.0.len());
    let mut start = 0;
    std::iter::from_fn(move || {
        let last_start = last_start?;
        while start <= last_start {
            let candidate = match self.0[0] {
                // Anchor on the first fixed byte to skip most positions quickly.
                Some(first) => {
                    start + haystack[start..=last_start].iter().position(|&b| b == first)?
                }
                None => start,
            };
            start = candidate + 1;
            if self.matches_at(haystack, candidate) {
                return Some(candidate);
            }
        }
        None
    })
}
```

`matches` yields *every* match, in order, not just the first. The caller decides which one it wants.

- **Returning an iterator.** `impl Iterator<Item = usize>` means "some iterator of `usize`; the exact type is the
  compiler's business". Callers can loop over it with `for`, or call `.next()` for just the first match. It's
  **lazy**: no searching happens until someone asks for the next item, and a caller that stops early never pays for
  the rest of the search.
- **`std::iter::from_fn`** builds an iterator from a closure that's called once per `next()`. It returns `Some(item)`
  for each item and `None` when there are no more. The closure is `move`, so it owns `start` and remembers it between
  calls: that's where the search resumes.
- **The `?` operator.** On an `Option`, `expr?` means "if this is `None`, return `None` from the function (here the
  closure) right now, otherwise give me the value". `checked_sub` returns `None` if the subtraction would go below
  zero, i.e. the haystack is shorter than the pattern. One `?` handles that case. On a `Result`, `?` does the same
  with errors *(Book ch. 9.2)*.
- **Ranges and slicing.** `start..=last_start` is a range including both ends (`..` excludes the end).
  `haystack[range]` is a sub-slice. Indexing out of bounds panics instead of reading garbage memory, which is one of
  Rust's safety guarantees.
- **Iterator adapters.** `.iter().position(|&b| b == first)` finds the index of the first matching byte. `|&b|` is a
  closure whose argument pattern `&b` unpacks the reference, so `b` is a plain `u8`.

```rust
fn matches_at(&self, haystack: &[u8], start: usize) -> bool {
    self.0
        .iter()
        .zip(&haystack[start..start + self.0.len()])
        .all(|(want, &have)| want.is_none_or(|w| w == have))
}
```

`.zip` walks two sequences side by side as pairs. `.all` is true if the closure is true for every pair.
`want.is_none_or(...)` reads naturally: the pattern byte is a wildcard, *or* it equals the game's byte. In a release
build, a chain like this usually compiles to the same machine code as a hand-written loop.

---

## Step 9: finding the game's code, [src/memory.rs](../src/memory.rs), `Image`

```rust
pub struct Image {
    base: *mut u8,
    size: usize,
    code: Vec<Range<usize>>,
}
```

**Raw pointers.** `*mut u8` is a **raw pointer** to bytes. Unlike references (`&`, `&mut`), raw pointers have no
borrow checking and can be null or dangling. You may create and copy them freely, but reading or writing through one
needs `unsafe`. They're how Rust talks about memory it doesn't own, like the game's.

`Range<usize>` is the type behind `a..b`. Each one is a code section's start and end, measured from `base`.

### Reading the PE headers

```rust
pub fn main_module() -> Option<Self> {
    unsafe {
        let base: *mut u8 = GetModuleHandleW(ptr::null()).cast();
        if base.is_null() { return None; }
        let dos = base.cast::<IMAGE_DOS_HEADER>().read_unaligned();
        if dos.e_magic != IMAGE_DOS_SIGNATURE { return None; }
        let nt_ptr = base.add(usize::try_from(dos.e_lfanew).ok()?);
        let nt = nt_ptr.cast::<IMAGE_NT_HEADERS64>().read_unaligned();
        // ... then the section table, keeping sections flagged IMAGE_SCN_MEM_EXECUTE
    }
}
```

- `GetModuleHandleW(null)` returns the `.exe`'s base address. `.cast()` reinterprets the pointer as a different
  pointee type, from "unknown data" to "bytes" to "a DOS header".
- `.read_unaligned()` copies a whole struct out of memory. "Unaligned" because some types must sit at addresses that
  are multiples of 4 or 8, and nothing guarantees the headers do. A normal read of a misaligned value is undefined
  behaviour, so the code doesn't assume alignment.
- `usize::try_from(x).ok()?` converts between integer types, returning `None` if the value doesn't fit (a negative
  offset, say). Rust never silently converts between integer types, so every conversion is visible.
- The `e_magic` and `Signature` checks make sure this really is a PE file before trusting any other field.

The section table is read with an iterator chain:

```rust
let mut code: Vec<Range<usize>> = (0..usize::from(nt.FileHeader.NumberOfSections))
    .map(|i| sections.add(i).read_unaligned())
    .filter(|section| section.Characteristics & IMAGE_SCN_MEM_EXECUTE != 0)
    .map(|section| {
        let start = section.VirtualAddress as usize;
        start..start + section.Misc.VirtualSize as usize
    })
    .collect();
```

Read it as a pipeline: for each section index, read the header, keep only executable ones, turn each into a range,
and collect into a `Vec`. `&` is bitwise AND, which tests whether a flag bit is set. `as usize` is an integer
conversion that can't lose information here (`u32` to `usize` on 64-bit).

### Searching it

```rust
pub fn find(&self, pattern: Pattern, mut accept: impl FnMut(&[u8]) -> bool) -> Option<*mut u8> {
    const CHUNK: usize = 1 << 20;
    // Consecutive chunks overlap so a match straddling their border is still seen.
    let overlap = pattern.len() - 1;
    let mut buffer = Vec::new();
    for section in &self.code {
        for run in self.readable_runs(section.clone()) {
            let mut start = run.start;
            while start < run.end {
                buffer.resize((CHUNK + overlap).min(run.end - start), 0);
                if read_into(self.base.wrapping_add(start), &mut buffer) {
                    for offset in pattern.matches(&buffer) {
                        if accept(&buffer[offset..offset + pattern.len()]) {
                            return Some(self.base.wrapping_add(start + offset));
                        }
                    }
                }
                start += CHUNK;
            }
        }
    }
    None
}
```

- **Taking a closure.** `accept` is a closure the caller passes in: given the matched bytes, it says whether this
  match will do. `impl FnMut(&[u8]) -> bool` means "any closure with that shape" *(Book ch. 13.1)*. `apply` passes one
  that compares the site's bytes with `expected`, so a match where they differ is skipped. Tests pass `|_| true`, "any
  match".
- **Copying instead of borrowing.** The game's code is never turned into a Rust slice where it lies. A `&[u8]` is a
  promise that nobody writes to those bytes while the slice exists, and the game's anti-tamper layer does rewrite
  its own code at run time. So `read_into` copies each chunk into `buffer`, which the mod owns, with Windows'
  `ReadProcessMemory`. That call also checks that the memory is readable, and fails instead of crashing if it isn't.
- **One buffer, reused.** `buffer.resize` grows or shrinks the same `Vec` for each chunk instead of allocating a new
  one each time.

`readable_runs` asks Windows about each memory region:

```rust
fn query(address: *const u8) -> Option<MEMORY_BASIC_INFORMATION> {
    let mut info = mem::MaybeUninit::<MEMORY_BASIC_INFORMATION>::uninit();
    let size = mem::size_of::<MEMORY_BASIC_INFORMATION>();
    unsafe {
        (VirtualQuery(address.cast(), info.as_mut_ptr(), size) == size).then(|| info.assume_init())
    }
}
```

**`MaybeUninit`.** Windows functions often fill in a struct you give them. Rust normally refuses to let you touch an
uninitialized variable, so `MaybeUninit` is an explicit "this memory isn't filled yet" wrapper. After `VirtualQuery`
reports success, `assume_init()` says "now it is". `bool::then` turns `true` into `Some(value)` and `false` into
`None`.

---

## Step 10: checking the sites, `apply`

Back in [lib.rs](../src/lib.rs). Each site is looked up first:

```rust
enum Location {
    Verified(*mut u8),
    Unexpected(*mut u8),
    Missing,
}

fn locate(site: &Site, image: &Image) -> Location {
    let expected = site.offset..site.offset + site.expected.len();
    let verified = image.find(site.pattern, |bytes| {
        bytes[expected.clone()] == *site.expected
    });
    if let Some(found) = verified {
        return Location::Verified(found.wrapping_add(site.offset));
    }
    match image.find(site.pattern, |_| true) {
        Some(found) => Location::Unexpected(found.wrapping_add(site.offset)),
        None => Location::Missing,
    }
}
```

This is the safety check from Part 1. The first search accepts only a match whose bytes at the site are exactly
`expected`. If there is none, a second search with `|_| true` tells apart "the pattern is there, but the code isn't
what the patch was written for" (`Unexpected`) from "the pattern isn't there at all" (`Missing`). The two are handled
differently: the first is a permanent failure, the second is worth retrying.

The closure **captures** `expected` and `site` from the surrounding function: it can use them without taking them as
arguments. `bytes[expected.clone()]` slices the matched bytes. The compile-time assertion from Step 7 guarantees the
`expected` range lies inside the match, so this never panics. `wrapping_add(offset)` moves a pointer forward by
`offset` bytes. See Step 12 for why it's not `+`.

```rust
fn apply(feature: &Feature, image: &Image) -> Outcome {
    let locations: Vec<Location> = feature
        .sites
        .iter()
        .map(|site| locate(site, image))
        .collect();
    if locations.iter().any(|l| matches!(l, Location::Missing)) {
        return Outcome::NotFound;
    }

    let mut addresses = Vec::with_capacity(locations.len());
    for (site, location) in feature.sites.iter().zip(locations) {
        match location {
            Location::Verified(address) => addresses.push(address),
            Location::Unexpected(address) => {
                return Outcome::Failed(Error::UnexpectedBytes { site: site.name, address, /* … */ });
            }
            Location::Missing => unreachable!("checked above"),
        }
    }
    // ... patch_sites ...
}
```

- **`matches!(value, pattern)`** is a macro that is `true` if the value fits the pattern, a one-line `match`.
- **`unreachable!`** panics with a message. It documents a case the code above has already ruled out. The compiler
  can't see that, and `match` must cover every variant.
- **`.zip(locations)`** takes `locations` by value, so the loop gets each `Location` itself rather than a reference.

The result type:

```rust
enum Outcome {
    NotFound,
    Applied(Vec<*mut u8>),
    Failed(Error),
}
```

Three results, each carrying what matters for it. That's clearer than juggling `Option`s and `Result`s, and `run`
handles each case with a `match`.

---

## Step 11: errors, [src/error.rs](../src/error.rs)

```rust
#[derive(Debug)]
pub enum Error {
    UnexpectedBytes { site: &'static str, address: *const u8, found: Vec<u8>, expected: &'static [u8] },
    NoCaveMemory,
    CavePageFull,
    OutOfJumpRange,
    Protect { address: *const u8 },
    CodeInUse,
    PartlyApplied { cause: Box<Error> },
}
```

`PartlyApplied` holds another `Error`, the one that started the trouble. An enum can't contain itself directly,
because its size would be infinite, so the inner error is behind a **`Box`**: a pointer to a value on the heap
*(Book ch. 15.1)*. The box has a fixed size whatever it points to.

The idiomatic error type is an enum listing every way things can go wrong. Code that gets an `Error` can `match` on
it, and a new kind of failure is a new variant the compiler makes you handle.

To print an error as text, the type implements the **`Display`** trait:

```rust
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoCaveMemory => f.write_str("no free memory within jump range of the game's code"),
            Self::Protect { address } => write!(f, "VirtualProtect failed at {address:p}"),
            // ...
        }
    }
}

impl std::error::Error for Error {}
```

After that, `{error}` in a format string works. Implementing `std::error::Error` (empty, the defaults are fine) marks
the type as a proper error that works with the rest of the Rust ecosystem.

The same file has a small helper:

```rust
pub struct Hex<'a>(pub &'a [u8]);

impl fmt::Display for Hex<'_> { /* writes "F3 0F 11" */ }
```

This is the **newtype pattern**: wrap an existing type in a tuple struct to give it different behaviour, here a
different way to print. `<'a>` is a **lifetime parameter**: `Hex` holds a borrowed slice, and the lifetime ties the
`Hex` to the data it borrows, so it can't outlive it. `Hex<'_>` lets the compiler infer it.

---

## Step 12: building and writing the patches

### Pointers and "provenance"

You'll notice the code is fussy about pointers versus plain numbers. It uses `ptr.addr()` to get a pointer's numeric
address, `ptr.wrapping_add(n)` instead of `+`, and `ptr::without_provenance(number)` in one place.

The reason: Rust (like C) tracks not just *where* a pointer points, but *which allocation it came from*, its
**provenance**. Turning a number back into a pointer loses that, and the compiler's optimizer may then make wrong
assumptions. The rules this code follows:

- **Keep pointers as pointers.** Every address used to read or write starts from a pointer Windows handed out
  (`GetModuleHandleW`, `VirtualAlloc`) and is moved around with pointer methods, so its provenance is never lost.
- **Use `.addr()` when only the number matters**, like computing a jump distance.
- **`ptr::without_provenance(n)`** makes a pointer that must never be read through. It's used for addresses that are
  only *hints* to Windows, like "allocate near here".
- **`wrapping_add` vs `add`.** `add` is `unsafe` and promises the result stays inside the same allocation.
  `wrapping_add` is safe and promises nothing. The actual read or write later is where safety gets checked.

### Jump encoding: pure functions

```rust
pub fn jmp_rel32(from: usize, to: usize) -> Option<[u8; 5]> {
    let next = i64::try_from(from).ok()?.checked_add(5)?;
    let displacement = i32::try_from(i64::try_from(to).ok()?.checked_sub(next)?).ok()?;
    let mut out = [0xE9, 0, 0, 0, 0];
    out[1..].copy_from_slice(&displacement.to_le_bytes());
    Some(out)
}
```

This is the formula from [Part 2](2-how-patching-works.md#the-jump-jmp-rel32) in code. The `i32::try_from` is the
"within ±2 GB" check: if the distance doesn't fit in 32 bits, it returns `None`. `to_le_bytes()` gives the
little-endian bytes. Every step that could overflow is `checked_` and `?`-ed, so bad input gives `None` instead of
a wrong jump.

It and its siblings `cave_code` and `hook_code` take plain numbers and return bytes. They don't touch memory, so the
unit tests check them directly with easy numbers like `0x1000`. Keeping the "think" parts free of side effects and
the "do" parts small is a good habit in any language.

```rust
pub fn cave_code(cave: usize, shellcode: &[u8], return_to: usize) -> Result<Vec<u8>, Error> {
    let jmp = jmp_rel32(cave + shellcode.len(), return_to).ok_or(Error::OutOfJumpRange)?;
    Ok([shellcode, &jmp].concat())
}
```

`.ok_or(error)` turns an `Option` into a `Result`, so `?` can return a proper error. `[a, b].concat()` joins two
slices into a new `Vec`.

### The cave page: ownership and `Drop`

```rust
pub struct CavePage {
    base: NonNull<u8>,
    size: usize,
    used: usize,
}
```

`NonNull<u8>` is a raw pointer guaranteed not to be null. `VirtualAlloc`'s result is checked once, and the type
remembers the check.

```rust
impl Drop for CavePage {
    fn drop(&mut self) {
        unsafe { VirtualFree(self.base.as_ptr().cast(), 0, MEM_RELEASE) };
    }
}
```

**`Drop` and RAII.** When a value's owner goes away (the variable goes out of scope, or an error makes the function
return early), Rust calls its `drop` method automatically. This is **RAII** ("resource acquisition is
initialization"): tie a resource to a value, and cleanup can't be forgotten *(Book ch. 15.3)*. `File` closes itself
this way, `Vec` frees its memory this way, and `CavePage` returns its page to Windows this way.

But once the game jumps into the page, it must *never* be freed:

```rust
pub fn seal(&self) -> Result<(), Error> {
    // ... VirtualProtect to read + execute ...
}

pub fn keep(self) {
    mem::forget(self);
}
```

`seal` takes `&self`: it changes the page's protection but leaves the caller owning it. If the writes that follow
fail and are undone, the caller can still let the page drop and be freed.

`keep` takes `self`, not `&self`: it takes **ownership** of the page, so the caller can't use it afterwards. The
compiler enforces that. `mem::forget(self)` then consumes it *without* running `drop`. The page is intentionally kept
forever. Every path out of `patch_sites` either calls `keep` or drops the page, and ownership makes it impossible to do
both or neither.

The same idea, on a smaller scale, makes code writable just for the duration of a write:

```rust
pub unsafe fn write_code(address: *mut u8, bytes: &[u8]) -> Result<(), Error> {
    let end = address.addr() + bytes.len();
    let mut guards: [Option<Unprotect>; 2] = [None, None];
    let mut cursor = address;
    for guard in &mut guards {
        if cursor.addr() >= end {
            break;
        }
        let info = query(cursor).ok_or(Error::Protect { address: cursor })?;
        let region_end = (info.BaseAddress.addr() + info.RegionSize).min(end);
        *guard = Some(Unprotect::new(cursor, region_end - cursor.addr(), PAGE_EXECUTE_READWRITE)?);
        cursor = cursor.with_addr(region_end);
    }
    if cursor.addr() < end {
        return Err(Error::Protect { address: cursor });
    }
    unsafe {
        ptr::copy_nonoverlapping(bytes.as_ptr(), address, bytes.len());
        FlushInstructionCache(GetCurrentProcess(), address.cast(), bytes.len());
    }
    Ok(())
}
```

`Unprotect::new` makes the memory writable and remembers the old protection. Its `Drop` puts the old protection back.
The guards live in `guards` until the end of the function, so protection is restored however the function exits.

A patch might cross from one memory region into the next, and the two can have different protections. So the loop
makes one guard per region, each remembering its own region's protection. All of them are in place before any byte is
copied: if the second `VirtualProtect` fails, `?` returns, the first guard restores its region, and nothing was
written.

The guards are kept in a fixed-size **array** of two `Option`s, not a `Vec`, because this function must not allocate
(the next section explains why). Two is enough for any patch smaller than a page. A range that would need more is
refused. `cursor.with_addr(n)` is a pointer to address `n` with `cursor`'s provenance.

`ptr::copy_nonoverlapping` is Rust's `memcpy`.

### Pausing the game's threads, [src/threads.rs](../src/threads.rs)

The game keeps running while the mod patches it. A game thread could be paused by Windows halfway through the bytes
being replaced, and resume into a half-written instruction. So before writing, the mod suspends every other thread:

```rust
pub struct Suspended {
    handles: Vec<HANDLE>,
    _exclusive: MutexGuard<'static, ()>,
}

impl Drop for Suspended {
    fn drop(&mut self) {
        for &handle in self.handles.iter().filter(|handle| !handle.is_null()) {
            unsafe {
                ResumeThread(handle);
                CloseHandle(handle);
            }
        }
    }
}
```

`Suspended::others()` lists the process's threads with `CreateToolhelp32Snapshot`, opens each one and calls
`SuspendThread`. The `Drop` resumes them, so they can't be left suspended, even if the code in between returns early.

`any_inside` asks Windows where each suspended thread will continue (`GetThreadContext`, which returns the `Rip`
register, the address of the next instruction) and checks whether that's inside one of the sites.

**No allocation while suspended.** A suspended thread may be in the middle of allocating memory, holding the lock
that protects the heap. If our thread then allocated, it would wait for that lock forever, and the game would freeze.
So `others()` opens every handle (which builds a `Vec`) *before* suspending the first thread, and nothing allocates
until the threads are resumed. Rust can't check this rule for us; the comments state it where it applies.

**`MutexGuard`.** If two threads ran `others()` at the same moment, each could suspend the other, and both would stop
for good. This can only happen in the tests, which run in parallel, but it's cheap to prevent: `others()` first locks
a `static Mutex`, and the `MutexGuard` it gets back unlocks it when dropped *(Book ch. 16.3)*. Struct fields drop after
the `drop` method body runs, so the lock is released only after every thread has been resumed.

**`#[repr(C, align(16))]`.** `GetThreadContext` needs its `CONTEXT` buffer at an address that's a multiple of 16,
and the `windows-sys` declaration doesn't guarantee that. Wrapping it in a one-field struct with `align(16)` does.

### Putting it together: `patch_sites`

```rust
struct Write {
    address: *mut u8,
    new: Vec<u8>,
    old: &'static [u8],
}

fn patch_sites(sites: &[Site], addresses: &[*mut u8]) -> Result<(), Error> {
    let mut page: Option<CavePage> = None;
    let mut writes = Vec::with_capacity(sites.len());
    for (site, &address) in sites.iter().zip(addresses) {
        let new = match site.patch {
            Patch::Bytes(bytes) => bytes.to_vec(),
            Patch::Cave { overwrite_len, shellcode } => {
                let page = match &mut page {
                    Some(page) => page,
                    None => page.insert(CavePage::allocate(address)?),
                };
                let cave = page.add(shellcode, address.wrapping_add(overwrite_len))?;
                memory::hook_code(address.addr(), cave.addr(), overwrite_len)?
            }
        };
        let old = &site.expected[..new.len()];
        writes.push(Write { address, new, old });
    }
    if let Some(page) = &page {
        page.seal()?;
    }
    match write_all(&writes) {
        Ok(()) => {
            if let Some(page) = page {
                page.keep();
            }
            Ok(())
        }
        Err((cause, false)) => Err(cause),
        Err((cause, true)) => {
            if let Some(page) = page {
                page.keep();
            }
            Err(Error::PartlyApplied { cause: Box::new(cause) })
        }
    }
}
```

This is the order of operations from [Part 2](2-how-patching-works.md#order-of-operations) as code:

1. **Prepare.** For each site, work out the bytes to write. A byte patch is just its bytes. A cave patch builds the
   cave, then the jump into it. Nothing in the game changes yet: the results go into `writes`, together with the
   game's original bytes (`old`), which are the verified `expected` bytes.
2. **Seal** the cave page, if any caves were made.
3. **Write** everything with `write_all`, which suspends the game's threads, retries if one of them is stopped inside
   a site, and on a failed write puts back the sites already written.
4. **Keep** the page if the game now jumps into it. Otherwise it's dropped and freed.

`write_all` returns `Result<(), (Error, bool)>`: on failure, the error plus a flag saying whether putting the old
bytes back failed too. A **tuple** is a quick way to return two things without naming a new type.

Details worth noticing:

- **Lazy creation.** The page is only allocated if a cave is needed. The lock-on feature never allocates one.
  `match &mut page { Some(page) => page, None => page.insert(...) }` means "use the existing page, or create one and
  use that". `Option::insert` stores the value and returns a reference to it.
- **`?` everywhere.** Any failure while preparing returns the error at once, and `page` is dropped on the way out and
  its memory freed, thanks to `Drop`.
- **`match` on `site.patch`** destructures the enum, giving direct names to `overwrite_len` and `shellcode`.
- **Patterns in `match` arms** can hold literal values: `Err((cause, false))` matches only when the flag is `false`.

---

## Step 13: the retry loop and ownership

Finally, the loop in `run` that ties it all together:

```rust
for attempt in 1..=SEARCH_ATTEMPTS {
    let mut not_found = Vec::new();
    for feature in pending {
        match apply(&feature, &image) {
            Outcome::NotFound => not_found.push(feature),
            Outcome::Applied(addresses) => { /* log each site */ }
            Outcome::Failed(error) => log!(log, "{}: NOT applied: {error}", feature.name),
        }
    }
    pending = not_found;
    if pending.is_empty() {
        return;
    }
    if attempt < SEARCH_ATTEMPTS {
        thread::sleep(SEARCH_INTERVAL);
    }
}
```

This loop shows ownership in action:

- `for feature in pending` **moves** each `Feature` out of `pending`. After the loop starts, `pending` can't be used:
  the compiler would reject it, because its contents have been given away.
- Features that weren't found are **moved** into `not_found`. Applied or failed ones aren't moved anywhere, so they're
  dropped at the end of their loop iteration.
- `pending = not_found` gives the variable a new value: just the ones to retry.

No feature is copied, and none can be accidentally processed twice or lost. The types make that impossible.

---

## Step 14: the module path, and Windows strings

```rust
fn module_path(module: HMODULE) -> Option<PathBuf> {
    let mut buffer = vec![0u16; MAX_PATH as usize];
    loop {
        let capacity = u32::try_from(buffer.len()).ok()?;
        let len = unsafe { GetModuleFileNameW(module, buffer.as_mut_ptr(), capacity) } as usize;
        if len == 0 {
            return None;
        }
        if len < buffer.len() {
            return Some(OsString::from_wide(&buffer[..len]).into());
        }
        buffer.resize(buffer.len() * 2, 0);
    }
}
```

Windows uses **UTF-16** text (`u16` per unit, hence the `W` for "wide" in `GetModuleFileNameW`), while Rust's `String`
is UTF-8. `OsString` is Rust's type for "text the way the operating system has it", and `from_wide` builds one from
UTF-16 without losing anything. `.into()` converts it to a `PathBuf`. `into` works between any two types that declare
a conversion with the `From` trait.

`loop` repeats forever until a `return`. If the path didn't fit, Windows fills the whole buffer, so the code doubles
the buffer and tries again.

---

## Step 15: the end-to-end test

The most interesting test is `yaw_cave_keeps_old_value` at the bottom of [lib.rs](../src/lib.rs). It assembles a tiny
function by hand, as bytes:

```rust
let mut function = vec![
    0x56,                   // push rsi
    0x48, 0x89, 0xCE,       // mov rsi,rcx
    0xF3, 0x0F, 0x10, 0x02, // movss xmm0,[rdx]
];
function.extend_from_slice(game_data::CAMADJUST_YAW_Z.expected); // movss [rsi+174],xmm0
function.extend_from_slice(&[0x5E, 0xC3]); // pop rsi; ret
```

It puts the function in executable memory, then turns the address into something callable:

```rust
let call = unsafe {
    std::mem::transmute::<*const u8, extern "win64" fn(*mut u8, *const f32)>(entry)
};
```

`transmute` reinterprets the bits of one type as another, here "pointer to bytes" as "function pointer". It's one of
the most dangerous operations in Rust, and fine here because we wrote those exact bytes as a valid function.
`extern "win64"` is the Windows 64-bit calling convention: first argument in `rcx`, second in `rdx`, matching the
assembly above.

Then it calls the function before and after running the real `patch_sites` on it, and checks that the "yaw" in a fake
camera buffer changes before the patch and doesn't change after. So the code-cave machinery is tested by actually
running machine code, without the game.

Next to it, `failed_write_restores_earlier_sites` patches two sites, the second at an address that's never mapped
(the lowest 64 KB of the address space). The second write fails, and the test checks that the first site's bytes are
back to what they were.

---

## Where to go next

- **Try changing things.** Add a test, rename a setting, break a pattern on purpose and read the compiler error. Run
  `cargo test` and `cargo clippy` after each change.
- **Read the Book chapters** referenced above, especially 4 (ownership), 6 (enums and `match`), 9 (errors), 10
  (generics, traits, lifetimes), 13 (closures and iterators), 15 (`Drop`) and 20 (unsafe, macros).
- **[Rust by Example](https://doc.rust-lang.org/rust-by-example/)** has short runnable examples of everything here.
- **[The Rustonomicon](https://doc.rust-lang.org/nomicon/)** is the guide to `unsafe` Rust. Read it once the Book
  feels comfortable, since this project relies on what it covers.
