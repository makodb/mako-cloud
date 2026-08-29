//! Cross-origin access for an application's own API.
//!
//! A browser application served from its own origin -- its development
//! server, or the host its pages are deployed to -- calls its project's API
//! on another origin, and the browser hands it the answer only if the
//! server names that origin back. Which origins may be named is a setting
//! of the **environment**, so the same list applies wherever that
//! environment's API is served: the platform's own hostname and every
//! custom domain alike.
//!
//! Nothing is emitted unless an origin is on that list. There is no
//! wildcard, no credentialed mode, and no cross-origin header on a route a
//! browser application does not call -- the management API, the operator
//! API, the developer workspace, and the service-credential routes are
//! never labelled, whatever origin asks.
//!
//! The list itself lives in the service -- the data plane reads what the
//! control plane installed for the environment the path names, the edge
//! gateway what the resolved function route carries -- behind
//! [`CrossOriginPolicy`], which answers one question: may this origin be
//! answered for this request?

use std::{fmt, sync::Arc};

use tiny_http::Header;

use crate::{HttpMethod, HttpMiddleware, HttpRequestHead, HttpResponse};

const ORIGIN_HEADER: &str = "origin";

/// The methods an application API answers; a preflight is told exactly
/// these and never more.
pub const CORS_ALLOW_METHODS: &str = "GET, POST, PUT, PATCH, DELETE, OPTIONS";
/// The request headers a browser client sends: the session or key
/// credential, the body's type, the idempotency key, and the conditional
/// headers the object routes honor. Cookies are not part of it, so no
/// credentialed mode is granted.
pub const CORS_ALLOW_HEADERS: &str =
    "authorization, content-type, x-mako-key, idempotency-key, if-none-match, if-match";
/// The response headers a browser client may read cross-origin.
pub const CORS_EXPOSE_HEADERS: &str = "etag, x-mako-request-id, content-type";
/// How long a browser may cache one preflight answer.
pub const CORS_MAX_AGE_SECONDS: u32 = 600;

/// Which browser origins may be answered, as the service holds the answer.
///
/// `origin` is verbatim what the browser sent, already checked to be shaped
/// like an origin. The request head says what is being addressed: an
/// implementation reads the path to find the environment whose allowlist
/// applies, and answers `false` for a route that is not an application
/// route at all, for an environment it cannot determine, and for an origin
/// that environment does not list.
pub trait CrossOriginPolicy: Send + Sync {
    /// Whether `origin` may be answered for this request.
    fn allows_origin(&self, request: &HttpRequestHead, origin: &str) -> bool;
}

/// What a request's `Origin` and the policy together decide.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CrossOriginDecision {
    /// No cross-origin header is emitted: the request carries no usable
    /// `Origin`, the path is not one a browser application calls, or the
    /// origin is not listed for the environment it names. The browser
    /// refuses the response on the application's behalf.
    NotAllowed,
    /// The origin is listed and is echoed back verbatim.
    Allowed(String),
}

impl CrossOriginDecision {
    /// The origin to echo, when there is one.
    #[must_use]
    pub fn allowed_origin(&self) -> Option<&str> {
        match self {
            Self::Allowed(origin) => Some(origin.as_str()),
            Self::NotAllowed => None,
        }
    }
}

/// Answers cross-origin preflights and labels every other response, for a
/// request from an origin the addressed environment allows.
pub struct CrossOriginMiddleware {
    policy: Arc<dyn CrossOriginPolicy>,
}

impl fmt::Debug for CrossOriginMiddleware {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CrossOriginMiddleware")
            .finish_non_exhaustive()
    }
}

impl CrossOriginMiddleware {
    #[must_use]
    pub fn new(policy: Arc<dyn CrossOriginPolicy>) -> Self {
        Self { policy }
    }

    /// Installs the middleware on a router, as a service composes it.
    #[must_use]
    pub fn shared(policy: Arc<dyn CrossOriginPolicy>) -> Arc<dyn HttpMiddleware> {
        Arc::new(Self::new(policy))
    }

