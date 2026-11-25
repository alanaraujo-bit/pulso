//! Hardware probe.
//!
//!   cargo run --bin probe          read every device Pulso supports
//!   cargo run --bin probe -- --all also list every HID collection on the machine (paste this
//!                                  into a device-support issue; it sends nothing to unknown devices)
use pulso::{centurion, compx, hidpp};

fn main() {
    let api = hidapi::HidApi::new().expect("hidapi");

    if std::env::args().any(|a| a == "--all") {
        println!("== All HID collections (vid:pid  interface  usage_page:usage  product)");
        let mut rows: Vec<String> = api
            .device_list()
            .map(|d| {
                format!(
                    "{:04x}:{:04x}  if={:<2}  {:04x}:{:04x}  {}",
                    d.vendor_id(),
                    d.product_id(),
                    d.interface_number(),
                    d.usage_page(),
                    d.usage(),
                    d.product_string().unwrap_or("").trim()
                )
            })
            .collect();
        rows.sort();
        rows.dedup();
        rows.iter().for_each(|r| println!("  {r}"));
        println!();
    }

    for (pid, mut dg) in compx::open_dongles(&api) {
        let t = std::time::Instant::now();
        let r = dg.read_battery();
        println!(
            "== CompX dongle {:04x}:{pid:04x} model={:04x?} ({}) battery={r:?} ({} ms)",
            compx::VID,
            dg.model,
            compx::model_name(dg.model),
            t.elapsed().as_millis()
        );
    }

    for (pid, mut ch) in hidpp::open_channels(&api) {
        println!("== Logitech HID++ 046d:{pid:04x}");
        for idx in [1u8, 2, 3, 4, 5, 6, 0xFF] {
            let t = std::time::Instant::now();
            match ch.feature_index(idx, hidpp::F_DEVICE_NAME) {
                Ok(Some(fi)) => {
                    let name = ch.device_name(idx, fi);
                    let bat = hidpp::find_battery(&mut ch, idx);
                    print!("  slot {idx:#04x}: {name:?} battery_feature={bat:?}");
                    if let Ok(Some(b)) = bat {
                        print!(" -> {:?}", hidpp::read_battery(&mut ch, idx, b));
                    }
                    println!("  ({} ms)", t.elapsed().as_millis());
                }
                Ok(None) => println!("  slot {idx:#04x}: no DeviceName feature"),
                Err(e) => println!("  slot {idx:#04x}: {e:?} (empty, asleep or off)"),
            }
        }
    }

    for (pid, name, mut hs) in centurion::open_headsets(&api) {
        let t = std::time::Instant::now();
        println!("== Logitech Centurion 046d:{pid:04x} {name}");
        println!("  connected: {:?}", hs.headset_connected());
        println!("  battery: {:?} ({} ms)", hs.read_battery(), t.elapsed().as_millis());
    }
}
