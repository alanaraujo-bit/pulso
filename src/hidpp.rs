//! Logitech HID++ 2.0 over a LIGHTSPEED / Unifying / Bolt receiver or a wired device.
//!
//! Only the read-only features a battery panel needs: Root (0x0000), DeviceName (0x0005),
//! UnifiedBattery (0x1004), BatteryStatus (0x1000) and BatteryVoltage (0x1001).

use crate::model::{BatterySample, Charge, Kind};
use hidapi::{HidApi, HidDevice};
use std::time::{Duration, Instant};

const REPORT_SHORT: u8 = 0x10;
const REPORT_LONG: u8 = 0x11;
const LONG_LEN: usize = 20;
// Any nibble 1..=15 works; picked to stay clear of G HUB / Solaar defaults (1 and 8).
const SW_ID: u8 = 0x0B;
const TIMEOUT: Duration = Duration::from_millis(600);

pub const F_ROOT: u16 = 0x0000;
pub const F_DEVICE_NAME: u16 = 0x0005;
pub const F_BATTERY_STATUS: u16 = 0x1000;
pub const F_BATTERY_VOLTAGE: u16 = 0x1001;
pub const F_UNIFIED_BATTERY: u16 = 0x1004;

#[derive(Debug)]
pub enum Error {
    Io(String),
    Timeout,
    /// HID++ error code from the device or receiver.
    Device(u8),
    Unsupported,
}

impl From<hidapi::HidError> for Error {
    fn from(e: hidapi::HidError) -> Self {
        Error::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// HID++ 1.0 error 0x09 (ERR_UNKNOWN_DEVICE / device not paired or not reachable) and
/// HID++ 2.0 timeouts both mean "nobody home at this index right now".
pub fn is_unreachable(e: &Error) -> bool {
    matches!(e, Error::Timeout | Error::Device(0x09) | Error::Device(0x0B))
}

/// Unsolicited message seen while waiting (battery broadcasts, connect/disconnect).
#[derive(Debug, Clone)]
pub struct Event {
    pub index: u8,
    pub feature_index: u8,
    pub function: u8,
    pub data: Vec<u8>,
}

/// Receiver notification 0x41: a paired device connected (woke up) or lost its link.
pub const DEVICE_CONNECTION: u8 = 0x41;

pub struct Channel {
    dev: HidDevice,
    /// Short-report collection of the same receiver: carries 0x41 connect/disconnect notifications.
    short: Option<HidDevice>,
    pub events: Vec<Event>,
}

impl Channel {
    pub fn new(dev: HidDevice, short: Option<HidDevice>) -> Self {
        Self { dev, short, events: Vec::new() }
    }

    /// `Some(true)` = link up, `Some(false)` = link lost, for a 0x41 event.
    pub fn connection(ev: &Event) -> Option<bool> {
        (ev.feature_index == DEVICE_CONNECTION && ev.data.len() >= 2).then(|| ev.data[1] & 0x40 == 0)
    }

    /// Sends a long request and waits for the matching reply. Unrelated reports are kept as events.
    pub fn request(&mut self, index: u8, feature_index: u8, function: u8, params: &[u8]) -> Result<[u8; 16]> {
        let mut out = [0u8; LONG_LEN];
        out[0] = REPORT_LONG;
        out[1] = index;
        out[2] = feature_index;
        out[3] = (function << 4) | SW_ID;
        out[4..4 + params.len().min(16)].copy_from_slice(&params[..params.len().min(16)]);
        self.dev.write(&out)?;

        let deadline = Instant::now() + TIMEOUT;
        let mut buf = [0u8; 64];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(Error::Timeout);
            }
            let n = self.dev.read_timeout(&mut buf, left.as_millis() as i32)?;
            if n < 7 || (buf[0] != REPORT_LONG && buf[0] != REPORT_SHORT) || buf[1] != index {
                continue;
            }
            // HID++ 1.0 error: 10 idx 8F <sub_id> <addr> <code>
            if buf[2] == 0x8F && buf[3] == feature_index {
                return Err(Error::Device(buf[5]));
            }
            // HID++ 2.0 error: 11 idx FF <feat> <fn|sw> <code>
            if buf[2] == 0xFF && buf[3] == feature_index && buf[4] == out[3] {
                return Err(Error::Device(buf[5]));
            }
            if buf[2] == feature_index && buf[3] == out[3] {
                let mut r = [0u8; 16];
                r.copy_from_slice(&buf[4..20]);
                return Ok(r);
            }
            if buf[3] & 0x0F == 0 {
                self.events.push(Event {
                    index: buf[1],
                    feature_index: buf[2],
                    function: buf[3] >> 4,
                    data: buf[4..n].to_vec(),
                });
            }
        }
    }

