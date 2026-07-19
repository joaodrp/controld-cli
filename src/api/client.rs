//! The HTTP client and the retry contract (D12).
//!
//! reqwest's built-in retries are disabled (`retry::never()`) — they replay
//! immediately, ignore `Retry-After`, and are not GET-only. The loop below is
//! cdctl's: GETs retry with full-jitter backoff under 3-attempt/30-second
//! caps; **writes are sent exactly once, always**.

use std::io::IsTerminal;
use std::time::{Duration, Instant, SystemTime};

use bytes::Bytes;
use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::{Method, StatusCode, Url, redirect};
use secrecy::{ExposeSecret, SecretString};

use super::envelope::Envelope;

use crate::error::{
    Error, Exit, classify_response, classify_transport, classify_unconfirmed_success,
    classify_unparseable, unparseable_body_note,
};

pub const DEFAULT_BASE_URL: &str = "https://api.controld.com";
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
pub const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// A request still on the wire after this long draws a one-line stderr
/// notice (TTY-only, dropped by `--quiet`).
const SLOW_REQUEST_NOTICE_AFTER: Duration = Duration::from_secs(2);
const MAX_REDIRECTS: usize = 5;

/// The GET-only retry contract. Fields are configurable so tests can shrink
/// the delays; the defaults are the D12 caps.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub enabled: bool,
    /// Total attempts, the original request included.
    pub max_attempts: u32,
    pub max_elapsed: Duration,
    pub base_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            enabled: true,
            max_attempts: 3,
            max_elapsed: Duration::from_secs(30),
            base_delay: Duration::from_millis(500),
        }
    }
}

/// A request body in one of the two D9b encodings — a form body, pre-encoded
/// once by [`encode_form`] wherever the pairs are first known (literal keys,
/// percent-encoded values), or bytes sent verbatim as JSON. Nothing is ever
/// sniffed (D16).
#[derive(Debug)]
pub enum RawBody {
    Form(String),
    Json(Vec<u8>),
}

/// The transport-level response parts, before any interpretation.
#[derive(Debug)]
struct WireResponse {
    status: StatusCode,
    retry_after: RetryAfter,
    body: Bytes,
}

#[derive(Debug)]
pub struct ClientConfig {
    /// Invariant: the path always ends in `/` — [`base_url_from_raw`] (the
    /// sole production source of this field) enforces it once, so
    /// [`join_pinned_to_origin`] can join a single-leading-slash path
    /// directly, with no per-call check.
    pub base_url: Url,
    pub token: Option<SecretString>,
    pub timeout: Duration,
    pub connect_timeout: Duration,
    pub retry: RetryPolicy,
    pub debug: bool,
    pub quiet: bool,
    /// D9 escape hatch: lets a token accompany a non-default origin. Set from
    /// `CONTROLD_UNSAFE_BASE_URL=1`, or explicitly by tests aiming at wiremock.
    pub allow_unpinned_origin: bool,
}

impl ClientConfig {
    /// Base URL and origin policy from the environment: `CONTROLD_API_URL`
    /// overrides the origin (it exists for tests); [`Client::new`] enforces D9.
    pub fn from_env(
        token: Option<SecretString>,
        timeout_secs: Option<u64>,
        no_retry: bool,
        debug: bool,
        quiet: bool,
    ) -> Result<Self, Error> {
        let base_url = base_url_from_raw(crate::config::env_var("CONTROLD_API_URL")?)?;

        Ok(Self {
            base_url,
            token,
            timeout: timeout_secs.map_or(DEFAULT_TIMEOUT, Duration::from_secs),
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            retry: RetryPolicy {
                enabled: !no_retry,
                ..RetryPolicy::default()
            },
            debug,
            quiet,
            allow_unpinned_origin: crate::config::env_var("CONTROLD_UNSAFE_BASE_URL")?
                .is_some_and(|v| v == "1"),
        })
    }
}

/// `raw` (`CONTROLD_API_URL`, already [`env_var`](crate::config::env_var)-policy)
/// parsed, or the default base when absent — either way, normalized so the
/// path ends in `/` before it becomes [`ClientConfig::base_url`]. Every
/// command path is absolute (`/profiles`, ...), and WHATWG's plain
/// `Url::join` treats a single leading `/` as replacing the base's whole
/// path — without the trailing slash a path-routing gateway's prefix (e.g.
/// `https://gw.example/controld`) would be dropped, not extended as a
/// directory ([`join_pinned_to_origin`]). Normalizing here, once, at the
/// base URL's sole production constructor, means every later join can rely on
/// the invariant instead of re-deriving it per call. Touches only the path,
/// never the origin, so this cannot weaken D9's origin pin; [`DEFAULT_BASE_URL`]
/// already parses with path `/`, so the default (and every existing e2e URL
/// built from it) is unchanged by this normalization.
fn base_url_from_raw(raw: Option<String>) -> Result<Url, Error> {
    let mut url = match raw {
        Some(raw) => {
            let url = Url::parse(&raw)
                .map_err(|e| Error::usage(format!("CONTROLD_API_URL is not a valid URL: {e}")))?;
            validate_base_url_scheme(&url, &raw)?;
            url
        }
        None => Url::parse(DEFAULT_BASE_URL).expect("the default base URL is valid"),
    };
    if !url.path().ends_with('/') {
        let with_trailing_slash = format!("{}/", url.path());
        url.set_path(&with_trailing_slash);
    }
    Ok(url)
}

/// A URL can parse and still be unusable as a request origin — `mailto:x` or
/// a typo'd `foo://host/` both parse fine but carry no scheme this client
/// could ever send a request over, and `Url::join`/`origin()` degrade
/// silently rather than erroring on them (a cannot-be-a-base URL has no
/// `host_str` at all). Left unchecked, every later request would fail, but
/// misattributed to the request path rather than to `CONTROLD_API_URL`,
/// which is the actual problem. Caught once, here, at construction.
fn validate_base_url_scheme(url: &Url, raw: &str) -> Result<(), Error> {
    let scheme_ok = matches!(url.scheme(), "http" | "https");
    if scheme_ok && url.host_str().is_some() {
        return Ok(());
    }
    Err(Error::usage(format!(
        "CONTROLD_API_URL must be an http or https URL with a host, got {raw:?}"
    )))
}

