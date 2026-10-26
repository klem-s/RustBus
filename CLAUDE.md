# RustBus — notes for agents

Rust desktop audio mixer for Windows (clean-room Voicemeeter Banana rebuild). Feature status: `FEATURES.md`. Reverse-engineering notes: `docs/ANALYSIS.md`.

## Build and test
- Toolchain is `stable-x86_64-pc-windows-gnu`. Plain `cargo build` fails (no dlltool/as on PATH); always use the wrapper, which adds WinLibs:
  - `.\build.ps1` (debug build), `.\build.ps1 test`, `.\build.ps1 build -Release`
- `target\release\rustbus.exe` is locked while RustBus runs; close it first (it may be minimized to the tray).
- Live hardware tests are `#[ignore]`: `asio_live` (Focusrite ASIO), `autostart_roundtrip` (writes HKCU Run key). Don't run them unasked.

## Rules
- Real-time path (`engine/mixer.rs`, `dsp/*`, audio callbacks in `engine/mod.rs` and `engine/asio.rs`): no allocation, no locks other than the existing `try_lock`, no per-sample dynamic dispatch.
- Settings XML must stay byte-compatible with Voicemeeter: never change existing element/attribute shapes; RustBus extras go in separate `RustBus*` elements (`settings.rs` tests enforce this).
- `engine/asio.rs` `static LIVE` is required by the ASIO C ABI; keep it.
- Every change must leave `.\build.ps1 test` passing. One logical change per commit; refactors and behaviour changes in separate commits.

## Refactor plan (agreed)
Phases, in order: 0 fix GUI params clobber race (`gui/mod.rs` write-back of the whole `MixParams`) · 1 characterization tests (golden mixer output, settings XML fixture) · 2 break module cycles (settings↔midi, settings↔vban, engine↔recorder) and split mixed I/O modules · 3 split `Engine::start` and unify WASAPI/ASIO/internal master rendering with one fallback (also fixes silent failure when the WASAPI master `play()` fails) · 4 extract a `Session` struct from `App` with a single `request_restart()` · 5 split `gui/mod.rs` into strip/bus/player/topbar/help · 6 (optional) two-crate workspace.
Full details with file references: `docs/REFACTOR_PLAN.md`.
Not doing: public AudioHost/Network/MidiHost traits, DTOs/use-case objects, many crates, dyn dispatch in the render path.
