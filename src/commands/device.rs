//! `cdctl device` — the account's devices, which the API calls endpoints
//! (commands.md#device-api-endpoints). `update` never trusts the PUT
//! response: it echoes stale state (the pre-switch profile, live probe
//! 2026-08-19, write-verification.md), so every write is verified by
//! re-reading `GET /devices`.

use clap::Subcommand;
use reqwest::Method;

use crate::cli::Globals;
use crate::error::{Error, Exit};
use crate::model::device::{Analytics, ApiDevice, Device, DeviceProfile, DeviceStatus};
use crate::output::{self, emit, print_doc, print_key_values, render_table};

use super::plan::{self, DeviceUpdateIntent, DryRun};

/// `clap` stays out of `model::device` (D18): the impls live here, the
/// wire/normalized types there. `pending` is not offered — the API accepts
/// writing it, but un-using a device is not a state a user can mean.
impl clap::ValueEnum for DeviceStatus {
    fn value_variants<'a>() -> &'a [Self] {
        &[Self::Active, Self::SoftDisabled, Self::HardDisabled]
    }

    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        Some(clap::builder::PossibleValue::new(self.as_str()))
    }
}

impl clap::ValueEnum for Analytics {
    fn value_variants<'a>() -> &'a [Self] {
        &[Self::None, Self::Some, Self::Full]
    }

    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        Some(clap::builder::PossibleValue::new(self.as_str()))
    }
}

#[derive(Debug, Subcommand)]
pub enum DeviceCommand {
    /// List devices
    List,
    /// Show one device by id or name
    Get {
        /// Device id or name (case-insensitive, an ambiguous name is an
        /// error)
        selector: String,
    },
    /// Update a device (only the given fields change)
    Update {
        /// Device id or name (case-insensitive, an ambiguous name is an
        /// error)
        selector: String,
        /// New device name
        #[arg(long)]
        name: Option<String>,
        /// Profile to enforce (name or PK)
        #[arg(long, value_name = "PK|name")]
        enforce: Option<String>,
        /// Lifecycle state (soft-disabled serves unfiltered DNS, hard-disabled serves none)
        #[arg(long, value_enum)]
        status: Option<DeviceStatus>,
        /// Analytics level
        #[arg(long, value_enum)]
        analytics: Option<Analytics>,
        #[command(flatten)]
        dry_run: DryRun,
    },
}

pub async fn run(command: DeviceCommand, globals: &Globals) -> Result<(), Error> {
    match command {
        DeviceCommand::List => list(globals).await,
        DeviceCommand::Get { selector } => get(&selector, globals).await,
        DeviceCommand::Update {
            selector,
            name,
            enforce,
            status,
            analytics,
            dry_run,
        } => {
            update(
                &selector,
                Changes {
                    name,
                    enforce,
                    status,
                    analytics,
                },
                dry_run.dry_run,
                globals,
            )
            .await
        }
    }
}

/// The requested patch: `None` = leave untouched (nothing is sent).
struct Changes {
    name: Option<String>,
    enforce: Option<String>,
    status: Option<DeviceStatus>,
    analytics: Option<Analytics>,
}

fn validate_update(changes: &Changes, globals: &Globals) -> Result<(), Error> {
    if let Some(new_name) = &changes.name {
        super::validate::reject_control_chars(new_name, "the new device name")?;
        // The API 400s over 32 (write-verification.md); fail before any request.
        if new_name.chars().count() > 32 {
            return Err(Error::usage(
                "device names are capped at 32 characters (API limit)",
            ));
        }
    }
    if let Some(profile) = &changes.enforce {
        super::validate::reject_control_chars(profile, "the profile selector")?;
    }
    // The global `--profile` (profile scope) is a different flag: eating it
    // silently here would drop part of what the user typed.
    if globals.profile_from_flag {
        return Err(Error::usage(
            "device update writes the enforced profile with --enforce <PK|name>; the global \
             --profile selects a profile scope, which device commands don't use",
        ));
    }
    if changes.name.is_none()
        && changes.enforce.is_none()
        && changes.status.is_none()
        && changes.analytics.is_none()
    {
        return Err(Error::usage(
            "device update needs at least one change: --name, --enforce, --status, or \
             --analytics",
        ));
    }
    Ok(())
}