// Debug is safe: secrecy renders the token as REDACTED.
#[derive(Debug)]
pub struct Client {
    http: reqwest::Client,
    base_url: Url,
    token: Option<SecretString>,
    retry: RetryPolicy,
    debug: bool,
    quiet: bool,
}

impl Client {
    /// The D9 origin check lives here, at the choke point — the token cannot
    /// leave the pinned origin no matter how the config was assembled.
    pub fn new(config: ClientConfig) -> Result<Self, Error> {
        let is_default_origin = config.base_url.origin()
            == Url::parse(DEFAULT_BASE_URL)
                .expect("the default base URL is valid")
                .origin();
        if config.token.is_some() && !is_default_origin && !config.allow_unpinned_origin {
            return Err(Error::new(
                "usage.unsafe_base_url",
                format!(
                    "refusing to send the API token to {}: it leaves the pinned origin ({DEFAULT_BASE_URL})",
                    config.base_url
                ),
                Exit::Usage,
            )
            .with_hint("Set CONTROLD_UNSAFE_BASE_URL=1 if this endpoint really should receive the token."));
        }

        let http = reqwest::Client::builder()
            // D13: ours or none (module doc).
            .retry(reqwest::retry::never())
            .timeout(config.timeout)
            .connect_timeout(config.connect_timeout)
            .redirect(same_origin_redirects())
            .user_agent(concat!("cdctl/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error::generic(format!("could not build the HTTP client: {e}")))?;
        Ok(Self {
            http,
            base_url: config.base_url,
            token: config.token,
            retry: config.retry,
            debug: config.debug,
            quiet: config.quiet,
        })
    }

    /// GET with the D12 retry loop: full-jitter backoff, `Retry-After`
    /// honored, capped attempts and elapsed time, every retry logged (a
    /// silent backoff is indistinguishable from a hang).
    pub async fn get(&self, path: &str, resource: &'static str) -> Result<Envelope, Error> {
        self.get_with(path, |wire| interpret_response(wire, resource))
            .await
    }

    /// The retry loop itself, shared by every GET interpretation.
    async fn get_with<T>(
        &self,
        path: &str,
        interpret: impl Fn(&WireResponse) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let started = Instant::now();
        let mut attempt: u32 = 1;
        loop {
            let attempt_result = self
                .send(Method::GET, path, None)
                .await
                .and_then(|wire| interpret(&wire));
            let error = match attempt_result {
                Ok(value) => return Ok(value),
                Err(error) => error,
            };
            if !self.retry.enabled || !error.retryable() || attempt >= self.retry.max_attempts {
                return Err(error);
            }
            let delay = self.backoff_delay(attempt, error.retry_after);
            if started.elapsed() + delay > self.retry.max_elapsed {
                return Err(error);
            }
            attempt += 1;
            crate::output::info(
                self.quiet,
                format_args!(
                    "GET {path} failed ({code}); retrying in {secs:.1}s (attempt {attempt}/{max})",
                    code = error.code,
                    secs = delay.as_secs_f64(),
                    max = self.retry.max_attempts,
                ),
            );
            tokio::time::sleep(delay).await;
        }
    }

    /// A mutation: sent exactly once, whatever happens. A retryable failure
    /// keeps exit 8 but the hint says the write may have landed — re-fetching
    /// state is the caller's (or the agent's) job, never a blind replay.
    pub async fn write(
        &self,
        method: Method,
        path: &str,
        form: &[(&str, String)],
        resource: &'static str,
    ) -> Result<Envelope, Error> {
        // Encoded directly from the borrowed pairs — no intermediate owned
        // copy that `encode_form` would only re-walk a second time.
        let body = (!form.is_empty()).then(|| RawBody::Form(encode_form(form)));
        self.write_with(method, path, body, |wire| {
            interpret_response(wire, resource)
        })
        .await
    }

    /// Raw GET for `cdctl api` (D9): the body bytes verbatim, with the D12
    /// retry loop. Errors still classify through the standard rules.
    pub async fn get_raw(&self, path: &str, resource: &'static str) -> Result<Bytes, Error> {
        self.get_with(path, |wire| interpret_raw(wire, resource))
            .await
    }

    /// Raw mutation for `cdctl api` (D9): sent exactly once, body bytes back
    /// verbatim. Same never-replay contract as [`Client::write`].
    pub async fn write_raw(
        &self,
        method: Method,
        path: &str,
        body: Option<RawBody>,
        resource: &'static str,
    ) -> Result<Bytes, Error> {
        self.write_with(method, path, body, |wire| interpret_raw(wire, resource))
            .await
    }

    /// The mutation pipeline shared by [`Client::write`] and
    /// [`Client::write_raw`] — the exactly-once contract lives here, once.
    async fn write_with<T>(
        &self,
        method: Method,
        path: &str,
        body: Option<RawBody>,
        interpret: impl Fn(&WireResponse) -> Result<T, Error>,
    ) -> Result<T, Error> {
        debug_assert_ne!(
            method,
            Method::GET,
            "mutations only; GETs retry via get_with()"
        );
        self.send(method, path, body)
            .await
            .and_then(|wire| interpret(&wire))
            .map_err(warn_write_may_have_landed)
    }

    /// One request over the wire: join the path, attach the bearer token and
    /// body, send, collect the response parts. No interpretation.
    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<RawBody>,
    ) -> Result<WireResponse, Error> {
        let url =
            join_pinned_to_origin(&self.base_url, path).map_err(|rejection| match rejection {
                JoinRejection::Malformed(error) => error,
                JoinRejection::OffOrigin => Error::usage(format!(
                    "request path {path:?} resolves outside the API origin ({origin})",
                    origin = self.base_url.origin().ascii_serialization()
                )),
            })?;

        if self.debug {
            // The Authorization header is never traced (token redaction, D6).
            eprintln!("debug: > {method} {url}");
        }
        let mut request = self.http.request(method, url);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token.expose_secret());
        }
        match body {
            Some(RawBody::Form(encoded)) => {
                request = request
                    .header(
                        reqwest::header::CONTENT_TYPE,
                        "application/x-www-form-urlencoded",
                    )
                    .body(encoded);
            }
            Some(RawBody::Json(bytes)) => {
                request = request
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .body(bytes);
            }
            None => {}
        }

