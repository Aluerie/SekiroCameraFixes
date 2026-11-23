# 1. What the patches do

This part is about the game, not about Rust. To understand the patches you need a little background on how a
program looks once it's compiled, so that comes first.

## Background: machine code in five minutes

### Code is just bytes

When a game is compiled, every line of its source becomes a few **instructions** for the CPU, and every instruction
is stored as a handful of **bytes**. `sekiro.exe` is about 70 MB of such bytes (plus data). Nobody outside FromSoftware
has the source, so modders read these bytes with a **disassembler**, a tool that turns bytes back into human-readable
instructions called **assembly**.

Bytes are written in **hexadecimal** (base 16, digits `0-9` then `A-F`). One byte is two hex digits, from `00` to
`FF` (0 to 255). In Rust and C, a hex number is written with a `0x` prefix: `0x174` is 372 in decimal.

### Registers and memory

The CPU works on a small set of named slots called **registers**. Sekiro is a 64-bit x86 program, so it uses
x86-64 registers. The ones that show up here:

- `rsi`, `rbp`, `rax`: general-purpose 64-bit registers. In this code they hold **addresses**, i.e. pointers to
  places in memory.
- `xmm0`, `xmm1`, `xmm4`: 128-bit registers for floating-point math. Each holds up to four 32-bit `float`s.

Memory is one huge array of bytes, and an address is an index into it. In assembly, square brackets mean "the memory
at this address": `[rsi+174]` means "the memory 0x174 bytes after wherever `rsi` points".

### The camera object

In the camera code, `rsi` points to the game's **camera object**, a block of memory that holds everything about the
camera, a bit like a `struct`. uberhalit worked out which parts matter for us:

| Offset from `rsi` | Size | What it is |
|---|---|---|
| `0x170` | 4 bytes (`float`) | camera **pitch** (looking up/down) |
| `0x174` | 4 bytes (`float`) | camera **yaw** (looking left/right) |
| `0x2A3` | 1 byte (`bool`) | "the camera needs to reset" flag |

### The instructions you'll see

| Assembly | Meaning |
|---|---|
| `movss xmm0,[rsi+174]` | **load** one float from memory into `xmm0` (`mov` = move, `ss` = scalar single-precision) |
| `movss [rsi+174],xmm0` | **store** the float in `xmm0` into memory |
| `movaps [rbp+870],xmm4` | store all 16 bytes (four floats) of `xmm4` into memory |
| `mov byte [rsi+2A3],1` | store the number 1 into one byte of memory |
| `call X` / `jmp X` / `je X` | call a function / jump to another place / jump only if the last comparison was equal |
| `nop` | do nothing ("no operation"), one byte: `90` |

### How an instruction becomes bytes

Take `movss [rsi+174],xmm0`. It's encoded as eight bytes:

```text
F3  0F 11  86  74 01 00 00
│   │      │   └──────────── the offset 0x174, as a 4-byte number
│   │      └──────────────── "ModRM" byte: which register (xmm0) and which address form ([rsi + 4-byte offset])
│   └─────────────────────── the opcode: 0F 11 = store, 0F 10 = load
└─────────────────────────── prefix: "scalar single float"
```

Two details matter:

- **Little-endian.** x86 stores multi-byte numbers lowest byte first. `0x00000174` is written `74 01 00 00`. You'll
  see this everywhere in this project.
- **Load and store differ by one byte.** `F3 0F 10 86 74 01 00 00` is the load `movss xmm0,[rsi+174]`.
  `F3 0F 11 86 74 01 00 00` is the store `movss [rsi+174],xmm0`. Only the third byte differs.

You don't need to memorize encodings. The project lists every instruction's bytes with the assembly next to it in a
comment, as in [src/game_data.rs](../src/game_data.rs).

## Finding the code: byte patterns

The mod has to find these instructions in the game's memory. It can't use a fixed address, because addresses
change between game versions and can change between runs. Instead it searches for a **pattern**: a sequence of bytes
that, as far as anyone knows, appears only around the instruction we want.

Patterns are written like this:

```text
E8 ?? ?? ?? ?? F3 0F 11 86 ?? ?? 00 00 E9
```

Each `??` is a **wildcard** that matches any byte. Wildcards cover bytes likely to change between versions, such as
the target of a `call` (`E8` plus a 4-byte distance) or an offset inside the camera object. The fixed bytes describe
the *shape* of the code: "a call, then a float store to `[rsi + something small]`, then a jump".

Each site also has an **offset**: the distance from the start of the match to the instruction being patched. In the
pattern above, the store starts 5 bytes in, after the 5-byte `call`.

All patterns, offsets and replacement bytes come from uberhalit's tool. This mod adds one safety step on top: before
writing, it checks that the bytes at the site are *exactly* the instructions the patch was written for (the `expected`
field). A match where they differ is skipped, and the search goes on to the next one. If no match has the expected
bytes, for example because a future game version changed the code, the mod refuses to patch rather than write over the
wrong thing.

## Feature 1: Disable camera reset on lock-on

When you press lock-on with no enemy in range, Sekiro recenters the camera behind you and blocks your camera input
until that's done. It does this by setting the flag at `[rsi+2A3]` to 1. Other camera code checks that flag on the next
frame and starts the reset.

The instruction that sets the flag:

```text
C6 86 A3 02 00 00 01      mov byte [rsi+2A3],1
F3 0F 10 8E B4 02 00 00   movss xmm1,[rsi+2B4]
```

