use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Mouse,
    Keyboard,
    Headset,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Charge {
    Discharging,
    Charging,
    Full,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatterySample {
    pub percent: u8,
    pub charge: Charge,
    pub voltage_mv: Option<u16>,
}

/// Raw reading from one transport poll, before history / estimates.
#[derive(Debug, Clone)]
pub struct Reading {
    /// Stable id: "<transport>:<pid>:<slot>"
    pub id: String,
    pub name: String,
    pub kind: Kind,
    pub online: bool,
    pub sample: Option<BatterySample>,
}
