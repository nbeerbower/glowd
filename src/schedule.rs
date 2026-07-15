//! Schedule model and the pure logic for deciding when one fires.
//! The actual timer thread lives in `server.rs`, where the shared state is.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    pub id: u64,
    /// 24h local time, "HH:MM".
    pub time: String,
    /// Days of week, 0 = Sunday .. 6 = Saturday. Empty means every day.
    #[serde(default)]
    pub days: Vec<u8>,
    /// "on", "off", or "color".
    pub action: String,
    /// Normalized "#rrggbb"; required when action is "color".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hex: Option<String>,
    /// Target device MAC; None means all devices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mac: Option<String>,
    pub enabled: bool,
}

pub fn is_due(s: &Schedule, weekday: u8, hhmm: &str) -> bool {
    s.enabled && s.time == hhmm && (s.days.is_empty() || s.days.contains(&weekday))
}

pub fn validate(s: &Schedule) -> Result<(), String> {
    let time_ok = matches!(s.time.split_once(':'), Some((h, m))
        if h.len() == 2 && m.len() == 2
            && h.parse::<u8>().is_ok_and(|h| h < 24)
            && m.parse::<u8>().is_ok_and(|m| m < 60));
    if !time_ok {
        return Err("time must be HH:MM (24h)".into());
    }
    if s.days.iter().any(|d| *d > 6) {
        return Err("days must be 0 (Sun) through 6 (Sat)".into());
    }
    match s.action.as_str() {
        "on" | "off" => Ok(()),
        "color" if s.hex.is_some() => Ok(()),
        "color" => Err("color action needs a hex value".into()),
        _ => Err("action must be on, off, or color".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sched(time: &str, days: Vec<u8>, enabled: bool) -> Schedule {
        Schedule {
            id: 1,
            time: time.into(),
            days,
            action: "off".into(),
            hex: None,
            mac: None,
            enabled,
        }
    }

    #[test]
    fn fires_at_matching_minute() {
        assert!(is_due(&sched("22:30", vec![], true), 3, "22:30"));
        assert!(!is_due(&sched("22:30", vec![], true), 3, "22:31"));
    }

    #[test]
    fn respects_day_filter_and_enabled_flag() {
        assert!(is_due(&sched("07:00", vec![1, 2, 3], true), 2, "07:00"));
        assert!(!is_due(&sched("07:00", vec![1, 2, 3], true), 0, "07:00"));
        assert!(!is_due(&sched("07:00", vec![], false), 0, "07:00"));
    }

    #[test]
    fn validates_time_days_and_action() {
        assert!(validate(&sched("23:59", vec![0, 6], true)).is_ok());
        assert!(validate(&sched("24:00", vec![], true)).is_err());
        assert!(validate(&sched("9:00", vec![], true)).is_err());
        assert!(validate(&sched("nope", vec![], true)).is_err());
        assert!(validate(&sched("07:00", vec![7], true)).is_err());

        let mut s = sched("07:00", vec![], true);
        s.action = "color".into();
        assert!(validate(&s).is_err()); // color without hex
        s.hex = Some("#ff8800".into());
        assert!(validate(&s).is_ok());
        s.action = "explode".into();
        assert!(validate(&s).is_err());
    }
}
