//! Exercise the console's grant and document flow through the production Caddy
//! template and real services. Requires Caddy and Python with Jinja2; override
//! their paths with MAKO_SMOKE_CADDY and MAKO_SMOKE_PYTHON.

use std::{
    collections::BTreeMap,
    fs,
    net::TcpStream,
    process::{Child, Command, Stdio},
    thread::sleep,
    time::{Duration, Instant},
};

use mako_smoke::{
    await_readiness, binary_directory, free_ports, mint_developer_session, request, run_bootstrap,
    scratch_root, service_environment, start_service,
};
use serde_json::{Value, json};

struct Proxy(Child);
impl Drop for Proxy {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn console_explorer_works_through_the_public_proxy() {
    let binaries = binary_directory();
    let workspace = tempfile::Builder::new()
        .prefix("mako-explorer-proxy-")
        .tempdir_in(scratch_root())
        .unwrap();
    let root = workspace.path();
    let mut environment = service_environment(root);
    run_bootstrap(&binaries, &environment);
    let [data_port, control_port, proxy_port, custom_port] = free_ports::<4>();
    environment.insert(
        "MAKO_DATA_PLANE_ENDPOINT".into(),
        format!("127.0.0.1:{data_port}"),
    );
    let _data = start_service(
        "mako-data-plane",
        &binaries,
        &environment,
        data_port,
        root.join("data.log"),
    );
    let _control = start_service(
        "mako-control-plane",
        &binaries,
        &environment,
        control_port,
        root.join("control.log"),
    );
    await_readiness(data_port, "mako-data-plane");
    await_readiness(control_port, "mako-control-plane");

    let template = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../infra/ansible/roles/runtime/templates/Caddyfile.j2");
    let config = root.join("Caddyfile");
    // Render the complete template. Only TLS, listening addresses, and upstream
    // ports change for loopback; route allowlists and handlers stay intact.
    let rendered =
        Command::new(std::env::var("MAKO_SMOKE_PYTHON").unwrap_or_else(|_| "python3".into()))
            .arg("-c")
            .arg(
                r#"
import sys
from pathlib import Path
from jinja2 import Environment, StrictUndefined
template, output, data, control, platform, custom = sys.argv[1:]
env = Environment(undefined=StrictUndefined)
env.filters['bool'] = bool
result = env.from_string(Path(template).read_text()).render(
    mako_acme_contact='', mako_acme_environment='production',
    mako_public_fqdn='http://127.0.0.1:' + platform,
    mako_public_admission_mode='risk_accepted_preview', mako_hsts_enabled=False)
result = result.replace('admin off', 'admin off\n\tauto_https off')
result = result.replace('\nhttps:// {', '\nhttp://127.0.0.1:' + custom + ' {')
result = result.replace('\ttls {\n\t\ton_demand\n\t}', '')
result = result.replace('127.0.0.1:8080', '127.0.0.1:' + data)
result = result.replace('127.0.0.1:8081', '127.0.0.1:' + control)
Path(output).write_text(result)
"#,
            )
            .arg(&template)
            .arg(&config)
            .arg(data_port.to_string())
            .arg(control_port.to_string())
            .arg(proxy_port.to_string())
            .arg(custom_port.to_string())
            .output()
            .expect("Python with Jinja2 must be installed");
    assert!(
        rendered.status.success(),
        "render Caddy: {}",
        String::from_utf8_lossy(&rendered.stderr)
    );
    let log_path = root.join("caddy.log");
    let log = fs::File::create(&log_path).unwrap();
    let mut proxy = Proxy(
        Command::new(std::env::var("MAKO_SMOKE_CADDY").unwrap_or_else(|_| "caddy".into()))
            .args(["run", "--config"])
            .arg(&config)
            .args(["--adapter", "caddyfile"])
            .env("XDG_DATA_HOME", root.join("caddy-data"))
            .env("XDG_CONFIG_HOME", root.join("caddy-config"))
            .stdout(Stdio::from(log.try_clone().unwrap()))
            .stderr(Stdio::from(log))
            .spawn()
            .expect("Caddy must be installed"),
    );
    let deadline = Instant::now() + Duration::from_secs(10);
    while TcpStream::connect(("127.0.0.1", proxy_port)).is_err() {
        assert!(
            proxy.0.try_wait().unwrap().is_none() && Instant::now() < deadline,
            "Caddy did not start: {}",
            fs::read_to_string(&log_path).unwrap()
        );
        sleep(Duration::from_millis(50));
    }

