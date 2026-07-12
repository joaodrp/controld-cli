//! The HTTP client and the retry contract (D12).
//!
//! reqwest's built-in retries are disabled (`retry::never()`) — they replay
//! immediately, ignore `Retry-After`, and are not GET-only. The loop below is
//! cdctl's: GETs retry with full-jitter backoff under 3-attempt/30-second
//! caps; **writes are sent exactly once, always**.

use std::time::{Duration, Instant, SystemTime};

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

/// A request body in one of the two D9b encodings — form pairs with literal
/// keys, or bytes sent verbatim as JSON. Nothing is ever sniffed (D16).
#[derive(Debug)]
pub enum RawBody {
    Form(Vec<(String, String)>),
    Json(Vec<u8>),
}

/// The transport-level response parts, before any interpretation.
#[derive(Debug)]
struct WireResponse {
    status: StatusCode,
    retry_after: RetryAfter,
    body: Vec<u8>,
}

#[derive(Debug)]
pub struct ClientConfig {
    pub base_url: Url,
    pub token: Option<SecretString>,
    pub timeout: Duration,
    pub connect_timeout: Duration,
    pub retry: RetryPolicy,
    pub debug: bool,
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
    ) -> Result<Self, Error> {
        let base_url = match crate::config::env_var("CONTROLD_API_URL")? {
            Some(raw) => Url::parse(&raw)
                .map_err(|e| Error::usage(format!("CONTROLD_API_URL is not a valid URL: {e}")))?,
            None => Url::parse(DEFAULT_BASE_URL).expect("the default base URL is valid"),
        };

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
            allow_unpinned_origin: crate::config::env_var("CONTROLD_UNSAFE_BASE_URL")?
                .is_some_and(|v| v == "1"),
        })
    }
}