        // Responsiveness over silence: a slow link can sit mute for the full
        // timeout (D12), which reads as a hang. One advisory line after 2s,
        // at a TTY only (a script's stderr is no place for liveness chatter)
        // and never under --quiet. Per attempt, and only while genuinely on
        // the wire — backoff sleeps have the retry notice instead. Covers
        // connect through response headers; a slow body read stays mute but
        // bounded by the request timeout.
        let send = request.send();
        tokio::pin!(send);
        let response = match tokio::time::timeout(SLOW_REQUEST_NOTICE_AFTER, &mut send).await {
            Ok(result) => result,
            Err(_still_in_flight) => {
                if !self.quiet && std::io::stderr().is_terminal() {
                    crate::output::info(
                        self.quiet,
                        format_args!(
                            "waiting on {} ...",
                            self.base_url.host_str().unwrap_or("the API")
                        ),
                    );
                }
                send.await
            }
        }
        .map_err(|e| classify_transport(&e))?;
        let status = response.status();
        let retry_after = parse_retry_after(response.headers());
        if self.debug {
            let pop = trace_header(response.headers(), "x-controld-pop");
            let srv = trace_header(response.headers(), "x-controld-srv");
            eprintln!("debug: < {status} (pop: {pop}, srv: {srv})");
            if retry_after == RetryAfter::Unparseable {
                eprintln!("debug: Retry-After header was not parseable; treated as absent");
            }
        }
        let body = response.bytes().await.map_err(|e| classify_transport(&e))?;

        Ok(WireResponse {
            status,
            retry_after,
            body,
        })
    }

    fn backoff_delay(&self, attempt: u32, retry_after: Option<u64>) -> Duration {
        if let Some(secs) = retry_after {
            return Duration::from_secs(secs);
        }
        // Full jitter: uniform over (0, base * 2^attempt].
        let cap = self
            .retry
            .base_delay
            .saturating_mul(2u32.saturating_pow(attempt));
        let cap_ms = u64::try_from(cap.as_millis()).unwrap_or(u64::MAX);
        Duration::from_millis(fastrand::u64(1..=cap_ms.max(1)))
    }
}

/// Why [`join_pinned_to_origin`] refused a path. Callers word the
/// off-origin message for their own audience (the choke point names the
/// real origin; `validate_path` must not leak its probe base).
#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "rejection paths are cold; matches the crate-wide result_large_err allowance"
)]
pub(crate) enum JoinRejection {
    /// The path itself did not parse; carries the precise diagnosis.
    Malformed(Error),
    OffOrigin,
}

/// Join `path` onto `base`, refusing any result that leaves `base`'s origin
/// (D9), and preserving any path prefix `base` carries (a path-routing
/// gateway, e.g. `https://gw.example/controld`) — every command's path is
/// absolute (`/profiles`, ...), and WHATWG's plain `base.join` treats a
/// single leading `/` as replacing the base's whole path, dropping the
/// prefix. A single-leading-slash `path` is therefore joined as relative
/// against `base` instead, so the prefix is kept as a directory rather than a
/// replaced last segment; the query string travels with it since the
/// relative reference carries it. This relies on `base`'s path already
/// ending in `/` — [`base_url_from_raw`]'s invariant, guaranteed once at
/// construction, so this join needs no per-call clone or check to uphold it.
/// Every other shape (relative paths, `//host/x`, full URLs, backslash forms)
/// keeps the plain `base.join` behavior unchanged — those are exactly the
/// shapes D9 relies on to detect an origin escape (a normalized-to-two-slashes
/// prefix must still resolve as a network-path reference and replace the
/// host). Either way the joined result is checked against `base`'s origin,
/// never the input's shape. `cdctl api` runs the same algorithm pre-auth,
/// against a sentinel base that is itself already normalized (a bare
/// `Url::parse` with no path).
pub(crate) fn join_pinned_to_origin(base: &Url, path: &str) -> Result<Url, JoinRejection> {
    // Two leading slash-equivalents (`//`, `/\`, `\\`) are a network-path
    // reference even after stripping one `/` — those must keep falling
    // through to the plain join below so the origin check still catches them.
    let is_single_leading_slash =
        path.starts_with('/') && !matches!(path.as_bytes().get(1), Some(b'/' | b'\\'));

    let url = if is_single_leading_slash {
        // `./` is RFC 3986's disambiguation for a relative reference whose
        // first segment contains `:` (or reads as a URL): without it,
        // stripping the slash from `/https://example/x` or `/mailto:x`
        // would hand the remainder to scheme parsing and reject a
        // perfectly on-origin path as an origin escape.
        base.join(&format!("./{}", &path[1..]))
    } else {
        base.join(path)
    }
    .map_err(|e| {
        JoinRejection::Malformed(Error::usage(format!("invalid request path {path:?}: {e}")))
    })?;
    if url.origin() != base.origin() {
        return Err(JoinRejection::OffOrigin);
    }
    // The base's path prefix is part of the pinned target, not a default a
    // path may opt out of: dot segments (literal or `%2e` — WHATWG
    // normalizes both before this point) could otherwise stay on-origin yet
    // route around a gateway prefix. Checked on the joined result, so no
    // input shape can dodge it; the default base's prefix is `/`, which no
    // normalized path can escape.
    if !url.path().starts_with(base.path()) {
        return Err(JoinRejection::Malformed(Error::usage(format!(
            "request path {path:?} resolves outside the base URL's path {:?}",
            base.path()
        ))));
    }
    Ok(url)
}

/// Form body with **literal keys**: live verification proved bracketed keys
/// (`hostnames[]=`) against the backend; the percent-encoded `%5B%5D` form a
/// stock encoder emits was never proven, and this API earns no benefit of the
/// doubt (Open item 9, resolved). Values still percent-encode — framing
/// (`&`, `=`) must survive any value; the server decodes them back.
///
/// Generic over the pair type so both callers encode straight from what they
/// already hold — [`Client::write`]'s borrowed `&[(&str, String)]` and
/// `cdctl api`'s owned `Vec<(String, String)>` — without first copying either
/// into the other's shape just to call this.
pub(crate) fn encode_form<K: AsRef<str>, V: AsRef<str>>(pairs: &[(K, V)]) -> String {
    let mut body = String::new();
    for (key, value) in pairs {
        if !body.is_empty() {
            body.push('&');
        }
        body.push_str(key.as_ref());
        body.push('=');
        body.extend(form_urlencoded::byte_serialize(value.as_ref().as_bytes()));
    }
    body
}

