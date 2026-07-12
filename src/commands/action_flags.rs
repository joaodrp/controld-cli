//! The shared action flags. Reused by `rule`, `folder`, `service`, and
//! `profile default` (commands.md#the-shared-action-flags). **No clap-level
//! defaults** — a parser default would silently send `status=1` on every
//! update (`folder update 2 --name New` must not re-enable a disabled
//! folder).

use crate::api::client::Client;
use crate::error::Error;
use crate::model::action::Action;
use crate::model::proxy::ApiProxy;

/// `clap` stays out of `model::action` (D18's per-noun layering): the impl
/// lives here, the wire/normalized type there.
impl clap::ValueEnum for Action {
    fn value_variants<'a>() -> &'a [Self] {
        &[Self::Block, Self::Bypass, Self::Spoof, Self::Redirect]
    }

    fn to_possible_value(&self) -> Option<clap::builder::PossibleValue> {
        Some(clap::builder::PossibleValue::new(self.name()))
    }
}

#[derive(Debug, clap::Args)]
pub struct ActionFlags {
    /// Action to apply
    #[arg(long, value_enum)]
    pub action: Option<Action>,
    /// Spoof target (IP or CNAME), or the redirect proxy PK (see `proxy list`)
    #[arg(long, value_name = "IP|CNAME|proxy")]
    pub via: Option<String>,
    /// Spoof-only IPv6 target; rejected on folders and the profile default (D16)
    #[arg(long, value_name = "IPv6")]
    pub via6: Option<String>,
    /// Enable
    #[arg(long, conflicts_with = "disabled")]
    pub enabled: bool,
    /// Disable
    #[arg(long)]
    pub disabled: bool,
}

impl ActionFlags {
    /// No flag given at all — callers OR this into their command-specific
    /// flags (e.g. folder update's "at least one change" guard).
    pub fn is_empty(&self) -> bool {
        self.action.is_none()
            && self.via.is_none()
            && self.via6.is_none()
            && !self.enabled
            && !self.disabled
    }
}

/// What one caller's action flags mean: whether `--via6` is documented at
/// all, whether `--action` is mandatory, and what "neither `--enabled` nor
/// `--disabled`" defaults to (commands.md's per-command defaults table).
#[derive(Debug, Clone, Copy)]
pub struct Caps {
    pub via6_allowed: bool,
    pub action_required: bool,
    pub default_enabled: bool,
}

/// The validated, normalized action — ready to form-encode or plan.
#[derive(Debug, Clone)]
pub struct ActionSpec {
    pub action: Option<Action>,
    pub via: Option<String>,
    pub via6: Option<String>,
    pub enabled: Option<bool>,
    /// Private so `validate` — and this module's own tests — are the only
    /// constructors; an unvalidated spec must never reach `form_pairs`.
    #[expect(
        dead_code,
        reason = "construction-gating marker; its value is never read"
    )]
    validated: (),
}

impl ActionSpec {
    /// Fixed wire order: `do`, `status`, `via`, `via_v6` — only the `Some`
    /// fields, so an omitted flag is genuinely absent from the form, never a
    /// stale re-send (write-verification.md's merge semantics depend on it).
    pub fn form_pairs(&self) -> Vec<(&'static str, String)> {
        let mut pairs = Vec::new();
        if let Some(action) = self.action {
            pairs.push(("do", action.to_do().to_string()));
        }
        if let Some(enabled) = self.enabled {
            pairs.push(("status", u8::from(enabled).to_string()));
        }
        if let Some(via) = &self.via {
            pairs.push(("via", via.clone()));
        }
        if let Some(via6) = &self.via6 {
            pairs.push(("via_v6", via6.clone()));
        }
        pairs
    }
}

