//! Prove applications sign users in through an external provider and by
//! magic link, under settings the developer manages.
//!
//! A developer registers an OpenID Connect provider and a redirect
//! allowlist; an application starts a sign-in, the "browser" follows the
//! provider round trip, the callback lands the user on the registered
//! redirect with a one-time code, and the application exchanges it for a
//! session. The same subject signs in again as the same user. A redirect
//! outside the allowlist and a forged state are refused. A magic link is
//! mailed through the relay, redeemed once, and refused the second time.
//! Updating the settings without resending the secret keeps it.
use std::collections::BTreeMap;
use std::time::Duration;

use mako_smoke::{
    OidcProviderStub, SmtpCaptureStub, await_readiness, binary_directory, free_ports,
    mint_developer_session, request, run_bootstrap, scratch_root, service_environment,
    start_service, try_request_full,
};
use serde_json::{Value, json};

const DEVELOPER_ID: &str = "dev_localboot";
const DEVELOPER_EMAIL: &str = "developer@local.test";
const APP_REDIRECT: &str = "http://localhost:3000/after-sign-in";

fn await_active(control_port: u16, headers: &BTreeMap<String, String>, path: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(90);
    loop {
        let (status, body) = request(control_port, "GET", path, headers, None);
        assert_eq!(status, 200, "reading {path} failed: {body}");
        let record: Value = serde_json::from_str(&body).expect("lifecycle json");
        match record["state"].as_str() {
            Some("active") => return,
            Some("provisioning") => {}
            other => panic!("{path} reached {other:?}: {body}"),
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{path} did not become active"
        );
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn query_parameter(url: &str, name: &str) -> Option<String> {
    let (_, query) = url.split_once('?')?;
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| percent_decode(value))
    })
}