The last byte of the first instruction, `01`, is the value being stored. The patch changes that one byte to `00`:

```text
C6 86 A3 02 00 00 00      mov byte [rsi+2A3],0
```

Now pressing lock-on with nothing in range "sets" the flag to false, so the reset never starts. Everything else about
lock-on is unchanged. This is the simplest kind of patch, an **in-place byte patch**: same length, same instruction,
different constant.

The pattern is `C6 86 ?? ?? 00 00 ?? F3 0F 10 8E ?? ?? 00 00` with offset 6, which is exactly that `01` byte.

## Feature 2: Disable camera auto rotate on movement

Every frame, Sekiro's camera code takes the angle you set with the mouse, works out a new, automatically adjusted
angle based on how you're moving, and stores the result into the camera object. That automatic part makes the camera
drift when you run sideways or toward the screen.

uberhalit found four places where an adjusted angle gets stored. The fix at each is the same idea: **at the moment the
game stores its new angle, store the old one instead.** The old angle is the one already in the camera object, which
came from your mouse.

Most of these can't be done by changing a byte or two in place, because the fix needs *more* instructions than fit in
the space. Instead each one uses a **code cave**: the original instruction is replaced by a jump to some new code
elsewhere, which runs the replacement instructions and jumps back. [Part 2](2-how-patching-works.md#code-caves)
explains how that works. Here we only look at *what* each cave does.

### Site 1: camera pitch

```text
0F 29 A5 70 08 00 00   movaps [rbp+870],xmm4   ← replaced by the jump to the cave
0F 29 A5 80 08 00 00   movaps [rbp+880],xmm4   ← the cave jumps back here
0F 29 A6 70 01 00 00   movaps [rsi+170],xmm4   ; store the new angles into the camera
```

`xmm4` holds the newly calculated values, and the third instruction stores them into the camera object at `+0x170`.
`movaps` moves 16 bytes at once, so this block covers pitch (`+0x170`), yaw (`+0x174`) and the 8 bytes after them.

The cave runs:

```text
0F 28 A6 70 01 00 00   movaps xmm4,[rsi+170]   ; NEW: reload xmm4 from the camera object
0F 29 A5 70 08 00 00   movaps [rbp+870],xmm4   ; the instruction the jump replaced
```

Before the game's own stores run, `xmm4` is overwritten with what the camera already holds. All three stores then
write the current values back, so the automatic adjustment is thrown away. The cave also has to repeat the
instruction the jump replaced, or it would be lost.

### Sites 2 and 4: camera yaw

Two separate code paths compute an automatic yaw. uberhalit calls them the "Z-axis" and "XY-axis" movement cases.
Both end in the same instruction:

```text
F3 0F 11 86 74 01 00 00   movss [rsi+174],xmm0   ; store the new yaw
```

The cave runs:

```text
F3 0F 10 86 74 01 00 00   movss xmm0,[rsi+174]   ; load the current yaw into xmm0
F3 0F 11 86 74 01 00 00   movss [rsi+174],xmm0   ; store it back (no change)
```

The net effect is that the yaw in memory doesn't change, and `xmm0` holds the current yaw for any code after it that
reads `xmm0`.

### Site 3: camera pitch on XY movement (skipped with `controller = true`)

```text
F3 0F 10 00               movss xmm0,[rax]       ; load the new pitch from wherever rax points
F3 0F 11 86 70 01 00 00   movss [rsi+170],xmm0   ; store it as the camera pitch
```

Here the new pitch isn't in a register but in memory at `[rax]`. The cave puts the current pitch in both places:

```text
F3 0F 10 86 70 01 00 00   movss xmm0,[rsi+170]   ; load the current pitch
F3 0F 11 00               movss [rax],xmm0       ; overwrite the "new" pitch with it
F3 0F 10 00               movss xmm0,[rax]       ; the two original instructions,
F3 0F 11 86 70 01 00 00   movss [rsi+170],xmm0   ;   now moving the old value around
```

According to uberhalit, this site breaks pitch control and deadzones on gamepads, so the tool skips it for controller
users. Their notes only describe the symptom. A likely cause is that `[rax]` is also read by the stick-input code, but
that's a guess. The mod skips this site when `controller = true` in `SekiroCameraFixes.ini`.

### All or nothing

The four camera sites are applied together or not at all. Applying only some would give a camera that half
auto-rotates, which is confusing and isn't something the original tool would ever do. The lock-on patch is
independent, so one can work while the other doesn't.

## Reading your log

Here is a real log from this mod:

```text
[   0.01s] Disable camera auto rotate on movement: applied
[   0.01s]   camera pitch at 0x14073e056
[   0.01s]   camera yaw (Z) at 0x14073e081
[   0.01s]   camera pitch (XY) at 0x14073e5df
[   0.01s]   camera yaw (XY) at 0x14073e6d2
[   0.01s] Disable camera reset on lock-on: applied
[   0.01s]   camera reset on lock-on at 0x14073ded4
```

- `sekiro.exe` is loaded at address `0x140000000`, so all five sites are about 7.2 MB into the executable.
- All five are within about 2 KB of each other. They're all in the same large camera-update function, which matches
  uberhalit's notes (`0x14073AF86` and so on, from an older game version).
- `0.01s` means the code was already in memory when the DLL loaded, so none of the retries were needed.

Next: [2. How patching works](2-how-patching-works.md)