/// One value percent-encoded for a URL path segment: everything but the
/// RFC 3986 unreserved set, so a wildcard hostname's `*` becomes `%2A` in
/// `DELETE /profiles/{id}/rules/{hostname}` paths.
pub(crate) fn encode_path_segment(raw: &str) -> String {
    use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
    const KEEP: &percent_encoding::AsciiSet = &NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'.')
        .remove(b'_')
        .remove(b'~');
    utf8_percent_encode(raw, KEEP).to_string()
}

/// The D12 write contract's failure hint: a retryable error keeps exit 8 but
/// the caller must re-fetch state, never blindly replay.
fn warn_write_may_have_landed(error: Error) -> Error {
    if error.retryable() {
        error.with_hint(
            "this write was not retried and may still have landed; re-fetch state before retrying",
        )
    } else {
        error
    }
}

/// Defensive parsing per reference/error-codes.md: `success: true` on a 2xx
/// is the only success; JSON errors classify via D4b; empty/non-JSON bodies
/// classify on the HTTP status alone.
fn interpret_response(wire: &WireResponse, resource: &'static str) -> Result<Envelope, Error> {
    let retry_after = wire.retry_after.seconds();
    match serde_json::from_slice::<Envelope>(&wire.body) {
        Ok(envelope) if envelope.is_success() => {
            if wire.status.is_success() {
                Ok(envelope)
            } else {
                Err(classify_unconfirmed_success(
                    wire.status.as_u16(),
                    retry_after,
                ))
            }
        }
        Ok(envelope) => Err(classify_envelope_error(&envelope, wire, resource)),
        Err(parse_err) => Err(
            classify_unparseable(wire.status.as_u16(), resource, retry_after)
                .with_debug_note(unparseable_body_note(&wire.body, &parse_err)),
        ),
    }
}

/// The passthrough interpretation (D9): a 2xx body is data unless it
/// affirmatively carries the error envelope (`error` present or
/// `success: false`); non-envelope 2xx bodies — the binary `/mobileconfig`
/// response — pass through verbatim. Non-2xx classifies through the same
/// rules as typed commands.
///
/// Production `Envelope` parsing accepts unknown fields, so on a non-2xx
/// *any* JSON object parses as an all-`None` envelope. Only an affirmative
/// `success: true` may claim the unconfirmed-success stance; a marker-less
/// body classifies on the HTTP status — a plain-JSON 404 from a gateway or
/// an undocumented endpoint must stay terminal exit 3, never retryable.
fn interpret_raw(wire: &WireResponse, resource: &'static str) -> Result<Bytes, Error> {
    let retry_after = wire.retry_after.seconds();
    match serde_json::from_slice::<Envelope>(&wire.body) {
        Ok(envelope) if envelope.error.is_some() || envelope.success == Some(false) => {
            Err(classify_envelope_error(&envelope, wire, resource))
        }
        Ok(_) | Err(_) if wire.status.is_success() => Ok(wire.body.clone()),
        Ok(envelope) if envelope.is_success() => Err(classify_unconfirmed_success(
            wire.status.as_u16(),
            retry_after,
        )),
        Ok(envelope) => Err(classify_envelope_error(&envelope, wire, resource)),
        // For the escape hatch the raw body (an HTML block page, gateway
        // text) is often the only actionable evidence — say where it went.
        Err(parse_err) => Err(
            classify_unparseable(wire.status.as_u16(), resource, retry_after)
                .with_debug_note(unparseable_body_note(&wire.body, &parse_err))
                .with_hint("re-run with --debug to see the unparseable response body"),
        ),
    }
}

/// One envelope-error classification for both interpreters — the exit-code
/// contract must never fork between typed commands and the passthrough.
fn classify_envelope_error(
    envelope: &Envelope,
    wire: &WireResponse,
    resource: &'static str,
) -> Error {
    let upstream = envelope.error.as_ref();
    classify_response(
        wire.status.as_u16(),
        upstream.and_then(|e| e.code),
        upstream.and_then(|e| e.message.as_deref()),
        resource,
        wire.retry_after.seconds(),
    )
}

/// The parse verdict for `Retry-After`, kept three-valued so `--debug` can
/// say a header existed but was dropped — `null` in the envelope is
/// documented as "absent or unparseable" (D4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetryAfter {
    Absent,
    Seconds(u64),
    Unparseable,
}

impl RetryAfter {
    fn seconds(self) -> Option<u64> {
        match self {
            Self::Seconds(seconds) => Some(seconds),
            Self::Absent | Self::Unparseable => None,
        }
    }
}

/// `Retry-After` arrives as integer seconds or as an HTTP-date; a date in the
/// past clamps to zero.
fn parse_retry_after(headers: &HeaderMap) -> RetryAfter {
    let Some(raw) = headers.get(RETRY_AFTER) else {
        return RetryAfter::Absent;
    };
    let Ok(value) = raw.to_str() else {
        return RetryAfter::Unparseable;
    };
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return RetryAfter::Seconds(seconds);
    }
    match httpdate::parse_http_date(value.trim()) {
        Ok(when) => RetryAfter::Seconds(
            when.duration_since(SystemTime::now())
                .map_or(0, |d| d.as_secs()),
        ),
        Err(_) => RetryAfter::Unparseable,
    }
}

/// D9: redirects are followed same-origin only; a cross-origin redirect is
/// returned as-is rather than followed, so credentials cannot travel.
fn same_origin_redirects() -> redirect::Policy {
    redirect::Policy::custom(|attempt| {
        if attempt.previous().len() > MAX_REDIRECTS {
            return attempt.error("too many redirects");
        }
        let same_origin = attempt
            .previous()
            .last()
            .is_some_and(|previous| previous.origin() == attempt.url().origin());
        if same_origin {
            attempt.follow()
        } else {
            attempt.stop()
        }
    })
}

