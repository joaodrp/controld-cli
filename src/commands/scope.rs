//! Name-or-id resolution (commands.md#name-resolution) for both profiles and
//! folders: exact id match wins, else exact case-insensitive name; multiple
//! matches are an error (never guess — exit 2), no match is exit 3.

use crate::api::client::Client;
use crate::cli::Globals;
use crate::error::{Error, Exit};
use crate::model::folder::ApiFolder;
use crate::model::profile::ApiProfile;
use crate::output::escape_controls;

/// One `GET /profiles` for every caller — there is no `GET /profiles/{id}`;
/// `profile get` filters client-side (commands.md). Lives beside resolution
/// since every caller that resolves a profile pays this extra GET.
pub(crate) async fn fetch_profiles(client: &Client) -> Result<Vec<ApiProfile>, Error> {
    client
        .get("/profiles", "profile")
        .await?
        .keyed_as("profiles")
}

pub(crate) fn find_profile<'a>(
    profiles: &'a [ApiProfile],
    selector: &str,
) -> Result<&'a ApiProfile, Error> {
    if let Some(profile) = profiles.iter().find(|p| p.pk == selector) {
        return Ok(profile);
    }
    let named: Vec<&ApiProfile> = profiles
        .iter()
        .filter(|p| p.name.eq_ignore_ascii_case(selector))
        .collect();
    match named.as_slice() {
        [profile] => Ok(profile),
        [] => Err(Error::new(
            "profile.not_found",
            format!("no profile matches {selector:?}"),
            Exit::NotFound,
        )),
        matches => Err(Error::usage(format!(
            "{selector:?} matches {} profiles ({}); use the id",
            matches.len(),
            matches
                .iter()
                .map(|p| p.pk.as_str())
                .collect::<Vec<_>>()
                .join(", "),
        ))),
    }
}

/// The resolved profile target for a command, plus whether it was named
/// explicitly (`--profile`/`CONTROLD_PROFILE`) or fell back to the config
/// file's `default_profile` (D8: only the latter makes `--yes` ignorable).
pub(crate) struct ProfileScope {
    pub id: String,
    pub name: String,
    pub explicit: bool,
}

/// Resolve the profile a command should operate on: `--profile`/
/// `CONTROLD_PROFILE` (`Globals::resolve` merges the env fallback into
/// `globals.profile`, flag winning) wins as **explicit**; otherwise
/// `config_default` (the config file's `default_profile`, loaded once by the
/// caller) is **implicit** (D8); neither is exit 2, `usage.no_profile`.
pub(crate) async fn resolve_profile(
    client: &Client,
    globals: &Globals,
    config_default: Option<&str>,
) -> Result<ProfileScope, Error> {
    let (selector, explicit) = if let Some(selector) = &globals.profile {
        (selector.clone(), true)
    } else {
        let Some(default) = config_default else {
            return Err(Error::new(
                "usage.no_profile",
                "no profile given: pass --profile <id|name>, set CONTROLD_PROFILE, \
                 or run `cdctl config set default_profile <id|name>`",
                Exit::Usage,
            ));
        };
        (default.to_owned(), false)
    };
    super::validate::reject_control_chars(&selector, "the profile selector")
        .map_err(|e| hint_implicit_selector(e, explicit))?;

    let profiles = fetch_profiles(client).await?;
    let profile =
        find_profile(&profiles, &selector).map_err(|e| hint_implicit_selector(e, explicit))?;
    let scope = ProfileScope {
        id: profile.pk.clone(),
        name: profile.name.clone(),
        explicit,
    };
    if !explicit {
        // The caller may have forgotten the ambient default is in play
        // (D8's rationale for treating it as a lesser trust level).
        globals.info(format_args!(
            "using default profile \"{}\" ({}) from config",
            escape_controls(&scope.name),
            escape_controls(&scope.id)
        ));
    }
    // The resolved id enters URL paths (profile-scoped commands build
    // `/profiles/{id}/...` paths); every caller pays this check once, here.
    super::validate::validate_path_bound(&scope.id, "the profile id")?;
    Ok(scope)
}

/// When the profile came from the config's `default_profile` (implicit), a
/// failure resolving it should say so — the caller may not know the
/// selector wasn't theirs. Never clobbers a hint the error already carries.
fn hint_implicit_selector(error: Error, explicit: bool) -> Error {
    if explicit || error.hint.is_some() {
        return error;
    }
    error.with_hint(
        "this selector came from the config file's default_profile; change it with \
         `cdctl config set default_profile <id|name>` or pass --profile",
    )
}