    /// What the request's `Origin` and the policy decide. Nothing but the
    /// head is consulted, so a preflight is answered without reading a body
    /// and every response is decided the same way.
    #[must_use]
    pub fn decide(&self, request: &HttpRequestHead) -> CrossOriginDecision {
        // A repeated or malformed `Origin` is no origin at all: the browser
        // sends exactly one, and echoing anything else would be echoing an
        // attacker's string.
        let Some(origin) = request
            .header(ORIGIN_HEADER)
            .filter(|origin| is_origin(origin))
        else {
            return CrossOriginDecision::NotAllowed;
        };
        if self.policy.allows_origin(request, origin) {
            CrossOriginDecision::Allowed(origin.to_owned())
        } else {
            CrossOriginDecision::NotAllowed
        }
    }
}

/// The preflight answer for an allowed origin: `204`, with the methods and
/// request headers the API accepts.
fn preflight_response(origin: &str) -> HttpResponse {
    let max_age = CORS_MAX_AGE_SECONDS.to_string();
    with_headers(
        HttpResponse::empty(204),
        &[
            ("Access-Control-Allow-Origin", origin),
            ("Access-Control-Allow-Methods", CORS_ALLOW_METHODS),
            ("Access-Control-Allow-Headers", CORS_ALLOW_HEADERS),
            ("Access-Control-Max-Age", max_age.as_str()),
            ("Vary", "Origin"),
        ],
    )
}

impl HttpMiddleware for CrossOriginMiddleware {
    fn before_dispatch(&self, request: &HttpRequestHead) -> Option<HttpResponse> {
        // Only a preflight from an allowed origin is answered here. An
        // `OPTIONS` that is not one routes exactly as it did before this
        // existed -- to the transport's `405`, or to a function that
        // handles its own -- so nothing is taken away from a request the
        // platform is not answering on the browser's behalf.
        if request.method() != HttpMethod::Options {
            return None;
        }
        let decision = self.decide(request);
        decision.allowed_origin().map(preflight_response)
    }

    fn after_dispatch(&self, request: &HttpRequestHead, response: HttpResponse) -> HttpResponse {
        match self.decide(request) {
            CrossOriginDecision::Allowed(origin) => with_headers(
                response,
                &[
                    ("Access-Control-Allow-Origin", origin.as_str()),
                    ("Access-Control-Expose-Headers", CORS_EXPOSE_HEADERS),
                    ("Vary", "Origin"),
                ],
            ),
            CrossOriginDecision::NotAllowed => response,
        }
    }
}

/// Adds every header, or none of them: a half-labelled response is worse
/// than an unlabelled one, which the browser simply blocks. None of these
/// names is transport-owned, and every value is a constant or an origin the
/// policy matched exactly, so the refusal branch is unreachable in practice
/// and is still not a place to panic.
fn with_headers(mut response: HttpResponse, headers: &[(&str, &str)]) -> HttpResponse {
    let built = headers
        .iter()
        .map(|(name, value)| Header::from_bytes(name.as_bytes(), value.as_bytes()).ok())
        .collect::<Option<Vec<_>>>();
    if let Some(built) = built {
        response.headers.extend(built);
    }
    response
}

/// Whether a value is shaped like a browser `Origin`: a scheme, `://`, and
/// a host with an optional port -- no whitespace, control character, path,
/// or separator that could smuggle a second header into the echo. The
/// allowlist decides which origins are permitted; this only decides what is
/// safe to compare and echo at all.
fn is_origin(value: &str) -> bool {
    if value.is_empty() || value.len() > MAXIMUM_ORIGIN_BYTES {
        return false;
    }
    let Some(authority) = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
    else {
        return false;
    };
    !authority.is_empty()
        && authority.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b':' | b'[' | b']')
        })
}

