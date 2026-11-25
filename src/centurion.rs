//! Logitech "Centurion" protocol used by recent LIGHTSPEED headsets (G PRO X 2, G522...).
//!
//! The dongle exposes a HID++-like feature table over 64-byte frames on usage page 0xFFA0.
//! The headset itself sits behind a bridge feature (0x0003); its requests are wrapped in a
//! bridge "send fragment" message and its replies come back as bridge message events.
//! Protocol layout learned from HeadsetControl's documentation of the PRO X 2 (GPL code not copied).

use crate::model::{BatterySample, Charge};
use hidapi::{HidApi, HidDevice};
use std::time::{Duration, Instant};

const FRAME: usize = 64;
const SW_ID: u8 = 0x0B;
const BRIDGE_FN_SEND: u8 = 0x10;
const TIMEOUT: Duration = Duration::from_millis(900);

pub const F_ROOT: u16 = 0x0000;
pub const F_BRIDGE: u16 = 0x0003;
pub const F_BATTERY_SOC: u16 = 0x0104;

#[derive(Debug)]
pub enum Error {
    Io(String),
    Timeout,
    Offline,
    Unsupported,
    Protocol(&'static str),
}

impl From<hidapi::HidError> for Error {
    fn from(e: hidapi::HidError) -> Self {
        Error::Io(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

pub enum Event {
    Connection(bool),
    Battery(BatterySample),
}

pub struct Headset {
    dev: HidDevice,
    prefix: Vec<u8>,
    bridge: Option<u8>,
    battery_idx: Option<u8>,
    pub events: Vec<Event>,
}

/// Frame: <prefix> <len = payload+1> <flags> <payload...>
fn frame(prefix: &[u8], payload: &[u8], flags: u8) -> [u8; FRAME] {
    let mut f = [0u8; FRAME];
    let h = prefix.len();
    f[..h].copy_from_slice(prefix);
    f[h] = (payload.len() + 1) as u8;
    f[h + 1] = flags;
    let n = payload.len().min(FRAME - h - 2);
    f[h + 2..h + 2 + n].copy_from_slice(&payload[..n]);
    f
}

fn unframe<'a>(prefix: &[u8], f: &'a [u8]) -> Option<&'a [u8]> {
    let h = prefix.len();
    if f.len() < h + 3 || &f[..h] != prefix {
        return None;
    }
    let len = f[h] as usize;
    if len <= 1 || len + h + 1 > f.len() {
        return None;
    }
    Some(&f[h + 2..h + 1 + len])
}

pub fn parse_battery(r: &[u8]) -> Option<BatterySample> {
    let pct = *r.first()?;
    if pct > 100 {
        return None;
    }
    let charge = match r.get(2).copied().unwrap_or(0) {
        1 | 2 => Charge::Charging,
        3 => Charge::Full,
        _ => Charge::Discharging,
    };
    Some(BatterySample { percent: pct, charge, voltage_mv: None })
}

impl Headset {
    pub fn new(dev: HidDevice) -> Self {
        Self { dev, prefix: vec![0x51], bridge: None, battery_idx: None, events: Vec::new() }
    }

    fn read(&mut self, wait: Duration, buf: &mut [u8; FRAME]) -> Result<Option<Vec<u8>>> {
        let n = self.dev.read_timeout(buf, wait.as_millis() as i32)?;
        if n == 0 {
            return Ok(None);
        }
        Ok(unframe(&self.prefix, &buf[..n]).map(|p| p.to_vec()))
    }

    /// Records unsolicited bridge traffic (connect/disconnect, battery broadcasts).
    fn note_event(&mut self, reply: &[u8]) -> bool {
        let Some(b) = self.bridge else { return false };
        if reply.len() >= 4 && reply[0] == b && reply[1] == 0x00 {
            self.events.push(Event::Connection(reply[3] != 0));
            return true;
        }
        // Bridge message event with software id 0 = headset-originated notification.
        if reply.len() >= 8 && reply[0] == b && reply[1] == BRIDGE_FN_SEND && reply[4] == 0 && reply[6] & 0x0F == 0 {
            if Some(reply[5]) == self.battery_idx {
                if let Some(s) = parse_battery(&reply[7..]) {
                    self.events.push(Event::Battery(s));
                }
            }
            return true;
        }
        false
    }

    /// Request to a feature of the dongle itself.
    fn direct(&mut self, feature_index: u8, function: u8, params: &[u8]) -> Result<Vec<u8>> {
        let fsw = (function & 0xF0) | SW_ID;
        let mut payload = vec![feature_index, fsw];
        payload.extend_from_slice(params);
        self.dev.write(&frame(&self.prefix, &payload, 0))?;
        let deadline = Instant::now() + TIMEOUT;
        let mut buf = [0u8; FRAME];
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            let Some(reply) = self.read(left, &mut buf)? else { continue };
            if reply.len() >= 2 && reply[0] == feature_index && reply[1] == fsw {
                return Ok(reply[2..].to_vec());
            }
            self.note_event(&reply);
        }
        Err(Error::Timeout)
    }