/// Validate the raw flags into an [`ActionSpec`], every rule exit 2, all
/// before any mutating request. `--action redirect`'s `--via` check is a
/// live `GET /proxies` read — it runs under `--dry-run` too (dry runs
/// validate everything, persist nothing).
pub async fn validate(
    flags: &ActionFlags,
    caps: Caps,
    client: &Client,
) -> Result<ActionSpec, Error> {
    if let Some(via) = &flags.via {
        super::validate::reject_control_chars(via, "--via")?;
    }
    if let Some(via6) = &flags.via6 {
        super::validate::reject_control_chars(via6, "--via6")?;
    }
    if flags.via6.is_some() && !caps.via6_allowed {
        return Err(Error::usage(
            "--via6 is accepted by rules and services only; folders and the profile default \
             have no documented via_v6 field (D16)",
        ));
    }

    let action = flags.action;
    if caps.action_required && action.is_none() {
        return Err(Error::usage("--action is required"));
    }
    if flags.via.is_some() && !matches!(action, Some(Action::Spoof | Action::Redirect)) {
        return Err(Error::usage(
            "--via requires --action spoof or --action redirect (the action context is needed \
             to validate it)",
        ));
    }
    if matches!(action, Some(Action::Spoof | Action::Redirect)) && flags.via.is_none() {
        return Err(Error::usage(format!(
            "--action {} requires --via",
            action.expect("matched Some above")
        )));
    }
    if flags.via6.is_some() && action != Some(Action::Spoof) {
        return Err(Error::usage("--via6 requires --action spoof"));
    }

    match action {
        Some(Action::Spoof) => {
            validate_spoof_via(flags.via.as_deref().expect("checked above"))?;
        }
        Some(Action::Redirect) => {
            validate_redirect_via(flags.via.as_deref().expect("checked above"), client).await?;
        }
        _ => {}
    }

    let enabled = if flags.enabled {
        Some(true)
    } else if flags.disabled {
        Some(false)
    } else {
        caps.default_enabled.then_some(true)
    };

    Ok(ActionSpec {
        action,
        via: flags.via.clone(),
        via6: flags.via6.clone(),
        enabled,
        validated: (),
    })
}

fn validate_spoof_via(via: &str) -> Result<(), Error> {
    if via.parse::<std::net::IpAddr>().is_ok() {
        return Ok(());
    }
    // Loose on purpose: a real hostname/CNAME check would need DNS syntax
    // rules this CLI has no business enforcing; just reject the obviously
    // wrong shapes.
    if via.is_empty() || via.contains(' ') {
        return Err(Error::usage(format!(
            "--via {via:?} is neither a valid IP nor a plausible hostname"
        )));
    }
    Ok(())
}

