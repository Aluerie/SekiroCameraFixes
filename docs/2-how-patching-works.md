# 2. How patching works

[Part 1](1-what-the-patches-do.md) covered *what* bytes change. This part covers *how*: how a DLL ends up inside the
game, how it finds and changes code that Windows normally protects, and how a code cave is built.

## Getting inside the game: DLLs and `DllMain`

A **DLL** (dynamic-link library) is a file of compiled code that a program loads while it runs. Windows itself is
mostly DLLs. Once a DLL is loaded into a process, its code runs *inside* that process and can read and write the
process's memory directly, as if it were part of the program.

**Mod Engine 3** (me3) uses this to load mods. When you launch with a profile like

```toml
[[natives]]
path = "sekiro_camera_fixes.dll"
```

me3 starts `sekiro.exe` and loads each `[[natives]]` DLL into it. Windows then calls the DLL's entry point, a
function named **`DllMain`**, with the reason `DLL_PROCESS_ATTACH`, meaning "you've just been loaded".

`DllMain` runs under a Windows lock called the **loader lock**, which blocks other DLLs from loading until it returns.
Doing slow work there can freeze or deadlock the game, so this mod's `DllMain` does only two things:

1. Starts a new **thread**: a second line of execution that runs alongside the game.
2. Returns immediately.

All the real work happens on that thread once the loader lock is released.

## The executable in memory

When Windows loads `sekiro.exe`, it copies the file into memory in a layout described by the file's **PE headers**
(PE = Portable Executable, the Windows format for `.exe` and `.dll` files). The loaded copy is called the **image**.
Its start address, the **base**, is `0x140000000` for Sekiro.

The image is split into **sections**, each with a name and flags. The important split is code versus data. A typical
layout (exact addresses vary by executable):

```text
0x140000000  ┌───────────────────┐
             │ headers           │  "MZ…", section table
0x140001000  ├───────────────────┤
             │ .text             │  executable: the game's code. The camera code is in here.
             ├───────────────────┤
             │ .rdata, .data, …  │  not executable: constants, global variables, …
             └───────────────────┘
```

The mod reads the headers to find which sections are executable, and searches only those. This matters for two
reasons:

- **Speed**: there's less memory to search.
- **Correctness**: the game writes to its data sections constantly from other threads, and reading memory while
  another thread writes it is undefined behaviour in Rust. Code sections aren't written while the game runs, except
  by this mod.

## Finding code: pattern scanning

Searching works like a text search with wildcards: slide the pattern along the memory one byte at a time and check
whether every fixed byte matches. uberhalit's tool takes the first match. This mod takes the first match whose bytes at
the patch site are exactly the `expected` ones. When the first match is the right one, as it is on every known game
version, the two agree. If an earlier, unrelated piece of code happened to fit a loose pattern, the mod skips past it
instead of giving up.

One shortcut makes it faster. The first byte of most patterns is fixed (for example `E8`), so the search first jumps to
the next position where that byte appears, then checks the rest. Most positions are skipped this way.

Memory is also split into **pages** (4 KB each), and every page has its own permissions. Some pages in a section might
not be readable, and touching one would crash the game. Before reading, the mod asks Windows about each region with
`VirtualQuery` and only searches regions that are committed and readable.

The mod doesn't search the game's code where it lies. It copies it, 1 MB at a time, into a buffer of its own with
`ReadProcessMemory` and searches the copy. Sekiro's anti-tamper layer rewrites some of its own code while the game
runs, and reading memory that another thread may be writing at the same moment isn't allowed in Rust. A copy made by
Windows avoids that. Consecutive chunks overlap by one byte less than the pattern, so a match across a chunk border
isn't missed.

### Why retry?

Some games, or anti-tamper layers around them, aren't fully unpacked in memory at the moment a mod loads. If a pattern
isn't found, the mod waits 250 ms and tries again, for up to 60 seconds. In practice Sekiro's code is ready at once.
Your log shows everything applied at `0.01s`.

## Changing code: memory protection

Every page has a **protection**: a set of permissions such as read, write and execute. Code pages are normally
**read + execute**. Writing to them crashes the program with an access violation. This stops bugs, and some exploits,
from rewriting code.

To patch, the mod asks Windows to change the protection with `VirtualProtect`:

```text
1. VirtualProtect(site, len, READ+WRITE+EXECUTE)  → returns the old protection
2. copy the new bytes over the site
3. VirtualProtect(site, len, old protection)       → put it back
4. FlushInstructionCache(site, len)                → see below
```

**Instruction cache.** CPUs keep a copy of recently run code in a cache. `FlushInstructionCache` tells Windows the code
changed, so no stale copy gets run. On x86 this is mostly a formality, but it's the documented rule.

A patch can, rarely, cross from one page into the next. Then each page gets its own `VirtualProtect` call, so each
gets back exactly the protection it had. Both pages are made writable before any byte is copied, so if either call
fails, nothing has been changed.

**Thread safety.** The game's own threads could be running the exact instruction being overwritten. A thread that
reads half-old, half-new bytes would crash. So would a thread paused *between* two instructions the patch replaces:
it would resume in the middle of the new `jmp`. uberhalit's tool doesn't guard against this; it relies on the window
being tiny. The mod closes it:

1. It **suspends** every other thread in the game (`SuspendThread`).
2. It reads each suspended thread's next instruction address (`GetThreadContext`). If any thread is stopped inside a
   site, past its first byte, it resumes them all, waits a millisecond, and tries again.
3. It writes all the sites, then resumes the threads.

While the threads are suspended, the mod must not allocate memory. A suspended thread might be holding the lock that
guards the heap, and waiting for that lock would hang the game for good. So everything the writes need is prepared
beforehand.

## Code caves

An in-place patch only works if the new code fits in the old space. The lock-on fix does: one byte becomes another.
The camera fixes don't. Site 1 needs *two* 7-byte instructions where there is room for one. The solution is a
**code cave**:

1. Allocate a small block of new memory, the **cave**.
2. Write the replacement instructions into the cave, followed by a jump **back** to the instruction after the
   patched one.
3. Overwrite the original instruction with a jump **to** the cave.

```text
   game code                                   cave (new memory)
   ─────────                                   ─────────────────
   …                                     ┌───► movss xmm0,[rsi+174]   ; replacement code
   call  …                               │     movss [rsi+174],xmm0   ;
   jmp cave  ─────────────────────────────┘     jmp back ──┐
   nop  nop  nop   (padding)                                │
   cmp byte [rsi+2A3],0   ◄─────────────────────────────────┘
   …
```

### The jump: `jmp rel32`

The jump is the 5-byte x86 instruction **`jmp rel32`**: the byte `E9` followed by a 4-byte signed number, the
**displacement**. The CPU computes the destination as

```text
destination = (address of the jmp) + 5 + displacement
```

The `+ 5` is there because the displacement counts from the *end* of the 5-byte instruction. A 4-byte signed number
ranges about ±2 GB, so **the cave must be within 2 GB of the game's code**. That's why the mod can't just ask for
memory anywhere: `CavePage::allocate` walks the address space near the game, from about 1.75 GB below to 1.75 GB
above, and claims the first free spot. It allocates one 4 KB page, and all the caves share it.

### A worked example

Take site 2, camera yaw (Z), at `0x14073E081` from your log. It overwrites 8 bytes. Suppose the cave landed at
`0x13FFF0000` (the real address is chosen at run time and isn't logged).

**The jump into the cave**, written over the site:

```text
displacement = cave - (site + 5)
             = 0x13FFF0000 - 0x14073E086
             = -0x74E086
             = 0xFF8B1F7A   (the same number as a 32-bit two's-complement value)
bytes        = E9 7A 1F 8B FF          ← little-endian
padding      = 90 90 90                ← 3 nops, to fill all 8 overwritten bytes
```

The `nop`s are never executed, since the CPU jumps away first. They're there so no half-instruction bytes are left
behind if anyone disassembles the code later.

**The jump back**, at the end of the cave. The replacement code is 16 bytes, so the jump sits at `0x13FFF0010`, and it
must land at `site + 8 = 0x14073E089`, the instruction after the overwritten one:

```text
displacement = 0x14073E089 - (0x13FFF0010 + 5)
             = 0x74E074
bytes        = E9 74 E0 74 00
```

This is the arithmetic `jmp_rel32` does in [src/memory.rs](../src/memory.rs), and the unit tests there check it with
small, easy numbers.

### Order of operations

For the four camera sites, the mod:

1. Finds all four sites, before changing anything. Otherwise one patch could change where the next pattern matches.
2. Checks every site's bytes against `expected`.
3. Allocates the cave page and writes all four caves into it while it's still writable.
4. **Seals** the page: changes its protection to read + execute, so it can run but not be changed.
5. Suspends the game's other threads and writes the four jumps into the game's code.
6. Marks the page as never to be freed, because the game now jumps into it every frame.

If anything fails in steps 1–4, the game's code hasn't been touched and the page is freed. If one of the writes in
step 5 fails, the sites already written get their original bytes back, so a feature is never left half applied. No
game thread ran while the jumps were in place, so the page is still freed. Only if putting the bytes back fails too is
the feature left partly applied; the log says so, and the page is kept.

### Testing a cave without the game

The test `yaw_cave_keeps_old_value` in [src/lib.rs](../src/lib.rs) does all of this for real, outside the game:

1. It writes a tiny function into executable memory: "load a float from the second argument, store it at
   `[first argument + 0x174]`". That's the same store instruction as the game's yaw site.
2. It calls the function and checks that the "yaw" changed.
3. It runs the real `patch_sites` on that function.
4. It calls the function again and checks that the yaw *didn't* change this time.

A second test, `failed_write_restores_earlier_sites`, patches two sites where the second one can't be written, and
checks that the first site's bytes are back to what they were.

## Where this differs from uberhalit's tool

uberhalit's tool is a separate program. It reaches into the game from outside with `ReadProcessMemory`,
`WriteProcessMemory` and `VirtualAllocEx`. This mod runs inside the game. It still copies code out with
`ReadProcessMemory` to search it, but writes it directly with pointers, while the game's other threads are suspended.
The patches themselves are identical.

Next: [3. The Rust code, file by file](3-the-rust-code.md)