    /// Waits up to `wait` for unsolicited reports and stores them in `events`.
    pub fn pump(&mut self, wait: Duration) -> Result<()> {
        let mut buf = [0u8; 64];
        let half = (wait.as_millis() as i32 / 2).max(1);
        if let Some(short) = &self.short {
            // Drain everything queued on the short collection; only 0x41 matters there.
            loop {
                let n = short.read_timeout(&mut buf, 0)?;
                if n == 0 {
                    break;
                }
                if n >= 7 && buf[0] == REPORT_SHORT && buf[2] == DEVICE_CONNECTION {
                    self.events.push(Event {
                        index: buf[1],
                        feature_index: DEVICE_CONNECTION,
                        function: 0,
                        data: buf[3..n].to_vec(),
                    });
                }
            }
        }
        let n = self.dev.read_timeout(&mut buf, if self.short.is_some() { half } else { half * 2 })?;
        if n >= 7 && (buf[0] == REPORT_LONG || buf[0] == REPORT_SHORT) && buf[3] & 0x0F == 0 {
            self.events.push(Event {
                index: buf[1],
                feature_index: buf[2],
                function: buf[3] >> 4,
                data: buf[4..n].to_vec(),
            });
        }
        Ok(())
    }

    /// Root.getFeature: returns the feature index, or None when the device lacks it.
    pub fn feature_index(&mut self, index: u8, feature: u16) -> Result<Option<u8>> {
        let r = self.request(index, 0x00, 0, &feature.to_be_bytes())?;
        Ok(if r[0] == 0 && feature != F_ROOT { None } else { Some(r[0]) })
    }

    pub fn device_name(&mut self, index: u8, fidx: u8) -> Result<(String, Kind)> {
        let len = self.request(index, fidx, 0, &[])?[0] as usize;
        let mut name = Vec::with_capacity(len);
        while name.len() < len {
            let chunk = self.request(index, fidx, 1, &[name.len() as u8])?;
            let take = (len - name.len()).min(16);
            name.extend_from_slice(&chunk[..take]);
        }
        let kind = match self.request(index, fidx, 2, &[]) {
            Ok(r) => kind_from_hidpp(r[0]),
            Err(_) => Kind::Other,
        };
        let name = String::from_utf8_lossy(&name).trim_end_matches('\0').trim().to_string();
        Ok((name, kind))
    }
}

pub fn kind_from_hidpp(t: u8) -> Kind {
    match t {
        0 => Kind::Keyboard,
        1 | 2 => Kind::Other, // remote control / numpad
        3..=5 => Kind::Mouse, // mouse / trackpad / trackball
        8 => Kind::Headset,
        _ => Kind::Other,
    }
}

/// Which battery feature a device exposes, with its index.
#[derive(Debug, Clone, Copy)]
pub enum BatteryFeature {
    Unified(u8),
    Status(u8),
    Voltage(u8),
}

impl BatteryFeature {
    pub fn index(self) -> u8 {
        match self {
            Self::Unified(i) | Self::Status(i) | Self::Voltage(i) => i,
        }
    }
}

pub fn find_battery(ch: &mut Channel, index: u8) -> Result<Option<BatteryFeature>> {
    if let Some(i) = ch.feature_index(index, F_UNIFIED_BATTERY)? {
        return Ok(Some(BatteryFeature::Unified(i)));
    }
    if let Some(i) = ch.feature_index(index, F_BATTERY_STATUS)? {
        return Ok(Some(BatteryFeature::Status(i)));
    }
    if let Some(i) = ch.feature_index(index, F_BATTERY_VOLTAGE)? {
        return Ok(Some(BatteryFeature::Voltage(i)));
    }
    Ok(None)
}

pub fn read_battery(ch: &mut Channel, index: u8, feat: BatteryFeature) -> Result<BatterySample> {
    let fn_status = match feat {
        BatteryFeature::Unified(_) => 1, // getStatus
        _ => 0,                          // getBatteryLevelStatus / getBatteryVoltage
    };
    let r = ch.request(index, feat.index(), fn_status, &[])?;
    parse_battery(feat, &r).ok_or(Error::Unsupported)
}

