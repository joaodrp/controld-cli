//! Name-or-id resolution (commands.md#name-resolution): exact id match wins,
//! else exact case-insensitive name; multiple matches are an error (never
//! guess — exit 2), no match is exit 3.

use crate::api::client::Client;
use crate::error::{Error, Exit};
use crate::model::profile::ApiProfile;

/// One `GET /profiles` for both `profile` verbs — there is no
/// `GET /profiles/{id}`; `profile get` filters client-side (commands.md).
/// Lives beside resolution since every caller that resolves a profile pays
/// this extra GET.
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

#[cfg(test)]
mod tests {
    use super::*;
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
}
