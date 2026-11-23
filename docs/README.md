# How SekiroCameraFixes works

These notes explain the mod from the ground up: what it changes inside the game, how a DLL can change a running
program at all, and how the Rust code does it. They assume you can read some code but are new to Rust and to game
modding.

Read them in order. Each one builds on the one before.

1. **[What the patches do](1-what-the-patches-do.md)**: the game side. What the camera code looks like in machine code,
   which instructions the mod changes, and why that removes the auto-rotate and the lock-on reset.
2. **[How patching works](2-how-patching-works.md)**: the operating-system side. How the DLL gets into the game,
   finds the right code, makes it writable, and redirects it with a "code cave".
3. **[The Rust code, file by file](3-the-rust-code.md)**: a guided tour of `src/`, in the order the code runs,
   explaining each Rust feature where it first appears.

## The short version

1. Mod Engine 3 starts `sekiro.exe` and loads `sekiro_camera_fixes.dll` into it.
2. Windows calls the DLL's `DllMain` function, which starts a background thread and returns.
3. The thread reads `SekiroCameraFixes.ini` and searches the game's code for five byte patterns.
4. For each match, it checks that the bytes are exactly what it expects, then changes them:
   - **Lock-on reset**: one byte changes from `1` to `0`, so the "please reset the camera" flag is never set.
   - **Auto-rotate**: four instructions that store the camera's new, automatically adjusted angle are redirected to a
     few bytes of new code that store the *old* angle instead.
5. It writes what happened to `SekiroCameraFixes.log` and the thread ends. The patched code stays in place until the
   game exits.

## Where things live

| File | What it holds |
|---|---|
| [src/lib.rs](../src/lib.rs) | The entry point (`DllMain`) and the overall flow: find, check, patch, log. |
| [src/game_data.rs](../src/game_data.rs) | The five patch sites: patterns, expected bytes, replacement bytes. |
| [src/pattern.rs](../src/pattern.rs) | Byte patterns like `"0F 29 ?? 00"` and searching for them. |
| [src/memory.rs](../src/memory.rs) | Everything that touches the game's memory: scanning, writing, code caves. |
| [src/threads.rs](../src/threads.rs) | Pausing the game's other threads while its code is rewritten. |
| [src/config.rs](../src/config.rs) | Reading `SekiroCameraFixes.ini`. |
| [src/logfile.rs](../src/logfile.rs) | Writing `SekiroCameraFixes.log`. |
| [src/error.rs](../src/error.rs) | The reasons a patch can fail, and how they're printed. |
