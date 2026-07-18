//! `cdctl profile` — the account's profiles (commands.md#profile).
//! Phase 3 ships the read surface; writes arrive with Protection (v0.3).

use clap::Subcommand;

use crate::cli::Globals;
use crate::error::Error;
use crate::model::profile::Profile;
use crate::output::{self, emit, print_key_values, render_table, time};

#[derive(Debug, Subcommand)]
pub enum ProfileCommand {
    /// List profiles
    List,
    /// Show one profile by id or name
    Get {
        /// Profile id or name (case-insensitive, an ambiguous name is an
        /// error)
        selector: String,
    },
}

pub async fn run(command: ProfileCommand, globals: &Globals) -> Result<(), Error> {
    match command {
        ProfileCommand::List => list(globals).await,
        ProfileCommand::Get { selector } => get(&selector, globals).await,
    }
}

async fn list(globals: &Globals) -> Result<(), Error> {
    output::validate_fields(globals.fields.as_deref(), Profile::FIELDS)?;
    let (client, _source, _config) = super::authenticated_client(globals)?;
    let api_profiles = super::scope::fetch_profiles(&client).await?;
    let profiles = api_profiles
        .iter()
        .map(Profile::from_api)
        .collect::<Result<Vec<_>, _>>()?;

    emit(globals.mode, globals.fields.as_deref(), &profiles, || {
        let now = time::now();
        let rows = profiles
            .iter()
            .map(|profile| {
                vec![
                    profile.name.clone(),
                    profile.id.clone(),
                    profile.enabled_rules.to_string(),
                    time::relative(profile.updated_unix, now),
                ]
            })
            .collect();
        println!(
            "{}",
            render_table(&["NAME", "ID", "RULES", "UPDATED"], rows, globals.plain)
        );
    })
}

async fn get(selector: &str, globals: &Globals) -> Result<(), Error> {
    super::validate::reject_control_chars(selector, "the profile selector")?;
    output::validate_fields(globals.fields.as_deref(), Profile::FIELDS)?;
    let (client, _source, _config) = super::authenticated_client(globals)?;
    let api_profiles = super::scope::fetch_profiles(&client).await?;
    let profile = Profile::from_api(super::scope::find_profile(&api_profiles, selector)?)?;

    emit(globals.mode, globals.fields.as_deref(), &profile, || {
        print_key_values(&[
            ("name", profile.name.clone()),
            ("id", profile.id.clone()),
            ("enabled_rules", profile.enabled_rules.to_string()),
            ("enabled_filters", profile.enabled_filters.to_string()),
            ("enabled_services", profile.enabled_services.to_string()),
            ("folders", profile.folders.to_string()),
            ("options", profile.options.to_string()),
            ("default_action", profile.default_action.to_string()),
            ("enabled", profile.enabled.to_string()),
            (
                "disabled_until",
                profile
                    .disabled_until
                    .clone()
                    .unwrap_or_else(|| "-".to_owned()),
            ),
            ("updated", profile.updated.clone()),
        ]);
    })
}
