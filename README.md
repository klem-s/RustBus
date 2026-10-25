# RustBus

RustBus is a virtual audio mixer written in Rust: a clean-room reimplementation of Voicemeeter-Banana-style behaviour for
Windows. It has 3 hardware strips, 3 virtual strips, buses A1–A3 and B1–B3 (the 3rd virtual strip and B3 go beyond Banana), a tape deck,
VBAN, MIDI mapping and preset scenes. It reads and writes the same settings XML format.

- **docs/ANALYSIS.md** — reverse-engineering report on `voicemeeterpro.exe`: features,
  screens, file formats, mechanics, and evidence.
- **FEATURES.md** — what is done, partial, not done, or excluded.

No original code, artwork or help text is used. The analysis recorded the behaviour,
and this code is new.

## Build

Requirements (already installed on this machine):
- rustup with `stable-x86_64-pc-windows-gnu`
- WinLibs MinGW-w64 (`winget install BrechtSanders.WinLibs.POSIX.UCRT`) for `dlltool`/`as`

```powershell
.\build.ps1                 # debug build
.\build.ps1 -Release        # target\release\voicemeeter-rs.exe
.\build.ps1 test            # unit tests
.\build.ps1 run             # run the app
```

## Using it without the VB-Audio drivers

The original's virtual inputs and B buses are VB-Audio kernel drivers (VAIO). In this
rebuild you bind them to any Windows endpoint instead:
- Virtual input strip → choose a capture device that applications play into
  (e.g. "CABLE Output" of VB-CABLE).
- B1/B2/B3 → choose a render device that other applications record from.

A1 is the master clock. Without an A1 device the engine runs on an internal 48 kHz clock.

## Remote control

Send parameter scripts as VBAN-TEXT packets (VBAN dialog → "Incoming VBAN-TEXT commands"),
or type them in Menu → Command Console:

```
Strip[0].Gain = -6; Strip[3].B1 = 1; Bus[0].mode.TVMix = 1; Recorder.record = 1;
```

Settings are stored in `%APPDATA%\VoicemeeterRS`, separate from the original application's.

## Fonts

The UI bundles **Inter** and **JetBrains Mono** (`assets/fonts`), both under the SIL Open Font License; the licence texts are next to the font files.
