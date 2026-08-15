//! Plans and applies protected operator entitlements through the loopback API.

#![forbid(unsafe_code)]

use std::{
    collections::BTreeMap,
    env,
    fs::File,
    io::Read,
    net::SocketAddr,
    os::unix::fs::MetadataExt as _,
    path::{Path, PathBuf},
    process::ExitCode,
};

use mako_api::{EnvironmentId, ProjectId, TenantScope};
use mako_internal_rpc::{
    DeploymentKey, InternalCaller, InternalHttpClient, InternalHttpClientConfig, InternalRoute,
    OPERATOR_ADMIN_ENVIRONMENT_ID, OPERATOR_ADMIN_PROJECT_ID, OperatorEntitlementApplyResponse,
    OperatorEntitlementCommand, OperatorEntitlementPlanResponse,
};

fn main() -> ExitCode {
    match run(env::args().skip(1)) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("operator administration failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: impl IntoIterator<Item = String>) -> Result<String, String> {
    let options = parse_options(arguments)?;
    let mode = required(&options, "mode")?;
    if !matches!(mode, "plan" | "apply") {
        return Err("--mode must be plan or apply".to_owned());
    }
    let input_path = required_path(&options, "input")?;
    let secret_path = required_path(&options, "secret-file")?;
    let endpoint = required(&options, "endpoint")?
        .parse::<SocketAddr>()
        .map_err(|_| "--endpoint must be an IP address and port".to_owned())?;
    if !endpoint.ip().is_loopback() {
        return Err("--endpoint must be loopback".to_owned());
    }
    let input = read_private(&input_path, 64 * 1024, "input")?;
    let mut command: OperatorEntitlementCommand =
        serde_json::from_slice(&input).map_err(|_| "protected input is invalid".to_owned())?;
    let plan_path = options.get("plan-file").map(PathBuf::from);
    if mode == "plan" {
        if command.typed_confirmation.is_some() || plan_path.is_some() {
            return Err("plan input must omit typedConfirmation and --plan-file".to_owned());
        }
    } else if let Some(plan_path) = plan_path {
        if !plan_path.is_absolute() || command.typed_confirmation.is_some() {
            return Err(
                "--plan-file must be absolute and apply input must omit typedConfirmation"
                    .to_owned(),
            );
        }
        let plan_bytes = read_private(&plan_path, 64 * 1024, "plan")?;
        let plan: OperatorEntitlementPlanResponse = serde_json::from_slice(&plan_bytes)
            .map_err(|_| "protected plan is invalid".to_owned())?;
        if plan.environment_binding != command.environment_binding {
            return Err("protected plan environment does not match apply input".to_owned());
        }
        command.typed_confirmation = Some(plan.typed_confirmation);
    } else if command.typed_confirmation.is_none() {
        return Err("apply input must contain typedConfirmation or use --plan-file".to_owned());
    }
    let secret = read_private(&secret_path, 64 * 1024, "secret")?;
    let secret = std::str::from_utf8(&secret)
        .map_err(|_| "secret file is invalid".to_owned())?
        .trim_end_matches(['\r', '\n']);
    let key = DeploymentKey::derive(secret).map_err(|_| "secret file is invalid".to_owned())?;
    let client = InternalHttpClient::new(
        InternalHttpClientConfig::loopback(endpoint),
        key,
        InternalCaller::OperatorAdmin,
    )
    .map_err(|_| "operator administration client configuration is invalid".to_owned())?;
    let tenant = TenantScope::new(
        ProjectId::parse(OPERATOR_ADMIN_PROJECT_ID)
            .map_err(|_| "operator administration scope is invalid".to_owned())?,
        EnvironmentId::parse(OPERATOR_ADMIN_ENVIRONMENT_ID)
            .map_err(|_| "operator administration scope is invalid".to_owned())?,
    );
    let route = if mode == "apply" {
        InternalRoute::OperatorEntitlementApply
    } else {
        InternalRoute::OperatorEntitlementPlan
    };
    let request_id = operator_admin_request_id(mode);
    let response = client
        .call(
            route,
            &tenant,
            &request_id,
            &command.idempotency_key,
            &command,
        )
        .map_err(|error| format!("loopback operator administration request failed: {error}"))?;
    if mode == "apply" {
        let value: OperatorEntitlementApplyResponse = serde_json::from_slice(&response.body)
            .map_err(|_| "operator administration response is invalid".to_owned())?;
        serde_json::to_string_pretty(&value)
            .map_err(|_| "operator administration response is invalid".to_owned())
    } else {
        let value: OperatorEntitlementPlanResponse = serde_json::from_slice(&response.body)
            .map_err(|_| "operator administration response is invalid".to_owned())?;
        serde_json::to_string_pretty(&value)
            .map_err(|_| "operator administration response is invalid".to_owned())
    }
}

fn operator_admin_request_id(mode: &str) -> String {
    format!("req_operator_admin_{mode}")
}

fn parse_options(
    arguments: impl IntoIterator<Item = String>,
) -> Result<BTreeMap<String, String>, String> {
    let arguments = arguments.into_iter().collect::<Vec<_>>();
    let mut options = BTreeMap::new();
    let mut chunks = arguments.chunks_exact(2);
    for pair in &mut chunks {
        let name = pair[0]
            .strip_prefix("--")
            .ok_or_else(|| "options must use --name value pairs".to_owned())?;
        if name.is_empty() || options.insert(name.to_owned(), pair[1].clone()).is_some() {
            return Err("options must be unique --name value pairs".to_owned());
        }
    }
    if !chunks.remainder().is_empty() {
        return Err("options must use --name value pairs".to_owned());
    }
    let expected = ["endpoint", "input", "mode", "plan-file", "secret-file"];
    if options.keys().any(|key| !expected.contains(&key.as_str())) {
        return Err("an unsupported option was provided".to_owned());
    }
    Ok(options)
}

fn required<'a>(options: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str, String> {
    options
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| format!("--{name} is required"))
}