/// `--action redirect`'s `--via` must be a known proxy PK (`GET /proxies`,
/// exact and case-sensitive — PKs are upper). On failure, hint the nearest
/// matches: proxies whose PK, city, or country contains the input
/// case-insensitively, up to 3 (`LON` isn't a proxy; `LHR` is).
async fn validate_redirect_via(via: &str, client: &Client) -> Result<(), Error> {
    let proxies: Vec<ApiProxy> = client.get("/proxies", "proxy").await?.keyed_as("proxies")?;
    if proxies.iter().any(|proxy| proxy.pk == via) {
        return Ok(());
    }

    let needle = via.to_ascii_lowercase();
    let hints: Vec<String> = proxies
        .iter()
        .filter(|proxy| {
            proxy.pk.to_ascii_lowercase().contains(&needle)
                || proxy.city.to_ascii_lowercase().contains(&needle)
                || proxy.country_name.to_ascii_lowercase().contains(&needle)
        })
        .take(3)
        .map(|proxy| format!("{} ({}, {})", proxy.pk, proxy.city, proxy.country_name))
        .collect();

    let mut error = Error::usage(format!(
        "--via {via:?} is not a known proxy PK; see `cdctl proxy list`"
    ));
    if !hints.is_empty() {
        error = error.with_hint(format!("nearest matches: {}", hints.join(", ")));
    }
    Err(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::client::{ClientConfig, RetryPolicy};
    use crate::error::Exit;
    use secrecy::SecretString;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn no_flags() -> ActionFlags {
        ActionFlags {
            action: None,
            via: None,
            via6: None,
            enabled: false,
            disabled: false,
        }
    }

    fn folder_caps() -> Caps {
        Caps {
            via6_allowed: false,
            action_required: false,
            default_enabled: false,
        }
    }

    fn dead_client() -> Client {
        // Never actually called in tests that don't reach the redirect
        // branch; an unreachable address makes that a hard failure instead
        // of a silent pass if a validation path regresses to call it.
        Client::new(ClientConfig {
            base_url: reqwest::Url::parse("http://127.0.0.1:1").expect("valid URL"),
            token: None,
            timeout: std::time::Duration::from_millis(50),
            connect_timeout: std::time::Duration::from_millis(50),
            retry: RetryPolicy {
                enabled: false,
                ..RetryPolicy::default()
            },
            debug: false,
            allow_unpinned_origin: true,
        })
        .expect("client builds")
    }

    #[tokio::test]
    async fn control_characters_in_via_are_rejected() {
        let flags = ActionFlags {
            via: Some("1.2.3.4\u{7}".into()),
            ..no_flags()
        };
        let error = validate(&flags, folder_caps(), &dead_client())
            .await
            .expect_err("rejected");
        assert_eq!(error.exit(), Exit::Usage);
    }

    #[tokio::test]
    async fn via6_is_rejected_when_the_caller_does_not_allow_it() {
        let flags = ActionFlags {
            action: Some(Action::Spoof),
            via: Some("192.0.2.1".into()),
            via6: Some("2001:db8::1".into()),
            ..no_flags()
        };
        let error = validate(&flags, folder_caps(), &dead_client())
            .await
            .expect_err("via6 unsupported on folders");
        assert_eq!(error.exit(), Exit::Usage);
        assert!(error.message.contains("via_v6"));
    }

    #[tokio::test]
    async fn a_required_action_must_be_given() {
        let error = validate(
            &no_flags(),
            Caps {
                action_required: true,
                ..folder_caps()
            },
            &dead_client(),
        )
        .await
        .expect_err("missing --action");
        assert_eq!(error.exit(), Exit::Usage);
    }

    #[tokio::test]
    async fn via_without_an_action_is_rejected_even_on_updates() {
        let flags = ActionFlags {
            via: Some("192.0.2.1".into()),
            ..no_flags()
        };
        let error = validate(&flags, folder_caps(), &dead_client())
            .await
            .expect_err("via needs action context");
        assert_eq!(error.exit(), Exit::Usage);
    }

    #[tokio::test]
    async fn spoof_or_redirect_without_via_is_rejected() {
        for action in [Action::Spoof, Action::Redirect] {
            let flags = ActionFlags {
                action: Some(action),
                ..no_flags()
            };
            let error = validate(&flags, folder_caps(), &dead_client())
                .await
                .expect_err("via required");
            assert_eq!(error.exit(), Exit::Usage);
        }
    }

    #[tokio::test]
    async fn via6_requires_spoof_specifically() {
        let flags = ActionFlags {
            action: Some(Action::Redirect),
            via: Some("LHR".into()),
            via6: Some("2001:db8::1".into()),
            ..no_flags()
        };
        let error = validate(
            &flags,
            Caps {
                via6_allowed: true,
                ..folder_caps()
            },
            &dead_client(),
        )
        .await
        .expect_err("via6 only with spoof");
        assert_eq!(error.exit(), Exit::Usage);
    }

    #[tokio::test]
    async fn spoof_via_must_be_an_ip_or_a_plausible_hostname() {
        let bad = ActionFlags {
            action: Some(Action::Spoof),
            via: Some("not a host".into()),
            ..no_flags()
        };
        let error = validate(&bad, folder_caps(), &dead_client())
            .await
            .expect_err("not IP-like or hostname-like");
        assert_eq!(error.exit(), Exit::Usage);

        for via in ["192.0.2.53", "2001:db8::1", "spoof.example.com"] {
            let flags = ActionFlags {
                action: Some(Action::Spoof),
                via: Some(via.to_owned()),
                ..no_flags()
            };
            let spec = validate(&flags, folder_caps(), &dead_client())
                .await
                .expect("legal spoof target");
            assert_eq!(spec.via.as_deref(), Some(via));
        }
    }

    async fn proxies_client() -> (MockServer, Client) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/proxies"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(
                    std::fs::read_to_string(
                        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                            .join("tests/fixtures/api")
                            .join("proxies.json"),
                    )
                    .expect("fixture exists"),
                    "application/json",
                ),
            )
            .mount(&server)
            .await;
        let client = Client::new(ClientConfig {
            base_url: reqwest::Url::parse(&server.uri()).expect("valid URL"),
            token: Some(SecretString::from("test-token")),
            timeout: std::time::Duration::from_secs(2),
            connect_timeout: std::time::Duration::from_secs(2),
            retry: RetryPolicy {
                enabled: false,
                ..RetryPolicy::default()
            },
            debug: false,
            allow_unpinned_origin: true,
        })
        .expect("client builds");
        (server, client)
    }

    #[tokio::test]
    async fn redirect_accepts_a_known_proxy_pk() {
        let (_server, client) = proxies_client().await;
        let flags = ActionFlags {
            action: Some(Action::Redirect),
            via: Some("LHR".into()),
            ..no_flags()
        };
        let spec = validate(&flags, folder_caps(), &client)
            .await
            .expect("LHR is a known proxy");
        assert_eq!(spec.via.as_deref(), Some("LHR"));
    }

    #[tokio::test]
    async fn redirect_hints_the_nearest_matches_on_an_unknown_pk() {
        let (_server, client) = proxies_client().await;
        let flags = ActionFlags {
            action: Some(Action::Redirect),
            via: Some("LON".into()),
            ..no_flags()
        };
        let error = validate(&flags, folder_caps(), &client)
            .await
            .expect_err("LON is not a proxy PK");
        assert_eq!(error.exit(), Exit::Usage);
        let hint = error.hint.expect("hint present");
        assert!(hint.contains("LHR"), "got: {hint}");
    }

    #[tokio::test]
    async fn enabled_defaults_per_caller_caps() {
        let create_like = validate(
            &no_flags(),
            Caps {
                default_enabled: true,
                ..folder_caps()
            },
            &dead_client(),
        )
        .await
        .expect("no flags still validates");
        assert_eq!(create_like.enabled, Some(true));

        let update_like = validate(&no_flags(), folder_caps(), &dead_client())
            .await
            .expect("no flags still validates");
        assert_eq!(
            update_like.enabled, None,
            "omitted on updates: never re-enables"
        );

        let disabled = validate(
            &ActionFlags {
                disabled: true,
                ..no_flags()
            },
            folder_caps(),
            &dead_client(),
        )
        .await
        .expect("explicit --disabled");
        assert_eq!(disabled.enabled, Some(false));
    }

    #[test]
    fn form_pairs_freeze_the_wire_order() {
        let spec = ActionSpec {
            action: Some(Action::Spoof),
            via: Some("192.0.2.1".into()),
            via6: Some("2001:db8::1".into()),
            enabled: Some(true),
            validated: (),
        };
        assert_eq!(
            spec.form_pairs(),
            vec![
                ("do", "2".to_owned()),
                ("status", "1".to_owned()),
                ("via", "192.0.2.1".to_owned()),
                ("via_v6", "2001:db8::1".to_owned()),
            ]
        );
    }

    #[test]
    fn form_pairs_omits_absent_fields_entirely() {
        let spec = ActionSpec {
            action: None,
            via: None,
            via6: None,
            enabled: None,
            validated: (),
        };
        assert!(spec.form_pairs().is_empty());
    }

    #[test]
    fn is_empty_covers_every_flag() {
        assert!(no_flags().is_empty());
        assert!(
            !ActionFlags {
                action: Some(Action::Block),
                ..no_flags()
            }
            .is_empty()
        );
        assert!(
            !ActionFlags {
                via: Some("192.0.2.1".into()),
                ..no_flags()
            }
            .is_empty()
        );
        assert!(
            !ActionFlags {
                via6: Some("2001:db8::1".into()),
                ..no_flags()
            }
            .is_empty()
        );
        assert!(
            !ActionFlags {
                enabled: true,
                ..no_flags()
            }
            .is_empty()
        );
        assert!(
            !ActionFlags {
                disabled: true,
                ..no_flags()
            }
            .is_empty()
        );
    }
}
