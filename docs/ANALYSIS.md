# Voicemeeter Banana — Reverse-Engineering Analysis

Target: `C:\Program Files (x86)\VB\Voicemeeter\voicemeeterpro.exe`
- Product: **VoiceMeeter Banana 2.1.3.0** (VB-Audio, "VoiceMeeter Pro" internal name)
- PE32 x86, MSVC CRT, Win32 + GDI (no framework), 2392 functions, 2403 defined strings
- SHA-256 `70f83e4381654c056a317ea59b44483adab453612473408e8fbf92cc9d0e1867`

Tooling: REA (Ghidra 12.1.4 provider, REA PE-resource parser). Evidence IDs are
cited as `ev_…` (prefix only). Every statement is tagged:

- **[O]** observed in the shipped binary or shipped files (strings, resources, decompiled code)
- **[I]** inferred from observations (names, format strings, help text)
- **[U]** unknown / not analysed (proprietary DSP internals, obfuscated assets)

---

## 1. Binary composition

| Item | Finding | Evidence |
|---|---|---|
| Resources | 5 RCDATA blobs (ids 100–104, 0.7–1.9 MB) whose sizes equal raw 24-bit BMP sizes (e.g. 921 654 = 640×480×3+54) but whose headers are scrambled → **obfuscated skin bitmaps** [O/I] | `ev_bd90bfc9` |
| Menu | `MYMAINAPPMENU`: *Options → Set Max Latency 1024 … 8192 smp* (ids 101–108) [O] | `ev_bd90bfc9` |
| Icons | 12 icon groups (32/48 px) [O] | `ev_bd90bfc9` |
| Manifest | `asInvoker`, no UI access [O] | `ev_bd90bfc9` |
| Imports | WINMM (wave/midi), SETUPAPI, WS2_32, WININET, MSACM32, GDI32, COMDLG32, ADVAPI32 (registry), ole32 (WASAPI via COM) [O] | strings 1757–2115 |
| Dynamic loads | `ksuser.dll` (KsCreatePin), `Avrt.dll` (MMCSS "Pro Audio"), `lame_enc.dll` (MP3), `libmpg123-0.dll`, `Mfplat/Mfreadwrite` (m4a/mp4), `Wmvcore` (wma/wmv), `Gdiplus` (JPEG for VBAN video), `iphlpapi` [O] | strings 525–551, 642–651, 738 |

The skin artwork is VB-Audio's copyrighted material and is **not** reused; the
rebuild draws its own UI.

## 2. Mixer topology (Banana)

[O] help XML + strings, [I] structure:

```
 HW IN #1 ─┐                               ┌─> A1 (physical out, master clock, up to 8ch)
 HW IN #2 ─┤ stereo strips (3)             ├─> A2 (physical out)
 HW IN #3 ─┤   gain/pan/EQ/comp/gate/lim   ├─> A3 (physical out)
 VIRT IN 1 ┤ 8ch virtual strips (2)        ├─> B1 (virtual out "Voicemeeter Output")
 VIRT IN 2 ┘   gain/pan/3-band EQ/karaoke  └─> B2 (virtual out "Voicemeeter AUX Output")
 TAPE PLAYER ─────────────────────────────────> any of A1..B2 (Recorder.A1.. B2)
 VBAN IN 1-8 ─────────────> mixed into a chosen strip (VBANStreamIn in='…')
 BUS ───────> VBAN OUT 1-8 (VBANStreamOut bus='…') ; BUS/strips ──> Tape recorder
```

- 5 strips: indices 0–2 hardware (stereo), 3–4 virtual (8 ch: L R C LFE SL SR BL BR) [O strings 856–877, help STRIP0–4]
- 5 buses: A1, A2, A3 (physical), B1, B2 (virtual), each up to 8 channels [O help BUSA1–BUSB2]
- Each strip has routing buttons A1 A2 A3 B1 B2 (`busa busa2 busa3 busb busb2`) [O settings format]
- **A1 is the master clock**: engine start fails if A1 can't open; A1 SR must be one
  of 32000/44100/48000/88200/96000/176400/192000; channels clamped 2..8; bits snapped
  to 16 or 32; buffer > 2048 samples rejected [O decompiled `FUN_00537d90`, `ev_217ea5f0`]
