//! The `/devices` wire shape and the normalized device (D2,
//! commands.md#device-api-endpoints). The API calls these "endpoints"; the URL
//! segment and this CLI say "device".

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Error;
use crate::output::time;

/// One entry of `body.devices[]`. Only spec-required fields are required
/// here: an absent optional costs one `null`, never the whole listing.
#[derive(Debug, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))]
pub struct ApiDevice {
    #[serde(rename = "PK")]
    pub pk: String,
    pub device_id: String,
    #[serde(default)]
    #[expect(dead_code, reason = "modeled for the strict fixture tests; never read")]
    pub ts: Option<Value>,
    pub name: String,
    #[serde(default)]
    #[expect(dead_code, reason = "modeled for the strict fixture tests; never read")]
    pub user: Option<Value>,
    #[serde(default)]
    pub stats: Option<u8>,
    pub status: u8,
    #[serde(default)]
    pub client_count: Option<u64>,
    pub learn_ip: u8,
    #[serde(default)]
    pub ctrld: Option<ApiCtrld>,
    pub resolvers: ApiResolvers,
    #[serde(default)]
    pub icon: Option<String>,
    pub profile: ApiDeviceProfile,
    // Not surfaced (`profile2` is undocumented, D16): modeled so captured
    // fixtures still deserialize strictly.
    #[serde(default)]
    #[expect(dead_code, reason = "modeled for the strict fixture tests; never read")]
    pub profile2: Option<Value>,
    #[serde(default)]
    #[expect(dead_code, reason = "modeled for the strict fixture tests; never read")]
    pub ip_count: Option<Value>,
}

/// The `ctrld` daemon's self-report: undocumented, so fully optional. Its
/// `status` int has no documented meaning and is never surfaced (D10).
#[derive(Debug, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))]
pub struct ApiCtrld {
    #[serde(default)]
    #[expect(dead_code, reason = "modeled for the strict fixture tests; never read")]
    pub status: Option<Value>,
    #[serde(default)]
    pub last_fetch: Option<i64>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    #[expect(dead_code, reason = "modeled for the strict fixture tests; never read")]
    pub version_target: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))]
pub struct ApiResolvers {
    #[serde(default)]
    #[expect(dead_code, reason = "modeled for the strict fixture tests; never read")]
    pub uid: Option<Value>,
    pub doh: String,
    pub dot: String,
    #[serde(default)]
    pub v4: Vec<String>,
    #[serde(default)]
    pub v6: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))]
pub struct ApiDeviceProfile {
    #[serde(rename = "PK")]
    pub pk: String,
    #[serde(default)]
    #[expect(dead_code, reason = "modeled for the strict fixture tests; never read")]
    pub updated: Option<Value>,
    pub name: String,
}

/// The normalized device — the documented JSON schema, stable key set.
#[derive(Debug, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub profile: DeviceProfile,
    pub status: DeviceStatus,
    pub analytics: Option<Analytics>,
    pub clients: Option<u64>,
    pub learn_ip: bool,
    pub icon: Option<String>,
    pub ctrld: Option<Ctrld>,
    pub resolvers: Resolvers,
}

#[derive(Debug, Serialize)]
pub struct DeviceProfile {
    pub id: String,
    pub name: String,
}

/// The API's `status` ints 0-3, named. Devices are born `pending` and flip
/// to `active` on their first DNS query; the two disabled states differ
/// (soft serves unfiltered DNS, hard serves none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceStatus {
    Pending,
    Active,
    SoftDisabled,
    HardDisabled,
}

