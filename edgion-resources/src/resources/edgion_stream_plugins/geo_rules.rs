//! The per-profile `geo` block of `GlobalConnectionIpRestriction`.
//!
//! L4 supports country matching only — no continent, city, or subdivision. At L4
//! there is only a connection, with no request attributes to compose against, so
//! fine-grained geographic rules are hard to operate and easy to over-block.
//! City-level decisions belong in `request_restriction`.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GeoRules {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow_countries: Vec<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny_countries: Vec<String>,

    /// Disposition when no GeoIP database is loaded at all. **No default** — the
    /// operator states the intent explicitly and owns the consequence.
    /// `false` denies during the window; `true` behaves as if the block were absent.
    pub fail_open: bool,
}

impl GeoRules {
    pub fn compile(&mut self) -> Result<(), String> {
        if self.allow_countries.is_empty() && self.deny_countries.is_empty() {
            return Err("geo block requires at least one of allowCountries / denyCountries".to_string());
        }
        for v in self.allow_countries.iter_mut().chain(self.deny_countries.iter_mut()) {
            *v = v.to_ascii_uppercase();
        }
        for v in self.allow_countries.iter().chain(self.deny_countries.iter()) {
            if !crate::geoip_codes::is_valid_country(v) {
                tracing::warn!("GlobalConnectionIpRestriction geo: '{}' is not a known country code", v);
            }
        }
        Ok(())
    }

    pub fn denies_country(&self, c: &str) -> bool {
        self.deny_countries.iter().any(|v| v == c)
    }

    pub fn allows_country(&self, c: &str) -> bool {
        self.allow_countries.iter().any(|v| v == c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geo_block_deserializes_with_snake_free_camel_keys() {
        let yaml = r#"
allowCountries: ["CN", "HK"]
denyCountries: ["RU"]
failOpen: false
"#;
        let g: GeoRules = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(g.allow_countries, vec!["CN", "HK"]);
        assert!(!g.fail_open);
    }

    #[test]
    fn fail_open_has_no_default() {
        let yaml = "denyCountries: [\"RU\"]\n";
        assert!(serde_yaml::from_str::<GeoRules>(yaml).is_err());
    }

    #[test]
    fn an_empty_geo_block_is_rejected() {
        let mut g = GeoRules {
            allow_countries: vec![],
            deny_countries: vec![],
            fail_open: false,
        };
        assert!(g.compile().unwrap_err().contains("at least one"));
    }

    #[test]
    fn country_values_are_uppercased_at_compile_time() {
        let mut g = GeoRules {
            allow_countries: vec!["cn".into()],
            deny_countries: vec!["ru".into()],
            fail_open: false,
        };
        g.compile().unwrap();
        assert_eq!(g.allow_countries, vec!["CN".to_string()]);
        assert_eq!(g.deny_countries, vec!["RU".to_string()]);
    }
}
