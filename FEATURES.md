# RustBus — feature tracker

Status of the Rust rebuild against the features mapped from `voicemeeterpro.exe`
(Voicemeeter Banana 2.1.3.0). Details and evidence are in [docs/ANALYSIS.md](docs/ANALYSIS.md).

Legend: ✅ done · 🟡 partial · ❌ not done · ⛔ excluded on purpose

## Mixer topology and engine

| Feature | Status | Notes |
|---|---|---|
| 3 HW stereo strips + 2 virtual 8-ch strips, buses A1 A2 A3 B1 B2 (8 ch) | ✅ | `params.rs`, `engine/mixer.rs` |
| **Third virtual input strip ("Virtual Input 3")** (beyond Banana, like Potato) | ✅ | 8 ch, 3-band EQ, pan, fader, A1..B3 routing, device select; MIDI IDs (`VInGainFader3`, `VInMute3`, `Strip6A1`…), script `Strip[5]`, VBAN in destination, recorder arm `ArmIn6`, XML `LabelVirtualStrip3`, composite inputs extended. M.C. stays on virtual #1, karaoke on #2 |
| **Extra virtual bus B3** (beyond Banana, like Potato) | ✅ | Routing button on every strip and the tape deck, fader/EQ/modes, output device, MIDI IDs (`StripNB3`, `VOutGainFader3`…), script `Bus[5]` / `Strip[i].B3`, XML `busb3` / `LabelVirtualBus3` / `ArmBus6` |
| A1 is the master clock; internal clock fallback | ✅ | Same SR whitelist as the original (32k…192k) |
| Every other device resampled against the master clock (drift-compensated) | ✅ | `engine/ring.rs` AdaptiveReader |
| Auto restart on device error | ✅ | Menu option; restarts after 2 s |
| Restart audio engine (Ctrl+R) | ✅ | |
| Device back-ends: WDM (WASAPI shared) | ✅ | via `cpal` |
| Device back-ends: ASIO (A1 + hardware-strip inputs) | ✅ | Own COM host in `engine/asio.rs` (no SDK). A1 only, as in the original; ASIO inputs patch to hardware strips (pairs or mono). Buffer size, preferred SR and driver control panel in System Settings. Driver reset requests restart the engine. Verified on a Scarlett 2i2 3rd Gen (48 kHz, 192 samples). |
| Device back-ends: MME / KS | ❌ | WASAPI covers these devices. Buffering values are kept in the file. |
| WASAPI exclusive mode, engine modes ("Swift"…) | ❌ | |
| Virtual inputs / B buses on VB-Audio VAIO drivers | ⛔ | Kernel drivers are out of scope. Virtual strips and B1/B2 can be bound to any endpoint instead (e.g. VB-CABLE). |
| Virtual ASIO insert / PatchInsert | ⛔ | Needs the virtual ASIO driver. `PatchInsert` is preserved in files. |
| Monitoring delay msA1..A3, VAIO latency/sync | ❌ | Values preserved in files only |

## Strips

| Feature | Status | Notes |
|---|---|---|
| Gain fader −60…+12 dB, double-click reset, wheel ±0.5 dB | ✅ | |
| Limiter (pre-fader, Ctrl+drag on meter) | ✅ | |
| **Denoise: AI voice noise removal** (RustBus extension) | ✅ | RNNoise via the pure-Rust `nnnoiseless` crate, hardware strips, before the gate. Button + right-click amount (dry/denoised blend). 480-sample (10 ms) delay while on. Saved as `<RustBusStrip denoise=…>`, script `Strip[i].Denoise` / `.Denoise.Amount`, MIDI `InDenoise1..3`. Measured 33–54 dB less room/fan-type noise. Trained for 48 kHz; at other rates it still runs but less accurately. |
| Mute / Solo / Mono | ✅ | Mono uses (L+R)/2; help text says L+R |
| Routing A1 A2 A3 B1 B2 | ✅ | |
| Compressor knob + detailed StripComp params | ✅ | Own DSP. Knob → ratio 1…6:1 (design choice) |
| Gate knob + detailed StripGate params | ✅ | Own DSP. Knob → threshold −60…0 dB. Sidechain BP filter not applied. |
| Intellipan: colour panel / 3D position / modulation (chorus) | 🟡 | Same controls and behaviour intent; algorithms are our own |
| Virtual strip 3-band EQ (bass/med/treble ±12 dB) | ✅ | Frequencies are our choice (120 Hz / 1 kHz / 7 kHz) |
| Multichannel pan x/y on virtual strips | ✅ | |
| M.C. (mute centre) on virtual #1 | ✅ | |
| Karaoke modes 1–4 on virtual #2 | 🟡 | 4 voice-removal variants; the original algorithms are unknown |
| Strip menu: copy/paste/reset (all, EQ, pan, intellipan, comp, gate) | ✅ | |
| Strip menu: load/save strip-settings files | ❌ | |
| Labels (right-click → Edit Label) | ✅ | |
| `stripmode` multichannel modes | ❌ | Value preserved in files |

## Buses

| Feature | Status | Notes |
|---|---|---|
| Fader, mute, limiter (post-fader) | ✅ | |
| Mono, and click again to swap L/R | ✅ | |
| 14 bus modes (Normal, Mix-Down A/B, Repeat, Composite, TV-Mix, Up-Mix 2.1/4.1/6.1, L/R/C/LFE/Rear only) | ✅ | Formulas from the shipped help; unit-tested |
| Composite patch (pre/post fader, 8 ch) | ✅ | System Settings dialog |
| 6-band parametric EQ per channel ("EQPro-G6"), trim, delay | ✅ | With a response curve; load/save EQ file |
| EQ A/B memories | 🟡 | Written to files (copy of the current EQ); no A/B switching in the UI |
| SEL / monitor bus | 🟡 | Stored only |

