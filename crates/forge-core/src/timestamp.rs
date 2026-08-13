use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

/// A UTC instant that is RFC 3339 on the wire and in SQLite alike.
///
/// One representation everywhere keeps the DB rows, the JSON API and the
/// TypeScript mirror from drifting apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(OffsetDateTime);

impl Timestamp {
    pub fn now() -> Self {
        Self(OffsetDateTime::now_utc())
    }

    pub const fn from_offset(value: OffsetDateTime) -> Self {
        Self(value)
    }

    pub const fn into_offset(self) -> OffsetDateTime {
        self.0
    }
}

impl fmt::Display for Timestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Rfc3339 only fails for years outside its range, which `OffsetDateTime`
        // cannot reach from `now_utc` or a parsed RFC 3339 string.
        match self.0.format(&Rfc3339) {
            Ok(text) => f.write_str(&text),
            Err(_) => Err(fmt::Error),
        }
    }
}

/// Returned when a string is not a valid RFC 3339 timestamp.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidTimestamp(pub String);

impl fmt::Display for InvalidTimestamp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid RFC 3339 timestamp: {}", self.0)
    }
}

impl std::error::Error for InvalidTimestamp {}

impl FromStr for Timestamp {
    type Err = InvalidTimestamp;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        OffsetDateTime::parse(s, &Rfc3339)
            .map(|value| Self(value.to_offset(time::UtcOffset::UTC)))
            .map_err(|_| InvalidTimestamp(s.to_owned()))
    }
}

impl Serialize for Timestamp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for Timestamp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        raw.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_its_string_form() {
        let now = Timestamp::now();
        assert_eq!(now.to_string().parse::<Timestamp>().unwrap(), now);
    }

    #[test]
    fn json_is_an_rfc3339_string() {
        let ts: Timestamp = "2026-08-13T09:30:00Z".parse().unwrap();
        assert_eq!(
            serde_json::to_string(&ts).unwrap(),
            "\"2026-08-13T09:30:00Z\""
        );
        assert_eq!(
            serde_json::from_str::<Timestamp>("\"2026-08-13T09:30:00Z\"").unwrap(),
            ts
        );
    }

    #[test]
    fn offsets_are_normalised_to_utc() {
        let melbourne: Timestamp = "2026-08-13T19:30:00+10:00".parse().unwrap();
        let utc: Timestamp = "2026-08-13T09:30:00Z".parse().unwrap();
        assert_eq!(melbourne, utc);
        assert_eq!(melbourne.to_string(), utc.to_string());
    }

    #[test]
    fn garbage_is_rejected() {
        assert!("yesterday".parse::<Timestamp>().is_err());
    }
}