fn required_path(options: &BTreeMap<String, String>, name: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(required(options, name)?);
    if !path.is_absolute() {
        return Err(format!("--{name} must be an absolute path"));
    }
    Ok(path)
}

fn read_private(path: &Path, maximum_bytes: usize, label: &str) -> Result<Vec<u8>, String> {
    let mut file = File::open(path).map_err(|_| format!("{label} file could not be opened"))?;
    let metadata = file
        .metadata()
        .map_err(|_| format!("{label} file metadata is unavailable"))?;
    if !metadata.is_file() || metadata.mode() & 0o777 != 0o600 {
        return Err(format!("{label} file must be regular and mode 0600"));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(u64::try_from(maximum_bytes.saturating_add(1)).unwrap_or(u64::MAX))
        .read_to_end(&mut bytes)
        .map_err(|_| format!("{label} file could not be read"))?;
    if bytes.is_empty() || bytes.len() > maximum_bytes {
        return Err(format!("{label} file size is invalid"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::{operator_admin_request_id, parse_options};

    #[test]
    fn private_client_uses_runtime_accepted_correlation_ids() {
        for mode in ["plan", "apply"] {
            let request_id = operator_admin_request_id(mode);
            assert!(request_id.starts_with("req_"));
            assert!((8..=128).contains(&request_id.len()));
            assert!(
                request_id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            );
        }
    }

    #[test]
    fn protected_apply_can_carry_the_plan_without_human_copy_back() {
        let options = parse_options([
            "--endpoint".to_owned(),
            "127.0.0.1:8081".to_owned(),
            "--input".to_owned(),
            "/run/mako/request.json".to_owned(),
            "--mode".to_owned(),
            "apply".to_owned(),
            "--plan-file".to_owned(),
            "/run/mako/plan.json".to_owned(),
            "--secret-file".to_owned(),
            "/etc/mako/internal-auth".to_owned(),
        ])
        .expect("plan file option");
        assert_eq!(
            options.get("plan-file").map(String::as_str),
            Some("/run/mako/plan.json")
        );
    }
}