fn trace_header<'h>(headers: &'h HeaderMap, name: &str) -> &'h str {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("-")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Exit;
    use wiremock::matchers::{body_string_contains, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_config(server_url: &str, retry: RetryPolicy) -> ClientConfig {
        ClientConfig {
            base_url: Url::parse(server_url).expect("mock server URL is valid"),
            token: Some(SecretString::from("test-token")),
            timeout: Duration::from_secs(2),
            connect_timeout: Duration::from_secs(2),
            retry,
            debug: false,
            quiet: false,
            allow_unpinned_origin: true, // deliberately pointed at wiremock
        }
    }

    fn test_client(server_url: &str, retry: RetryPolicy) -> Client {
        Client::new(test_config(server_url, retry)).expect("client builds")
    }

    fn fast_retry() -> RetryPolicy {
        RetryPolicy {
            enabled: true,
            max_attempts: 3,
            max_elapsed: Duration::from_secs(10),
            base_delay: Duration::from_millis(2),
        }
    }

    fn success_body() -> serde_json::Value {
        serde_json::json!({"body": {"profiles": []}, "success": true})
    }

    #[test]
    fn form_keys_stay_verbatim_while_values_encode() {
        let body = encode_form(&[
            ("hostnames[]", "*.example.com"),
            ("name", "a&b=c d"),
            ("note", "café"),
        ]);
        assert_eq!(
            body,
            "hostnames[]=*.example.com&name=a%26b%3Dc+d&note=caf%C3%A9"
        );
    }

    #[test]
    fn empty_form_encodes_to_an_empty_body() {
        assert_eq!(encode_form::<&str, &str>(&[]), "");
    }

    #[test]
    fn path_segments_encode_everything_but_unreserved() {
        assert_eq!(encode_path_segment("*.example.com"), "%2A.example.com");
        assert_eq!(
            encode_path_segment("plain-host_1.example~"),
            "plain-host_1.example~"
        );
        assert_eq!(encode_path_segment("a/b?c#d%e"), "a%2Fb%3Fc%23d%25e");
    }

    /// A `CONTROLD_API_URL` naming a path-routing gateway (e.g.
    /// `https://gw.example/controld`) must keep that path prefix on every
    /// request — `base.join("/profiles")` alone replaces the whole path and
    /// silently drops it.
    #[test]
    fn join_preserves_a_base_path_prefix() {
        let base = Url::parse("https://gw.example/controld/").expect("base");
        let joined = join_pinned_to_origin(&base, "/profiles").expect("same-origin");
        assert_eq!(joined.as_str(), "https://gw.example/controld/profiles");
    }

    /// A slash-prefixed path whose remainder happens to parse as an absolute
    /// URL or opaque URI (`/https://example/x`, `/mailto:x`) is still a path
    /// under the API origin — the `./` disambiguation must keep it out of
    /// scheme parsing rather than rejecting it off-origin.
    /// Dot segments (literal or percent-encoded — WHATWG normalizes both)
    /// must not escape a configured base path prefix while staying
    /// on-origin: the prefix is part of the pinned target, not a default.
    #[test]
    fn join_rejects_a_dot_segment_escape_of_the_base_path_prefix() {
        let base = Url::parse("https://gw.example/controld/").expect("base");
        for path in ["/../users", "/%2e%2e/users", "/a/../../users"] {
            assert!(
                join_pinned_to_origin(&base, path).is_err(),
                "{path:?} must not escape the /controld/ prefix"
            );
        }
        // Dot segments that stay inside the prefix keep resolving.
        let inside = join_pinned_to_origin(&base, "/a/../users").expect("inside the prefix");
        assert_eq!(inside.as_str(), "https://gw.example/controld/users");
        // With the default base the prefix is `/`, which nothing can escape.
        let default_base = Url::parse("https://api.controld.com/").expect("base");
        let joined = join_pinned_to_origin(&default_base, "/../users").expect("still under /");
        assert_eq!(joined.as_str(), "https://api.controld.com/users");
    }

    #[test]
    fn join_keeps_a_url_shaped_path_on_the_api_origin() {
        let base = Url::parse("https://api.controld.com").expect("base");
        let joined =
            join_pinned_to_origin(&base, "/https://example/x").expect("still a same-origin path");
        assert_eq!(
            joined.as_str(),
            "https://api.controld.com/https://example/x"
        );
        let joined = join_pinned_to_origin(&base, "/mailto:x").expect("still a same-origin path");
        assert_eq!(joined.as_str(), "https://api.controld.com/mailto:x");
    }

    /// The trailing-slash normalization lives at construction
    /// ([`base_url_from_raw`]), not in `join_pinned_to_origin` itself — so
    /// this pins it through the real construction path a `CONTROLD_API_URL`
    /// without a trailing slash takes, rather than hand-building an
    /// already-normalized `Url` and only testing the join.
    #[test]
    fn join_preserves_a_base_path_prefix_without_a_trailing_slash() {
        let base = base_url_from_raw(Some("https://gw.example/controld".to_owned()))
            .expect("constructs and normalizes");
        let joined = join_pinned_to_origin(&base, "/profiles").expect("same-origin");
        assert_eq!(joined.as_str(), "https://gw.example/controld/profiles");
    }

    #[test]
    fn base_url_from_raw_normalizes_a_missing_trailing_slash() {
        let base =
            base_url_from_raw(Some("https://gw.example/controld".to_owned())).expect("constructs");
        assert_eq!(base.path(), "/controld/");
    }

    #[test]
    fn base_url_from_raw_leaves_the_normalized_default_unchanged() {
        let base = base_url_from_raw(None).expect("default base");
        assert_eq!(base.as_str(), "https://api.controld.com/");
    }

    /// A URL that parses but carries no usable request scheme (a
    /// cannot-be-a-base `mailto:` URI) must be rejected at construction, not
    /// left to fail every later request misattributed to the request path.
    #[test]
    fn base_url_from_raw_rejects_a_non_http_scheme() {
        let error = base_url_from_raw(Some("mailto:x".to_owned())).expect_err("mailto is not http");
        assert_eq!(error.exit(), Exit::Usage);
        assert!(
            error.message.contains("CONTROLD_API_URL"),
            "got: {}",
            error.message
        );
    }

    /// Same gate, a bogus scheme with a host — parses fine as a `Url`, still
    /// unusable as an HTTP(S) origin.
    #[test]
    fn base_url_from_raw_rejects_an_unknown_scheme_with_a_host() {
        let error =
            base_url_from_raw(Some("foo://host/".to_owned())).expect_err("foo is not http(s)");
        assert_eq!(error.exit(), Exit::Usage);
        assert!(
            error.message.contains("CONTROLD_API_URL"),
            "got: {}",
            error.message
        );
    }

    /// The gate must not reject what it exists to allow: a plain `https://`
    /// base still constructs.
    #[test]
    fn base_url_from_raw_still_accepts_an_https_base() {
        base_url_from_raw(Some("https://gw.example/controld".to_owned()))
            .expect("https base is fine");
    }

    #[test]
    fn join_with_a_base_path_prefix_keeps_the_query_string() {
        let base = Url::parse("https://gw.example/controld/").expect("base");
        let joined = join_pinned_to_origin(&base, "/x?y=%2A").expect("same-origin");
        assert_eq!(joined.path(), "/controld/x");
        assert_eq!(joined.query(), Some("y=%2A"));
    }

    #[test]
    fn join_with_the_default_base_is_unchanged() {
        let base = Url::parse(DEFAULT_BASE_URL).expect("default base");
        let joined = join_pinned_to_origin(&base, "/profiles").expect("same-origin");
        assert_eq!(joined.as_str(), "https://api.controld.com/profiles");
    }

    /// A base path prefix must never weaken the D9 origin pin: a
    /// scheme-relative path still replaces the host and is rejected.
    #[test]
    fn join_still_rejects_a_scheme_relative_path_with_a_base_prefix() {
        let base = Url::parse("https://gw.example/controld").expect("base");
        assert!(matches!(
            join_pinned_to_origin(&base, "//evil.example/x"),
            Err(JoinRejection::OffOrigin)
        ));
    }

    #[tokio::test]
    async fn get_retries_transient_failures_then_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/profiles"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(2)
            .expect(2)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/profiles"))
            .respond_with(ResponseTemplate::new(200).set_body_json(success_body()))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri(), fast_retry());
        let envelope = client
            .get("/profiles", "profile")
            .await
            .expect("third attempt succeeds");
        assert!(envelope.is_success());
    }

    #[tokio::test]
    async fn get_stops_at_the_attempt_cap() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/profiles"))
            .respond_with(ResponseTemplate::new(500))
            .expect(3)
            .mount(&server)
            .await;

        let client = test_client(&server.uri(), fast_retry());
        let error = client
            .get("/profiles", "profile")
            .await
            .expect_err("gives up");
        assert_eq!(error.code, "upstream.error");
        assert_eq!(error.exit(), Exit::Retryable);
    }

    #[tokio::test]
    async fn get_honors_retry_after_and_carries_it() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/profiles"))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "0"))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/profiles"))
            .respond_with(ResponseTemplate::new(200).set_body_json(success_body()))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri(), fast_retry());
        client
            .get("/profiles", "profile")
            .await
            .expect("retried after 429");
    }

    #[tokio::test]
    async fn no_retry_disables_the_loop() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/profiles"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(
            &server.uri(),
            RetryPolicy {
                enabled: false,
                ..fast_retry()
            },
        );
        let error = client
            .get("/profiles", "profile")
            .await
            .expect_err("no retry");
        assert!(
            error.retryable(),
            "still classified retryable for the caller"
        );
    }

    /// The D12 core: under 429, 500, and timeout, a write endpoint receives
    /// exactly one request. `.expect(1)` panics on a second request.
    #[tokio::test]
    async fn writes_receive_exactly_one_request_under_500() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/profiles/p1/rules"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri(), fast_retry());
        let error = client
            .write(
                Method::POST,
                "/profiles/p1/rules",
                &[("do", "0".into())],
                "rule",
            )
            .await
            .expect_err("500 fails");
        assert_eq!(error.exit(), Exit::Retryable);
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|h| h.contains("may still have landed")),
            "write failures must warn the write may have landed"
        );
    }

    #[tokio::test]
    async fn writes_receive_exactly_one_request_under_429() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/profiles/p1/rules"))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "0"))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri(), fast_retry());
        let error = client
            .write(
                Method::POST,
                "/profiles/p1/rules",
                &[("do", "0".into())],
                "rule",
            )
            .await
            .expect_err("429 fails without a replay");
        assert_eq!(error.code, "ratelimit.exceeded");
        assert_eq!(error.retry_after, Some(0));
    }

    #[tokio::test]
    async fn writes_receive_exactly_one_request_under_timeout() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/profiles/p1/rules"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(success_body())
                    .set_delay(Duration::from_secs(5)),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = Client::new(ClientConfig {
            timeout: Duration::from_millis(100),
            ..test_config(&server.uri(), fast_retry())
        })
        .expect("client builds");
        let error = client
            .write(
                Method::POST,
                "/profiles/p1/rules",
                &[("do", "0".into())],
                "rule",
            )
            .await
            .expect_err("timeout");
        assert_eq!(error.code, "network.error");
        assert_eq!(error.exit(), Exit::Retryable);
    }

    #[tokio::test]
    async fn bearer_token_and_form_encoding_reach_the_wire() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/profiles/p1/rules"))
            .and(header("authorization", "Bearer test-token"))
            .and(header("content-type", "application/x-www-form-urlencoded"))
            .and(body_string_contains("do=0"))
            .and(body_string_contains("hostnames[]=a.com"))
            .respond_with(ResponseTemplate::new(200).set_body_json(success_body()))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri(), fast_retry());
        client
            .write(
                Method::POST,
                "/profiles/p1/rules",
                &[("do", "0".into()), ("hostnames[]", "a.com".into())],
                "rule",
            )
            .await
            .expect("matched mock proves headers and body encoding");
    }

    #[tokio::test]
    async fn zero_byte_json_500_synthesizes_the_null_upstream() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users"))
            .respond_with(
                ResponseTemplate::new(500).insert_header("content-type", "application/json"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(
            &server.uri(),
            RetryPolicy {
                enabled: false,
                ..fast_retry()
            },
        );
        let error = client
            .get("/users", "account")
            .await
            .expect_err("0-byte 500");
        assert_eq!(error.code, "upstream.error");
        assert_eq!(error.exit(), Exit::Retryable);
        let upstream = error.upstream.expect("synthesized upstream");
        assert_eq!(upstream.code, None);
        assert_eq!(upstream.http_status, Some(500));
        assert_eq!(upstream.message, None);
    }

    #[test]
    fn retry_after_parses_integers_and_http_dates() {
        let mut headers = HeaderMap::new();
        assert_eq!(parse_retry_after(&headers), RetryAfter::Absent);

        headers.insert(RETRY_AFTER, "17".parse().expect("valid header"));
        assert_eq!(parse_retry_after(&headers), RetryAfter::Seconds(17));

        let future = SystemTime::now() + Duration::from_secs(10);
        headers.insert(
            RETRY_AFTER,
            httpdate::fmt_http_date(future)
                .parse()
                .expect("valid header"),
        );
        let parsed = parse_retry_after(&headers).seconds().expect("date parses");
        assert!(
            (8..=10).contains(&parsed),
            "seconds until the date, saw {parsed}"
        );

        let past = SystemTime::now() - Duration::from_secs(60);
        headers.insert(
            RETRY_AFTER,
            httpdate::fmt_http_date(past).parse().expect("valid header"),
        );
        assert_eq!(
            parse_retry_after(&headers),
            RetryAfter::Seconds(0),
            "past dates clamp to zero"
        );

        headers.insert(RETRY_AFTER, "soon".parse().expect("valid header"));
        assert_eq!(
            parse_retry_after(&headers),
            RetryAfter::Unparseable,
            "garbage is distinguishable from absent"
        );
        assert_eq!(parse_retry_after(&headers).seconds(), None);
    }

    /// The backoff contract directly: `Retry-After` overrides the jitter
    /// exactly; without it the delay falls in `(0, base * 2^attempt]`.
    #[test]
    fn backoff_delay_honors_retry_after_and_jitters_otherwise() {
        let client = test_client("http://127.0.0.1:1", fast_retry());
        assert_eq!(
            client.backoff_delay(1, Some(3)),
            Duration::from_secs(3),
            "Retry-After wins verbatim"
        );
        for attempt in 1..=3 {
            let cap = fast_retry().base_delay * 2u32.pow(attempt);
            let delay = client.backoff_delay(attempt, None);
            assert!(
                delay > Duration::ZERO && delay <= cap,
                "attempt {attempt}: {delay:?} outside (0, {cap:?}]"
            );
        }
    }

    /// D12's elapsed cap: a huge `Retry-After` must not put the CLI to sleep;
    /// the loop gives up instead of honoring a delay past the cap.
    #[tokio::test]
    async fn retry_gives_up_when_retry_after_exceeds_max_elapsed() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/profiles"))
            .respond_with(ResponseTemplate::new(429).insert_header("retry-after", "9999"))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(
            &server.uri(),
            RetryPolicy {
                max_elapsed: Duration::from_secs(1),
                ..fast_retry()
            },
        );
        let error = client
            .get("/profiles", "profile")
            .await
            .expect_err("gives up");
        assert_eq!(error.code, "ratelimit.exceeded");
        assert_eq!(
            error.retry_after,
            Some(9999),
            "the directive is still reported"
        );
    }

    /// GETs retry only *retryable* failures — a 404 must not triple-hit the API.
    #[tokio::test]
    async fn terminal_get_failures_are_not_retried() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/profiles"))
            .respond_with(ResponseTemplate::new(404))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri(), fast_retry());
        let error = client.get("/profiles", "profile").await.expect_err("404");
        assert_eq!(error.code, "profile.not_found");
        assert_eq!(error.exit(), Exit::NotFound);
    }

    /// The raw passthrough returns the body byte-for-byte: no re-encoding,
    /// no reshaping, trailing whitespace included.
    #[tokio::test]
    async fn get_raw_returns_the_body_verbatim() {
        let raw_body = "{\"body\": {\"profiles\": []},   \"success\": true}\n";
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/profiles"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(raw_body, "application/json"))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri(), fast_retry());
        let bytes = client
            .get_raw("/profiles", "resource")
            .await
            .expect("2xx success envelope");
        assert_eq!(bytes, raw_body.as_bytes());
    }

    /// A 2xx body that is not an envelope (the binary /mobileconfig response)
    /// is data, never an error — the passthrough must not impose the envelope.
    #[tokio::test]
    async fn get_raw_passes_non_envelope_bodies_through() {
        let binary: &[u8] = &[0x3c, 0x3f, 0x78, 0x6d, 0x6c, 0x00, 0xff, 0xfe];
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/mobileconfig/abc"))
            .respond_with(
                ResponseTemplate::new(200).set_body_raw(binary, "application/x-apple-aspen-config"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri(), fast_retry());
        let bytes = client
            .get_raw("/mobileconfig/abc", "resource")
            .await
            .expect("binary 2xx is data");
        assert_eq!(bytes, binary);
    }

    /// Raw errors classify through the standard rules — the escape hatch
    /// keeps the exit-code contract.
    #[tokio::test]
    async fn get_raw_classifies_error_envelopes() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/nope"))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "body": [],
                "success": false,
                "error": {"code": 40401, "message": "No such thing"}
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri(), fast_retry());
        let error = client
            .get_raw("/nope", "resource")
            .await
            .expect_err("classified");
        assert_eq!(error.code, "resource.not_found");
        assert_eq!(error.exit(), Exit::NotFound);
        assert_eq!(error.upstream.expect("verbatim upstream").code, Some(40401));
    }

    /// A marker-less JSON body on a non-2xx (no `success`, no `error`)
    /// classifies on the HTTP status — terminal, never "unconfirmed
    /// success". The `.expect(1)` proves the 404 is not retried.
    #[tokio::test]
    async fn get_raw_classifies_marker_less_bodies_on_the_http_status() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/nope"))
            .respond_with(
                ResponseTemplate::new(404).set_body_raw(r#"{"body": []}"#, "application/json"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri(), fast_retry());
        let error = client
            .get_raw("/nope", "resource")
            .await
            .expect_err("terminal");
        assert_eq!(error.code, "resource.not_found");
        assert_eq!(error.exit(), Exit::NotFound);
    }

    /// A 2xx that affirmatively says `success: false` cannot be data; with
    /// no error code the classifier falls to the anomalous-response arm.
    #[tokio::test]
    async fn get_raw_rejects_a_2xx_that_denies_success() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/odd"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_raw(r#"{"body": [], "success": false}"#, "application/json"),
            )
            .expect(1..)
            .mount(&server)
            .await;

        let client = test_client(
            &server.uri(),
            RetryPolicy {
                enabled: false,
                ..fast_retry()
            },
        );
        let error = client
            .get_raw("/odd", "resource")
            .await
            .expect_err("not data");
        assert_eq!(error.code, "upstream.error");
        assert_eq!(error.exit(), Exit::Retryable);
    }

    /// Raw GETs keep the D12 retry loop.
    #[tokio::test]
    async fn get_raw_retries_transient_failures() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/profiles"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(1)
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/profiles"))
            .respond_with(ResponseTemplate::new(200).set_body_json(success_body()))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri(), fast_retry());
        client
            .get_raw("/profiles", "resource")
            .await
            .expect("second attempt succeeds");
    }

    /// Raw mutations keep the exactly-once contract and the landed hint.
    #[tokio::test]
    async fn write_raw_sends_exactly_one_request_under_500() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/profiles"))
            .respond_with(ResponseTemplate::new(500))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri(), fast_retry());
        let error = client
            .write_raw(
                Method::POST,
                "/profiles",
                Some(RawBody::Form(encode_form(&[("name", "x")]))),
                "resource",
            )
            .await
            .expect_err("500 fails, once");
        assert_eq!(error.exit(), Exit::Retryable);
        assert!(
            error
                .hint
                .as_deref()
                .is_some_and(|h| h.contains("may still have landed")),
        );
    }

    /// A JSON body reaches the wire verbatim with its content type — no
    /// validation, no reshaping (D9b).
    #[tokio::test]
    async fn write_raw_sends_stdin_json_verbatim() {
        let raw_json = "{\"filters\": [ {\"filter\":\"ads\",\"status\":1} ]}";
        let server = MockServer::start().await;
        Mock::given(method("PUT"))
            .and(path("/profiles/p1/filters"))
            .and(header("content-type", "application/json"))
            .and(wiremock::matchers::body_string(raw_json))
            .respond_with(ResponseTemplate::new(200).set_body_json(success_body()))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(&server.uri(), fast_retry());
        client
            .write_raw(
                Method::PUT,
                "/profiles/p1/filters",
                Some(RawBody::Json(raw_json.as_bytes().to_vec())),
                "resource",
            )
            .await
            .expect("matched mock proves the body went verbatim");
    }

    /// D9: no path may steer a request off the base origin — absolute URLs,
    /// scheme-relative forms, and the WHATWG `\`-as-`/` trick all resolve to
    /// a foreign origin after the join, and the joined URL is what's checked.
    #[tokio::test]
    async fn paths_cannot_escape_the_pinned_origin() {
        let server = MockServer::start().await;
        // The mock proves no request leaves the process at all.
        Mock::given(wiremock::matchers::any())
            .respond_with(ResponseTemplate::new(200).set_body_json(success_body()))
            .expect(0)
            .mount(&server)
            .await;

        let client = test_client(&server.uri(), fast_retry());
        for path in [
            "https://evil.example/x",
            "//evil.example/x",
            "/\\evil.example/x",
            "\\\\evil.example/x",
            "https://user:pw@evil.example/x",
        ] {
            let error = client.get(path, "resource").await.expect_err("escapes");
            assert_eq!(error.exit(), Exit::Usage, "path {path:?} must be rejected");
            assert!(
                error.upstream.is_none(),
                "rejected before any request: {path:?}"
            );
        }
    }

    /// A token must never ride to a non-default origin unless explicitly
    /// allowed — however the config was built (D9 at the choke point).
    #[test]
    fn client_refuses_a_token_bound_for_an_unpinned_origin() {
        let config = ClientConfig {
            allow_unpinned_origin: false,
            ..test_config("http://127.0.0.1:1", fast_retry())
        };
        let error = Client::new(config).expect_err("refused");
        assert_eq!(error.code, "usage.unsafe_base_url");
        assert_eq!(error.exit(), Exit::Usage);

        // Without a token there is nothing to protect.
        let config = ClientConfig {
            token: None,
            allow_unpinned_origin: false,
            ..test_config("http://127.0.0.1:1", fast_retry())
        };
        assert!(Client::new(config).is_ok());
    }

    /// D9: a cross-origin redirect is not followed — the bearer token cannot
    /// travel; same-origin redirects work.
    #[tokio::test]
    async fn redirects_stop_at_the_origin_boundary() {
        let server_a = MockServer::start().await;
        let server_b = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("{}/steal", server_b.uri()).as_str()),
            )
            .expect(1)
            .mount(&server_a)
            .await;
        // The other origin must see zero requests.
        Mock::given(wiremock::matchers::any())
            .respond_with(ResponseTemplate::new(200).set_body_json(success_body()))
            .expect(0)
            .mount(&server_b)
            .await;

        // No retries: an unfollowed 3xx classifies retryable, and a replay
        // would double-count against the .expect(1) above.
        let client = test_client(
            &server_a.uri(),
            RetryPolicy {
                enabled: false,
                ..fast_retry()
            },
        );
        let error = client
            .get("/users", "account")
            .await
            .expect_err("302 is final");
        assert_eq!(
            error.exit(),
            Exit::Retryable,
            "an unfollowed 3xx cannot confirm success"
        );

        // Same origin: followed.
        Mock::given(method("GET"))
            .and(path("/old"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("{}/new", server_a.uri()).as_str()),
            )
            .expect(1)
            .mount(&server_a)
            .await;
        Mock::given(method("GET"))
            .and(path("/new"))
            .respond_with(ResponseTemplate::new(200).set_body_json(success_body()))
            .expect(1)
            .mount(&server_a)
            .await;
        client
            .get("/old", "account")
            .await
            .expect("same-origin follow");
    }

    /// A success body riding a non-2xx cannot confirm success.
    #[tokio::test]
    async fn success_body_on_a_non_2xx_is_not_trusted() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users"))
            .respond_with(ResponseTemplate::new(502).set_body_json(success_body()))
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(
            &server.uri(),
            RetryPolicy {
                enabled: false,
                ..fast_retry()
            },
        );
        let error = client
            .get("/users", "account")
            .await
            .expect_err("not trusted");
        assert_eq!(error.code, "upstream.error");
        assert_eq!(error.exit(), Exit::Retryable);
    }

    /// The unparseable-body evidence survives into --debug notes.
    #[tokio::test]
    async fn unparseable_bodies_keep_their_evidence() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users"))
            .respond_with(
                ResponseTemplate::new(403)
                    .set_body_string("<html><body>Blocked by proxy</body></html>"),
            )
            .expect(1)
            .mount(&server)
            .await;

        let client = test_client(
            &server.uri(),
            RetryPolicy {
                enabled: false,
                ..fast_retry()
            },
        );
        let error = client
            .get("/users", "account")
            .await
            .expect_err("HTML block page");
        assert_eq!(error.code, "permission.denied");
        let note = error.debug_notes.first().expect("evidence retained");
        assert!(
            note.contains("Blocked by proxy"),
            "note carries the snippet: {note}"
        );
        assert!(
            note.contains("bytes) did not parse"),
            "note carries the length: {note}"
        );
    }
}
