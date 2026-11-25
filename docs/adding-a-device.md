# Adding a device

This guide walks through teaching Pulso a new device, from "it does not show up" to a merged PR.
You do not need to finish every step: a device request with good probe output (step 1 and 2)
is already a real contribution.

## 1. Check what Windows sees

Turn the device on, wake it up (move it, press a key) and run:

```powershell
cargo run --bin probe -- --all
```

The first block lists every HID collection on the machine:

```
046d:c543  if=2   ff00:0002  USB Receiver
3554:fa09  if=1   ff02:0002  2.4G Wireless Receiver
```

Unplug the receiver, run it again and compare: the lines that disappear belong to your device.
Battery data almost always lives on a **vendor-defined usage page** (`ff00` to `ffff`).

The probe never writes to devices it does not already support, so this is safe to run.

## 2. Confirm the battery is readable at all

Pulso can only read what the device reports. Check that the official software (or the vendor's
web driver) shows the battery level. If nothing official can see it, there is nothing to read.

At this point, open a [device request](https://github.com/alanaraujo-bit/pulso/issues/new?template=device-support.yml)
with the probe output, the exact model name, the connection type and what the official app shows.

## 3. Find the protocol

In order of effort:

1. **Known protocol, new product.** Many brands share chipsets. Logitech uses HID++ or Centurion,
   lots of Chinese 2.4G keyboards use CompX dongles (`3554:*`). Try the existing module first:
   adding a product id or a model code is often the whole change.
2. **Existing open source.** Search GitHub for the VID:PID (`"3554" "fa09"`, `"0x0af7"`). Projects
   such as HeadsetControl, Solaar, OpenRGB and vigil-watt document many protocols. Learn the frame
   layout from their docs and tests; do not copy GPL code into this MIT project.
3. **Web drivers.** Many vendors ship a WebHID driver site. Its JavaScript shows the exact
   battery query and reply format.
4. **USB capture.** Install [USBPcap](https://desowin.org/usbpcap/) and Wireshark, open the official
   app, and look for the request that precedes a reply containing the percentage (look for the
   current battery value as a byte, e.g. `0x56` for 86%). Change the charge state and capture
   again to find the charging flag.

Write down: report id, report length, the request bytes, the reply bytes, which byte is the
percentage and which bit means charging. Capture real frames for several levels and for
charging.

## 4. Implement it

Create `src/<protocol>.rs` following the existing modules (`compx.rs` is the smallest):

- A predicate `is_<thing>(d: &hidapi::DeviceInfo) -> bool` matching VID, PID, usage page and usage.
- A pure `parse(...)` that turns reply bytes into `BatterySample { percent, charge, voltage_mv }`.
  Reject percentages above 100. Sleeping devices often report `0` or `1`: treat those as asleep.
- A small struct holding the `HidDevice` with `read_battery()` and, if the device pushes updates
  on its own, `pump(wait)` to collect them.
- Tests with the **real frames** you captured, in hex.

Then wire it in:

- `src/lib.rs`: `pub mod <protocol>;`
- `src/monitor.rs`: open matching collections in the enumeration block and add a
  `poll_<thing>` function that pushes `Reading`s (look at `poll_dongle`).
  Use a stable `id` (it keys the battery history on disk).
- `src/estimate.rs`: add the model to `prior()` with its rated battery life (hours) and charge
  speed (% per hour) from the spec sheet.
- `src/bin/probe.rs`: print the new device so others can test it.

## 5. Test on hardware and open the PR

- `cargo run --bin probe` reads the device.
- `cargo run`: the device shows in the panel and the tray, wakes and sleeps correctly, and the
  charging state flips when you plug the cable in.
- `cargo fmt`, `cargo clippy --all-targets -- -D warnings`, `cargo test --lib`.

In the PR, list the hardware you tested, the frames you used and anything you could not verify
(for example "full-charge state not seen yet").