impl DeviceStatus {
    fn from_api(code: u8) -> Result<Self, Error> {
        match code {
            0 => Ok(Self::Pending),
            1 => Ok(Self::Active),
            2 => Ok(Self::SoftDisabled),
            3 => Ok(Self::HardDisabled),
            other => Err(crate::error::upstream_shape(format_args!(
                "unknown device status {other}"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Active => "active",
            Self::SoftDisabled => "soft-disabled",
            Self::HardDisabled => "hard-disabled",
        }
    }

    /// The wire int for a status write (`PUT /devices/{id}` `status=`).
    pub fn api_code(self) -> u8 {
        match self {
            Self::Pending => 0,
            Self::Active => 1,
            Self::SoftDisabled => 2,
            Self::HardDisabled => 3,
        }
    }
}

impl Serialize for DeviceStatus {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// The API's `stats` ints 0-2, named after its own `analytics levels`
/// catalogue (`No/Some/Full Analytics`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Analytics {
    None,
    Some,
    Full,
}

impl Analytics {
    fn from_api(code: u8) -> Result<Self, Error> {
        match code {
            0 => Ok(Self::None),
            1 => Ok(Self::Some),
            2 => Ok(Self::Full),
            other => Err(crate::error::upstream_shape(format_args!(
                "unknown analytics level {other}"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Some => "some",
            Self::Full => "full",
        }
    }

    /// The wire int for an analytics write (`PUT /devices/{id}` `stats=`).
    pub fn api_code(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Some => 1,
            Self::Full => 2,
        }
    }
}

impl Serialize for Analytics {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

#[derive(Debug, Serialize)]
pub struct Ctrld {
    pub version: Option<String>,
    pub last_fetch: Option<String>,
}

/// The resolver identities of one device, every form the API offers. `v4`/
/// `v6` are `[]` when the API omits them, never absent.
#[derive(Debug, Serialize)]
pub struct Resolvers {
    pub doh: String,
    pub dot: String,
    pub v4: Vec<String>,
    pub v6: Vec<String>,
}

impl Device {
    /// The struct's serialized top-level key set, in order — `device
    /// list`/`get`'s upfront `--fields` check (`output::validate_fields`)
    /// validates against exactly this, before any request.
    /// `device_fields_matches_the_serialized_key_set` (below) is the drift
    /// guard.
    pub const FIELDS: &'static [&'static str] = &[
        "id",
        "name",
        "profile",
        "status",
        "analytics",
        "clients",
        "learn_ip",
        "icon",
        "ctrld",
        "resolvers",
    ];

    pub fn from_api(api: &ApiDevice) -> Result<Self, Error> {
        // `id` is `device_id`, the identifier the API's own paths key on.
        // The docs only observe that `PK` equals it: a divergence must fail
        // loud, never pick one silently.
        if api.pk != api.device_id {
            return Err(crate::error::upstream_shape(format_args!(
                "device PK {:?} differs from device_id {:?}",
                api.pk, api.device_id
            )));
        }
        let ctrld = api
            .ctrld
            .as_ref()
            .map(|ctrld| {
                let last_fetch = match ctrld.last_fetch {
                    Some(ts) if ts < 0 => {
                        return Err(crate::error::upstream_shape(format_args!(
                            "negative timestamp ctrld.last_fetch={ts}"
                        )));
                    }
                    Some(ts) => Some(time::rfc3339(ts)),
                    None => None,
                };
                Ok(Ctrld {
                    version: ctrld.version.clone(),
                    last_fetch,
                })
            })
            .transpose()?;
        // 0/1 documented; any other code is a shape error (`ApiAction::enabled`).
        let learn_ip = match api.learn_ip {
            0 => false,
            1 => true,
            other => {
                return Err(crate::error::upstream_shape(format_args!(
                    "unknown learn_ip={other}"
                )));
            }
        };
        Ok(Self {
            id: api.device_id.clone(),
            name: api.name.clone(),
            profile: DeviceProfile {
                id: api.profile.pk.clone(),
                name: api.profile.name.clone(),
            },
            status: DeviceStatus::from_api(api.status)?,
            analytics: api.stats.map(Analytics::from_api).transpose()?,
            clients: api.client_count,
            learn_ip,
            icon: api.icon.clone(),
            ctrld,
            resolvers: Resolvers {
                doh: api.resolvers.doh.clone(),
                dot: api.resolvers.dot.clone(),
                v4: api.resolvers.v4.clone(),
                v6: api.resolvers.v6.clone(),
            },
        })
    }
}

#[cfg(test)]
pub(crate) fn devices_fixture(name: &str) -> Vec<ApiDevice> {
    super::fixture_list(name, "devices")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal(overrides: &serde_json::Value) -> ApiDevice {
        let mut doc = serde_json::json!({
            "PK": "dev1", "device_id": "dev1", "name": "X", "status": 1,
            "client_count": 1, "learn_ip": 0,
            "resolvers": {"doh": "https://dns.example/dev1", "dot": "dev1.dns.example"},
            "profile": {"PK": "pk1", "name": "Home"}
        });
        for (key, value) in overrides.as_object().expect("object") {
            doc[key] = value.clone();
        }
        serde_json::from_value(doc).expect("minimal device deserializes")
    }

    /// Drift guard for [`Device::FIELDS`].
    #[test]
    fn device_fields_matches_the_serialized_key_set() {
        let device = Device::from_api(&minimal(&serde_json::json!({}))).expect("normalizes");
        let value = serde_json::to_value(&device).expect("serializes");
        let keys: Vec<&str> = value
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, Device::FIELDS);
    }

    #[test]
    fn the_captured_list_normalizes() {
        let devices = devices_fixture("devices.json");
        assert_eq!(devices.len(), 10);
        let first = Device::from_api(&devices[0]).expect("normalizes");
        assert_eq!(first.id, "dev6ed2281d");
        assert_eq!(first.name, "Device 1");
        assert_eq!(first.profile.id, "pk4cada1c5");
        assert_eq!(first.profile.name, "Aggressive");
        assert_eq!(first.status, DeviceStatus::Active);
        assert_eq!(first.analytics, Some(Analytics::Full));
        assert_eq!(first.clients, Some(1));
        assert!(!first.learn_ip);
        assert_eq!(first.icon.as_deref(), Some("desktop-linux"));
        let ctrld = first.ctrld.expect("first device runs ctrld");
        assert_eq!(ctrld.version.as_deref(), Some("v1.5.3"));
        assert_eq!(ctrld.last_fetch.as_deref(), Some("2026-07-11T10:22:50Z"));
        assert_eq!(first.resolvers.dot, "dev6ed2281d.dns.controld.com");
        assert!(first.resolvers.v4.is_empty());
        assert_eq!(first.resolvers.v6.len(), 2);
    }

    #[test]
    fn absent_optionals_normalize_to_null() {
        let devices = devices_fixture("devices.json");
        let bare = Device::from_api(&devices[9]).expect("normalizes");
        assert_eq!(bare.analytics, None);
        assert_eq!(bare.icon, None);
        assert!(bare.ctrld.is_none());
    }

    #[test]
    fn every_status_and_analytics_level_is_named() {
        for (code, name) in [
            (0, "pending"),
            (1, "active"),
            (2, "soft-disabled"),
            (3, "hard-disabled"),
        ] {
            let device = Device::from_api(&minimal(&serde_json::json!({"status": code})))
                .expect("normalizes");
            assert_eq!(device.status.as_str(), name);
        }
        for (code, name) in [(0, "none"), (1, "some"), (2, "full")] {
            let device = Device::from_api(&minimal(&serde_json::json!({"stats": code})))
                .expect("normalizes");
            assert_eq!(device.analytics.expect("reported").as_str(), name);
        }
    }

    #[test]
    fn unknown_status_and_analytics_codes_are_shape_errors() {
        let error = Device::from_api(&minimal(&serde_json::json!({"status": 4})))
            .expect_err("unknown status");
        assert!(error.message.contains("status 4"));
        let error = Device::from_api(&minimal(&serde_json::json!({"stats": 3})))
            .expect_err("unknown level");
        assert!(error.message.contains("level 3"));
    }

    #[test]
    fn a_partial_ctrld_report_and_a_missing_client_count_normalize_to_null() {
        let device = Device::from_api(&minimal(&serde_json::json!({
            "ctrld": {"status": 0}, "client_count": null
        })))
        .expect("normalizes");
        let ctrld = device.ctrld.expect("present but partial");
        assert_eq!(ctrld.version, None);
        assert_eq!(ctrld.last_fetch, None);
        assert_eq!(device.clients, None);
    }

    #[test]
    fn an_unknown_learn_ip_code_is_a_shape_error() {
        let error = Device::from_api(&minimal(&serde_json::json!({"learn_ip": 2})))
            .expect_err("tri-state learn_ip");
        assert!(error.message.contains("learn_ip=2"));
    }

    #[test]
    fn pk_device_id_divergence_is_a_shape_error() {
        let error = Device::from_api(&minimal(&serde_json::json!({"PK": "other"})))
            .expect_err("divergent ids");
        assert!(error.message.contains("differs from device_id"));
    }

    #[test]
    fn a_negative_ctrld_timestamp_is_a_shape_error() {
        let error = Device::from_api(&minimal(&serde_json::json!({
            "ctrld": {"version": "v1", "last_fetch": -1}
        })))
        .expect_err("negative last_fetch");
        assert!(error.message.contains("last_fetch=-1"));
    }
}
