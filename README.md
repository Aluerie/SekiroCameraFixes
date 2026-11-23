# 🎥 SekiroCameraFixes

A native DLL mod for **Sekiro: Shadows Die Twice**, written in Rust and loaded by
[Mod Engine 3 (me3)](https://github.com/garyttierney/me3). It does exactly what these two
settings of uberhalit's [Sekiro FPS Unlocker and More](https://github.com/uberhalit/SekiroFpsUnlockAndMore) do,
and nothing else:

- **Disable camera auto rotate on movement**: the camera no longer turns by itself when you walk.
- **Disable camera reset on lock-on**: pressing lock-on with no target in range no longer snaps the camera forward.

## 📦 Install

The quickest way: download `SekiroCameraFixes-v<version>.zip` from the
[latest release](https://github.com/Aluerie/SekiroCameraFixes/releases/latest), unzip it anywhere, and double-click
`SekiroCameraFixes.me3` (or run `me3 launch -p SekiroCameraFixes.me3` in that folder). The zip holds the DLL and a
ready-made profile.

To set it up by hand, or add it to a profile you already have:

1. Build it (`cargo build --release`) or take `sekiro_camera_fixes.dll` from a release.
2. Put the DLL next to a `.me3` profile and add it as a native:

   ```toml
   profileVersion = "v1"

   [[supports]]
   game = "sekiro"

   [[natives]]
   path = "sekiro_camera_fixes.dll"
   ```

   [SekiroCameraFixes.me3](SekiroCameraFixes.me3) is a ready-made profile. To add it to an existing profile, only the
   `[[natives]]` block is needed. `path` is relative to the `.me3` file.
3. Launch with `me3 launch -p SekiroCameraFixes.me3` (or through ME3 Manager).

Any other DLL loader works too, because the mod does all its work from `DllMain`.

## ⚙️ Settings

On first launch the mod writes `SekiroCameraFixes.ini` next to the DLL:

```ini
[SekiroCameraFixes]
disable_auto_rotate = true
disable_lockon_reset = true
controller = false
```

Set `controller = true` if you play on a gamepad. The original tool asks "Are you using a mouse as input?" when you
first enable auto-rotate removal. Answering "No" leaves one of its four camera patches (pitch adjustment on XY movement)
off, because that patch breaks stick pitch and deadzones. `controller = true` does the same thing. Even then,
uberhalit warns that auto-rotate removal is meant for mice and that sticks lose some smoothing.

`;` or `#` starts a comment, also after a value. Save the file as UTF-8. If it can't be read, the mod uses the
defaults and says so in the log, but leaves the file alone.

Each game start writes a `SekiroCameraFixes.log` next to the DLL. It lists each patch with its address, or the reason it
wasn't applied.

## 🔧 How it works

The patterns, offsets and injected instructions are copied byte-for-byte from uberhalit's `GameData.cs`:

| Site | Patch |
|---|---|
| camera pitch | cave: reload `xmm4` from `[rsi+170]` before it is stored |
| camera yaw (Z movement) | cave: reload `xmm0` from `[rsi+174]` so the stored yaw doesn't change |
| camera pitch (XY movement), skipped with `controller = true` | cave: write the old pitch to `[rax]` and `xmm0` |
| camera yaw (XY movement) | cave: reload `xmm0` from `[rsi+174]` |
| lock-on reset | `mov byte [rsi+2A3],1` becomes `,0` |

A "cave" is the same technique the original tool uses: the instruction is replaced with a `jmp` to a small block of
code allocated within ±2 GB of the game. That block runs the replacement instructions and jumps back.

Differences from the original tool, none of which change what the game does:

- It runs inside the game, so no separate program has to be open, and the patches are in place from the start.
- Before writing, it checks that the game's bytes at each site are exactly the instructions the patch was written
  against. On an unknown game version it refuses to patch and says so in the log, instead of writing into the wrong code.
- Patterns are only searched in the game's executable sections, not its data. Every match is in code anyway. Matches
  whose bytes aren't the expected instructions are skipped, so a loose pattern that also fits some unrelated code
  earlier on doesn't stop the patch. On known game versions the first match is the right one, as in the original.
- The game's other threads are suspended while its code is overwritten, so none of them can run a half-written
  instruction.
- The four camera sites are all-or-nothing, as in the original. If one of them can't be written, the ones already
  written are put back. The lock-on patch is independent of them.
- Patterns are retried for up to 60 seconds, in case the game's code isn't in place yet when the DLL loads.

## 🛠️ Development

```sh
cargo test            # pattern/config/jump encoding tests, plus a test that runs a real cave
cargo build --release # target/release/sekiro_camera_fixes.dll
```

To get the ready-to-go folder that players download, run `scripts\package.ps1`. It builds the DLL and creates
`dist\SekiroCameraFixes\` (DLL, `.me3` profile, README) and `dist\SekiroCameraFixes-v<version>.zip`.

### 🤖 CI

[.github/workflows/ci.yml](.github/workflows/ci.yml) runs on every push and pull request: `cargo fmt --check`,
`cargo clippy` with warnings as errors, `cargo test`, then the packaging script. The packaged folder can be downloaded
from the run's "Artifacts" section.

### 🚀 Releasing

1. Bump `version` in `Cargo.toml` and run `cargo build` so `Cargo.lock` picks it up. Commit both.
2. Tag the commit `v<version>`, e.g. `v0.2.0`, and push the tag.

CI then checks that the tag matches `Cargo.toml`, builds and tests everything, and publishes a GitHub Release with the
zip attached and release notes generated from the commits since the last tag.

## 🙏 Credits

Patterns and patch bytes: [uberhalit/SekiroFpsUnlockAndMore](https://github.com/uberhalit/SekiroFpsUnlockAndMore)
(MIT), including the offsets credited there to Cielos.