/// A profile's groups (API name for folders) listing/create path. Shared by
/// [`fetch_folders`] and every `folder` handler that lists or creates.
pub(crate) fn groups_path(profile_id: &str) -> String {
    format!("/profiles/{profile_id}/groups")
}

/// One folder's path, for the `folder` handlers that update or delete a
/// single resolved folder (its pk is always the path segment — a name in
/// the path 404s upstream, folder.rs's module doc).
pub(crate) fn group_path(profile_id: &str, folder_pk: i64) -> String {
    format!("{}/{folder_pk}", groups_path(profile_id))
}

/// One `GET /profiles/{id}/groups` for the folder verbs that resolve or
/// list folders (`list`, `update`, `delete` — `create` never calls it);
/// resolution costs this extra GET (commands.md#name-resolution). Shared by
/// `folder` and `rule`, which resolves folders identically.
pub(crate) async fn fetch_folders(
    client: &Client,
    profile_id: &str,
) -> Result<Vec<ApiFolder>, Error> {
    client
        .get(&groups_path(profile_id), "folder")
        .await?
        .keyed_as("groups")
}

/// Exact id match wins (only when the selector parses *and* matches a PK);
/// else a unique case-insensitive name; ambiguous names name the candidate
/// ids (folder names are not unique, so no coin-flip); no match is exit 3.
pub(crate) fn find_folder<'a>(
    folders: &'a [ApiFolder],
    selector: &str,
) -> Result<&'a ApiFolder, Error> {
    if let Ok(id) = selector.parse::<i64>() {
        if let Some(folder) = folders.iter().find(|f| f.pk == id) {
            return Ok(folder);
        }
    }
    let named: Vec<&ApiFolder> = folders
        .iter()
        .filter(|f| f.name.eq_ignore_ascii_case(selector))
        .collect();
    match named.as_slice() {
        [folder] => Ok(folder),
        [] => Err(Error::new(
            "folder.not_found",
            format!("no folder matches {selector:?}"),
            Exit::NotFound,
        )),
        matches => Err(Error::usage(format!(
            "{selector:?} matches {} folders (ids {}); use the id",
            matches.len(),
            matches
                .iter()
                .map(|f| f.pk.to_string())
                .collect::<Vec<_>>()
                .join(", "),
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::folder::folders_fixture;
    use crate::model::profile::profiles_fixture;

    #[test]
    fn exact_id_wins() {
        let list = profiles_fixture("profiles.json");
        assert_eq!(find_profile(&list, "pk69038f3c").expect("id").name, "Kids");
    }

    #[test]
    fn unique_name_matches_case_insensitively() {
        let list = profiles_fixture("profiles.json");
        assert_eq!(find_profile(&list, "kids").expect("name").pk, "pk69038f3c");
    }

    #[test]
    fn ambiguous_name_is_a_usage_error_naming_the_candidates() {
        let list = profiles_fixture("profiles_dup_names.json");
        let error = find_profile(&list, "Home").expect_err("ambiguous");
        assert_eq!(error.exit(), Exit::Usage);
        assert!(error.message.contains("pk11aa22bb"));
        assert!(error.message.contains("pk33cc44dd"));
    }

    #[test]
    fn no_match_is_exit_3() {
        let list = profiles_fixture("profiles.json");
        let error = find_profile(&list, "nope").expect_err("missing");
        assert_eq!(error.exit(), Exit::NotFound);
        assert_eq!(error.code, "profile.not_found");
    }

    #[test]
    fn exact_id_wins_over_a_name_that_looks_like_the_id() {
        let folders = folders_fixture("p_groups_full.json");
        let found = find_folder(&folders, "1").expect("id match");
        assert_eq!(found.pk, 1);
    }

    #[test]
    fn unique_folder_name_matches_case_insensitively() {
        let folders = folders_fixture("p_groups_full.json");
        let found = find_folder(&folders, "noaction").expect("name match");
        assert_eq!(found.pk, 1);
    }

    #[test]
    fn ambiguous_ci_duplicate_folder_names_name_the_candidate_ids() {
        let folders = folders_fixture("p_groups_full.json");
        let error = find_folder(&folders, "ads").expect_err("ambiguous");
        assert_eq!(error.exit(), Exit::Usage);
        assert!(error.message.contains('4'));
        assert!(error.message.contains('5'));
    }

    #[test]
    fn no_folder_match_is_exit_3() {
        let folders = folders_fixture("p_groups_full.json");
        let error = find_folder(&folders, "nope").expect_err("missing");
        assert_eq!(error.exit(), Exit::NotFound);
        assert_eq!(error.code, "folder.not_found");
    }
}
