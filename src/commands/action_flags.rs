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
    /// Spoof-only IPv6 target (the API documents no via6 field on folders
    /// or the profile default)
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
    /// constructors; an unvalidated spec must never reach `form_pairs`. The
    /// other fields are `pub` regardless: this marker gates *construction*,
    /// not mutation — a caller can still read and clone them freely. Treat a
    /// validated `ActionSpec` as frozen once built; if a field would need to
    /// change, re-validate a new one rather than mutating this one in place,
    /// or the marker's guarantee (every live `ActionSpec` passed validation)
    /// stops holding.
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

    /// `--action`/`--via`/`--via6`, rendered from a validated spec for a
    /// `retry_argv` (`rule create`'s builder). Callers append their own
    /// `enabled`/`--folder`/targets tail — the caps that validated `self`
    /// already decide what "no `--action` at all" or "omit `enabled`" means
    /// per verb, so this stays a pure spelling of the three flags this type
    /// owns.
    pub fn retry_flags(&self) -> Vec<String> {
        action_via_flags(self.action, self.via.as_deref(), self.via6.as_deref())
    }

    /// The one part of validation that needs the API: `--action redirect`'s
    /// `--via` must name a known proxy. A no-op for every other action, so a
    /// caller can call it unconditionally right after a client exists,
    /// without re-checking `self.action` itself.
    pub async fn check_redirect_via(&self, client: &Client) -> Result<(), Error> {
        if self.action == Some(Action::Redirect) {
            validate_redirect_via(
                self.via
                    .as_deref()
                    .expect("redirect requires --via (checked by validate)"),
                client,
            )
            .await?;
        }
        Ok(())
    }
}

/// The single spelling of `--action`/`--via`/`--via6`, beside the clap
/// definitions that own it — [`ActionSpec::retry_flags`] and `rule
/// update`'s `retry_argv` (whose patch is a [`RuleUpdateChanges`], not an
/// `ActionSpec`, but the same three fields) both render through this.
///
/// [`RuleUpdateChanges`]: super::plan::RuleUpdateChanges
pub(crate) fn action_via_flags(
    action: Option<Action>,
    via: Option<&str>,
    via6: Option<&str>,
) -> Vec<String> {
    let mut argv = Vec::new();
    if let Some(action) = action {
        argv.push("--action".to_owned());
        argv.push(action.name().to_owned());
    }
    if let Some(via) = via {
        argv.push("--via".to_owned());
        argv.push(via.to_owned());
    }
    if let Some(via6) = via6 {
        argv.push("--via6".to_owned());
        argv.push(via6.to_owned());
    }
    argv
}