fn fragment_parameter(url: &str, name: &str) -> Option<String> {
    let (_, fragment) = url.split_once('#')?;
    fragment.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then(|| percent_decode(value))
    })
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(byte) = u8::from_str_radix(&value[index + 1..index + 3], 16)
        {
            out.push(byte);
            index += 3;
        } else if bytes[index] == b'+' {
            out.push(b' ');
            index += 1;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[test]
fn applications_sign_users_in_through_providers_and_magic_links() {
    let binaries = binary_directory();
    let workspace = tempfile::Builder::new()
        .prefix("mako-signin-")
        .tempdir_in(scratch_root())
        .expect("smoke workspace");
    let root = workspace.path();
    let provider = OidcProviderStub::start("smoke-client", "subject-7", "mia@app.test");
    let relay = SmtpCaptureStub::start();
    let environment = service_environment(root);
    run_bootstrap(&binaries, &environment);
    let [data_port, control_port] = free_ports::<2>();
    let mut data_environment = environment.clone();
    data_environment.insert(
        "MAKO_BIND_ADDR".to_owned(),
        format!("127.0.0.1:{data_port}"),
    );
    let _data = start_service(
        "mako-data-plane",
        &binaries,
        &data_environment,
        data_port,
        root.join("data-plane.log"),
    );
    let mut control_environment = environment.clone();
    control_environment.insert(
        "MAKO_DATA_PLANE_ENDPOINT".to_owned(),
        format!("127.0.0.1:{data_port}"),
    );
    // Application mail leaves through the same relay as developer mail; a
    // plaintext loopback relay needs no credentials.
    for (name, value) in [
        ("MAKO_DEVELOPER_REGISTRATION_ENABLED", "true"),
        ("MAKO_DEVELOPER_SMTP_RELAY_HOSTNAME", "127.0.0.1"),
        ("MAKO_DEVELOPER_SMTP_PORT", &relay.port.to_string()),
        ("MAKO_DEVELOPER_SMTP_TLS_MODE", "plaintext"),
        (
            "MAKO_DEVELOPER_SMTP_SENDER",
            "Mako Smoke <no-reply@smoke.local>",
        ),
        (
            "MAKO_DEVELOPER_MAIL_ENCRYPTION_SECRET_REF",
            "env:MAKO_SMOKE_MAIL_KEY",
        ),
        (
            "MAKO_SMOKE_MAIL_KEY",
            "smoke-developer-mail-encryption-secret",
        ),
    ] {
        control_environment.insert(name.to_owned(), value.to_owned());
    }
    let _control = start_service(
        "mako-control-plane",
        &binaries,
        &control_environment,
        control_port,
        root.join("control-plane.log"),
    );
    await_readiness(data_port, "mako-data-plane");
    await_readiness(control_port, "mako-control-plane");
    let session =
        mint_developer_session(&binaries, root, control_port, DEVELOPER_ID, DEVELOPER_EMAIL);
    let bearer = BTreeMap::from([("authorization".to_owned(), format!("Bearer {session}"))]);
    let manage = |key: &str| {
        let mut headers = bearer.clone();
        headers.insert("idempotency-key".to_owned(), format!("sign-in-smoke-{key}"));
        headers
    };

    // --- A project, an environment, and a public key. -----------------------
    let (status, body) = request(
        control_port,
        "POST",
        "/v1/projects",
        &manage("project"),
        Some(&json!({ "name": "Sign In", "region": "local" })),
    );
    assert!(
        (200..300).contains(&status),
        "project creation failed: {body}"
    );
    let project: Value = serde_json::from_str(&body).expect("project json");
    let project_id = project["id"].as_str().expect("project id").to_owned();
    await_active(control_port, &bearer, &format!("/v1/projects/{project_id}"));
    let (status, body) = request(
        control_port,
        "POST",
        &format!("/v1/projects/{project_id}/environments"),
        &manage("environment"),
        Some(&json!({ "name": "production" })),
    );
    assert!(
        (200..300).contains(&status),
        "environment creation failed: {body}"
    );
    let environment_record: Value = serde_json::from_str(&body).expect("environment json");
    let environment_id = environment_record["id"]
        .as_str()
        .expect("environment id")
        .to_owned();
    let scope = format!("/v1/projects/{project_id}/environments/{environment_id}");
    await_active(control_port, &bearer, &scope);
    let (status, body) = request(
        control_port,
        "POST",
        &format!("{scope}/signing-keys/actions/initialize"),
        &manage("signing"),
        None,
    );
    assert!(
        (200..300).contains(&status),
        "signing key init failed: {body}"
    );
    let (status, body) = request(
        control_port,
        "POST",
        &format!("{scope}/credentials/public"),
        &manage("public-key"),
        Some(&json!({ "id": "key_signin0001" })),
    );
    assert!((200..300).contains(&status), "public key failed: {body}");
    let issued: Value = serde_json::from_str(&body).expect("key json");
    let public_key = issued["value"].as_str().expect("key value").to_owned();
    let keyed = BTreeMap::from([("x-mako-key".to_owned(), public_key)]);

    // --- Before any settings exist, the provider is unknown. ---------------
    let (status, body) = request(
        data_port,
        "POST",
        &format!("{scope}/auth/providers/stub/start"),
        &keyed,
        Some(&json!({ "redirectUrl": APP_REDIRECT })),
    );
    assert!(
        (400..500).contains(&status),
        "an unconfigured provider must be refused: {status} {body}"
    );

    // --- The developer registers the provider, redirects and magic links. --
    let settings_update = |secret: Option<&str>| {
        let mut entry = json!({
            "name": "stub",
            "kind": { "type": "oidc", "issuer": provider.endpoint },
            "clientId": provider.client_id,
            "scopes": ["openid", "email", "profile"],
            "enabled": true,
        });
        if let Some(secret) = secret {
            entry["clientSecret"] = json!(secret);
        }
        json!({
            "providers": [entry],
            "redirectUrls": [APP_REDIRECT],
            "magicLinks": { "enabled": true, "linkTtlSeconds": 900 },
        })
    };
    let (status, body) = request(
        control_port,
        "PUT",
        &format!("{scope}/auth-settings"),
        &manage("settings-1"),
        Some(&settings_update(Some("smoke-client-secret"))),
    );
    assert_eq!(status, 200, "settings update failed: {body}");
    let view: Value = serde_json::from_str(&body).expect("settings json");
    assert_eq!(view["providers"][0]["name"], "stub");
    assert_eq!(view["providers"][0]["hasSecret"], true);
    assert!(
        !body.contains("smoke-client-secret"),
        "the client secret must never be returned: {body}"
    );
    let (status, body) = request(
        control_port,
        "GET",
        &format!("{scope}/auth-settings"),
        &bearer,
        None,
    );
    assert_eq!(status, 200, "settings read failed: {body}");
    assert!(!body.contains("smoke-client-secret"));
    let read: Value = serde_json::from_str(&body).expect("settings json");
    assert_eq!(read["redirectUrls"], json!([APP_REDIRECT]));
    assert_eq!(read["magicLinks"]["enabled"], true);

    // --- An application signs a user in through the provider. --------------
    let sign_in_through_provider = || -> Value {
        let (status, body) = request(
            data_port,
            "POST",
            &format!("{scope}/auth/providers/stub/start"),
            &keyed,
            Some(&json!({ "redirectUrl": APP_REDIRECT })),
        );
        assert_eq!(status, 200, "sign-in start failed: {body}");
        let started: Value = serde_json::from_str(&body).expect("start json");
        let authorization_url = started["authorizationUrl"]
            .as_str()
            .expect("authorization url")
            .to_owned();
        assert!(
            authorization_url.starts_with(&format!("{}/authorize?", provider.endpoint)),
            "the browser is sent to the provider: {authorization_url}"
        );
        let state = query_parameter(&authorization_url, "state").expect("state");
        let nonce = query_parameter(&authorization_url, "nonce").expect("nonce");
        assert_eq!(
            query_parameter(&authorization_url, "client_id").as_deref(),
            Some(provider.client_id.as_str())
        );
        let callback = query_parameter(&authorization_url, "redirect_uri").expect("redirect uri");
        assert!(
            callback.ends_with(&format!("{scope}/auth/providers/stub/callback")),
            "the provider sends the browser back to the platform: {callback}"
        );
        // The browser comes back from the provider with a code and the state.
        provider.expect_nonce(&nonce);
        let (status, headers, body) = try_request_full(
            data_port,
            "GET",
            &format!(
                "{scope}/auth/providers/stub/callback?code=provider-code-1&state={}",
                state.replace('+', "%2B")
            ),
            &BTreeMap::new(),
            None,
        )
        .expect("callback completes");
        assert_eq!(status, 302, "callback did not redirect: {body}");
        let location = headers.get("location").expect("redirect location");
        assert!(
            location.starts_with(&format!("{APP_REDIRECT}#")),
            "the user lands on the registered redirect: {location}"
        );
        let code = fragment_parameter(location, "code").expect("one-time code");
        let (status, body) = request(
            data_port,
            "POST",
            &format!("{scope}/auth/providers/exchange"),
            &keyed,
            Some(&json!({ "code": code })),
        );
        assert_eq!(status, 200, "code exchange failed: {body}");
        let session: Value = serde_json::from_str(&body).expect("session json");
        assert!(
            session["accessToken"]
                .as_str()
                .is_some_and(|t| !t.is_empty())
        );
        assert_eq!(session["user"]["email"], provider.email);
        // The code was one-time.
        let (status, _) = request(
            data_port,
            "POST",
            &format!("{scope}/auth/providers/exchange"),
            &keyed,
            Some(&json!({ "code": code })),
        );
        assert!(
            (400..500).contains(&status),
            "a spent code must be refused: {status}"
        );
        session
    };
    let first = sign_in_through_provider();
    let second = sign_in_through_provider();
    assert_eq!(
        first["user"]["id"], second["user"]["id"],
        "the same provider subject is the same application user"
    );
    let token_requests = provider.token_requests();
    assert_eq!(token_requests.len(), 2, "one token exchange per sign-in");
    assert!(
        token_requests[0].contains("provider-code-1"),
        "the provider's code reaches its token endpoint: {}",
        token_requests[0]
    );

    // --- A redirect outside the allowlist and a forged state are refused. --
    let (status, body) = request(
        data_port,
        "POST",
        &format!("{scope}/auth/providers/stub/start"),
        &keyed,
        Some(&json!({ "redirectUrl": "https://elsewhere.example/steal" })),
    );
    assert_eq!(status, 400, "unregistered redirect: {body}");
    let (status, headers, _) = try_request_full(
        data_port,
        "GET",
        &format!("{scope}/auth/providers/stub/callback?code=provider-code-x&state=forged"),
        &BTreeMap::new(),
        None,
    )
    .expect("callback completes");
    assert!(
        (400..500).contains(&status) || (status == 302 && headers["location"].contains("error=")),
        "a forged state must not sign anyone in: {status} {headers:?}"
    );

    // --- A magic link is mailed, redeemed once, and refused the second time.
    let (status, body) = request(
        data_port,
        "POST",
        &format!("{scope}/auth/magic-link"),
        &keyed,
        Some(&json!({ "email": "lee@app.test", "redirectUrl": APP_REDIRECT })),
    );
    assert_eq!(status, 202, "magic link request failed: {body}");
    let mail = relay
        .wait_for("lee@app.test", Duration::from_secs(120))
        .unwrap_or_else(|| {
            panic!(
                "the magic link mail never reached the relay; captured: {:?}",
                relay.messages()
            )
        });
    let text = mail.text();
    let link = text
        .split_whitespace()
        .find(|word| word.contains("#magic_link_token="))
        .unwrap_or_else(|| panic!("the mail carries the link: {text}"));
    assert!(
        link.starts_with(APP_REDIRECT),
        "the link lands on the app: {link}"
    );
    let token = fragment_parameter(link, "magic_link_token").expect("magic link token");
    let (status, body) = request(
        data_port,
        "POST",
        &format!("{scope}/auth/magic-link/redeem"),
        &keyed,
        Some(&json!({ "token": token })),
    );
    assert_eq!(status, 200, "magic link redemption failed: {body}");
    let session: Value = serde_json::from_str(&body).expect("session json");
    assert_eq!(session["user"]["email"], "lee@app.test");
    let (status, _) = request(
        data_port,
        "POST",
        &format!("{scope}/auth/magic-link/redeem"),
        &keyed,
        Some(&json!({ "token": token })),
    );
    assert!(
        (400..500).contains(&status),
        "a redeemed link must be refused: {status}"
    );
    // An unknown address is accepted identically and mails nothing.
    let (status, _) = request(
        data_port,
        "POST",
        &format!("{scope}/auth/magic-link"),
        &keyed,
        Some(&json!({ "email": "not-an-email", "redirectUrl": APP_REDIRECT })),
    );
    assert_eq!(status, 202);

    // --- Updating the settings without the secret keeps it. ----------------
    let (status, body) = request(
        control_port,
        "PUT",
        &format!("{scope}/auth-settings"),
        &manage("settings-2"),
        Some(&settings_update(None)),
    );
    assert_eq!(status, 200, "settings update without secret failed: {body}");
    let view: Value = serde_json::from_str(&body).expect("settings json");
    assert_eq!(view["providers"][0]["hasSecret"], true);
    let third = sign_in_through_provider();
    assert_eq!(first["user"]["id"], third["user"]["id"]);

    // A new provider without any secret is refused.
    let mut without_secret = settings_update(None);
    without_secret["providers"][0]["name"] = json!("other");
    let (status, body) = request(
        control_port,
        "PUT",
        &format!("{scope}/auth-settings"),
        &manage("settings-3"),
        Some(&without_secret),
    );
    assert_eq!(status, 400, "a new provider needs a secret: {body}");

    // --- With verification on, a sign-up confirms its address first. -------
    let (status, body) = request(
        data_port,
        "POST",
        &format!("{scope}/auth/signup"),
        &keyed,
        Some(&json!({ "email": "ana@app.test", "password": "correct horse battery" })),
    );
    assert_eq!(status, 202, "sign-up without verification: {body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).expect("signup json")["verificationRequired"],
        false
    );
    let mut verifying = settings_update(None);
    verifying["emailVerification"] = json!({ "required": true });
    let (status, body) = request(
        control_port,
        "PUT",
        &format!("{scope}/auth-settings"),
        &manage("settings-4"),
        Some(&verifying),
    );
    assert_eq!(status, 200, "turning verification on failed: {body}");
    let view: Value = serde_json::from_str(&body).expect("settings json");
    assert_eq!(view["emailVerification"]["required"], true);
    let sign_up = |redirect: Option<&str>| {
        let mut body = json!({ "email": "kai@app.test", "password": "correct horse battery" });
        if let Some(redirect) = redirect {
            body["redirectUrl"] = json!(redirect);
        }
        request(
            data_port,
            "POST",
            &format!("{scope}/auth/signup"),
            &keyed,
            Some(&body),
        )
    };
    let (status, body) = sign_up(None);
    assert_eq!(status, 400, "a verifying sign-up needs a redirect: {body}");
    let (status, body) = sign_up(Some("https://elsewhere.example/steal"));
    assert_eq!(status, 400, "an unregistered redirect is refused: {body}");
    let (status, body) = sign_up(Some(APP_REDIRECT));
    assert_eq!(status, 202, "verifying sign-up failed: {body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).expect("signup json")["verificationRequired"],
        true
    );
    let sign_in = || {
        request(
            data_port,
            "POST",
            &format!("{scope}/auth/signin"),
            &keyed,
            Some(&json!({ "email": "kai@app.test", "password": "correct horse battery" })),
        )
        .0
    };
    assert_eq!(sign_in(), 401, "an unverified account cannot sign in");
    let mail = relay
        .wait_for("kai@app.test", Duration::from_secs(120))
        .unwrap_or_else(|| {
            panic!(
                "the verification mail never reached the relay; captured: {:?}",
                relay.messages()
            )
        });
    let text = mail.text();
    let link = text
        .split_whitespace()
        .find(|word| word.contains("#verification_token="))
        .unwrap_or_else(|| panic!("the mail carries the link: {text}"));
    assert!(
        link.starts_with(APP_REDIRECT),
        "the link lands on the app: {link}"
    );
    let token = fragment_parameter(link, "verification_token").expect("verification token");
    let verify = || {
        request(
            data_port,
            "POST",
            &format!("{scope}/auth/verify-email"),
            &keyed,
            Some(&json!({ "token": token })),
        )
    };
    let (status, body) = verify();
    assert_eq!(status, 200, "verification failed: {body}");
    assert_eq!(sign_in(), 200, "a verified account signs in");
    assert_eq!(verify().0, 401, "a spent link is refused");
    // Signing up again with the address mails nothing and answers the same.
    let (status, _) = sign_up(Some(APP_REDIRECT));
    assert_eq!(status, 202);
}
