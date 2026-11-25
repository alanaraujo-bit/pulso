//! Background worker: owns every HID handle, polls on a slow cadence and listens for
//! unsolicited battery / connection reports in between, so changes show up instantly.

use crate::model::{Kind, Reading};
use crate::{centurion, compx, hidpp};
use hidapi::HidApi;
use std::collections::HashSet;
use std::ffi::CString;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

const POLL_ONLINE: Duration = Duration::from_secs(30);
const POLL_OFFLINE: Duration = Duration::from_secs(20);
const DISCOVER_EVERY: Duration = Duration::from_secs(60);
const ENUMERATE_EVERY: Duration = Duration::from_secs(8);
// Listening window per transport per loop: unsolicited reports queue in the driver meanwhile.
const PUMP: Duration = Duration::from_millis(250);

pub enum Command {
    Refresh,
}

struct LgDevice {
    name: String,
    kind: Kind,
    battery: hidpp::BatteryFeature,
    online: bool,
    next_poll: Instant,
}

enum Slot {
    Unknown { next_try: Instant },
    Known(LgDevice),
    NoBattery,
}

struct Receiver_ {
    path: CString,
    ch: hidpp::Channel,
    slots: Vec<(u8, Slot)>,
}

struct HeadsetT {
    path: CString,
    name: String,
    hs: centurion::Headset,
    online: bool,
    misses: u8,
    next_poll: Instant,
}

struct DongleT {
    path: CString,
    dg: compx::Dongle,
    next_poll: Instant,
}

fn lg_id(name: &str) -> String {
    format!("lg:{name}")
}

