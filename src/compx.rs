//! CompX 2.4 GHz keyboard dongles (VID 0x3554, PID 0xFA09) used by AULA (F75, F99 Pro), MCHOSE, etc.
//!
//! Vendor collection 0xFF02/0x0002, 20-byte report 0x13, checksum = sum of the first 19 bytes.
//! Opening a session (`13 05 01`) makes the dongle answer with an ACK carrying the model code and
//! then push a battery frame `13 0A .. .. .. 05 <pct> <flags>`; flags bit 0x10 = charging.
//! Frame layout documented by the vigil-watt project (MIT, docs/hardware/aula.md).

use crate::model::{BatterySample, Charge};
use hidapi::{HidApi, HidDevice};
use std::time::{Duration, Instant};

pub const VID: u16 = 0x3554;
const REPORT: u8 = 0x13;
const LEN: usize = 20;
const CMD_SESSION: u8 = 0x05;
const CMD_PUSH: u8 = 0x0A;
const CMD_QUERY: u8 = 0x4A;
const PUSH_BATTERY: u8 = 0x05;
// An awake keyboard answers in ~230 ms; silence past this means it is asleep or off.
const WAIT: Duration = Duration::from_millis(1500);

pub fn frame(cmd: u8, param: u8) -> [u8; LEN] {
    let mut f = [0u8; LEN];
    f[0] = REPORT;
    f[1] = cmd;
    f[2] = param;
    f[LEN - 1] = f[..LEN - 1].iter().fold(0u8, |a, b| a.wrapping_add(*b));
    f
}

fn flags_to_charge(pct: u8, flags: u8) -> Charge {
    if flags & 0x10 != 0 {
        if pct >= 100 {
            Charge::Full
        } else {
            Charge::Charging
        }
    } else {
        Charge::Discharging
    }
}

pub enum Frame {
    /// Session ACK with the model code (0x0158 = AULA F99 Pro family).
    Ack(u16),
    Battery(BatterySample),
    /// Sleep / power state notification, or a battery frame with an invalid value (sleeping keyboard).
    Asleep,
    Other,
}

pub fn parse(input: &[u8]) -> Frame {
    // Windows may or may not keep the report id in the buffer.
    let f = if input.first() == Some(&REPORT) { input } else { return Frame::Other };
    if f.len() < 8 {
        return Frame::Other;
    }
    match f[1] {
        CMD_SESSION if f.len() >= 11 && f[4] == 0x0A => Frame::Ack(u16::from_be_bytes([f[9], f[10]])),
        CMD_PUSH if f[5] == PUSH_BATTERY => match f[6] {
            2..=100 => {
                Frame::Battery(BatterySample { percent: f[6], charge: flags_to_charge(f[6], f[7]), voltage_mv: None })
            }
            _ => Frame::Asleep,
        },
        CMD_PUSH if f[5] == 0x02 => Frame::Asleep,
        CMD_QUERY if (2..=100).contains(&f[5]) => {
            Frame::Battery(BatterySample { percent: f[5], charge: flags_to_charge(f[5], f[6]), voltage_mv: None })
        }
        _ => Frame::Other,
    }
}

pub struct Dongle {
    dev: HidDevice,
    pub model: Option<u16>,
    pub pushed: Option<BatterySample>,
}

impl Dongle {
    /// Opens a session and waits for the battery push. `Ok(None)` = dongle present, keyboard silent.
    pub fn read_battery(&mut self) -> Result<Option<BatterySample>, hidapi::HidError> {
        self.dev.write(&frame(CMD_SESSION, 0x01))?;
        let deadline = Instant::now() + WAIT;
        let mut buf = [0u8; 64];
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            let n = self.dev.read_timeout(&mut buf, left.as_millis().max(1) as i32)?;
            match parse(&buf[..n]) {
                Frame::Ack(m) => self.model = Some(m),
                Frame::Battery(s) => return Ok(Some(s)),
                Frame::Asleep => return Ok(None),
                Frame::Other => {}
            }
        }
        Ok(None)
    }

    /// Listens for unsolicited pushes (the dongle reports changes on its own while a session is open).
    pub fn pump(&mut self, wait: Duration) -> Result<(), hidapi::HidError> {
        let mut buf = [0u8; 64];
        let n = self.dev.read_timeout(&mut buf, wait.as_millis() as i32)?;
        if let Frame::Battery(s) = parse(&buf[..n]) {
            self.pushed = Some(s);
        }
        Ok(())
    }
}

/// Model name by session ACK code; unknown codes fall back to a generic name.
pub fn model_name(code: Option<u16>) -> &'static str {
    match code {
        Some(0x00CD) => "AULA F75",
        Some(0x0158) => "AULA F99 Pro",
        Some(0x0212) => "Lingbao K98",
        _ => "Teclado 2.4G",
    }
}

pub fn is_dongle(d: &hidapi::DeviceInfo) -> bool {
    d.vendor_id() == VID && d.product_id() == 0xFA09 && d.usage_page() == 0xFF02 && d.usage() == 0x0002
}

impl Dongle {
    pub fn new(dev: HidDevice) -> Self {
        Self { dev, model: None, pushed: None }
    }
}

pub fn open_dongles(api: &HidApi) -> Vec<(u16, Dongle)> {
    api.device_list()
        .filter(|d| is_dongle(d))
        .filter_map(|d| d.open_device(api).ok().map(|h| (d.product_id(), Dongle::new(h))))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn session_frame_checksum() {
        assert_eq!(frame(0x05, 0x01)[19], 0x19);
    }

    #[test]
    fn real_frames() {
        assert!(matches!(parse(&hex("130501000a030000000158010000000000000080")), Frame::Ack(0x0158)));
        match parse(&hex("130a010004055a01000000000000000000000082")) {
            Frame::Battery(s) => assert_eq!((s.percent, s.charge), (90, Charge::Discharging)),
            _ => panic!(),
        }
        match parse(&hex("130a01000405641100000000000000000000009c")) {
            Frame::Battery(s) => assert_eq!(s.charge, Charge::Full),
            _ => panic!(),
        }
        assert!(matches!(parse(&hex("130a0100040501000000000000000000000000")), Frame::Asleep));
    }
}