    fn bridge_index(&mut self) -> Result<u8> {
        if let Some(b) = self.bridge {
            return Ok(b);
        }
        let r = self.direct(0x00, 0x00, &F_BRIDGE.to_be_bytes())?;
        match r.first() {
            Some(&i) if i != 0 => {
                self.bridge = Some(i);
                Ok(i)
            }
            _ => Err(Error::Unsupported),
        }
    }

    /// Request to a feature of the headset, through the dongle's bridge.
    fn bridged(&mut self, sub_index: u8, function: u8, params: &[u8]) -> Result<Vec<u8>> {
        let b = self.bridge_index()?;
        let sub_fsw = (function & 0xF0) | SW_ID;
        let size = 3 + params.len();
        let mut payload =
            vec![b, BRIDGE_FN_SEND | SW_ID, ((size >> 8) & 0x0F) as u8, size as u8, 0x00, sub_index, sub_fsw];
        payload.extend_from_slice(params);
        self.dev.write(&frame(&self.prefix, &payload, 0))?;

        let deadline = Instant::now() + TIMEOUT;
        let mut buf = [0u8; FRAME];
        let mut acked = false;
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            let Some(reply) = self.read(left, &mut buf)? else { continue };
            // Dongle rejected the bridge send (headset off).
            if reply.len() >= 3 && reply[0] == 0xFF && reply[1] == b && reply[2] == BRIDGE_FN_SEND | SW_ID {
                return Err(if self.headset_connected()? {
                    Error::Protocol("bridge rejected")
                } else {
                    Error::Offline
                });
            }
            if reply.len() >= 4 && reply[0] == b && reply[1] == 0x00 {
                if reply[3] == 0 {
                    return Err(Error::Offline);
                }
                continue;
            }
            if reply.len() >= 2 && reply[0] == b && reply[1] == BRIDGE_FN_SEND | SW_ID {
                acked = true;
                continue;
            }
            if reply.len() >= 7 && reply[0] == b && reply[1] == BRIDGE_FN_SEND && reply[4] == 0 {
                if reply[5] == sub_index && reply[6] == sub_fsw {
                    return Ok(reply[7..].to_vec());
                }
                if reply[5] == 0xFF && reply[6] == sub_index {
                    return Err(Error::Protocol("headset rejected request"));
                }
            }
            self.note_event(&reply);
        }
        if acked && !self.headset_connected()? {
            return Err(Error::Offline);
        }
        Err(Error::Timeout)
    }

    /// Bridge.getStatus: params [.., .., connected_hi, connected_lo, ..].
    pub fn headset_connected(&mut self) -> Result<bool> {
        let b = self.bridge_index()?;
        let r = self.direct(b, 0x00, &[])?;
        Ok(!(r.len() >= 4 && r[2] == 0 && r[3] == 0))
    }

    pub fn read_battery(&mut self) -> Result<BatterySample> {
        let idx = match self.battery_idx {
            Some(i) => i,
            None => {
                let r = self.bridged(0x00, 0x00, &F_BATTERY_SOC.to_be_bytes())?;
                let i = *r.first().ok_or(Error::Protocol("empty getFeature"))?;
                if i == 0 {
                    return Err(Error::Unsupported);
                }
                self.battery_idx = Some(i);
                i
            }
        };
        let r = self.bridged(idx, 0x00, &[])?;
        parse_battery(&r).ok_or(Error::Protocol("bad battery reply"))
    }

    pub fn pump(&mut self, wait: Duration) -> Result<()> {
        let mut buf = [0u8; FRAME];
        if let Some(reply) = self.read(wait, &mut buf)? {
            self.note_event(&reply);
        }
        Ok(())
    }
}

pub fn is_headset(d: &hidapi::DeviceInfo) -> bool {
    d.vendor_id() == 0x046d && d.usage_page() == 0xFFA0
}

/// Opens every Centurion collection (usage page 0xFFA0) with its product name.
pub fn open_headsets(api: &HidApi) -> Vec<(u16, String, Headset)> {
    api.device_list()
        .filter(|d| is_headset(d))
        .filter_map(|d| {
            let name = d.product_string().unwrap_or("Headset").trim().to_string();
            d.open_device(api).ok().map(|h| (d.product_id(), name, Headset::new(h)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip() {
        let f = frame(&[0x51], &[0x02, 0x1B, 0x01, 0x04], 0);
        assert_eq!(&f[..7], &[0x51, 5, 0, 0x02, 0x1B, 0x01, 0x04]);
        assert_eq!(unframe(&[0x51], &f).unwrap(), &[0x02, 0x1B, 0x01, 0x04]);
        assert!(unframe(&[0x50], &f).is_none());
    }

    #[test]
    fn battery_states() {
        assert_eq!(parse_battery(&[89, 0, 0]).unwrap().charge, Charge::Discharging);
        assert_eq!(parse_battery(&[50, 0, 2]).unwrap().charge, Charge::Charging);
        assert!(parse_battery(&[150]).is_none());
    }
}