pub fn run(cmds: Receiver<Command>, mut emit: impl FnMut(Vec<Reading>)) {
    let mut api = match HidApi::new() {
        Ok(a) => a,
        Err(_) => return,
    };
    let mut receivers: Vec<Receiver_> = Vec::new();
    let mut headsets: Vec<HeadsetT> = Vec::new();
    let mut dongles: Vec<DongleT> = Vec::new();
    let mut last_enum = Instant::now() - ENUMERATE_EVERY;

    loop {
        let mut out = Vec::new();
        let now = Instant::now();
        let mut force = false;
        loop {
            match cmds.recv_timeout(Duration::ZERO) {
                Ok(Command::Refresh) => force = true,
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }

        // Hot-plug: open collections we don't hold yet. Dropped handles are removed on I/O error.
        if now.duration_since(last_enum) >= ENUMERATE_EVERY {
            last_enum = now;
            if api.refresh_devices().is_ok() {
                let open: HashSet<CString> = receivers
                    .iter()
                    .map(|r| r.path.clone())
                    .chain(headsets.iter().map(|h| h.path.clone()))
                    .chain(dongles.iter().map(|d| d.path.clone()))
                    .collect();
                for d in api.device_list() {
                    if open.contains(d.path()) {
                        continue;
                    }
                    let path = d.path().to_owned();
                    if hidpp::is_channel(d) {
                        if let Ok(h) = d.open_device(&api) {
                            let slots = [1u8, 2, 3, 4, 5, 6, 0xFF].map(|i| (i, Slot::Unknown { next_try: now })).into();
                            let short = hidpp::open_short(&api, d);
                            receivers.push(Receiver_ { path, ch: hidpp::Channel::new(h, short), slots });
                        }
                    } else if centurion::is_headset(d) {
                        if let Ok(h) = d.open_device(&api) {
                            let name = d.product_string().unwrap_or("Headset").trim().to_string();
                            headsets.push(HeadsetT { path, name, hs: centurion::Headset::new(h), online: false, misses: 0, next_poll: now });
                        }
                    } else if compx::is_dongle(d) {
                        if let Ok(h) = d.open_device(&api) {
                            dongles.push(DongleT { path, dg: compx::Dongle::new(h), next_poll: now });
                        }
                    }
                }
            }
        }

        // Flush after each transport so a slow one (sleeping device timeouts) never delays the others.
        // A transport that fails I/O was unplugged: drop it and report its devices offline.
        headsets.retain_mut(|h| {
            poll_headset(h, force, &mut out).is_ok() || {
                out.push(Reading { id: lg_id(&h.name), name: h.name.clone(), kind: Kind::Headset, online: false, sample: None });
                false
            }
        });
        if !out.is_empty() {
            emit(std::mem::take(&mut out));
        }
        dongles.retain_mut(|d| {
            poll_dongle(d, force, &mut out).is_ok() || {
                out.push(Reading { id: "compx:fa09".into(), name: String::new(), kind: Kind::Keyboard, online: false, sample: None });
                false
            }
        });
        if !out.is_empty() {
            emit(std::mem::take(&mut out));
        }
        receivers.retain_mut(|r| {
            poll_receiver(r, force, &mut out).is_ok() || {
                for (_, slot) in &r.slots {
                    if let Slot::Known(d) = slot {
                        out.push(Reading { id: lg_id(&d.name), name: d.name.clone(), kind: d.kind, online: false, sample: None });
                    }
                }
                false
            }
        });
        if !out.is_empty() {
            emit(out);
        }
        if receivers.is_empty() && headsets.is_empty() && dongles.is_empty() {
            std::thread::sleep(Duration::from_millis(500));
        }
    }
}

fn poll_receiver(r: &mut Receiver_, force: bool, out: &mut Vec<Reading>) -> Result<(), ()> {
    let now = Instant::now();
    for (idx, slot) in r.slots.iter_mut() {
        match slot {
            Slot::Unknown { next_try } if *next_try <= now || force => {
                match r.ch.feature_index(*idx, hidpp::F_DEVICE_NAME) {
                    Ok(Some(fi)) => {
                        let Ok((name, kind)) = r.ch.device_name(*idx, fi) else { continue };
                        match hidpp::find_battery(&mut r.ch, *idx) {
                            Ok(Some(battery)) => {
                                *slot = Slot::Known(LgDevice { name, kind, battery, online: false, next_poll: now })
                            }
                            Ok(None) => *slot = Slot::NoBattery,
                            Err(hidpp::Error::Io(_)) => return Err(()),
                            Err(_) => {}
                        }
                    }
                    Ok(None) => *slot = Slot::NoBattery,
                    Err(hidpp::Error::Io(_)) => return Err(()),
                    Err(_) => *next_try = now + DISCOVER_EVERY,
                }
            }
            _ => {}
        }
        if let Slot::Known(d) = slot {
            if d.next_poll <= now || force {
                let reading = match hidpp::read_battery(&mut r.ch, *idx, d.battery) {
                    Ok(s) => Some(Some(s)),
                    Err(hidpp::Error::Io(_)) => return Err(()),
                    Err(e) if hidpp::is_unreachable(&e) => Some(None),
                    Err(_) => None,
                };
                if let Some(sample) = reading {
                    d.online = sample.is_some();
                    out.push(Reading { id: lg_id(&d.name), name: d.name.clone(), kind: d.kind, online: d.online, sample });
                }
                d.next_poll = now + if d.online { POLL_ONLINE } else { POLL_OFFLINE };
            }
        }
    }

    r.ch.pump(PUMP).map_err(|_| ())?;
    for ev in std::mem::take(&mut r.ch.events) {
        let Some((_, slot)) = r.slots.iter_mut().find(|(i, _)| *i == ev.index) else { continue };
        let link = hidpp::Channel::connection(&ev);
        let d = match slot {
            Slot::Known(d) => d,
            // A device that just woke up or was paired announces itself: discover it right away.
            Slot::Unknown { next_try } => {
                if link != Some(false) {
                    *next_try = now;
                }
                continue;
            }
            Slot::NoBattery => continue,
        };
        if let Some(up) = link {
            if up {
                // Give the radio link a moment before asking.
                d.next_poll = now + Duration::from_millis(400);
            } else if d.online {
                d.online = false;
                d.next_poll = now + POLL_OFFLINE;
                out.push(Reading { id: lg_id(&d.name), name: d.name.clone(), kind: d.kind, online: false, sample: None });
            }
        } else if ev.feature_index == d.battery.index() && ev.function == 0 && ev.data.len() >= 4 {
            if let Some(s) = hidpp::parse_battery(d.battery, &ev.data) {
                d.online = true;
                out.push(Reading { id: lg_id(&d.name), name: d.name.clone(), kind: d.kind, online: true, sample: Some(s) });
            }
        } else {
            // Any traffic from a device that looked offline means it is back: poll soon.
            if !d.online {
                d.next_poll = now;
            }
        }
    }
    Ok(())
}

fn poll_headset(h: &mut HeadsetT, force: bool, out: &mut Vec<Reading>) -> Result<(), ()> {
    let now = Instant::now();
    let id = lg_id(&h.name);
    if h.next_poll <= now || force {
        match h.hs.read_battery() {
            Ok(s) => {
                h.online = true;
                h.misses = 0;
                out.push(Reading { id: id.clone(), name: h.name.clone(), kind: Kind::Headset, online: true, sample: Some(s) });
            }
            Err(centurion::Error::Io(_)) => return Err(()),
            Err(centurion::Error::Offline) => {
                h.online = false;
                out.push(Reading { id: id.clone(), name: h.name.clone(), kind: Kind::Headset, online: false, sample: None });
            }
            Err(_) => {
                h.misses += 1;
                if h.misses >= 2 && h.online {
                    h.online = false;
                    out.push(Reading { id: id.clone(), name: h.name.clone(), kind: Kind::Headset, online: false, sample: None });
                }
            }
        }
        h.next_poll = now + if h.online { POLL_ONLINE } else { POLL_OFFLINE };
    }
    h.hs.pump(PUMP).map_err(|_| ())?;
    for ev in std::mem::take(&mut h.hs.events) {
        match ev {
            centurion::Event::Connection(true) => h.next_poll = now + Duration::from_secs(2),
            centurion::Event::Connection(false) => {
                h.online = false;
                out.push(Reading { id: id.clone(), name: h.name.clone(), kind: Kind::Headset, online: false, sample: None });
            }
            centurion::Event::Battery(s) => {
                h.online = true;
                out.push(Reading { id: id.clone(), name: h.name.clone(), kind: Kind::Headset, online: true, sample: Some(s) });
            }
        }
    }
    Ok(())
}

fn poll_dongle(d: &mut DongleT, force: bool, out: &mut Vec<Reading>) -> Result<(), ()> {
    let now = Instant::now();
    let reading = |d: &DongleT, s: Option<crate::model::BatterySample>| {
        // The model is only known once the keyboard answers; an empty name keeps the last one.
        let name = if d.dg.model.is_some() { compx::model_name(d.dg.model).to_string() } else { String::new() };
        Reading { id: "compx:fa09".into(), name, kind: Kind::Keyboard, online: s.is_some(), sample: s }
    };
    if d.next_poll <= now || force {
        let s = d.dg.read_battery().map_err(|_| ())?;
        d.next_poll = now + if s.is_some() { POLL_ONLINE } else { Duration::from_secs(15) };
        out.push(reading(d, s));
    }
    d.dg.pump(PUMP).map_err(|_| ())?;
    if let Some(s) = d.dg.pushed.take() {
        out.push(reading(d, Some(s)));
    }
    Ok(())
}
