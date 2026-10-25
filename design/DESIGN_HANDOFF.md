# RustBus — UI redesign handoff

`rustbus-mixer.html` (same folder) is the reference. It's a single self-contained file: open it in a browser to see and click through the design. It's a mockup, not production code; port it to the app's real UI framework.

## Prompt for Claude Code

> Restyle the RustBus mixer UI to match `design/rustbus-mixer.html` and `design/DESIGN_HANDOFF.md`.
> Keep all existing functionality and the audio/backend code unchanged — this is a visual and layout change only.
> First identify which UI framework the project uses, then map the design tokens below into a theme/style module, and rebuild the top bar, input strips, bus strips and player bar to match the reference.
> Wire every control to the existing state (routing, mono/solo/mute/EQ, faders, knobs, pan, device selectors, transport, record). Don't invent new features.
> Show me a screenshot or describe any place where the framework can't match the design exactly.

## Design tokens

| Token | Value | Use |
|---|---|---|
| bg | `#0A0C0F` | window background |
| topbar | `#0D0F13` | header background, divider `#1A1E24` below |
| section | `#111419` | group panels (border `#1E232A`, radius 16) |
| card | `#171B21` | strips, player (border `#232830`, radius 12) |
| well | `#12151A` | buttons, selects, chips at rest |
| sunken | `#0F1216` | pan pad, meter tracks, fader track |
| border-ctl | `#262B33` | control borders; hover `#3A424D` |
| text | `#EEF1F4` | primary |
| text-2 | `#AEB6C2` | secondary, section titles |
| text-3 | `#8B93A1` | labels, muted, idle strips |
| accent-a | `#5EEAD4` (text on it `#06201C`) | A buses (hardware outs), EQ on, status, meter base |
| accent-b | `#A78BFA` (text on it `#170C33`) | B buses (virtual outs), virtual-input knobs |
| amber | `#FBBF24` (text on it `#2A1B00`) | Mono / Solo on, meter −6 dB zone |
| red | `#F87171` (text on it `#2B0606`) | Mute on, Rec on, meter clip zone |

Type: **Inter** 400/500/600/700 for UI; **JetBrains Mono** 500/600 for numbers (dB, sample rate, bus labels, time).
Sizes: strip name 14/600, body 12–13, labels 10–11, section titles 12/600 uppercase +0.08em tracking.
Spacing: 4 px grid; strip padding 12, gap 12; section padding 16; 10 px between strips, 16 px between sections.

## Layout

1. **Top bar** (height ~56): logo + "RustBus / Virtual audio mixer" · Menu, VBAN buttons · status pill (dot, `48 kHz`, clock source) · right side: "Hardware out" + A1/A2/A3 pills (label badge teal when assigned, grey when not).
2. **Body**: three section panels in a row, horizontally scrollable when the window is narrow:
   - Hardware inputs: 3 strips, 148 px wide
   - Virtual inputs: 3 strips, 148 px wide
   - Master: player bar on top, then 6 bus strips, 124 px wide

## Components

**Input strip** (top → bottom): colour dot + name · device select (30 px, chevron) · knobs (hardware: Comp, Gate; virtual: Treble, Mid, Bass) · "Pan" label (+ Color/3D/FX segmented control on hardware) and a 76 px XY pan pad with a crosshair and a 12 px puck · 3×2 routing grid A1 A2 A3 / B1 B2 B3 (26 px chips) · fader row: stereo meter (2 × 5 px bars, 180 px tall) + fader (24 px wide, 12 px thumb) + side column (dB readout, Mono, Solo, Mute, optional extra button such as M.C. or K).

**Bus strip**: dot + name · "PHYSICAL"/"VIRTUAL" tag · device select · mode select ("Mode · Normal") · meter (236 px) + fader · dB readout · 3-up buttons M / EQ / mute icon.

**Knob**: 270° arc (from 135° to 405°), 3 px track `#262B33`, value arc in the accent colour, 20 px inner disc `#222830`, white pointer line.

**Fader**: range −60 … +12 dB, so 0 dB sits at 83.3% of the height. Readout format `+0.0 dB` / `−6.0 dB` (true minus sign).

**Meter**: a gradient fixed to the full meter height (accent-a up to 68%, amber at 84%, red at 100%), revealed from the bottom by level, so colour reflects absolute level. A muted strip dims its meter to 25% opacity.

**States**: idle strip (no device) shows name in text-3 and a grey dot; route chip on = filled accent-a (A) or accent-b (B); Mono/Solo on = amber; Mute/Rec on = red; focus ring 2 px accent-a, 2 px offset. Minimum control height 26 px; transport buttons 36 px.

**Player bar**: prev · play/pause (filled white) · stop · next · Rec (red outline, filled when recording) · file/progress area with `00:00 / 00:00` · 6-up routing chips · Gain knob.

## Implementation notes (egui port)

- Tokens live in `src/gui/theme.rs`; custom-painted widgets in `src/gui/widgets.rs`; layout in `src/gui/mod.rs`.
- Fonts are bundled from `assets/fonts` (SIL OFL). egui has no font weights, so each weight is its own font family.
- Known differences from the mockup are listed in the commit/hand-back notes and in FEATURES.md ("UI redesign").