/// Validate the raw flags into an [`ActionSpec`] — every rule exit 2, purely
/// local (no client), so callers can run it before resolving auth: a
/// malformed `--action`/`--via`/`--via6` combination is a usage mistake
/// whether or not a token is configured, and must not be masked by
/// `auth.missing_token`. `--action redirect`'s `--via` still needs a live
/// `GET /proxies` read to confirm the PK exists — that's
/// [`ActionSpec::check_redirect_via`], run once a client exists (still
/// before any mutating request; dry runs validate everything, persist
/// nothing).
pub fn validate(flags: &ActionFlags, caps: Caps) -> Result<ActionSpec, Error> {
    if let Some(via) = &flags.via {
        super::validate::reject_control_chars(via, "--via")?;
    }
    if let Some(via6) = &flags.via6 {
        super::validate::reject_control_chars(via6, "--via6")?;
    }
    if flags.via6.is_some() && !caps.via6_allowed {
        return Err(Error::usage(
            "--via6 is accepted by rules and services only; folders and the profile default \
             have no documented via_v6 field",
        ));
    }
    // `--via6=` (the attached empty value) is the one spelling that requests
    // a via6 clear. The API has no clear operation while the spoof action
    // persists (write-verification.md) — reject locally rather than forward
    // it upstream, where it 400s anyway (`err_via6_clear.json`).
    if caps.via6_allowed && flags.via6.as_deref() == Some("") {
        return Err(Error::usage(
            "--via6= (empty) is not supported: the API cannot clear via_v6 while the spoof \
             action persists",
        )
        .with_hint(
            "there is no update that clears via6; delete the rule and recreate it with the \
             desired via6 (or omit --via6 to keep the current value)",
        ));
    }
    // A nonempty `--via6` must be a bare IPv6 literal (commands.md#the-shared-action-flags:
    // "IPv6, spoof only") — `Ipv6Addr::from_str` also rejects the bracketed
    // `[2001:db8::1]` form, which is fine: only the bare literal is accepted.
    if let Some(via6) = &flags.via6 {
        if via6.parse::<std::net::Ipv6Addr>().is_err() {
            return Err(Error::usage(format!(
                "--via6 {via6:?} is not an IPv6 literal"
            )));
        }
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

    if let Some(Action::Spoof) = action {
        validate_spoof_via(flags.via.as_deref().expect("checked above"))?;
    }
    // Redirect's `--via` is checked against the live proxy list separately
    // (`check_redirect_via`, after a client exists) — this function stays
    // client-free so it can run before auth resolves.

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
pub(crate) mod tests {
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

    /// Points at an unreachable address, so `check_redirect_via_is_a_noop_for_non_redirect_actions`
    /// below fails loudly if a non-redirect action ever makes a network call.
    pub(crate) fn dead_client() -> Client {
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
            quiet: false,
            allow_unpinned_origin: true,
        })
        .expect("client builds")
    }

    #[test]
    fn control_characters_in_via_are_rejected() {
        let flags = ActionFlags {
            via: Some("1.2.3.4\u{7}".into()),
            ..no_flags()
        };
        let error = validate(&flags, folder_caps()).expect_err("rejected");
        assert_eq!(error.exit(), Exit::Usage);
    }

    #[test]
    fn via6_is_rejected_when_the_caller_does_not_allow_it() {
        let flags = ActionFlags {
            action: Some(Action::Spoof),
            via: Some("192.0.2.1".into()),
            via6: Some("2001:db8::1".into()),
            ..no_flags()
        };
        let error = validate(&flags, folder_caps()).expect_err("via6 unsupported on folders");
        assert_eq!(error.exit(), Exit::Usage);
        assert!(error.message.contains("via_v6"));
    }

    #[test]
    fn empty_via6_is_rejected_as_an_unsupported_clear() {
        let flags = ActionFlags {
            via6: Some(String::new()),
            ..no_flags()
        };
        let error = validate(
            &flags,
            Caps {
                via6_allowed: true,
                ..folder_caps()
            },
        )
        .expect_err("empty --via6 is an unsupported clear");
        assert_eq!(error.exit(), Exit::Usage);
        assert!(error.hint.expect("hint present").contains("delete"));
    }

    #[test]
    fn folders_reject_via6_before_the_empty_string_check() {
        let flags = ActionFlags {
            via6: Some(String::new()),
            ..no_flags()
        };
        let error =
            validate(&flags, folder_caps()).expect_err("folders never accept via6, empty or not");
        assert!(error.message.contains("via_v6"));
    }

    #[test]
    fn a_required_action_must_be_given() {
        let error = validate(
            &no_flags(),
            Caps {
                action_required: true,
                ..folder_caps()
            },
        )
        .expect_err("missing --action");
        assert_eq!(error.exit(), Exit::Usage);
    }

    #[test]
    fn via_without_an_action_is_rejected_even_on_updates() {
        let flags = ActionFlags {
            via: Some("192.0.2.1".into()),
            ..no_flags()
        };
        let error = validate(&flags, folder_caps()).expect_err("via needs action context");
        assert_eq!(error.exit(), Exit::Usage);
    }

    #[test]
    fn spoof_or_redirect_without_via_is_rejected() {
        for action in [Action::Spoof, Action::Redirect] {
            let flags = ActionFlags {
                action: Some(action),
                ..no_flags()
            };
            let error = validate(&flags, folder_caps()).expect_err("via required");
            assert_eq!(error.exit(), Exit::Usage);
            assert!(
                error.message.contains("requires --via"),
                "got: {}",
                error.message
            );
        }
    }

    #[test]
    fn via6_requires_spoof_specifically() {
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
        )
        .expect_err("via6 only with spoof");
        assert_eq!(error.exit(), Exit::Usage);
    }

    #[test]
    fn via6_rejects_an_ipv4_literal() {
        let flags = ActionFlags {
            action: Some(Action::Spoof),
            via: Some("192.0.2.1".into()),
            via6: Some("192.0.2.1".into()),
            ..no_flags()
        };
        let error = validate(
            &flags,
            Caps {
                via6_allowed: true,
                ..folder_caps()
            },
        )
        .expect_err("an IPv4 literal is not an IPv6 literal");
        assert_eq!(error.exit(), Exit::Usage);
        assert!(
            error.message.contains("192.0.2.1"),
            "got: {}",
            error.message
        );
    }

    #[test]
    fn via6_rejects_a_hostname() {
        let flags = ActionFlags {
            action: Some(Action::Spoof),
            via: Some("192.0.2.1".into()),
            via6: Some("host.tld".into()),
            ..no_flags()
        };
        let error = validate(
            &flags,
            Caps {
                via6_allowed: true,
                ..folder_caps()
            },
        )
        .expect_err("a hostname is not an IPv6 literal");
        assert_eq!(error.exit(), Exit::Usage);
        assert!(error.message.contains("host.tld"), "got: {}", error.message);
    }

    #[test]
    fn via6_accepts_a_bare_ipv6_literal() {
        let flags = ActionFlags {
            action: Some(Action::Spoof),
            via: Some("192.0.2.1".into()),
            via6: Some("2001:db8::1".into()),
            ..no_flags()
        };
        let spec = validate(
            &flags,
            Caps {
                via6_allowed: true,
                ..folder_caps()
            },
        )
        .expect("a bare IPv6 literal is legal");
        assert_eq!(spec.via6.as_deref(), Some("2001:db8::1"));
    }

    #[test]
    fn via6_rejects_the_bracketed_form() {
        let flags = ActionFlags {
            action: Some(Action::Spoof),
            via: Some("192.0.2.1".into()),
            via6: Some("[2001:db8::1]".into()),
            ..no_flags()
        };
        let error = validate(
            &flags,
            Caps {
                via6_allowed: true,
                ..folder_caps()
            },
        )
        .expect_err("the bracketed form is not accepted; only the bare literal is");
        assert_eq!(error.exit(), Exit::Usage);
    }

    #[test]
    fn spoof_via_must_be_an_ip_or_a_plausible_hostname() {
        let bad = ActionFlags {
            action: Some(Action::Spoof),
            via: Some("not a host".into()),
            ..no_flags()
        };
        let error = validate(&bad, folder_caps()).expect_err("not IP-like or hostname-like");
        assert_eq!(error.exit(), Exit::Usage);

        for via in ["192.0.2.53", "2001:db8::1", "spoof.example.com"] {
            let flags = ActionFlags {
                action: Some(Action::Spoof),
                via: Some(via.to_owned()),
                ..no_flags()
            };
            let spec = validate(&flags, folder_caps()).expect("legal spoof target");
            assert_eq!(spec.via.as_deref(), Some(via));
        }
    }

    #[tokio::test]
    async fn check_redirect_via_is_a_noop_for_non_redirect_actions() {
        let flags = ActionFlags {
            action: Some(Action::Spoof),
            via: Some("192.0.2.1".into()),
            ..no_flags()
        };
        let spec = validate(&flags, folder_caps()).expect("valid spoof spec");
        // `dead_client` points at an unreachable address; a real request
        // here would fail (connection refused, or the 50ms cap) and trip
        // the expect.
        spec.check_redirect_via(&dead_client())
            .await
            .expect("non-redirect actions never touch the client");
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
            quiet: false,
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
        let spec = validate(&flags, folder_caps()).expect("passes local validation");
        spec.check_redirect_via(&client)
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
        let spec = validate(&flags, folder_caps()).expect("passes local validation");
        let error = spec
            .check_redirect_via(&client)
            .await
            .expect_err("LON is not a proxy PK");
        assert_eq!(error.exit(), Exit::Usage);
        let hint = error.hint.expect("hint present");
        assert!(hint.contains("LHR"), "got: {hint}");
    }

    #[test]
    fn enabled_defaults_per_caller_caps() {
        let create_like = validate(
            &no_flags(),
            Caps {
                default_enabled: true,
                ..folder_caps()
            },
        )
        .expect("no flags still validates");
        assert_eq!(create_like.enabled, Some(true));

        let update_like = validate(&no_flags(), folder_caps()).expect("no flags still validates");
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
        )
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