// Debug is safe: secrecy renders the token as REDACTED.
#[derive(Debug)]
pub struct Client {
    http: reqwest::Client,
    base_url: Url,
    token: Option<SecretString>,
    retry: RetryPolicy,
    debug: bool,
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
        })
    }

    /// GET with the D12 retry loop: full-jitter backoff, `Retry-After`
    /// honored, capped attempts and elapsed time, every retry logged (a
    /// silent backoff is indistinguishable from a hang).
    pub async fn get(&self, path: &str, resource: &'static str) -> Result<Envelope, Error> {
        self.get_with(path, |wire| {
            interpret_response(
                wire.status,
                wire.retry_after.seconds(),
                &wire.body,
                resource,
            )
        })
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
            eprintln!(
                "info: GET {path} failed ({code}); retrying in {secs:.1}s (attempt {attempt}/{max})",
                code = error.code,
                secs = delay.as_secs_f64(),
                max = self.retry.max_attempts,
            );
            tokio::time::sleep(delay).await;
        }
    }

    /// A mutation: sent exactly once, whatever happens. A retryable failure
    /// keeps exit 8 but the hint says the write may have landed — re-fetching
    /// state is the caller's (or the agent's) job, never a blind replay.
    #[allow(
        dead_code,
        reason = "first caller is Phase 2 (`cdctl api`); the exactly-once contract is tested now"
    )]
    pub async fn write(
        &self,
        method: Method,
        path: &str,
        form: &[(&str, String)],
        resource: &'static str,
    ) -> Result<Envelope, Error> {
        debug_assert_ne!(
            method,
            Method::GET,
            "writes go through write(), GETs through get()"
        );
        let body = (!form.is_empty()).then(|| {
            RawBody::Form(
                form.iter()
                    .map(|(key, value)| ((*key).to_owned(), value.clone()))
                    .collect(),
            )
        });
        self.send(method, path, body.as_ref())
            .await
            .and_then(|wire| {
                interpret_response(
                    wire.status,
                    wire.retry_after.seconds(),
                    &wire.body,
                    resource,
                )
            })
            .map_err(warn_write_may_have_landed)
    }

    /// Raw GET for `cdctl api` (D9): the body bytes verbatim, with the D12
    /// retry loop. Errors still classify through the standard rules.
    pub async fn get_raw(&self, path: &str, resource: &'static str) -> Result<Vec<u8>, Error> {
        self.get_with(path, |wire| {
            interpret_raw(wire.status, wire.retry_after.seconds(), &wire.body, resource)
        })
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
    ) -> Result<Vec<u8>, Error> {
        debug_assert_ne!(
            method,
            Method::GET,
            "raw GETs go through get_raw(), mutations through write_raw()"
        );
        self.send(method, path, body.as_ref())
            .await
            .and_then(|wire| {
                interpret_raw(wire.status, wire.retry_after.seconds(), &wire.body, resource)
            })
            .map_err(warn_write_may_have_landed)
    }

    /// One request over the wire: join the path, attach the bearer token and
    /// body, send, collect the response parts. No interpretation.
    async fn send(
        &self,
        method: Method,
        path: &str,
        body: Option<&RawBody>,
    ) -> Result<WireResponse, Error> {
        let url = self
            .base_url
            .join(path)
            .map_err(|e| Error::usage(format!("invalid request path {path:?}: {e}")))?;
        // D9: a join must never escape the pinned origin. WHATWG parsing
        // treats `\` like `/` and lets absolute or scheme-relative input
        // replace the host — so the joined result is checked, not the input.
        if url.origin() != self.base_url.origin() {
            return Err(Error::usage(format!(
                "request path {path:?} resolves outside the API origin ({base})",
                base = self.base_url.origin().ascii_serialization()
            )));
        }

        if self.debug {
            // The Authorization header is never traced (token redaction, D6).
            eprintln!("debug: > {method} {url}");
        }
        let mut request = self.http.request(method, url);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token.expose_secret());
        }
        match body {
            Some(RawBody::Form(pairs)) => request = request.form(pairs),
            Some(RawBody::Json(bytes)) => {
                request = request
                    .header(reqwest::header::CONTENT_TYPE, "application/json")
                    .body(bytes.clone());
            }
            None => {}
        }

        let response = request.send().await.map_err(|e| classify_transport(&e))?;
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
            body: body.to_vec(),
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
fn interpret_response(
    status: StatusCode,
    retry_after: Option<u64>,
    body: &[u8],
    resource: &'static str,
) -> Result<Envelope, Error> {
    match serde_json::from_slice::<Envelope>(body) {
        Ok(envelope) if envelope.is_success() => {
            if status.is_success() {
                Ok(envelope)
            } else {
                Err(classify_unconfirmed_success(status.as_u16(), retry_after))
            }
        }
        Ok(envelope) => {
            let upstream = envelope.error.as_ref();
            Err(classify_response(
                status.as_u16(),
                upstream.and_then(|e| e.code),
                upstream.and_then(|e| e.message.as_deref()),
                resource,
                retry_after,
            ))
        }
        Err(parse_err) => Err(classify_unparseable(status.as_u16(), resource, retry_after)
            .with_debug_note(unparseable_body_note(body, &parse_err))),
    }
}

/// The passthrough interpretation (D9): a 2xx body is data unless it
/// affirmatively carries the error envelope (`error` present or
/// `success: false`); non-envelope 2xx bodies — the binary `/mobileconfig`
/// response — pass through verbatim. Non-2xx classifies through the same
/// rules as typed commands.
fn interpret_raw(
    status: StatusCode,
    retry_after: Option<u64>,
    body: &[u8],
    resource: &'static str,
) -> Result<Vec<u8>, Error> {
    match serde_json::from_slice::<Envelope>(body) {
        Ok(envelope) if envelope.error.is_some() || envelope.success == Some(false) => {
            let upstream = envelope.error.as_ref();
            Err(classify_response(
                status.as_u16(),
                upstream.and_then(|e| e.code),
                upstream.and_then(|e| e.message.as_deref()),
                resource,
                retry_after,
            ))
        }
        Ok(_) | Err(_) if status.is_success() => Ok(body.to_vec()),
        Ok(_) => Err(classify_unconfirmed_success(status.as_u16(), retry_after)),
        Err(parse_err) => Err(classify_unparseable(status.as_u16(), resource, retry_after)
            .with_debug_note(unparseable_body_note(body, &parse_err))),
    }
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
            .and(body_string_contains("hostnames%5B%5D=a.com"))
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
                Some(RawBody::Form(vec![("name".into(), "x".into())])),
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
