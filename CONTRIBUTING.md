# Contributing to Pulso

Thanks for helping. Issues and pull requests are welcome in **English or Portuguese**.

## Ways to help

| You have | Do this |
|---|---|
| A device Pulso does not read | Open a [device request](https://github.com/alanaraujo-bit/pulso/issues/new?template=device-support.yml) with the probe output. No code needed. |
| A device that reads wrong (percent, charging, asleep) | Open a [bug report](https://github.com/alanaraujo-bit/pulso/issues/new?template=bug.yml) and compare with the official app. |
| Time to code | Pick an issue labeled [`good first issue`](https://github.com/alanaraujo-bit/pulso/labels/good%20first%20issue) or [`help wanted`](https://github.com/alanaraujo-bit/pulso/labels/help%20wanted), or add a protocol following [the guide](docs/adding-a-device.md). |
| Design or language skills | Translations, panel polish, icons. Start a thread in Discussions first. |

## Setup

- Windows 11, [Rust](https://rustup.rs) with the MSVC toolchain.
- Node.js only if you want to build the installer.

```powershell
git clone https://github.com/alanaraujo-bit/pulso
cd pulso
cargo test --lib
cargo run              # debug build, prints every reading to the console
```

Run only one Pulso at a time: a second launch just opens the running instance's panel.

## Project layout

```
src/
  hidpp.rs       Logitech HID++ 2.0 (receivers and wired devices)
  centurion.rs   Logitech Centurion (recent LIGHTSPEED headsets)
  compx.rs       CompX 2.4 GHz keyboard dongles (AULA and others)
  monitor.rs     background thread: owns HID handles, slow polling, event listening
  estimate.rs    per-device history, runtime / charge-time model, alerts
  model.rs       shared types (Reading, BatterySample, Kind, Charge)
  sys.rs         Win32: tray glyph rendering, theme, autostart, tray promotion, DWM corners
  main.rs        Tauri app: tray icons, panel window lifecycle, commands
  bin/probe.rs   hardware diagnostics
ui/              panel (plain HTML, CSS, JS)
dev/             mock preview of the panel for UI work without hardware
docs/            guides and README assets
```

Data flow: protocol modules turn HID reports into `Reading`s, `monitor.rs` emits them,
`estimate.rs` turns them into `DeviceView`s, and `main.rs` pushes those to the tray and the panel.

## Guidelines

- **Small, complete changes.** Solve the whole problem with the least new code. Reuse what the
  codebase already has before adding a dependency.
- **Protocol code is read-only.** Never send commands that change device settings. If a battery
  query needs a write (like a session-open frame), document why in the module header.
- **Every parser gets a test with real frames** captured from hardware. Put the hex in the test.
- **Never call a window API while holding the state mutex** in `main.rs`: moving or resizing the
  window fires events synchronously and their handlers take the same lock.
- **Comments explain why**, not what.
- Keep it light: the resting app must stay a single small process. Measure RAM and CPU if your
  change touches the monitor loop or the panel lifecycle.

Before opening a PR:

```powershell
cargo fmt
cargo clippy --all-targets -- -D warnings
cargo test --lib
```

CI runs the same three on Windows.

## Commits and pull requests

- [Conventional Commits](https://www.conventionalcommits.org): `feat:`, `fix:`, `docs:`,
  `refactor:`, `chore:`. Example: `feat(compx): read battery from MCHOSE G75`.
- One topic per PR. Describe what you tested and on which hardware.
- UI changes: add a before/after screenshot.

## Releases

Maintainers tag `vX.Y.Z`; the release workflow builds the installer and attaches it to a
GitHub release.

## Code of conduct

Be kind and assume good faith. See [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