    let developer = mint_developer_session(
        &binaries,
        root,
        control_port,
        "dev_localboot",
        "developer@local.test",
    );
    let authenticated = BTreeMap::from([("authorization".into(), format!("Bearer {developer}"))]);
    for team_id in [None, Some("org_localboot")] {
        exercise_project(proxy_port, custom_port, &authenticated, team_id);
    }
}

fn exercise_project(
    proxy_port: u16,
    custom_port: u16,
    authenticated: &BTreeMap<String, String>,
    team_id: Option<&str>,
) {
    let kind = if team_id.is_some() {
        "team"
    } else {
        "personal"
    };
    let create = |path: &str, body: Value, key: &str| {
        let mut headers = authenticated.clone();
        headers.insert(
            "idempotency-key".into(),
            format!("proxy-smoke-{kind}-{key}"),
        );
        let (status, response) = request(proxy_port, "POST", path, &headers, Some(&body));
        assert!(
            (200..300).contains(&status),
            "create {path}: {status} {response}"
        );
        serde_json::from_str::<Value>(&response).unwrap()
    };
    let project = create(
        "/v1/projects",
        json!({"name": format!("Proxy {kind}"), "region": "local", "teamId": team_id}),
        "project",
    );
    let project_id = project["id"].as_str().unwrap();
    let project_path = format!("/v1/projects/{project_id}");
    let environment = create(
        &format!("{project_path}/environments"),
        json!({"name": "production"}),
        "environment",
    );
    let environment_id = environment["id"].as_str().unwrap();
    let scope = format!("{project_path}/environments/{environment_id}");
    for path in [&project_path, &scope] {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let (status, body) = request(proxy_port, "GET", path, authenticated, None);
            assert_eq!(status, 200, "read provisioning state: {body}");
            if serde_json::from_str::<Value>(&body).unwrap()["state"] == "active" {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "provisioning did not finish: {body}"
            );
            sleep(Duration::from_millis(100));
        }
    }
    create(
        &format!("{scope}/collections"),
        json!({
            "id": "todos", "schemaVersion": 1, "primaryKey": {"kind": "field", "field": "id"},
            "jsonSchema": {"type": "object", "required": ["id", "title"], "properties": {
                "id": {"type": "string"}, "title": {"type": "string"}, "ownerId": {"type": "string"}, "updatedAt": {"type": "integer"}
            }, "additionalProperties": true}
        }),
        "collection",
    );
    let base = format!("{scope}/explorer");
    let collection = format!("{base}/collections/todos");
    let grant_request = json!({
        "tenant": {"projectId": project_id, "environmentId": environment_id},
        "collectionId": "todos", "mode": "administrative",
        "operations": ["get", "browse", "query", "plan", "history", "simulate", "mutate"],
        "applicationUserId": null, "reason": "Browse and manage documents in the cloud console", "durationSeconds": 300
    });
    assert_eq!(
        request(
            proxy_port,
            "POST",
            &format!("{base}/grants"),
            &BTreeMap::new(),
            Some(&grant_request)
        )
        .0,
        401
    );
    let (status, body) = request(
        proxy_port,
        "POST",
        &format!("{base}/grants"),
        authenticated,
        Some(&grant_request),
    );
    assert_eq!(status, 201, "grant failed: {body}");
    let grant: Value = serde_json::from_str(&body).unwrap();
    let mut capable = BTreeMap::from([
        (
            "x-mako-explorer-capability".into(),
            grant["capability"].as_str().unwrap().to_owned(),
        ),
        // The platform proxy must strip a forged custom-domain assertion.
        (
            "x-mako-custom-domain".into(),
            "unverified.local.test".into(),
        ),
    ]);
    let page_request = json!({"limit": 25, "cursor": null, "includeRetainedTombstones": false});
    let (status, body) = request(
        proxy_port,
        "POST",
        &format!("{collection}/browse"),
        &capable,
        Some(&page_request),
    );
    assert_eq!(status, 200, "browse failed: {body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["items"],
        json!([])
    );

