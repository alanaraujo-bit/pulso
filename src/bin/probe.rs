//! Hardware probe: prints every Logitech device Pulso can read. `cargo run --bin probe`
use pulso::{centurion, compx, hidpp};

fn main() {
    let api = hidapi::HidApi::new().expect("hidapi");
    for d in api.device_list().filter(|d| d.vendor_id() == compx::VID) {
        println!("compx col: pid={:04x} up={:04x} u={:04x} {:?}", d.product_id(), d.usage_page(), d.usage(), d.product_string());
    }
    for (pid, mut dg) in compx::open_dongles(&api) {
        let t = std::time::Instant::now();
        let r = dg.read_battery();
        println!("== CompX pid={pid:04x} model={:04x?} battery={r:?} ({} ms)", dg.model, t.elapsed().as_millis());
    }
    for (pid, mut ch) in hidpp::open_channels(&api) {
        println!("== HID++ channel pid={pid:04x}");
        for idx in [0xFFu8, 1, 2, 3, 4, 5, 6] {
            let t = std::time::Instant::now();
            match ch.feature_index(idx, hidpp::F_DEVICE_NAME) {
                Ok(Some(fi)) => {
                    let name = ch.device_name(idx, fi);
                    let bat = hidpp::find_battery(&mut ch, idx);
                    print!("  idx {idx:#04x}: {name:?} battery_feature={bat:?}");
                    if let Ok(Some(b)) = bat {
                        print!(" -> {:?}", hidpp::read_battery(&mut ch, idx, b));
                    }
                    println!("  ({} ms)", t.elapsed().as_millis());
                }
                Ok(None) => println!("  idx {idx:#04x}: no DeviceName feature"),
                Err(e) => println!("  idx {idx:#04x}: {e:?} ({} ms)", t.elapsed().as_millis()),
            }
        }
        println!("  events: {:?}", ch.events);
    }
    for (pid, name, mut hs) in centurion::open_headsets(&api) {
        let t = std::time::Instant::now();
        println!("== Centurion pid={pid:04x} {name}");
        println!("  connected: {:?}", hs.headset_connected());
        println!("  battery: {:?} ({} ms)", hs.read_battery(), t.elapsed().as_millis());
        let t = std::time::Instant::now();
        println!("  battery again: {:?} ({} ms)", hs.read_battery(), t.elapsed().as_millis());
    }
}
