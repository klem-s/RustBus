# RustBus refactor plan

Agreed after a 4-planner + supervisor review (2026-10-26). Line numbers refer to commit `51a293c` and will drift; re-check before editing.

Rules for every phase:
- Each commit builds and passes `.\build.ps1 test`. One logical change per commit.
- Refactors must not change behaviour. The only intended behaviour changes are the Phase 0 fix and the Phase 3 `play()` fallback, each in its own commit.
- No public AudioHost/Network/MidiHost/TapeStore traits, no DTOs or use-case objects, no dyn dispatch in the render path, keep `asio.rs` `static LIVE`.

## Phase 0: GUI params clobber fix (behaviour change, ~0.5 day)
Bug: `gui/mod.rs` (~1367) copies the whole `MixParams` into `before`/`p`; widgets edit `p`; at the end of the frame (~1444) the whole struct is written back if `p != before`. Concurrent single-field writes from MIDI (`midi.rs` ~537), the hotkey thread (`hotkey.rs` ~165-175) and the tray menu (`gui/desktop.rs` ~219) are lost, e.g. a push-to-talk mute while dragging a fader.
- At write-back, lock once and apply only the leaf fields where `p` differs from `before` onto the current shared value: `merge(before, p, &mut shared)`, implemented in `params.rs` (per strip/bus/tape/composite field).
- In-frame writers inside `dialogs::show` (preset recall ~`dialogs.rs:597`, console `run_script` ~789, reset ~866) must update `p` and `before` together, or be deferred until after the write-back.
- Note: `apply_settings`/`run_script` reached from `poll_background` run before the snapshot and are safe.
- Tests: shared mute changed while local gain changed → both survive; preset recall in the same frame as a widget edit.

## Phase 1: characterization tests (~1 day)
- Golden hash of mixer output: fixed `MixParams` (routes, EQ, comp, gate, bus modes, limiter), seeded deterministic input, 64 blocks × 256 frames @48 kHz, hash quantized outputs.
- `Core::block` test with hand-built rings (one AdaptiveReader input, one direct buffer, one VBAN input incl. ratio, one output).
- `asio_process` chunking with frames = 700 (> MAX_BLOCK).
- Full settings XML byte-for-byte fixture (in addition to `writer_matches_original_line_shapes`).
- Golden `EngineStatus` strings.

## Phase 2: break cycles, split mixed modules (~1 day, several commits)
Cycles: settings↔midi (`settings.rs:15` / `midi.rs:14`), settings↔vban (`settings.rs:17` / `vban.rs:15`), engine↔recorder (`engine/mod.rs:22` / `recorder.rs:11-12`).
- Move XML helpers `ai/af/ab` (`settings.rs` ~775-790) to `src/xml.rs`.
- Split `midi` → map (pure) / runtime (midir); `vban` → proto (header, codecs, config) / net (UDP threads); `recorder` → tape core (RT) / writer (hound, threads); `hotkey` → step+config / ffi. Keep `pub use` re-exports so callers don't change.
- Remove the engine↔recorder cycle (tape core next to the mixer/ring, or ring+mixer not under `engine`).
- `Engine::start` takes `&DeviceConfig, &VbanConfig` instead of `&Settings`.

## Phase 3: Engine::start decomposition + one master path (~1.5 days)
`Engine::start` is ~212 lines (`engine/mod.rs` ~201-412). WASAPI `build_master`, ASIO `asio_process` and `internal_clock` all take `Core` and call `core.block` in `MAX_BLOCK` chunks, copying bus A1 out.
- Pure `plan_a1(&DeviceConfig, device info) -> A1Plan` (decision only, unit-tested: missing device, 22050 Hz, ASIO selected).
- `wire_inputs`, `wire_outputs`, `wire_vban`.
- One `render_chunked(core, total, write)` used by all three masters.
- Private `enum Master { Wasapi(cpal::Stream), Asio(AsioStream), Internal(..) }` and one `start_master(plan, core) -> Result<Master, (String, Core)>` with a single fallback to the internal clock; removes the duplicated `Arc<Mutex<Option<Core>>>` slot logic.
- Separate commit (behaviour change): if the WASAPI master `play()` fails (~400-403) fall back to the internal clock instead of staying silent.
- Phase 1 goldens must stay unchanged.

## Phase 4: Session (~1.5 days)
`App` has ~45 fields mixing UI and lifecycle; 19 `restart_engine()`/`restart_pending` sites in `src/gui`.
- New `src/session.rs`: plain struct owning settings, engine, MIDI runtime, hotkey runtime, options, presets. Methods: `request_restart()` (single path, coalesced), `apply_settings()`, `snapshot_settings()`, `run_script()`, `tick()` (polls background channels), save/load, tape commands, device selection.
- Move persistence out of the GUI (Bus-EQ XML and fs calls in `dialogs.rs` ~380/388/707-760).
- `App` keeps only UI state plus `session`.
- Tests: Session with an internal-clock engine (no devices), apply/snapshot round-trip, restart coalescing.

## Phase 5: GUI split (~1 day, mechanical)
`gui/mod.rs` (~1600 lines) → `strip.rs`, `bus.rs`, `player.rs`, `topbar.rs` (top bar, pills, main menu, device menus), `help.rs`. Replace direct `shared.*` and `mixer::composite_label` reach-ins with Session accessors where cheap.

## Phase 6 (optional): two-crate workspace (~0.5-1 day)
`crates/rustbus-core` (params, dsp, mixer, ring, script, settings XML, midi map, vban proto, tape core, Core) and the app (cpal, asio, midir, UDP, hound, egui). Profiles at the root, `default-members` so `build.ps1` keeps working, fix `include_bytes!` font paths in `gui/theme.rs`. Check `Cargo.lock` unchanged and `cargo tree -p rustbus-core` shows no cpal/midir/egui.
