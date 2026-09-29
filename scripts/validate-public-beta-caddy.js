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

const rawMatcher = (name) => {
  const match = caddy.match(new RegExp(`^\\t@${name} path_regexp ${name} (.+)$`, "m"));
  assert(match?.[1], `missing ${name} route matcher`);
  return match[1];
};
const matcher = (name) => new RegExp(rawMatcher(name));

const data = matcher("data_api");
const control = matcher("control_api");
const operatorControl = matcher("operator_control_api");
const developerWorkspace = matcher("developer_workspace_api");
const explorerData = matcher("explorer_data_api");
const serviceCredential = matcher("service_credential_api");
const edge = matcher("edge_function");
const operatorAuth = matcher("operator_auth");
const openapiPaths = [...openapi.matchAll(/^ {2}(\/[^:]+):$/gm)].map((match) => match[1]);
assert(openapiPaths.length > 70, "unexpectedly small OpenAPI route inventory");
const consoleRoute = matcher("console_route");
const userBook = matcher("user_book");
assert(userBook.test("/docs/user-book"), "the public User Book route is not served");
assert(!userBook.test("/docs/user-book/private"), "the User Book route matches subpaths");
for (const path of [
  "/usage-and-plan",
  "/projects/prj_example0001/billing",
  "/projects/prj_example0001/domains",
  "/projects/prj_example0001/environments/env_example0001/api-docs",
  "/projects/prj_example0001/settings",
]) {
  assert(consoleRoute.test(path), `console client route ${path} is not served`);
}
assert(
  !consoleRoute.test("/projects/prj_example0001/environments/env_example0001/domains"),
  "domains is a project-level console route only",
);
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
    .replaceAll("{provider}", "google")
    .replaceAll("{templateKind}", "verification")
    .replaceAll("{bucketId}", "avatars")
    .replaceAll("{objectPath}", "users/42/me.png")
    .replaceAll("{webhookId}", "whk_abcdefghijklmnop")
    .replaceAll("{deliveryId}", "whd_abcdefghijklmnop")
    .replaceAll("{scheduleId}", "sch_abcdefghijklmnop")
    .replaceAll("{domainId}", "dom_abcdefghijklmnop")
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
    explorerData.test(path),
    edge.test(path),
  ];
  assert(
    matches.filter(Boolean).length === 1,
    `${template} is not owned by exactly one Caddy upstream`,
  );
  if (template.includes("/explorer/collections/")) {
    assert(explorerData.test(path), `${template} must reach the data plane`);
  } else if (template.includes("/explorer/grants")) {
    assert(developerWorkspace.test(path), `${template} must reach the control plane`);
  } else {
    assert(!explorerData.test(path), `${template} entered the explorer data matcher`);
  }
}

for (const [name, port] of [
  ["explorer_data_api", 8080],
  ["developer_workspace_api", 8081],
]) {
  assert(
    caddy.includes(`reverse_proxy @${name} 127.0.0.1:${port} {`),
    `${name} targets the wrong service`,
  );
}