- Output driver type selects buffering: type 1 = MME (default 1024), 8 = KS, else WDM
  (default 512) [O `FUN_00537d90`; strings 1082/1084]

## 3. Per-strip processing

| Control | Parameters / behaviour | Source |
|---|---|---|
| Gain fader | `dblevel` (dB, `%0.2f`); double-click → 0 dB; R-click edits value/label | [O] help STRIP_FADER, format 1135 |
| Limiter | `dblimit`, brick-wall **before** fader; set by click+drag on VU | [O] help STRIP_LIMITER |
| Mute / Solo / Mono | Solo: all non-solo strips muted. Mono: L=R=(L+R) | [O] help |
| A1..B2 assign | routing to buses | [O] |
| Compressor knob (HW) | `prg_c` / `Comp` 0..10; auto make-up; StripComp detail (gainin, attack, release, knee, ratio, threshold, automakeup, gainout) | [O] help STRIP_COMP, format 1136 |
| Gate knob (HW) | `prg_g` / `Gate` 0..10; StripGate (thresin, damping, bpsidechain, attack, hold, release) | [O] format 1137 |
| Intellipan (HW) | 3 panel types: **Color** (quick EQ, x/y), **3D position** (pan x/y), **Modulation** (chorus/phaser x/y) — `paneltype`, `Panel3Dx/y`, `ColorPanelx/y`, `ModPanelx/y` | [O] help STRIP_PANEL* |
| 3-band EQ (virtual) | `EQGain1/2/3` bass/med/treble, double-click → 0 dB | [O] help STRIP_EQ* |
| Pan (virtual) | multichannel pan x/y, `stripmode` multichannel mode | [O] help STRIP_PANPOT |
| M.C. (virtual #1) | mute centre channel (ch 3) | [O] help STRIP_MC |
| Karaoke (virtual #2) | removes centre (voice); several algorithms selected by click | [O] help STRIP_KARAOKE |
| VAIO ext / Virtual insert | ASIO insert return per strip | [O] — not reproducible (driver) |

**[U]** The exact curves of compressor/gate/intellipan, the karaoke algorithms and
EQ frequencies were not decompiled (they are proprietary DSP). The rebuild uses
standard textbook DSP with the same parameter semantics and ranges.

## 4. Bus processing

| Control | Behaviour | Source |
|---|---|---|
| Gain fader | `dblevel`; limiter **after** fader (`dblimit`) | [O] |
| Mute / Mono | Mono: L=R=L+R; click again swaps L/R (`mono` 0/1/2) | [O] help BUS_MONO |
| EQ | 6-band fully parametric "EQPro-G6", A/B memory (`VoiceMeeterBUSEQMem abmem`) per channel, cells with `EQon EQtype dblevel freq Q`; plus per-channel `trim` and `delay` | [O] formats 785–809 |
| Bus mode (`BusMode`) | Normal, Mix-Down A, Mix-Down B, Repeat (stereo repeat), Composite, TV-Mix, Up-Mix 2.1/4.1/6.1, Left/Right/Center/LFE/Rear Only (help gives exact formulas) | [O] help BUS_* |
| SEL / monitor | `SEL`, `monitor`, `VMOptions MonitorOnSEL` | [O] format 1140 |

Bus-mode formulas (verbatim from shipped help, implemented 1:1):
- MixDown A: L = FL + .7FC + SW + RL − SL ; R = RL + .7FC + SW − RR + SR *(as printed)*
- MixDown B: L = FL + .7FC + SW + RL + SL ; R = RL + .7FC + SW + RR + SR *(as printed)*
- Repeat: ch1+2 copied to 3+4, 5+6, 7+8
- TV-Mix: FL=L, FR=R, FC=.2(L+R), SW=.5(L+R), RL=SL=.7(L−R), RR=SR=.7(R−L)
- Up-Mix 2.1: FL=L, FR=R, SW=.5(L+R); 4.1 adds RL=L, RR=R; 6.1 adds SL=L, SR=R
- Left/Right/Center/LFE only: both outputs = that channel; Rear only: L=RL, R=RR

## 5. Screens (windows / dialogs)

From menu strings 1279–1327, dialog titles and window class names [O]:

1. **Main window** — 5 strips, 5 buses, tape deck, VBAN button, menu, A-BOX/R-BOX status, lock icon
2. **Main menu** (Restart audio engine, Auto-restart, Load/Save settings, Load-on-startup, Reset, System tray, Run on startup, Always visible, Lock GUI, Hook volume keys, Run companion apps, Shortcut keys, Limit remote gain, Contextual help…)
3. **System Settings / Options** (`CTRL+,`) — buffering MME/WDM/KS/ASIO, preferred SR, engine mode, WDM exclusive, Patch ASIO inputs, Patch composite, Patch insert, monitoring delay, out limiter, VAIO sync
4. **Tape Recorder Options** (`CTRL+5`) — directory, prefix, pre-record time, file type, SR, bits, channels, kbps, multitrack, play-on-load, stop after duration
5. **M.I.D.I. Mapping** (`CTRL+3`) — input/output/extra devices, map name, learn, coder types, feedback, paging
6. **VBAN** (`CTRL+1`) — 8 in streams, 8 out streams, MIDI/command data in/out, video frame out
7. **VBAN Chat** (`CTRL+2`)
8. **Preset Scene** (`CTRL+4`) — store/recall/rename/comment/copy/paste/load/save presets
9. **Windows Audio Map** (`CTRL+6`)
10. **Bus EQ dialog** (EQPro-G6) — scale −18/+18 … −72/+30 dB
11. **Strip context menu** — copy/paste/reset/load/save EQ, pan, intellipan, compressor, gate
12. **About / License**, **VAIO extension license**
13. **Device selection popups** — KS / MME / WDM / ASIO sections

## 6. File formats

All XML is hand-written with `sprintf` (single-quoted attributes, tab indent, LF) [O].

| File | Root | Content | Source |
|---|---|---|---|
| `VoicemeeterBanana_LastSettings.xml`, `…Default.xml`, `…TodaySettings.xml`, `…YesterdaySettings.xml` (in `%APPDATA%`) | `VBAudioVoicemeeterSettings` | DeviceConfiguration → Parameters → BUS-EQ → Recorder → MidiMap → VBAN | [O] `FUN_0053a310` (`ev_e751f903`), strings 888–895, real sample `VoiceMeeterDefault.xml` |
| `PresetScene_NN.xml` | `VBAudioVoicemeeterPresetScene` | PresetName/Comment/TimeStamp + VoiceMeeterParameters | [O] 1261–1273 |
| Strip settings | (Strip save) | Strip comp/gate/EQ/pan lines | [O] 1536–1543 |
| MIDI map | `VBAudioVoicemeeterMIDIMapping/VoiceMeeterMidiMap` | `MidiMapItem id b1..b6 coder codertype ptt feed1 feed2 omni disabled feedOn feedOff` | [O] 660–673, 926–931 |
| VBAN config | `VBAudioVoicemeeterVBANConfig/VBANConfiguration` | VBAN, VBANStreamIn/Out, DataIn/Out, CommandIn, FrameOut | [O] 952–1009 |
| Bus EQ | `VBAudioVoicemeeterBUSEQConfig/VoiceMeeterBUSEQ` | Bus/BusEQmem cells | [O] 785–801 |
| Recordings | WAV / BWF / AIFF / MP3 in `C:\VoicemeeterRecord` default, name `Record YYYY-MM-DD at HHhMMmSSs`, multitrack `_Track%i` | [O] 746–762 |
| Log | `VoicemeeterBanana_LogFile.txt` | `[MM/JJ/YYYY-Time]-…` | [O] 888–889 |
| Registry `HKCU\…\VB-Audio\VoiceMeeter` | SysTray, ShowOnTop, WinPos_X/Y, AutoRestart, LockGUI, VolumKeys*, TransportKeys, VMB_StartupFile, … | [O] 2286–2346 |

### 6.1 Parameter block bit layout (decompiled `FUN_005391b0`, `ev_94e26796`) [O]

Strip flag word: bit0 mute, bit1 solo, bit2 mono, bit3 muc (mute centre), **bit4 prg_c,
bit5 prg_g (1-bit enables, not knob values)**, bit12 A1, bit13 A2, bit14 A3, bit16 B1,
bit17 B2, bits20–23 paneltype, bit30 vaio, bit31 greenmode. Next word: bits0–3 karaoke
mode, bits4–7 stripmode.
Bus flag word: bit0 mute, bit2 mono, bits4–7 BusMode, bit8 EQon, bit9 cross,
bit28 SEL, bit29 monitor, bit30 vaio, bit31 greenmode.
`K7BUSRoute` = tape-deck routing (same A1..B2 bit positions) + tape gain.
Section order inside `VoiceMeeterParameters`: LabelStrip1..3, LabelVirtualStrip1..2,
VMOptions, K7BUSRoute, then per strip 1..5: Strip, StripComp, StripGate, then
(HW 1..3) Intellipan line or (virtual 4..5) EQ line; Bus 1..5; LabelBus1..3,
LabelVirtualBus1..2; VoiceMeeterBUSEQ; VoiceMeeterBUSEQMem ×(abmem 0,1) per bus;
PatchComposite; PatchInsert.

**[I]** `BusMode` numbering follows the order of the action table (strings 1589–1613):
0 normal, 1 Amix, 2 Bmix, 3 Repeat, 4 Composite, 5 TVMix, 6 UpMix21, 7 UpMix41,
8 UpMix61, 9 LeftOnly, 10 RightOnly, 11 CenterOnly, 12 LFEOnly, 13 RearOnly.

## 7. Control / scripting language

The app exposes a parameter language used by macro buttons, the Remote API,
VBAN-TEXT and MIDI [O strings 1545–1661]:

```
Strip[i].Gain = -6.0;  Strip[i].Mute = 1;  Strip[i].A1 = 1;  Strip[i].B2 = 0;
Strip[i].Mono / Solo / Karaoke / MC / Comp / Gate / Limit / EQGain1..3
Strip[i].Pan_x / Pan_y / Color_x / Color_y / Fx_x / Fx_y / Mode
Bus[i].Gain / Mute / Mono / EQ / Limit / mode.normal|Amix|Bmix|Repeat|Composite|TVMix|UpMix21|UpMix41|UpMix61|LeftOnly|RightOnly|CenterOnly|LFEOnly|RearOnly
Recorder.play / stop / record / pause / rew / ff / replay / A1..B2 / load = "path";
Strip[i].Device.wdm = "name";  Bus[i].Device.mme = "name";
```

## 8. Network: VBAN

[O] strings 698–709, 941–1035; protocol is publicly documented by VB-Audio.
- UDP port 6980 default; 8 incoming / 8 outgoing audio streams; MIDI (serial) data; TEXT command stream; video frames (JPEG)
- Per stream: name (16 chars), IP, port, SR, channels, format, net-quality (NQ 0..4 = Optimal … Very Slow)
- Incoming text commands use the language from §7

## 9. MIDI

[O] 654–673, 904–937: input device, extra input device, VBAN MIDI input, output (feedback)
device; learn mode; message types Note On/Off, CC, Program change, Pitch bend;
control types Linear / Coder offset / Coder sign-bit (Mackie); per-item feedback values.
Control IDs: `InGainFader1..3, VInGainFader1..2, OutGainFader1..3, VOutGainFader1..2,
InMute*, InSolo*, InComp*, InGate*, InLimit*, StripNA1.., OutMute*, OutEqOn*, BusLimit*,
LRPan*, VIn*EQHigh/Med/Bass, Tape*` [O 1337–1494].

## 10. Not reproducible / intentionally excluded

| Feature | Reason |
|---|---|
| VAIO virtual audio drivers (kernel `.sys`), virtual ASIO driver, ASIO insert | Kernel-mode/signed drivers, out of scope for an app rebuild. The rebuild routes virtual strips/buses through any existing endpoint (e.g. VB-CABLE). |
| License activation, footprint, challenge/response, online activation | Copy-protection; deliberately not replicated. |
| Voicemeeter Remote API (shared-memory IPC used by `VoicemeeterRemote.dll`) | Undocumented proprietary IPC; replaced by a local UDP/VBAN-TEXT command port. |
| Skin bitmaps | Obfuscated + copyrighted; new UI drawn instead. |
| KS / MME / ASIO back-ends | Rebuild uses WASAPI (shared mode) via `cpal`. |
