//! Per-device history and the runtime / charge-time estimates built from it.
//!
//! Drain is measured against *active* time (seconds the device was reachable), not wall clock,
//! so a headset left off overnight does not look like it has a tiny drain rate. Every estimate
//! is a blend of the observed rate and the model's rated battery life, weighted by how much
//! data has been observed: fresh devices show the spec, then converge to real usage.

use crate::model::{Charge, Kind, Reading};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

/// Offline gaps shorter than this keep the same "active since" session (sleep, brief dropouts).
const SESSION_GAP: i64 = 10 * 60;
/// Longest stretch credited as active time between two polls.
const MAX_STEP: i64 = 5 * 60;
const MAX_SAMPLES: usize = 3000;
/// Prior strength, in hours of pseudo-observation.
const PRIOR_HOURS_DRAIN: f64 = 6.0;
const PRIOR_HOURS_CHARGE: f64 = 0.4;
/// Only the most recent active hours of a discharge drive the rate (adapts to lighting/usage changes).
const WINDOW_HOURS: f64 = 24.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub t: i64,
    pub pct: u8,
    pub charge: Charge,
    /// Cumulative active seconds at the time of the sample.
    pub active: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub name: String,
    pub kind: Kind,
    pub samples: VecDeque<Sample>,
    pub active_total: i64,
    pub online: bool,
    pub last_poll: i64,
    pub last_online: i64,
    pub session_start: Option<i64>,
    /// False while the session start is just "when Pulso first saw it" (power-on not observed).
    #[serde(default)]
    pub session_observed: bool,
    /// When the device last stopped charging.
    pub unplugged_at: Option<i64>,
    pub voltage_mv: Option<u16>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DeviceView {
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub online: bool,
    pub percent: Option<u8>,
    pub charge: Option<Charge>,
    pub voltage_mv: Option<u16>,
    /// Epoch seconds when the current active session began.
    pub session_start: Option<i64>,
    pub session_observed: bool,
    pub unplugged_at: Option<i64>,
    pub last_seen: i64,
    /// Seconds of use left at the current rate (discharging only).
    pub remaining: Option<i64>,
    /// Seconds until full (charging only).
    pub to_full: Option<i64>,
    /// 0..1, share of the estimate that comes from observed data.
    pub confidence: f64,
    pub rated_hours: f64,
}

/// Rated battery life (hours) and charge speed (%/h) by model, from Logitech spec sheets.
pub fn prior(name: &str, kind: Kind) -> (f64, f64) {
    let n = name.to_lowercase();
    let known = [
        ("pro x 2", 50.0, 45.0),
        ("pro 2 lightspeed", 95.0, 60.0),
        ("superlight 2", 95.0, 60.0),
        ("superlight", 70.0, 60.0),
        ("g502 x", 130.0, 50.0),
        ("g305", 250.0, 0.0),
        ("g733", 29.0, 35.0),
        ("g435", 18.0, 50.0),
        ("pro x wireless", 20.0, 35.0),
        // 4000 mAh, RGB on; slow USB charge.
        ("aula f75", 60.0, 25.0),
    ];
    for (k, h, c) in known {
        if n.contains(k) {
            return (h, if c > 0.0 { c } else { 50.0 });
        }
    }
    match kind {
        Kind::Mouse => (70.0, 55.0),
        Kind::Keyboard => (40.0, 35.0),
        Kind::Headset => (30.0, 40.0),
        Kind::Other => (40.0, 40.0),
    }
}

impl Track {
    fn new(r: &Reading) -> Self {
        Self {
            name: if r.name.is_empty() { "Teclado 2.4G".into() } else { r.name.clone() },
            kind: r.kind,
            samples: VecDeque::new(),
            active_total: 0,
            online: false,
            last_poll: 0,
            last_online: 0,
            session_start: None,
            session_observed: false,
            unplugged_at: None,
            voltage_mv: None,
        }
    }

    pub fn update(&mut self, r: &Reading, now: i64) {
        if !r.name.is_empty() {
            self.name = r.name.clone();
        }
        if r.kind != Kind::Other {
            self.kind = r.kind;
        }
        let was_online = self.online;
        self.online = r.online;
        if r.online {
            if was_online {
                self.active_total += (now - self.last_poll).clamp(0, MAX_STEP);
            }
            if self.session_start.is_none() || now - self.last_online > SESSION_GAP {
                // A real power-on only if we had seen the device before and it was gone long enough.
                self.session_observed = self.last_online != 0;
                self.session_start = Some(now);
            }
            self.last_online = now;
        }
        self.last_poll = now;

        let Some(s) = r.sample else { return };
        self.voltage_mv = s.voltage_mv;
        let last = self.samples.back();
        if let Some(l) = last {
            if l.charge != Charge::Discharging && s.charge == Charge::Discharging {
                self.unplugged_at = Some(now);
            }
        }
        let changed = last.is_none_or(|l| l.pct != s.percent || l.charge != s.charge || now - l.t > 15 * 60);
        if changed {
            self.samples.push_back(Sample { t: now, pct: s.percent, charge: s.charge, active: self.active_total });
            while self.samples.len() > MAX_SAMPLES {
                self.samples.pop_front();
            }
        }
    }