/// The wire pairs for the patch, written-field order frozen by the tests.
fn form_pairs(changes: &Changes, profile: Option<&DeviceProfile>) -> Vec<(&'static str, String)> {
    let mut form = Vec::new();
    if let Some(name) = &changes.name {
        form.push(("name", name.clone()));
    }
    if let Some(profile) = profile {
        form.push(("profile_id", profile.id.clone()));
    }
    if let Some(status) = changes.status {
        form.push(("status", status.api_code().to_string()));
    }
    if let Some(analytics) = changes.analytics {
        form.push(("stats", analytics.api_code().to_string()));
    }
    form
}

async fn update(
    selector: &str,
    changes: Changes,
    dry_run: bool,
    globals: &Globals,
) -> Result<(), Error> {
    super::validate::reject_control_chars(selector, "the device selector")?;
    validate_update(&changes, globals)?;
    super::preflight_fields(globals, Device::FIELDS, dry_run)?;
    let (client, _source, _config) = super::authenticated_client(globals)?;

    // Both resolution GETs are independent: overlap them, devices error
    // first (deterministic, as validated_client does).
    let (api_devices, api_profiles) = match &changes.enforce {
        Some(_) => {
            let (devices, profiles) = tokio::join!(
                super::scope::fetch_devices(&client),
                super::scope::fetch_profiles(&client)
            );
            (devices?, Some(profiles?))
        }
        None => (super::scope::fetch_devices(&client).await?, None),
    };
    let device_id = super::scope::find_device(&api_devices, selector)?
        .device_id
        .clone();
    super::validate::validate_path_bound(&device_id, "the device id")?;
    let profile = match (&changes.enforce, &api_profiles) {
        (Some(selector), Some(profiles)) => {
            let profile = super::scope::find_profile(profiles, selector)?;
            super::validate::validate_path_bound(&profile.pk, "the profile id")?;
            Some(DeviceProfile {
                id: profile.pk.clone(),
                name: profile.name.clone(),
            })
        }
        _ => None,
    };

    let path = format!("/devices/{device_id}");
    if dry_run {
        return plan::print_one(
            globals,
            "PUT",
            path,
            &DeviceUpdateIntent {
                name: changes.name.clone(),
                profile,
                status: changes.status,
                analytics: changes.analytics,
            },
        );
    }

    let form = form_pairs(&changes, profile.as_ref());

    // An ambiguous (retryable) write error still falls through to the
    // read-back: if the intent landed, that is success, and a re-run
    // converges either way (the patch is idempotent).
    let write_error = match client.write(Method::PUT, &path, &form, "device").await {
        Ok(_envelope) => None,
        Err(e) if e.retryable() => Some(e),
        Err(e) => return Err(e),
    };

    let read_back =
        super::scope::fetch_devices(&client)
            .await
            .map_err(|error| match &write_error {
                Some(write_error) => super::landed_write_unverified(error, "device")
                    .with_debug_note(format!(
                        "the read-back could not resolve the preceding write error: {}",
                        write_error.message
                    )),
                None => super::landed_write_unverified(error, "device"),
            })?;
    let landed = read_back
        .iter()
        .find(|d| d.device_id == device_id)
        .ok_or_else(|| {
            crate::error::upstream_shape(format_args!(
                "device {device_id:?} vanished from the read-back"
            ))
        })?;
    if let Some(mismatch) = verify(landed, &changes, profile.as_ref()) {
        return Err(match write_error {
            Some(original) => original,
            None => Error::new(
                "device.unverified",
                format!("the API acknowledged the update, but the read-back shows {mismatch}"),
                Exit::Retryable,
            )
            .with_hint("re-run the same command; the patch is idempotent"),
        });
    }
    if write_error.is_some() {
        globals.info("the write reported an error but the read-back confirms it landed");
    }

    let device = Device::from_api(landed)?;
    emit(globals.mode, globals.fields.as_deref(), &device, || {
        print_key_values(&[
            ("name", device.name.clone()),
            ("id", device.id.clone()),
            (
                "profile",
                format!("{} ({})", device.profile.name, device.profile.id),
            ),
            ("status", device.status.as_str().to_owned()),
            (
                "analytics",
                dash_or(device.analytics.map(|a| a.as_str().to_owned())),
            ),
        ])
    })
}

