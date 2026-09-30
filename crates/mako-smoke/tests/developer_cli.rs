//! Prove the developer CLI completes a console workflow against real services.
//!
//! The unit tests in `packages/cli/test` drive every command against a mock
//! of the management API; this test boots the local data and control planes,
//! mints a developer session the way deployment tooling does, and runs the
//! built `mako-cloud` binary through the flow a developer follows in the console:
//! see what they own, create a project and wait for it, shape a collection,
//! activate a policy, issue a key, and read what the platform recorded.
//!
//! It skips with a notice when `packages/cli/dist` has not been built, so the
//! Rust suites do not depend on an npm build they did not run.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use mako_smoke::{
    await_readiness, binary_directory, free_ports, mint_developer_session, run_bootstrap,
    scratch_root, service_environment, start_service,
};
use serde_json::Value;

const DEVELOPER_ID: &str = "dev_localboot";
const DEVELOPER_EMAIL: &str = "developer@local.test";

/// The built CLI entry point, or `None` when the npm workspace was not built.
fn cli_entry() -> Option<PathBuf> {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root");
    let entry = workspace.join("packages/cli/dist/main.js");
    entry.is_file().then_some(entry)
}

struct Cli {
    entry: PathBuf,
    environment: BTreeMap<String, String>,
}

struct Run {
    status: i32,
    stdout: String,
    stderr: String,
}

impl Cli {
    fn run(&self, args: &[&str]) -> Run {
        let output = Command::new("node")
            .arg(&self.entry)
            .args(args)
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .envs(&self.environment)
            .output()
            .expect("node runs the CLI");
        Run {
            status: output.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }
    }

    /// Runs a command with `--json` and parses the single document it prints.
    fn json(&self, args: &[&str]) -> Value {
        let mut full: Vec<&str> = args.to_vec();
        full.push("--json");
        let run = self.run(&full);
        assert_eq!(
            run.status,
            0,
            "mako-cloud {} failed: {}{}",
            args.join(" "),
            run.stdout,
            run.stderr
        );
        serde_json::from_str(&run.stdout).unwrap_or_else(|error| {
            panic!(
                "mako-cloud {} did not print one JSON document ({error}): {}",
                args.join(" "),
                run.stdout
            )
        })
    }
}

#[test]
fn a_developer_completes_a_console_workflow_from_the_terminal() {
    let Some(entry) = cli_entry() else {
        eprintln!("skipping: packages/cli/dist is not built (npm run build -w @mako-cloud/cli)");
        return;
    };
    let binaries = binary_directory();
    let workspace = tempfile::Builder::new()
        .prefix("mako-cli-")
        .tempdir_in(scratch_root())
        .expect("smoke workspace");
    let root = workspace.path();
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

    // The session arrives the way CI supplies a credential: from the
    // environment, never touching the credential store.
    let config_dir = root.join("cli-config");
    let cli = Cli {
        entry,
        environment: BTreeMap::from([
            ("MAKO_TOKEN".to_owned(), session),
            ("MAKO_TOKEN_KIND".to_owned(), "developer_session".to_owned()),
            (
                "MAKO_ENDPOINT".to_owned(),
                format!("http://127.0.0.1:{control_port}"),
            ),
            (
                "MAKO_CONFIG_DIR".to_owned(),
                config_dir.to_string_lossy().into_owned(),
            ),
            ("MAKO_WAIT_INTERVAL_MS".to_owned(), "250".to_owned()),
        ]),
    };

    let status = cli.json(&["auth", "status"]);
    assert_eq!(status["credential"], "environment (MAKO_TOKEN)");
    assert!(
        !config_dir.join("credentials.json").exists(),
        "an environment token must never be written to the store"
    );

    let whoami = cli.json(&["auth", "whoami"]);
    assert_eq!(whoami["credential"], "MAKO_TOKEN");

    // The rest of the workflow is exercised once the command groups exist;
    // each step below asserts the same outcome the console would show.
    run_console_workflow(&cli);
}

