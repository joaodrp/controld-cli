//! `cdctl device` — the account's devices, which the API calls endpoints
//! (commands.md#device-api-endpoints). The read surface; writes arrive with
//! the rest of the fleet group (roadmap.md).

use clap::Subcommand;

use crate::cli::Globals;
use crate::error::Error;
use crate::model::device::Device;
use crate::output::{self, emit, print_doc, print_key_values, render_table};

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
}

pub async fn run(command: DeviceCommand, globals: &Globals) -> Result<(), Error> {
    match command {
        DeviceCommand::List => list(globals).await,
        DeviceCommand::Get { selector } => get(&selector, globals).await,
    }
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
