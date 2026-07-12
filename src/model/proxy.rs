//! `/proxies` wire shape. Validation-only for now — the `--action redirect`
//! `--via` check in `commands::action_flags` is the sole reader; `cdctl proxy
//! list` (commands.md#access-proxy) lands in a later phase.

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))]
pub struct ApiProxy {
    #[serde(rename = "PK")]
    pub pk: String,
    pub city: String,
    #[expect(dead_code, reason = "modeled for the strict fixture tests; never read")]
    pub country: String,
    pub country_name: String,
    #[serde(default)]
    #[expect(dead_code, reason = "modeled for the strict fixture tests; never read")]
    pub gps_lat: Option<f64>,
    #[serde(default)]
    #[expect(dead_code, reason = "modeled for the strict fixture tests; never read")]
    pub gps_long: Option<f64>,
    #[serde(default)]
    #[expect(dead_code, reason = "modeled for the strict fixture tests; never read")]
    pub uid: Option<String>,
    /// A handful of entries are internal/reserve capacity, hidden from
    /// `proxy list` in a later phase; irrelevant to redirect validation.
    #[serde(default)]
    #[expect(dead_code, reason = "modeled for the strict fixture tests; never read")]
    pub hidden: Option<bool>,
}

/// Shared by every test in this module that needs `Vec<ApiProxy>` from the
/// captured fixture (`commands::action_flags`'s tests mount the raw fixture
/// file directly, not this helper).
#[cfg(test)]
pub(crate) fn proxies_fixture(name: &str) -> Vec<ApiProxy> {
    super::fixture_list(name, "proxies")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_captured_list_deserializes_strictly() {
        let proxies = proxies_fixture("proxies.json");
        assert!(
            proxies
                .iter()
                .any(|p| p.pk == "LHR" && p.city == "London" && p.country_name == "United Kingdom")
        );
    }
}