## Tape deck (recorder / player)

| Feature | Status | Notes |
|---|---|---|
| Play / Stop (2nd stop = rewind to zero) / Rew / FF / Replay | ✅ | Rew/FF = ±5 s |
| Playback routed to A1..B2 with gain | ✅ | |
| Record armed buses and inputs to WAV (16/24/32f), target SR, 2–8 ch | ✅ | Default dir `C:\VoicemeeterRecord`, original naming pattern |
| Record pause (2nd REC press) | ✅ | |
| Multitrack (one mono file per channel, `_TrackN`) | ✅ | |
| Stop after duration | ✅ | |
| Loop, play on load | ✅ | |
| WAV/BWF playback | ✅ | |
| MP3/AIFF recording, MP3/M4A/WMA/video playback | ❌ | The original uses LAME / mpg123 / Media Foundation |
| Pre-recording buffer | ❌ | |

## Control and remote

| Feature | Status | Notes |
|---|---|---|
| Parameter script language (`Strip[i].Gain = x;`, `+=`/`-=`, Bus modes, EQ, Recorder.*, labels, devices, Command.*) | ✅ | `script.rs`; Command Console dialog |
| **Mic hotkey: push to talk / push to mute / toggle** (RustBus extension) | ✅ | `hotkey.rs`, Menu → "Mic Hotkey". Any key or mouse button (Mouse 4/5, F13–F24…), any strip. Polls `GetAsyncKeyState` every 5 ms, so it works while a game has focus; no keyboard hook is installed. Tested with a synthesized F13. |
| VBAN audio out (8 streams, any bus, SR/ch/format conversion) | ✅ | `vban.rs`; UDP loopback-tested |
| VBAN audio in (8 streams → any strip, IP filter, net-quality jitter buffer, stats) | ✅ | |
| VBAN-TEXT incoming commands (executes scripts) | ✅ | Replaces the Remote API for remote control |
| VBAN serial/MIDI data, VBAN video frame out, VBAN-Chat, VBAN service/ping | ❌ | Config entries preserved |
| M.I.D.I. mapping: 2 inputs + feedback output, learn, all original control IDs | ✅ | `midi.rs` |
| Control types: linear, coder offset, coder sign-bit (Mackie); PTT; omni; feedback | ✅ | |
| MIDI map load/save (`VBAudioVoicemeeterMIDIMapping`) | ✅ | |
| Voicemeeter Remote API (`VoicemeeterRemote.dll` shared memory) | ⛔ | Undocumented proprietary IPC. Use VBAN-TEXT instead. |
| Macro Buttons / Streamer View / BUS-Matrix / GEQ15 / VBAN2MIDI companion apps | ⛔ | Separate products |
| Keyboard hooks (volume keys, Ctrl+F10–F12, F5/F6/F8 transport) | ❌ | |

## Settings, files and app

| Feature | Status | Notes |
|---|---|---|
| `VBAudioVoicemeeterSettings` XML read/write, same layout, compatible both ways | ✅ | Unit-tested against an original-format file; rebuild-only devices go in extra `RustBusDevice` elements (old `VMRSDevice` still read) |
| LastSettings / Today / Yesterday backups | ✅ | In `%APPDATA%\VoicemeeterRS` so the original's files are never touched |
| Load / Save settings, load-on-startup file, reset to defaults | ✅ | |
| Preset scenes (16 × `PresetScene_NN.xml`, with/without devices) | ✅ | Count of 16 is our assumption |
| App options (registry keys) | ✅ | `options.ini` with the same key names |
| Main window, menu, contextual help caption | ✅ | Own UI and help wording; original skin not reused |
| **UI redesign** (design/rustbus-mixer.html) | ✅ | Tokens in `src/gui/theme.rs`, painted widgets in `src/gui/widgets.rs`. Differences: player bar stays one row (prompt truncates instead of wrapping); click-to-load instead of drag & drop; no keyboard focus ring on custom-painted controls; shadows approximated; strip dots use the A/B accents (no per-strip colour setting exists); 8-ch virtual strips meter as 2 bars (max of left/right-side channels) |
| System Settings, Recorder Options, MIDI Mapping, VBAN, Preset, Bus EQ, About dialogs | ✅ | |
| Always on top, lock GUI, zoom 50–100 %, Windows sound settings shortcuts | ✅ | |
| Windows Audio Map dialog | ❌ | |
| **System tray, start with Windows, close to tray** (RustBus extension of the original options) | ✅ | `gui/desktop.rs`: tray icon (Show / Mute mic / Exit), Menu → "Close to System Tray" and "Start with Windows (in tray)" (HKCU Run entry with `--tray`). Single-instance guard: a second launch shows the running window. Tested: hide on close, hidden start, re-show. |
| Log file (`VoicemeeterBanana_LogFile.txt`) | 🟡 | Messages go to the in-app console only |
| License activation / footprint / challenge-response | ⛔ | Copy protection, deliberately not replicated |

## Verification

- `cargo test`: 50 unit tests (DSP, bus modes, mixer routing, script language,
  settings round-trip including an original-format file, VBAN codec, UDP loopback,
  MIDI mapping, recorder record → playback, presets, options).
- `cargo run --example engine_smoke`: real-device check (default output = A1 master,
  default mic = strip 1, prints meters).
