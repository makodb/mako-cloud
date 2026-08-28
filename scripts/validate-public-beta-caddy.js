import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const [
  openapi,
  caddy,
  groupVariables,
  runtimeTasks,
  playbook,
  previewSchema,
  previewGuard,
  previewService,
  previewTimer,
  previewContext,
] = await Promise.all([
  readFile(resolve(root, "api/openapi/mako-cloud-v1.yaml"), "utf8"),
  readFile(resolve(root, "infra/ansible/roles/runtime/templates/Caddyfile.j2"), "utf8"),
  readFile(resolve(root, "infra/ansible/group_vars/public_beta.yml"), "utf8"),
  readFile(resolve(root, "infra/ansible/roles/runtime/tasks/main.yml"), "utf8"),
  readFile(resolve(root, "infra/ansible/playbooks/public-beta.yml"), "utf8"),
  readFile(resolve(root, "infra/public-beta/public-preview-approval.schema.json"), "utf8"),
  readFile(
    resolve(root, "infra/ansible/roles/runtime/files/mako-public-preview-admission"),
    "utf8",
  ),
  readFile(
    resolve(root, "infra/ansible/roles/runtime/files/mako-public-preview-admission.service"),
    "utf8",
  ),
  readFile(
    resolve(root, "infra/ansible/roles/runtime/files/mako-public-preview-admission.timer"),
    "utf8",
  ),
  readFile(
    resolve(root, "infra/ansible/roles/runtime/templates/mako-public-preview-context.json.j2"),
    "utf8",
  ),
]);

const assert = (condition, message) => {
  if (!condition) throw new Error(message);
};

const matcher = (name) => {
  const match = caddy.match(new RegExp(`^\\t@${name} path_regexp ${name} (.+)$`, "m"));
  assert(match?.[1], `missing ${name} route matcher`);
  return new RegExp(match[1]);
};

const data = matcher("data_api");
const control = matcher("control_api");
const operatorControl = matcher("operator_control_api");
const developerWorkspace = matcher("developer_workspace_api");
const serviceCredential = matcher("service_credential_api");
const edge = matcher("edge_function");
const operatorAuth = matcher("operator_auth");
const openapiPaths = [...openapi.matchAll(/^ {2}(\/[^:]+):$/gm)].map((match) => match[1]);
assert(openapiPaths.length > 70, "unexpectedly small OpenAPI route inventory");
assert(operatorAuth.test("/v1/operator-auth/sessions"), "operator auth no-store matcher is absent");
assert(
  operatorAuth.test("/v1/operator-auth/sessions/current/actions/verify-password"),
  "operator password verification is outside the no-store matcher",
);

const samplePath = (path) =>
  path
    .replaceAll("{teamId}", "org_example0001")
    .replaceAll("{invitationId}", "inv_example0001")
    .replaceAll("{developerIdentityId}", "dev_example0001")
    .replaceAll("{automationTokenId}", "aut_example0001")
    .replaceAll("{projectId}", "prj_example0001")
    .replaceAll("{environmentId}", "env_example0001")
    .replaceAll("{collectionId}", "documents")
    .replaceAll("{migrationId}", "mig_example0001")
    .replaceAll("{indexName}", "by_name")
    .replaceAll("{indexVersion}", "1")
    .replaceAll("{policyVersion}", "1")
    .replaceAll("{userId}", "usr_example0001")
    .replaceAll("{sessionId}", "ses_example0001")
    .replaceAll("{credentialId}", "crd_example0001")
    .replaceAll("{secretName}", "api-key")
    .replaceAll("{functionName}", "health")
    .replaceAll("{functionVersion}", "1")
    .replaceAll("{projectRef}", "prj_example0001")
    .replaceAll("{provisioningWorkflowId}", "prv_example0001")
    .replaceAll("{supportSessionId}", "sup_example0001");

for (const template of openapiPaths) {
  const path = samplePath(template);
  if (template.includes("/service/")) {
    assert(serviceCredential.test(path), `${template} is not blocked as service-credential API`);
    continue;
  }
  assert(!serviceCredential.test(path), `${template} entered the service-credential deny matcher`);
  const matches = [
    data.test(path),
    control.test(path),
    operatorControl.test(path),
    developerWorkspace.test(path),
    edge.test(path),
  ];
  assert(
    matches.filter(Boolean).length === 1,
    `${template} is not owned by exactly one Caddy upstream`,
  );
}

for (const path of [
  "/_internal/v1/identity/verify",
  "/readyz",
  "/healthz",
  "/v1/projects/prj_example0001/environments/env_example0001/service/collections/documents/doc_example0001",
  "/v1/projects/prj_example0001/not-documented",
  "/prj_example0001/functions/v1/health/private",
]) {
  assert(
    serviceCredential.test(path) ||
      (!data.test(path) &&
        !control.test(path) &&
        !operatorControl.test(path) &&
        !developerWorkspace.test(path) &&
        !edge.test(path)),
    `${path} escaped the route allowlist`,
  );
}