/// Decodes either a getStatus reply or a broadcast event (same layout) for `feat`.
pub fn parse_battery(feat: BatteryFeature, r: &[u8]) -> Option<BatterySample> {
    match feat {
        BatteryFeature::Unified(_) => {
            // [soc%, level flags, charging status, external power]
            let mut pct = r[0];
            if pct == 0 {
                // Devices without SoC report only a level bucket.
                pct = match r[1] {
                    f if f & 0x08 != 0 => 90,
                    f if f & 0x04 != 0 => 50,
                    f if f & 0x02 != 0 => 20,
                    f if f & 0x01 != 0 => 5,
                    _ => 0,
                };
            }
            let charge = match r[2] {
                1 | 2 => Charge::Charging,
                3 => Charge::Full,
                _ => Charge::Discharging,
            };
            Some(BatterySample { percent: pct.min(100), charge, voltage_mv: None })
        }
        BatteryFeature::Status(_) => {
            let charge = match r[2] {
                1 | 2 | 4 => Charge::Charging,
                3 => Charge::Full,
                _ => Charge::Discharging,
            };
            Some(BatterySample { percent: r[0].min(100), charge, voltage_mv: None })
        }
        BatteryFeature::Voltage(_) => {
            let mv = u16::from_be_bytes([r[0], r[1]]);
            let flags = r[2];
            let charge = if flags & 0x80 != 0 {
                match flags & 0x07 {
                    2 => Charge::Full,
                    _ => Charge::Charging,
                }
            } else {
                Charge::Discharging
            };
            Some(BatterySample { percent: voltage_to_percent(mv), charge, voltage_mv: Some(mv) })
        }
    }
}

/// Li-ion discharge curve used by Logitech G mice (from G HUB behaviour, close to Solaar's table).
pub fn voltage_to_percent(mv: u16) -> u8 {
    const CURVE: [(u16, u8); 11] = [
        (4186, 100),
        (4067, 90),
        (3989, 80),
        (3922, 70),
        (3859, 60),
        (3811, 50),
        (3778, 40),
        (3751, 30),
        (3717, 20),
        (3671, 10),
        (3500, 0),
    ];
    if mv >= CURVE[0].0 {
        return 100;
    }
    for w in CURVE.windows(2) {
        let (hi_v, hi_p) = w[0];
        let (lo_v, lo_p) = w[1];
        if mv >= lo_v {
            let t = (mv - lo_v) as f32 / (hi_v - lo_v) as f32;
            return (lo_p as f32 + t * (hi_p - lo_p) as f32).round() as u8;
        }
    }
    0
}

/// HID++ long-report collection of a receiver or wired device.
pub fn is_channel(d: &hidapi::DeviceInfo) -> bool {
    d.vendor_id() == 0x046d && d.usage_page() == 0xFF00 && d.usage() == 0x0002
}

/// Opens the short-report sibling (usage 0x0001) of a long collection: same device instance,
/// which on Windows shows up as `...&MI_02&Col01#8&993b888&0&0000#...` vs `...&Col02#8&993b888&0&0001#...`.
pub fn open_short(api: &HidApi, long: &hidapi::DeviceInfo) -> Option<HidDevice> {
    let key = |p: &std::ffi::CStr| -> Option<(String, String)> {
        let s = p.to_string_lossy().to_lowercase();
        let mut parts = s.split('#').skip(1);
        let hw = parts.next()?.split("&col").next()?.to_string();
        let inst = parts.next()?.rsplit_once('&')?.0.to_string();
        Some((hw, inst))
    };
    let target = key(long.path())?;
    api.device_list()
        .find(|d| {
            d.vendor_id() == 0x046d
                && d.usage_page() == 0xFF00
                && d.usage() == 0x0001
                && key(d.path()).as_ref() == Some(&target)
        })
        .and_then(|d| d.open_device(api).ok())
}

pub fn open_channels(api: &HidApi) -> Vec<(u16, Channel)> {
    api.device_list()
        .filter(|d| is_channel(d))
        .filter_map(|d| d.open_device(api).ok().map(|h| (d.product_id(), Channel::new(h, open_short(api, d)))))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voltage_curve() {
        assert_eq!(voltage_to_percent(4200), 100);
        assert_eq!(voltage_to_percent(3811), 50);
        assert_eq!(voltage_to_percent(3400), 0);
        let mid = voltage_to_percent(3955);
        assert!(mid > 70 && mid < 80);
    }

    #[test]
    fn unified_parse() {
        let s = parse_battery(BatteryFeature::Unified(6), &[86, 0x08, 0, 0]).unwrap();
        assert_eq!((s.percent, s.charge), (86, Charge::Discharging));
        let s = parse_battery(BatteryFeature::Unified(6), &[40, 0x04, 1, 1]).unwrap();
        assert_eq!(s.charge, Charge::Charging);
        let s = parse_battery(BatteryFeature::Unified(6), &[0, 0x02, 0, 0]).unwrap();
        assert_eq!(s.percent, 20);
    }
}