/// The written fields against the read-back, first mismatch described.
/// The PUT echo is never consulted (it can be stale); only this compare
/// proves the write.
fn verify(
    landed: &ApiDevice,
    changes: &Changes,
    profile: Option<&DeviceProfile>,
) -> Option<String> {
    if let Some(name) = &changes.name {
        if &landed.name != name {
            return Some(format!("name {:?}", landed.name));
        }
    }
    if let Some(profile) = profile {
        if landed.profile.pk != profile.id {
            return Some(format!("profile {:?}", landed.profile.pk));
        }
    }
    if let Some(status) = changes.status {
        if landed.status != status.api_code() {
            return Some(format!("status {}", landed.status));
        }
    }
    if let Some(analytics) = changes.analytics {
        // An absent `stats` reads as off (devices never assigned a level
        // omit the key), so `--analytics none` converges on `None` too —
        // desired state, not write receipt, is what read-back proves.
        let converged = match landed.stats {
            Some(code) => code == analytics.api_code(),
            None => analytics == Analytics::None,
        };
        if !converged {
            return Some(format!("stats {:?}", landed.stats));
        }
    }
    None
}

async fn list(globals: &Globals) -> Result<(), Error> {
    output::validate_fields(globals.fields.as_deref(), Device::FIELDS)?;
    let (client, _source, _config) = super::authenticated_client(globals)?;
    let api_devices = super::scope::fetch_devices(&client).await?;
    let devices = api_devices
        .iter()
        .map(Device::from_api)
        .collect::<Result<Vec<_>, _>>()?;

    emit(globals.mode, globals.fields.as_deref(), &devices, || {
        let rows = devices
            .iter()
            .map(|device| {
                vec![
                    device.name.clone(),
                    device.id.clone(),
                    device.profile.name.clone(),
                    device.status.as_str().to_owned(),
                    dash_or(device.clients.map(|n| n.to_string())),
                    dash_or(device.ctrld.as_ref().and_then(|c| c.version.clone())),
                ]
            })
            .collect();
        print_doc(render_table(
            &["NAME", "ID", "PROFILE", "STATUS", "CLIENTS", "CTRLD"],
            rows,
            globals.plain,
        ))
    })
}

async fn get(selector: &str, globals: &Globals) -> Result<(), Error> {
    super::validate::reject_control_chars(selector, "the device selector")?;
    output::validate_fields(globals.fields.as_deref(), Device::FIELDS)?;
    let (client, _source, _config) = super::authenticated_client(globals)?;
    let api_devices = super::scope::fetch_devices(&client).await?;
    let device = Device::from_api(super::scope::find_device(&api_devices, selector)?)?;

    emit(globals.mode, globals.fields.as_deref(), &device, || {
        print_key_values(&[
            ("name", device.name.clone()),
            ("id", device.id.clone()),
            (
                "profile",
                format!("{} ({})", device.profile.name, device.profile.id),
            ),
            ("status", device.status.as_str().to_owned()),
            (
                "analytics",
                dash_or(device.analytics.map(|a| a.as_str().to_owned())),
            ),
            ("clients", dash_or(device.clients.map(|n| n.to_string()))),
            ("learn_ip", device.learn_ip.to_string()),
            ("icon", dash_or(device.icon.clone())),
            (
                "ctrld_version",
                dash_or(device.ctrld.as_ref().and_then(|c| c.version.clone())),
            ),
            (
                "ctrld_last_fetch",
                dash_or(device.ctrld.as_ref().and_then(|c| c.last_fetch.clone())),
            ),
            ("resolver_doh", device.resolvers.doh.clone()),
            ("resolver_dot", device.resolvers.dot.clone()),
            ("resolver_v4", list_or_dash(&device.resolvers.v4)),
            ("resolver_v6", list_or_dash(&device.resolvers.v6)),
        ])
    })
}

fn dash_or(value: Option<String>) -> String {
    value.unwrap_or_else(|| "-".to_owned())
}

fn list_or_dash(items: &[String]) -> String {
    dash_or(Some(items.join(", ")).filter(|s| !s.is_empty()))
}