    /// Trailing samples that share the latest charge state.
    fn segment(&self) -> impl Iterator<Item = &Sample> {
        let Some(last) = self.samples.back() else { return self.samples.range(0..0) };
        let charging = last.charge != Charge::Discharging;
        let start =
            self.samples.iter().rposition(|s| (s.charge != Charge::Discharging) != charging).map_or(0, |i| i + 1);
        self.samples.range(start..)
    }

    pub fn view(&self, id: &str, now: i64) -> DeviceView {
        let (rated, charge_rate) = prior(&self.name, self.kind);
        let last = self.samples.back();
        let mut v = DeviceView {
            id: id.to_string(),
            name: self.name.clone(),
            kind: self.kind,
            online: self.online,
            percent: last.map(|s| s.pct),
            charge: last.map(|s| s.charge),
            voltage_mv: self.voltage_mv,
            session_start: if self.online { self.session_start } else { None },
            session_observed: self.session_observed,
            unplugged_at: self.unplugged_at,
            last_seen: self.last_online,
            remaining: None,
            to_full: None,
            confidence: 0.0,
            rated_hours: rated,
        };
        let Some(last) = last else { return v };
        let seg: Vec<&Sample> = self.segment().collect();

        match last.charge {
            Charge::Discharging => {
                let end_active = self.active_total;
                let first = seg
                    .iter()
                    .find(|s| (end_active - s.active) as f64 / 3600.0 <= WINDOW_HOURS)
                    .copied()
                    .unwrap_or(last);
                let hours = (end_active - first.active).max(0) as f64 / 3600.0;
                let drop = first.pct.saturating_sub(last.pct) as f64;
                let prior_rate = 100.0 / rated;
                let rate = (prior_rate * PRIOR_HOURS_DRAIN + drop) / (PRIOR_HOURS_DRAIN + hours);
                v.remaining = Some((last.pct as f64 / rate * 3600.0) as i64);
                v.confidence = hours / (hours + PRIOR_HOURS_DRAIN);
            }
            Charge::Charging => {
                let first = seg.first().copied().unwrap_or(last);
                let hours = (now - first.t).max(0) as f64 / 3600.0;
                let gain = last.pct.saturating_sub(first.pct) as f64;
                let rate = (charge_rate * PRIOR_HOURS_CHARGE + gain) / (PRIOR_HOURS_CHARGE + hours);
                v.to_full = Some((time_to_full(last.pct, rate) * 3600.0) as i64);
                v.confidence = hours / (hours + PRIOR_HOURS_CHARGE);
            }
            Charge::Full => v.to_full = Some(0),
        }
        v
    }
}

/// Hours to 100% with Li-ion CC/CV taper: full speed to 85%, half speed after.
pub fn time_to_full(pct: u8, rate: f64) -> f64 {
    let p = pct as f64;
    let knee = 85.0;
    if p >= knee {
        (100.0 - p) / (rate * 0.5)
    } else {
        (knee - p) / rate + (100.0 - knee) / (rate * 0.5)
    }
}

#[derive(Default, Serialize, Deserialize)]
pub struct Tracker {
    pub devices: HashMap<String, Track>,
}

impl Tracker {
    pub fn update(&mut self, r: &Reading, now: i64) {
        self.devices.entry(r.id.clone()).or_insert_with(|| Track::new(r)).update(r, now);
    }

    /// Devices seen in the last 30 days, plus present-but-silent ones (asleep since launch); online first.
    pub fn views(&self, now: i64) -> Vec<DeviceView> {
        let mut v: Vec<DeviceView> = self
            .devices
            .iter()
            .filter(
                |(_, t)| {
                    if t.samples.is_empty() {
                        now - t.last_poll < 120
                    } else {
                        now - t.last_online < 30 * 86400
                    }
                },
            )
            .map(|(id, t)| t.view(id, now))
            .collect();
        v.sort_by(|a, b| {
            b.online.cmp(&a.online).then(kind_rank(a.kind).cmp(&kind_rank(b.kind))).then(a.name.cmp(&b.name))
        });
        v
    }
}

/// Low-battery (15%, 5%) and charge-complete alerts, each fired once per discharge / charge cycle.
#[derive(Default)]
pub struct Alerts {
    /// Lowest threshold already announced per device; 0 = "full" announced.
    marks: HashMap<String, u8>,
}

const NONE: u8 = u8::MAX;