    let mutation = json!({"kind": "create", "documentId": "proxy-smoke-todo", "expectedRevision": null,
        "schemaVersion": 1, "idempotencyKey": "explorer-proxy-smoke-create",
        "content": {"id": "proxy-smoke-todo", "ownerId": "smoke-owner", "title": "browse through Caddy", "updatedAt": 1786752000000_i64}});
    capable.insert(
        "idempotency-key".into(),
        "explorer-proxy-smoke-create".into(),
    );
    for (operation, field) in [("simulate", "allowed"), ("mutate", "committed")] {
        let (status, body) = request(
            proxy_port,
            "POST",
            &format!("{collection}/{operation}"),
            &capable,
            Some(&mutation),
        );
        assert_eq!(status, 200, "{operation} failed: {body}");
        assert_eq!(
            serde_json::from_str::<Value>(&body).unwrap()[field],
            true,
            "{body}"
        );
    }
    let (status, body) = request(
        proxy_port,
        "POST",
        &format!("{collection}/browse"),
        &capable,
        Some(&page_request),
    );
    assert_eq!(status, 200, "browse populated collection: {body}");
    assert!(
        body.contains("browse through Caddy"),
        "created document absent: {body}"
    );
    for suffix in [
        "documents/proxy-smoke-todo",
        "documents/proxy-smoke-todo/history",
    ] {
        let (status, body) = request(
            proxy_port,
            "GET",
            &format!("{collection}/{suffix}"),
            &capable,
            None,
        );
        assert_eq!(status, 200, "{suffix} failed: {body}");
    }
    let query = json!({"predicates": [], "sort": [], "limit": 25, "cursor": null});
    for suffix in ["query/plan", "query"] {
        let (status, body) = request(
            proxy_port,
            "POST",
            &format!("{collection}/{suffix}"),
            &capable,
            Some(&query),
        );
        assert_eq!(status, 200, "{suffix} failed: {body}");
    }

    // Owning the developer session is insufficient without the scoped grant.
    for headers in [&BTreeMap::new(), authenticated] {
        assert_eq!(
            request(
                proxy_port,
                "POST",
                &format!("{collection}/browse"),
                headers,
                Some(&page_request)
            )
            .0,
            401
        );
    }
    for path in [
        collection.replace(project_id, "prj_otherproject"),
        collection.replace("todos", "othercollection"),
    ] {
        let status = request(
            proxy_port,
            "POST",
            &format!("{path}/browse"),
            &capable,
            Some(&page_request),
        )
        .0;
        assert!(
            [401, 403, 404].contains(&status),
            "grant escaped its scope: {status}"
        );
    }
    for (method, path, body) in [
        ("POST", format!("{base}/grants"), Some(&grant_request)),
        ("POST", format!("{collection}/browse"), Some(&page_request)),
        (
            "GET",
            format!("{collection}/documents/proxy-smoke-todo"),
            None,
        ),
    ] {
        assert_eq!(
            request(custom_port, method, &path, &capable, body).0,
            404,
            "management API exposed on a custom domain"
        );
    }
    assert_eq!(
        request(
            proxy_port,
            "GET",
            &format!("{collection}/browse"),
            &capable,
            None
        )
        .0,
        405
    );
    assert_eq!(
        request(
            proxy_port,
            "POST",
            &format!("{collection}/browse/private"),
            &capable,
            Some(&page_request)
        )
        .0,
        404
    );
    let (status, body) = request(
        proxy_port,
        "DELETE",
        &format!("{base}/grants/{}", grant["grantId"].as_str().unwrap()),
        authenticated,
        None,
    );
    assert_eq!(status, 200, "revoke failed: {body}");
    assert_eq!(
        request(
            proxy_port,
            "POST",
            &format!("{collection}/browse"),
            &capable,
            Some(&page_request)
        )
        .0,
        401
    );
}