/// The longest origin the public contract accepts.
const MAXIMUM_ORIGIN_BYTES: usize = 262;

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    const APPLICATION: &str = "/v1/projects/prj_example00/environments/env_example00/auth/signin";
    const MANAGEMENT: &str = "/v1/projects/prj_example00/environments/env_example00/collections";
    const ORIGIN: &str = "https://app.example.com";

    /// One environment's allowlist, on its own application routes only --
    /// the shape every service's policy has.
    struct OneOrigin;

    impl CrossOriginPolicy for OneOrigin {
        fn allows_origin(&self, request: &HttpRequestHead, origin: &str) -> bool {
            request.path() == APPLICATION && origin == ORIGIN
        }
    }

    fn middleware() -> CrossOriginMiddleware {
        CrossOriginMiddleware::new(Arc::new(OneOrigin))
    }

    fn head(method: HttpMethod, path: &str, headers: &[(&str, &str)]) -> HttpRequestHead {
        let mut normalized = BTreeMap::<String, Vec<String>>::new();
        for (name, value) in headers {
            normalized
                .entry(name.to_ascii_lowercase())
                .or_default()
                .push((*value).to_owned());
        }
        HttpRequestHead {
            method,
            path: path.to_owned(),
            headers: normalized,
            request_id: "req_test_cors0001".to_owned(),
        }
    }

    fn header(response: &HttpResponse, name: &str) -> Option<String> {
        response
            .headers
            .iter()
            .find(|header| header.field.as_str().as_str().eq_ignore_ascii_case(name))
            .map(|header| header.value.as_str().to_owned())
    }

    fn cross_origin_headers(response: &HttpResponse) -> Vec<String> {
        response
            .headers
            .iter()
            .map(|header| header.field.as_str().as_str().to_ascii_lowercase())
            .filter(|name| name.starts_with("access-control-") || name == "vary")
            .collect()
    }

    /// A preflight from a listed origin is answered by the middleware
    /// itself, before routing, with exactly the four preflight headers and
    /// `Vary: Origin`.
    #[test]
    fn a_preflight_from_a_listed_origin_is_answered_with_the_cross_origin_headers() {
        let response = middleware()
            .before_dispatch(&head(
                HttpMethod::Options,
                APPLICATION,
                &[("Origin", ORIGIN)],
            ))
            .expect("the preflight is answered without routing");
        assert_eq!(response.status, 204);
        assert_eq!(
            header(&response, "access-control-allow-origin").as_deref(),
            Some(ORIGIN)
        );
        assert_eq!(
            header(&response, "access-control-allow-methods").as_deref(),
            Some(CORS_ALLOW_METHODS)
        );
        assert_eq!(
            header(&response, "access-control-allow-headers").as_deref(),
            Some(CORS_ALLOW_HEADERS)
        );
        assert_eq!(
            header(&response, "access-control-max-age").as_deref(),
            Some("600")
        );
        assert_eq!(header(&response, "vary").as_deref(), Some("Origin"));
        // The preflight names methods and request headers, never the
        // response headers a real answer exposes, and never credentials.
        assert_eq!(header(&response, "access-control-expose-headers"), None);
        assert_eq!(header(&response, "access-control-allow-credentials"), None);
    }

    /// A preflight the platform is not answering on a browser's behalf is
    /// routed exactly as it was before cross-origin access existed: an
    /// unlisted origin, no origin at all, and a route a browser application
    /// does not call are all left to the router.
    #[test]
    fn an_unanswered_preflight_is_routed_and_never_labelled() {
        let middleware = middleware();
        for (path, origin) in [
            (APPLICATION, Some("https://attacker.example")),
            (APPLICATION, Some("http://app.example.com")),
            (APPLICATION, Some("https://app.example.com/")),
            (APPLICATION, Some("null")),
            (APPLICATION, None),
            (MANAGEMENT, Some(ORIGIN)),
        ] {
            let headers = origin.map_or_else(Vec::new, |origin| vec![("Origin", origin)]);
            let request = head(HttpMethod::Options, path, &headers);
            assert!(
                middleware.before_dispatch(&request).is_none(),
                "{path} {origin:?} must route"
            );
            let response = middleware.after_dispatch(&request, HttpResponse::empty(405));
            assert!(
                cross_origin_headers(&response).is_empty(),
                "{path} {origin:?} must receive no cross-origin header"
            );
        }
    }

    /// Every other response to a listed origin is labelled so the browser
    /// hands it to the application -- a success and a failure alike, since
    /// an application that cannot read a `401` cannot react to it.
    #[test]
    fn responses_to_a_listed_origin_carry_the_echo_and_the_exposed_headers() {
        let request = head(HttpMethod::Post, APPLICATION, &[("Origin", ORIGIN)]);
        let middleware = middleware();
        assert!(
            middleware.before_dispatch(&request).is_none(),
            "a real request is routed, not answered here"
        );
        for response in [HttpResponse::empty(200), HttpResponse::empty(401)] {
            let labelled = middleware.after_dispatch(&request, response);
            assert_eq!(
                header(&labelled, "access-control-allow-origin").as_deref(),
                Some(ORIGIN)
            );
            assert_eq!(
                header(&labelled, "access-control-expose-headers").as_deref(),
                Some(CORS_EXPOSE_HEADERS)
            );
            assert_eq!(header(&labelled, "vary").as_deref(), Some("Origin"));
            // A real response never carries the preflight's answers.
            assert_eq!(header(&labelled, "access-control-allow-methods"), None);
            assert_eq!(header(&labelled, "access-control-max-age"), None);
        }
    }

    /// A management route is never answered cross-origin, even from an
    /// origin the same environment lists for its application API.
    #[test]
    fn a_route_the_policy_does_not_cover_is_never_labelled() {
        let middleware = middleware();
        for method in [HttpMethod::Get, HttpMethod::Post, HttpMethod::Options] {
            let request = head(method, MANAGEMENT, &[("Origin", ORIGIN)]);
            assert_eq!(middleware.decide(&request), CrossOriginDecision::NotAllowed);
            assert!(middleware.before_dispatch(&request).is_none());
            let response = middleware.after_dispatch(&request, HttpResponse::empty(200));
            assert!(cross_origin_headers(&response).is_empty(), "{method}");
        }
    }

    /// A repeated header is not a header: two `Origin` values decide
    /// nothing, because there is no single origin to echo.
    #[test]
    fn a_repeated_origin_is_refused() {
        assert_eq!(
            middleware().decide(&head(
                HttpMethod::Get,
                APPLICATION,
                &[("Origin", ORIGIN), ("Origin", "https://attacker.example")],
            )),
            CrossOriginDecision::NotAllowed
        );
    }

    /// An `Origin` that is not shaped like one never reaches the policy and
    /// is never echoed, so no header value can be smuggled into a response.
    #[test]
    fn only_an_origin_shaped_value_is_ever_compared() {
        for malformed in [
            "",
            "app.example.com",
            "ftp://app.example.com",
            "https://app.example.com\r\nX-Injected: 1",
            "https://app.example.com/path",
            "https://app.example.com?q=1",
            "https://app.example.com trailing",
            "https://",
        ] {
            assert!(!is_origin(malformed), "{malformed:?}");
        }
        for valid in [
            "https://app.example.com",
            "http://127.0.0.1:5173",
            "https://app.example.com:8443",
            "http://[::1]:5173",
        ] {
            assert!(is_origin(valid), "{valid:?}");
        }
    }

    /// The router applies the middleware exactly as the transport does: a
    /// preflight never reaches a handler, and a routed response comes back
    /// labelled -- including the transport's own refusals, which a browser
    /// would otherwise hide from the application.
    #[cfg(feature = "test-support")]
    #[test]
    fn the_router_answers_preflights_and_labels_routed_responses() {
        use crate::{HttpRequest, HttpRouter};

        let mut router = HttpRouter::new();
        router
            .add_route(HttpMethod::Post, APPLICATION, |_| {
                Ok(HttpResponse::empty(201))
            })
            .expect("route");
        router.set_middleware(Arc::new(middleware()));
        let request = |method, path: &str| {
            HttpRequest::for_test(
                method,
                path.to_owned(),
                [("Origin".to_owned(), ORIGIN.to_owned())],
                Vec::new(),
                None,
            )
        };
        let preflight = router.respond_for_test(request(HttpMethod::Options, APPLICATION));
        assert_eq!(preflight.status_for_test(), 204);
        assert_eq!(
            preflight.header_for_test("access-control-allow-origin"),
            Some(ORIGIN)
        );
        let routed = router.respond_for_test(request(HttpMethod::Post, APPLICATION));
        assert_eq!(routed.status_for_test(), 201);
        assert_eq!(
            routed.header_for_test("access-control-allow-origin"),
            Some(ORIGIN)
        );
        assert_eq!(
            routed.header_for_test("access-control-expose-headers"),
            Some(CORS_EXPOSE_HEADERS)
        );
        // A management path the policy does not cover is routed and
        // answered without a cross-origin header.
        let management = router.respond_for_test(request(HttpMethod::Post, MANAGEMENT));
        assert_eq!(management.status_for_test(), 404);
        assert_eq!(
            management.header_for_test("access-control-allow-origin"),
            None
        );
    }
}