for (const required of [
  "route {",
  "respond @internal_rpc 404",
  "respond @service_credential_api 404",
  "respond @outside_qualification_sources 403",
  "max_size 32MB",
  "X-Content-Type-Options nosniff",
  "X-Frame-Options DENY",
  "-Strict-Transport-Security",
  "flush_interval -1",
  "respond 404",
]) {
  assert(caddy.includes(required), `Caddy configuration omits ${required}`);
}
const orderedRouteDirectives = [
  "\troute {",
  "respond @internal_rpc 404",
  "respond @service_credential_api 404",
  "reverse_proxy @replication_stream",
  "reverse_proxy @data_api",
  "reverse_proxy @control_api",
  "reverse_proxy @operator_control_api",
  "reverse_proxy @developer_workspace_api",
  "reverse_proxy @edge_function",
  "\trespond 404",
];
let previousRouteDirective = -1;
for (const directive of orderedRouteDirectives) {
  const index = caddy.indexOf(directive);
  assert(index > previousRouteDirective, `Caddy route order is unsafe at ${directive}`);
  previousRouteDirective = index;
}
assert(!caddy.includes("http://{{ mako_public_fqdn }}"), "plaintext application site exists");
assert(caddy.includes("127.0.0.1:8080"), "data-plane upstream is absent");
assert(caddy.includes("127.0.0.1:8081"), "control-plane upstream is absent");
assert(caddy.includes("127.0.0.1:8082"), "edge-gateway upstream is absent");
assert(
  groupVariables.includes("mako_public_admission_mode: disabled"),
  "default admission is not disabled",
);
assert(groupVariables.includes("mako_caddy_enabled: false"), "Caddy is enabled before admission");
assert(
  runtimeTasks.includes("APPROVE_PUBLIC_BETA:"),
  "approved-beta mode lacks a digest-bound approval",
);
for (const mode of ["disabled", "pre_gate", "risk_accepted_preview", "approved_beta"]) {
  assert(runtimeTasks.includes(mode), `runtime deployment omits ${mode} admission`);
}
assert(
  runtimeTasks.includes("Caddyfile.{{ item }}") &&
    runtimeTasks.includes("mako_render_admission_mode"),
  "deployment does not render separate admission configurations",
);
assert(
  runtimeTasks.includes("mako-public-preview-admission") &&
    playbook.includes("mako-public-preview-admission") &&
    playbook.includes("activate"),
  "preview activation does not pass through the admission guard",
);
const parsedPreviewSchema = JSON.parse(previewSchema);
assert(parsedPreviewSchema.additionalProperties === false, "preview approval schema is open");
for (const binding of [
  "operatorIdentity",
  "planHash",
  "releaseDigest",
  "blockerDigest",
  "persistence",
  "expiresAt",
]) {
  assert(parsedPreviewSchema.required.includes(binding), `preview approval omits ${binding}`);
}
for (const required of [
  "ACCEPT_PERSISTENT_PUBLIC_PREVIEW_RISK:",
  'choices=("verify", "activate", "guard", "pause")',
  '"manual operator pause"',
  'select_mode(paths, "pre_gate"',
  "mako_storage_backup_remote_verified",
  "mako_storage_backup_last_success_unixtime_seconds",
  "ssl.create_default_context()",
  'systemctl", "restart", "caddy.service',
]) {
  assert(previewGuard.includes(required), `preview guard omits ${required}`);
}
assert(
  previewGuard.includes("if changed and not no_reload:"),
  "preview guard restarts Caddy when admission mode is unchanged",
);
assert(previewService.includes("ProtectSystem=strict"), "preview guard is not hardened");
assert(
  previewService.includes("Wants=network-online.target caddy.service") &&
    !previewService.includes("Requires=caddy.service"),
  "preview guard hard-depends on the Caddy process it must restart",
);
assert(
  previewService.includes("ReadWritePaths=/etc/caddy /var/lib/mako-public-preview"),
  "preview guard write access is unbounded",
);
assert(
  previewTimer.includes("OnActiveSec=1s") &&
    previewTimer.includes("OnUnitActiveSec=1min") &&
    previewTimer.includes("AccuracySec=1s") &&
    previewTimer.includes("Persistent=true") &&
    !previewTimer.includes("OnBootSec="),
  "preview guard lacks a restart-safe one-minute schedule",
);
assert(
  runtimeTasks.includes("Restart public-preview admission timer"),
  "a changed preview timer is not restarted after systemd reload",
);
for (const safeguard of [
  "trustedHttpsAndHsts",
  "exactPublicRouteAllowlist",
  "applicationSecurity",
  "backupAndRecovery",
  "zeroAcknowledgedWriteLoss",
  "zeroIntegrityFailures",
  "serviceReadiness",
  "emergencyAdmissionStop",
]) {
  assert(previewContext.includes(safeguard), `preview context omits ${safeguard}`);
}
assert(
  runtimeTasks.includes("mako_acme_environment == 'production'"),
  "HSTS is not bound to production TLS",
);
assert(
  playbook.includes("mako_caddy_enabled | bool"),
  "playbook does not converge the explicit Caddy gate",
);

console.log(
  `validated ${openapiPaths.length} exact public API paths, four admission modes, preview guard, and fail-closed Caddy defaults`,
);