for (const path of [
  "/_internal/v1/identity/verify",
  "/readyz",
  "/healthz",
  "/v1/projects/prj_example0001/environments/env_example0001/service/collections/documents/doc_example0001",
  "/v1/projects/prj_example0001/not-documented",
  "/v1/projects/prj_example0001/bill/private",
  "/v1/projects/prj_example0001/environments/env_example0001/explorer/collections/documents/browse/private",
  "/v1/projects/prj_example0001/environments/env_example0001/explorer/collections//browse",
  "/v1/projects/prj_example0001/environments/env_example0001/explorer/collections/documents/query/plan/private",
]) {
  assert(
    serviceCredential.test(path) ||
      (!data.test(path) &&
        !control.test(path) &&
        !operatorControl.test(path) &&
        !developerWorkspace.test(path) &&
        !explorerData.test(path) &&
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
  "reverse_proxy @explorer_data_api",
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

// Custom domains: a catch-all HTTPS site whose certificates are issued on
// demand only for hostnames the control plane has verified, serving nothing
// but the application data-plane routes and function invocations.
const customDomainSiteStart = caddy.indexOf("\nhttps:// {");
assert(customDomainSiteStart > 0, "custom-domain catch-all site is absent");
assert(
  customDomainSiteStart > caddy.indexOf("{{ mako_public_fqdn }} {"),
  "custom-domain site must follow the platform hostname site",
);
const platformSite = caddy.slice(caddy.indexOf("{{ mako_public_fqdn }} {"), customDomainSiteStart);
const customDomainSite = caddy.slice(customDomainSiteStart);
assert(
  caddy.includes("on_demand_tls {") &&
    caddy.includes("ask http://127.0.0.1:8081/_internal/v1/custom-domains/ask"),
  "on-demand TLS is not gated by the control plane's ask endpoint",
);
assert(
  caddy.indexOf("on_demand_tls {") < caddy.indexOf("{{ mako_public_fqdn }} {"),
  "on_demand_tls must be a global option",
);
assert(
  /\ttls \{\n\t\ton_demand\n\t\}/u.test(customDomainSite),
  "custom-domain site does not use on-demand TLS",
);
assert(
  !platformSite.includes("on_demand"),
  "the platform hostname must never issue certificates on demand",
);
const customDomainMatcher = (name) => {
  const match = customDomainSite.match(new RegExp(`^\\t@${name} path_regexp ${name} (.+)$`, "m"));
  assert(match?.[1], `missing ${name} custom-domain route matcher`);
  return match[1];
};
assert(
  customDomainMatcher("custom_domain_data_api") === rawMatcher("data_api"),
  "custom-domain data API inventory differs from the platform hostname's",
);
assert(
  customDomainMatcher("custom_domain_replication_stream") === rawMatcher("replication_stream"),
  "custom-domain replication stream matcher differs from the platform hostname's",
);
const customDomainFunction = new RegExp(customDomainMatcher("custom_domain_function"));
assert(
  customDomainFunction.test("/functions/v1/health") &&
    !customDomainFunction.test("/prj_example0001/functions/v1/health") &&
    !customDomainFunction.test("/functions/v1/") &&
    !edge.test("/functions/v1/health"),
  "custom-domain function shape is not exactly /functions/v1/{name}",
);
// A function owns the path under its name: the gateway parses it and the
// runtime hands it to the function, so a function with more than one route is
// only reachable if the allowlist admits it. Requiring the bare name made
// every REST-shaped function unreachable in production.
assert(
  edge.test("/prj_example0001/functions/v1/health/private") &&
    customDomainFunction.test("/functions/v1/health/private") &&
    !edge.test("/prj_example0001/functions/v1/") &&
    !edge.test("/prj_example0001/functions/v1"),
  "a function's own paths are not admitted",
);
for (const template of openapiPaths) {
  const path = samplePath(template);
  const served =
    customDomainFunction.test(path) || (data.test(path) && !serviceCredential.test(path));
  assert(
    served === (data.test(path) && !template.includes("/service/")),
    `${template} custom-domain exposure differs from the data-plane inventory`,
  );
}
for (const path of [
  "/",
  "/login",
  "/projects/prj_example0001/domains",
  "/projects/prj_example0001/environments/env_example0001/api-docs",
  "/assets/index-abc123.js",
  "/v1/projects",
  "/v1/projects/prj_example0001/domains",
  "/v1/operator/overview",
  "/v1/developer-auth/sessions",
  "/v1/projects/prj_example0001/environments/env_example0001/explorer/grants",
  "/v1/projects/prj_example0001/environments/env_example0001/explorer/collections/documents/browse",
  "/v1/projects/prj_example0001/environments/env_example0001/explorer/collections/documents/documents/doc_example0001",
  "/prj_example0001/functions/v1/health",
  "/_internal/v1/custom-domains/ask",
  "/v1/projects/prj_example0001/environments/env_example0001/service/collections/documents/query",
]) {
  assert(
    !customDomainFunction.test(path) && !(data.test(path) && !serviceCredential.test(path)),
    `${path} would be served on a custom domain`,
  );
}
// Every asset kind the console's build emits must be served, or the browser
// silently does without it: the typeface went missing this way once.
{
  const consoleAssets = /@console_asset path_regexp console_asset \S*\(\?:([a-z0-9|]+)\)\$/.exec(
    caddy,
  );
  assert(consoleAssets !== null, "the console asset route is missing");
  const served = new Set(consoleAssets[1].split("|"));
  for (const extension of ["css", "js", "woff2", "png"]) {
    assert(served.has(extension), `the console asset route does not serve .${extension}`);
  }
}

for (const forbidden of [
  "127.0.0.1:8081",
  "@control_api",
  "@operator_control_api",
  "@developer_workspace_api",
  "@explorer_data_api",
  "@console_route",
  "@console_workspace_route",
  "@console_asset",
  "@user_book",
  "file_server",
  "/opt/mako/current/console",
  "@edge_function",
  'Strict-Transport-Security "max-age',
]) {
  assert(!customDomainSite.includes(forbidden), `custom-domain site contains ${forbidden}`);
}
for (const required of [
  "respond @internal_rpc 404",
  "respond @service_credential_api 404",
  "respond 503",
  "respond @outside_qualification_sources 403",
  "max_size 32MB",
  "X-Frame-Options DENY",
  "-Strict-Transport-Security",
  "reverse_proxy @custom_domain_replication_stream 127.0.0.1:8080",
  "reverse_proxy @custom_domain_data_api 127.0.0.1:8080",
  "reverse_proxy @custom_domain_function 127.0.0.1:8082",
  "\trespond 404\n",
]) {
  assert(customDomainSite.includes(required), `custom-domain site omits ${required}`);
}
assert(
  (customDomainSite.match(/header_up X-Mako-Custom-Domain \{http\.request\.host\}/gu) ?? [])
    .length === 3,
  "every custom-domain upstream must learn the hostname",
);
assert(
  !platformSite.includes("header_up X-Mako-Custom-Domain {"),
  "the platform hostname must not assert a custom domain",
);
// Cross-origin access belongs to the services, which answer it only for an
// origin the domain's allowlist names. A header directive here would label a
// response the platform decided not to label, and on the platform hostname it
// would label one that must never carry a cross-origin header at all.
for (const [name, site] of [
  ["platform hostname", platformSite],
  ["custom-domain", customDomainSite],
]) {
  assert(!/access-control-/iu.test(site), `${name} site emits its own Access-Control header`);
}
// Preflights have to reach the services to be answered from the environment's
// allowlist, and they arrive on both names: every application route on either
// site is matched by path alone, so OPTIONS is forwarded exactly like the
// request it precedes.
for (const [name, site] of [
  ["platform hostname", platformSite],
  ["custom-domain", customDomainSite],
]) {
  assert(
    !/^\t*(?:@\w+ )?(?:not )?method\s/mu.test(site),
    `${name} site filters by method, which would drop OPTIONS preflights`,
  );
}
assert(
  (platformSite.match(/header_up -X-Mako-Custom-Domain/gu) ?? []).length ===
    (platformSite.match(/reverse_proxy @/gu) ?? []).length,
  "every platform-hostname upstream must strip a client-supplied X-Mako-Custom-Domain",
);
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
  `validated ${openapiPaths.length} exact public API paths, four admission modes, preview guard, the custom-domain site (methods unfiltered, no proxy-owned CORS), and fail-closed Caddy defaults`,
);