fn run_console_workflow(cli: &Cli) {
    let teams = cli.json(&["teams", "list"]);
    assert!(
        teams.is_array(),
        "teams list prints the team array: {teams}"
    );

    // A project in the personal space and an environment in it, each waited
    // to active: both provision asynchronously behind a 202.
    let project = cli.json(&[
        "projects",
        "create",
        "CLI Smoke",
        "--region",
        "local",
        "--wait",
    ]);
    let project_id = project["id"].as_str().expect("project id").to_owned();
    assert_eq!(project["state"], "active", "{project}");
    assert!(project["teamId"].as_str().unwrap_or("").starts_with("org_"));
    let environment = cli.json(&[
        "envs",
        "create",
        "production",
        "--project",
        &project_id,
        "--wait",
    ]);
    let environment_id = environment["id"]
        .as_str()
        .expect("environment id")
        .to_owned();
    assert_eq!(environment["state"], "active", "{environment}");
    // The project also has its Development environment, and the list is in
    // id order, so the new one is looked for rather than expected first.
    let listed = cli.json(&["envs", "list", "--project", &project_id]);
    assert!(
        listed.as_array().is_some_and(|environments| environments
            .iter()
            .any(|listed| listed["id"] == environment_id)),
        "{listed}"
    );
    let tenant = ["--project", &project_id, "--env", &environment_id];

    // Shape a collection: the schema is published with the collection.
    let schema = r#"{"type":"object","primaryKey":"id","required":["id","ownerId","title","updatedAt"],"properties":{"id":{"type":"string"},"ownerId":{"type":"string"},"title":{"type":"string"},"updatedAt":{"type":"integer"}},"additionalProperties":true}"#;
    let created = cli.json(&scoped(
        &[
            "collections",
            "create",
            "notes",
            "--schema",
            schema,
            "--schema-version",
            "1",
        ],
        &tenant,
    ));
    assert_eq!(created["id"], "notes", "{created}");
    let fetched = cli.json(&scoped(&["collections", "get", "notes"], &tenant));
    assert_eq!(fetched["id"], "notes", "{fetched}");

    // A policy is drafted, validated, and activated with explicit confirmation.
    let policy = r#"{"version":1,"rules":[{"id":"owner-full-access","effect":"allow","operations":["create","read","update","delete"],"expression":"true"}]}"#;
    let draft = cli.json(&scoped(
        &["policies", "draft", "notes", "--input", policy],
        &tenant,
    ));
    assert!(draft.is_object(), "{draft}");
    let validation = cli.json(&scoped(&["policies", "validate", "notes", "1"], &tenant));
    assert!(validation.is_object(), "{validation}");
    let refused = cli.run(&scoped(&["policies", "activate", "notes", "1"], &tenant));
    assert_eq!(
        refused.status, 2,
        "activation without --yes is refused: {}",
        refused.stderr
    );
    let active = cli.json(&scoped(
        &["policies", "activate", "notes", "1", "--yes"],
        &tenant,
    ));
    assert_eq!(active["policy"]["state"], "active", "{active}");

    // Keys: the signing key is initialized, a public key is issued once, and
    // the secret is not repeated anywhere else.
    let signing = cli.json(&scoped(&["keys", "signing", "init"], &tenant));
    assert!(signing.is_object(), "{signing}");
    let key = cli.json(&scoped(
        &["keys", "public", "create", "--id", "key_cli01"],
        &tenant,
    ));
    let secret = key["secret"]
        .as_str()
        .expect("the secret is the json field");
    assert!(secret.len() >= 16, "{key}");
    let shown = cli.run(&scoped(&["keys", "get", "key_cli01"], &tenant));
    assert_eq!(shown.status, 0, "{}", shown.stderr);
    assert!(
        !shown.stdout.contains(secret),
        "a stored key never reveals its secret"
    );

    // What the platform recorded is readable from the terminal. Audit events
    // are served by the control plane; logs, usage, and health come from the
    // telemetry-query service, which this stack does not start, so those
    // commands are proven against the mock in `packages/cli/test`.
    let activity = cli.json(&scoped(&["activity"], &tenant));
    assert!(activity["items"].is_array(), "{activity}");
    let summary = cli.json(&scoped(&["workspace", "summary"], &tenant));
    assert!(summary["sections"].is_object(), "{summary}");
}

/// Appends the tenant scope to a command.
fn scoped<'a>(args: &[&'a str], tenant: &[&'a str]) -> Vec<&'a str> {
    let mut full = args.to_vec();
    full.extend_from_slice(tenant);
    full
}