impl Alerts {
    pub fn check(&mut self, v: &DeviceView) -> Option<(String, String)> {
        let (Some(pct), Some(charge)) = (v.percent, v.charge) else { return None };
        if !v.online {
            return None;
        }
        let mark = self.marks.entry(v.id.clone()).or_insert(NONE);
        match charge {
            Charge::Full => {
                let first = *mark != 0;
                *mark = 0;
                first.then(|| (format!("{} carregado", v.name), "Bateria em 100%. Já pode tirar do cabo.".into()))
            }
            Charge::Charging => {
                *mark = NONE;
                None
            }
            Charge::Discharging => {
                if *mark == 0 {
                    *mark = NONE;
                }
                let threshold = [5u8, 15].into_iter().find(|t| pct <= *t)?;
                if *mark <= threshold {
                    return None;
                }
                *mark = threshold;
                let left = v.remaining.map(|s| format!(" · cerca de {} de uso", human(s))).unwrap_or_default();
                Some((format!("Bateria baixa · {}", v.name), format!("{pct}% restantes{left}. Hora de carregar.")))
            }
        }
    }
}

pub fn human(secs: i64) -> String {
    let m = secs.max(0) / 60;
    match m {
        0..=59 => format!("{m} min"),
        60..=2879 => format!("{} h", m / 60),
        _ => format!("{} dias", m / 1440),
    }
}

fn kind_rank(k: Kind) -> u8 {
    match k {
        Kind::Mouse => 0,
        Kind::Keyboard => 1,
        Kind::Headset => 2,
        Kind::Other => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::BatterySample;

    fn reading(pct: u8, charge: Charge, online: bool) -> Reading {
        Reading {
            id: "t".into(),
            name: "PRO X 2 LIGHTSPEED".into(),
            kind: Kind::Headset,
            online,
            sample: Some(BatterySample { percent: pct, charge, voltage_mv: None }),
        }
    }

    #[test]
    fn fresh_device_uses_rated_life() {
        let mut t = Tracker::default();
        t.update(&reading(50, Charge::Discharging, true), 0);
        let v = t.views(0).remove(0);
        // 50% of a 50 h headset = 25 h
        assert_eq!(v.remaining.unwrap() / 3600, 25);
        assert_eq!(v.confidence, 0.0);
    }

    #[test]
    fn converges_to_observed_drain() {
        let mut t = Tracker::default();
        // Real drain 4%/h (twice the rated 2%/h), polled every minute for 20 h.
        for m in 0..=1200i64 {
            let pct = (100 - m * 4 / 60).max(20) as u8;
            t.update(&reading(pct, Charge::Discharging, true), m * 60);
        }
        let v = t.views(1200 * 60).remove(0);
        let hours = v.remaining.unwrap() as f64 / 3600.0;
        // 20% at ~4%/h is ~5 h; blended estimate must be close to that, far from the spec's 10 h.
        assert!(hours > 4.0 && hours < 6.5, "{hours}");
        assert!(v.confidence > 0.6);
    }

    #[test]
    fn offline_time_is_not_drain_time() {
        let mut t = Tracker::default();
        t.update(&reading(80, Charge::Discharging, true), 0);
        t.update(&reading(80, Charge::Discharging, false), 60);
        t.update(&reading(79, Charge::Discharging, true), 12 * 3600);
        let tr = &t.devices["t"];
        assert!(tr.active_total <= 60);
        // Long gap starts a new active session.
        assert_eq!(tr.session_start, Some(12 * 3600));
    }

    #[test]
    fn alerts_fire_once_per_cycle() {
        let mut t = Tracker::default();
        let mut a = Alerts::default();
        let mut fire = |t: &mut Tracker, pct, charge, at| {
            t.update(&reading(pct, charge, true), at);
            a.check(&t.views(at)[0]).is_some()
        };
        assert!(!fire(&mut t, 40, Charge::Discharging, 0));
        assert!(fire(&mut t, 15, Charge::Discharging, 60));
        assert!(!fire(&mut t, 14, Charge::Discharging, 120));
        assert!(fire(&mut t, 5, Charge::Discharging, 180));
        assert!(!fire(&mut t, 4, Charge::Discharging, 240));
        assert!(!fire(&mut t, 30, Charge::Charging, 300));
        assert!(fire(&mut t, 100, Charge::Full, 400));
        assert!(!fire(&mut t, 100, Charge::Full, 460));
        assert!(!fire(&mut t, 99, Charge::Discharging, 500));
        assert!(fire(&mut t, 15, Charge::Discharging, 600));
    }

    #[test]
    fn unplug_and_charge_eta() {
        let mut t = Tracker::default();
        t.update(&reading(40, Charge::Charging, true), 0);
        let v = t.views(0).remove(0);
        // Prior 45%/h: 45/45 + 15/22.5 = 1.67 h
        assert!((v.to_full.unwrap() as f64 / 3600.0 - 1.67).abs() < 0.05);
        t.update(&reading(41, Charge::Discharging, true), 100);
        assert_eq!(t.devices["t"].unplugged_at, Some(100));
    }
}
