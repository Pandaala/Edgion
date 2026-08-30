//! Unit-suffixed durations for certificate lifetimes (spec §5.2).
//! Form: `<integer><unit>`, units h / d(=24h) / w(=7d) / mo(=30d fixed) / y(=365d fixed).
//! Single term only; zero and overflow are parse errors.

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct CertDuration {
    hours: u64,
}

impl CertDuration {
    pub const fn from_hours(hours: u64) -> Self {
        Self { hours }
    }

    pub fn as_std(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.hours * 3600)
    }

    pub fn as_time(&self) -> time::Duration {
        time::Duration::hours(self.hours as i64)
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        let split = s
            .find(|c: char| !c.is_ascii_digit())
            .ok_or_else(|| format!("duration '{s}': missing unit (h/d/w/mo/y)"))?;
        let (num, unit) = s.split_at(split);
        if num.is_empty() {
            return Err(format!("duration '{s}': missing number"));
        }
        let n: u64 = num.parse().map_err(|_| format!("duration '{s}': invalid number"))?;
        if n == 0 {
            return Err(format!("duration '{s}': must be > 0"));
        }
        let per_unit: u64 = match unit {
            "h" => 1,
            "d" => 24,
            "w" => 24 * 7,
            "mo" => 24 * 30,
            "y" => 24 * 365,
            other => return Err(format!("duration '{s}': unknown unit '{other}' (use h/d/w/mo/y)")),
        };
        // `as_time()` casts `hours` to `i64` (`time::Duration::hours` computes
        // `hours * 3600` internally in `i64` space), so the overflow guard must
        // check against `i64::MAX`, not `u64::MAX`: an hour count whose seconds
        // fit in `u64` but exceed `i64::MAX` would silently overflow there.
        let hours = n
            .checked_mul(per_unit)
            .filter(|h| h.checked_mul(3600).is_some_and(|s| s <= i64::MAX as u64))
            .ok_or_else(|| format!("duration '{s}': overflow"))?;
        Ok(Self { hours })
    }
}

impl std::fmt::Display for CertDuration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}h", self.hours)
    }
}

impl Serialize for CertDuration {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for CertDuration {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        CertDuration::parse(&s).map_err(de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_units() {
        assert_eq!(CertDuration::parse("1h").unwrap().as_std().as_secs(), 3600);
        assert_eq!(CertDuration::parse("2d").unwrap().as_std().as_secs(), 2 * 24 * 3600);
        assert_eq!(CertDuration::parse("1w").unwrap().as_std().as_secs(), 7 * 24 * 3600);
        assert_eq!(CertDuration::parse("1mo").unwrap().as_std().as_secs(), 30 * 24 * 3600);
        assert_eq!(CertDuration::parse("1y").unwrap().as_std().as_secs(), 365 * 24 * 3600);
    }

    #[test]
    fn rejects_bad_inputs() {
        for bad in [
            "",
            "12",
            "h",
            "0h",
            "0d",
            "-1h",
            "1.5h",
            "1y6mo",
            "1s",
            "1m",
            "99999999999999999999h",
        ] {
            assert!(CertDuration::parse(bad).is_err(), "should reject {bad:?}");
        }
        // Overflow via checked mul: u64::MAX hours overflows seconds.
        assert!(CertDuration::parse(&format!("{}h", u64::MAX)).is_err());
    }

    #[test]
    fn rejects_hours_whose_seconds_exceed_i64_max_but_fit_u64() {
        // `h * 3600` fits comfortably in `u64` (the old bound) but exceeds
        // `i64::MAX` (the tightened bound `as_time()` actually needs, since it
        // casts `hours` to `i64` and `time::Duration::hours` multiplies by 3600
        // in `i64` space). This is the smallest `h` in that gap.
        let h = (i64::MAX as u64) / 3600 + 1;
        assert!(
            h.checked_mul(3600).is_some(),
            "sanity: h*3600 must still fit in u64 for this to test the intended gap"
        );
        assert!(
            h.checked_mul(3600).unwrap() > i64::MAX as u64,
            "sanity: h*3600 must exceed i64::MAX for this to test the intended gap"
        );
        assert!(CertDuration::parse(&format!("{h}h")).is_err());
    }

    #[test]
    fn serde_round_trip() {
        let d: CertDuration = serde_json::from_str("\"36h\"").unwrap();
        assert_eq!(d.as_std().as_secs(), 36 * 3600);
        assert_eq!(serde_json::to_string(&d).unwrap(), "\"36h\"");
        assert!(serde_json::from_str::<CertDuration>("\"0h\"").is_err());
        assert!(serde_json::from_str::<CertDuration>("36").is_err()); // bare integer rejected
    }
}
