#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import { resolve } from "node:path";

import { assertValidPlan, canonicalJson } from "./proxmox/public-beta-plan-lib.js";
import { buildTeardownPlan } from "./proxmox/public-beta-teardown-lib.js";

const root = resolve(import.meta.dirname, "..");
const read = (path) => readFile(resolve(root, path), "utf8");
const readBuiltConsole = async () => {
  try {
    return await read("apps/console/web-dist/index.html");
  } catch {
    throw new Error(
      "apps/console/web-dist is missing. It is generated, not committed. " +
        "Run: npm run build --workspace @mako-cloud/console",
    );
  }
};
const json = async (path) => JSON.parse(await read(path));
const digestPattern = /^[0-9a-f]{64}$/u;

const [
  schema,
  plan,
  request,
  metaData,
  networkData,
  userData,
  guestFirewall,
  hostFirewall,
  emergencyFirewall,
  serviceUnit,
  backupUnit,
  backupOrchestratorUnit,
  backupOrchestrator,
  healthUnit,
  healthCollector,
  groupVariables,
  releaseManifest,
  deploymentEvidence,
  admissionEvidence,
  observabilityEvidence,
  teardownEvidence,
  consoleIndex,
  configurationTasks,
  runtimeTasks,
  serviceConfigTemplate,
  caddyTemplate,
  alertRules,
  secretGenerator,
  secretRedaction,
  alertmanagerTemplate,
  observabilityTasks,
  alertmanagerHardening,
  releaseOperation,
] = await Promise.all([
  json("infra/proxmox/public-beta/plan.schema.json"),
  json("docs/evidence/public-beta-preflight-plan.json"),
  json("infra/proxmox/public-beta/request.json"),
  read("infra/proxmox/public-beta/cloud-init/meta-data.yaml.tmpl"),
  read("infra/proxmox/public-beta/cloud-init/network-data.yaml.tmpl"),
  read("infra/proxmox/public-beta/cloud-init/user-data.yaml.tmpl"),
  read("infra/ansible/roles/firewall/templates/nftables.conf.j2"),
  read("infra/proxmox/public-beta/firewall/mako-vm124.nft"),
  read("infra/proxmox/public-beta/firewall/mako-vm124-admission-stop.nft"),
  read("infra/ansible/roles/runtime/templates/mako-service.service.j2"),
  read("infra/ansible/roles/backup/files/mako-checkpoint-backup@.service"),
  read("infra/ansible/roles/backup/files/mako-checkpoint-backup-orchestrator@.service"),
  read("infra/ansible/roles/backup/files/mako-checkpoint-backup-orchestrate"),
  read("infra/ansible/roles/observability/files/mako-health-collect.service"),
  read("infra/ansible/roles/observability/templates/mako-health-collect"),
  read("infra/ansible/group_vars/public_beta.yml"),
  json("docs/evidence/public-beta-release-manifest.json"),
  json("docs/evidence/public-beta-current-deployment.json"),
  json("docs/evidence/public-beta-admission-stop.json"),
  json("docs/evidence/public-beta-observability.json"),
  json("docs/evidence/public-beta-teardown-plan.json"),
  readBuiltConsole(),
  read("infra/ansible/roles/configuration/tasks/main.yml"),
  read("infra/ansible/roles/runtime/tasks/main.yml"),
  read("infra/ansible/roles/configuration/templates/service-config.json.j2"),
  read("infra/ansible/roles/runtime/templates/Caddyfile.j2"),
  read("infra/local/prometheus-rules/mako-cloud-alerts.yaml"),
  read("scripts/generate-public-beta-secrets.sh"),
  read("scripts/verify-public-beta-secret-redaction.sh"),
  read("infra/ansible/roles/observability/templates/alertmanager.yml.j2"),
  read("infra/ansible/roles/observability/tasks/main.yml"),
  read("infra/ansible/roles/observability/files/prometheus-alertmanager-hardening.conf"),
  read("infra/ansible/roles/runtime/files/mako-release-operation"),
]);

const consoleBundlePath = consoleIndex.match(/src="(\/assets\/[^"/]+\.js)"/u)?.[1];
assert(consoleBundlePath !== undefined, "built console omits its JavaScript bundle");
const consoleBundle = await read(`apps/console/web-dist${consoleBundlePath}`);

validateSchema(schema);
assertValidPlan(plan);
assert(plan.identity.fqdn === request.identity?.fqdn, "request and retained plan FQDN differ");
assert(
  plan.identity.ipv4Address === request.identity?.ipv4Address,
  "request and retained plan address differ",
);
assert(
  plan.identity.guestType === request.identity?.guestType,
  "request and retained guest types differ",
);
assert(
  plan.resources.vcpus === request.resources?.vcpus,
  "request and retained vCPU counts differ",
);
assert(
  plan.resources.memoryMiB === request.resources?.memoryMiB,
  "request and retained memory differ",
);
assert(plan.checks.conflictFree === true, "retained provisioning plan is not conflict-free");
validateCloudInit({ metaData, networkData, userData });
validateFirewalls({ guestFirewall, hostFirewall, emergencyFirewall });
validateUnits({ serviceUnit, backupUnit, backupOrchestratorUnit, healthUnit });
validateMixedStorageReleaseOperation(releaseOperation, runtimeTasks);
assert(
  backupOrchestrator.includes('lock_file="$' + '{status_root}/checkpoint-transfer.lock"') &&
    backupOrchestrator.includes("/usr/bin/flock --exclusive --wait 240 9") &&
    backupOrchestrator.indexOf("/usr/bin/flock --exclusive --wait 240 9") <
      backupOrchestrator.indexOf('/usr/bin/systemctl stop "$application"'),
  "component checkpoint transfers are not serialized before service shutdown",
);
assert(
  healthCollector.includes("mako_certificate_expiry_unixtime_seconds"),
  "health collector omits the public certificate expiry metric",
);
assert(
  healthCollector.includes("-connect 127.0.0.1:443"),
  "certificate expiry probe is not loopback-only",
);
validateReleaseManifest(releaseManifest, plan.planHash);
validateHostedConsole(consoleIndex, consoleBundle);
validateDeveloperRegistrationInfrastructure({
  groupVariables,
  configurationTasks,
  guestFirewall,
  serviceUnit,
  serviceConfigTemplate,
  caddyTemplate,
  alertRules,
  secretGenerator,
  secretRedaction,
  releaseManifest,
});
validateOperatorAlertInfrastructure({
  groupVariables,
  guestFirewall,
  alertRules,
  alertmanagerTemplate,
  observabilityTasks,
  alertmanagerHardening,
});
validateCustomDomainInfrastructure({ caddyTemplate, guestFirewall, serviceUnit });
validateEvidence({
  plan,
  releaseManifest,
  deploymentEvidence,
  admissionEvidence,
  observabilityEvidence,
  teardownEvidence,
  groupVariables,
});

console.log(
  "validated offline public-beta plan schema, cloud-init, firewall, systemd, release manifest, and retained evidence",
);

function validateSchema(value) {
  assert(
    value.$schema === "https://json-schema.org/draft/2020-12/schema",
    "wrong plan schema draft",
  );
  assert(value.type === "object" && value.additionalProperties === false, "plan schema is open");
  const required = new Set(value.required);
  for (const field of [
    "schemaVersion",
    "generatedAt",
    "mode",
    "identity",
    "proxmox",
    "resources",
    "image",
    "network",
    "storage",
    "security",
    "checks",
    "changes",
    "planHash",
  ]) {
    assert(required.has(field), `plan schema does not require ${field}`);
  }
  assert(value.$defs?.sha256?.pattern === "^[0-9a-f]{64}$", "schema SHA-256 format is not exact");
  for (const field of [
    "identity",
    "proxmox",
    "resources",
    "image",
    "network",
    "security",
    "checks",
    "changes",
  ]) {
    assert(
      value.properties[field]?.additionalProperties === false,
      `${field} schema permits unknown fields`,
    );
  }
}

function validateCloudInit({ metaData, networkData, userData }) {
  const joined = `${metaData}\n${networkData}\n${userData}`;
  const placeholders = new Set([...joined.matchAll(/@@[A-Z0-9_]+@@/gu)].map((match) => match[0]));
  const expected = new Set([
    "@@DNS_SERVERS@@",
    "@@GATEWAY@@",
    "@@IPV4_ADDRESS@@",
    "@@MAC_ADDRESS@@",
    "@@PLAN_HASH@@",
    "@@PREFIX_LENGTH@@",
    "@@SSH_PUBLIC_KEY@@",
  ]);
  assert(sameSet(placeholders, expected), "cloud-init placeholders changed or are incomplete");
  for (const required of [
    "ssh_pwauth: false",
    "disable_root: true",
    "lock_passwd: true",
    "PasswordAuthentication no",
    "PermitRootLogin no",
  ])
    assert(userData.includes(required), `cloud-init omits ${required}`);
  assert(!/\b(?:password|passwd):\s*[^\s]/iu.test(userData), "cloud-init embeds a password");
  for (const required of [
    "dhcp4: false",
    "dhcp6: false",
    "accept-ra: false",
    "@@IPV4_ADDRESS@@/@@PREFIX_LENGTH@@",
    "via: @@GATEWAY@@",
  ])
    assert(networkData.includes(required), `network cloud-init omits ${required}`);
  assert(userData.includes("net.ipv6.conf.all.disable_ipv6=1"), "guest IPv6 is not disabled");
  assert(metaData.includes("@@PLAN_HASH@@"), "cloud-init identity is not bound to the plan hash");
}

function validateFirewalls({ guestFirewall, hostFirewall, emergencyFirewall }) {
  for (const source of [guestFirewall, hostFirewall]) {
    assert(source.includes("tcp dport 22 ip saddr @management_ipv4"), "SSH is not management-only");
    assert(source.includes("tcp dport { 80, 443 }"), "public ingress is not exactly HTTP/HTTPS");
    assert(
      source.includes("ether type ip6") || source.includes("meta nfproto ipv6"),
      "IPv6 is untreated",
    );
    assert(source.includes(" drop"), "firewall lacks a drop path");
  }
  assert(guestFirewall.includes("policy drop"), "guest input firewall is not default-deny");
  assert(hostFirewall.includes('oifname "tap124i0"'), "host firewall is not scoped to VM 124");
  assert(hostFirewall.includes("130.245.173.11"), "host firewall is not bound to the beta address");
  assert(
    emergencyFirewall.includes("priority -20"),
    "emergency admission stop does not precede ingress",
  );
  assert(
    emergencyFirewall.includes("tcp dport { 80, 443 } drop"),
    "emergency stop does not block both public ports",
  );
  assert(
    !emergencyFirewall.includes("tcp dport 22"),
    "emergency admission stop blocks management SSH",
  );
}

function validateUnits({ serviceUnit, backupUnit, backupOrchestratorUnit, healthUnit }) {
  for (const required of [
    "User={{ item.name }}",
    "Group={{ item.name }}",
    "NoNewPrivileges=yes",
    "CapabilityBoundingSet=",
    "AmbientCapabilities=",
    "PrivateDevices=yes",
    "ProtectHome=yes",
    "ProtectSystem=strict",
    "IPAddressDeny=any",
    "IPAddressAllow=localhost",
    "SocketBindDeny=any",
    "SocketBindAllow=ipv4:tcp:{{ item.port }}",
  ])
    assert(serviceUnit.includes(required), `application unit omits ${required}`);
  for (const [name, source] of [
    ["backup", backupUnit],
    ["backup orchestrator", backupOrchestratorUnit],
    ["health collector", healthUnit],
  ]) {
    for (const required of [
      "NoNewPrivileges=yes",
      "CapabilityBoundingSet=",
      "AmbientCapabilities=",
      "PrivateDevices=yes",
      "ProtectHome=yes",
      "ProtectSystem=strict",
      "IPAddressDeny=any",
    ])
      assert(source.includes(required), `${name} unit omits ${required}`);
  }
  assert(backupUnit.includes("User=mako-%i"), "backup unit does not use its storage owner");
  assert(healthUnit.includes("User=mako-health"), "health collector is not non-root");
}

function validateMixedStorageReleaseOperation(source, runtimeTasks) {
  for (const required of [
    "rocks_components=(data-plane edge-gateway telemetry-query)",
    "control_database=/srv/mako-data/sqlite/control-plane/live/control.sqlite3",
    '"$target_control_binary" inspect-sqlite',
    "control SQLite authority could not be fenced after service stop",
    "/usr/bin/flock --unlock 8",
  ])
    assert(source.includes(required), `release operation omits mixed-storage guard: ${required}`);
  assert(
    !source.includes("components=(data-plane control-plane edge-gateway telemetry-query)"),
    "release operation still treats control SQLite as a RocksDB volume",
  );
  assert(
    source.includes("snapshot-current") &&
      runtimeTasks.includes(
        "Preserve the outgoing release configuration before changing selectors",
      ) &&
      runtimeTasks.includes("snapshot-current") &&
      runtimeTasks.indexOf("snapshot-current") < runtimeTasks.indexOf("Atomically select"),
    "ordinary release selection does not preserve a rollback configuration snapshot",
  );
}

function validateReleaseManifest(manifest, planHash) {
  for (const digest of [
    manifest.releaseDigest,
    manifest.source?.digest,
    manifest.runtime?.digest,
    manifest.dependencies?.digest,
    manifest.artifacts?.digest,
  ])
    assert(digestPattern.test(digest), "release manifest contains an invalid digest");
  assert(manifest.schemaVersion === 1, "release manifest schemaVersion must be 1");
  assert(manifest.planHash === planHash, "release manifest is for a different provisioning plan");
  assert(
    manifest.source.digest === sha256(canonicalJson(manifest.source.files)),
    "source-set digest is invalid",
  );
  assert(
    manifest.runtime.digest === sha256(canonicalJson(manifest.runtime.values)),
    "runtime digest is invalid",
  );
  assert(
    manifest.dependencies.digest === sha256(canonicalJson(manifest.dependencies.files)),
    "dependency digest is invalid",
  );
  assert(
    manifest.artifacts.digest === sha256(canonicalJson(manifest.artifacts.files)),
    "artifact digest is invalid",
  );
  const identity = {
    schemaVersion: manifest.schemaVersion,
    storageCompatibility: manifest.storageCompatibility,
    planHash: manifest.planHash,
    source: manifest.source,
    runtime: manifest.runtime,
    dependencies: manifest.dependencies,
    artifacts: manifest.artifacts,
  };
  assert(
    manifest.releaseDigest === sha256(canonicalJson(identity)),
    "release identity digest is invalid",
  );
  const artifactPaths = manifest.artifacts.files.map((artifact) => artifact.path);
  assert(
    new Set(artifactPaths).size === artifactPaths.length,
    "release manifest repeats an artifact path",
  );
  for (const binary of [
    "mako-control-plane",
    "mako-control-session",
    "mako-operator-session",
    "mako-operator-admin",
    "mako-data-plane",
    "mako-edge-gateway",
    "mako-qualification-fixture",
    "mako-storage-ops",
    "mako-telemetry-query",
  ])
    assert(artifactPaths.includes(`bin/${binary}`), `release manifest omits ${binary}`);
  assert(
    artifactPaths.includes("console/index.html"),
    "release manifest omits the console entry point",
  );
  // The edge runtime's main worker is part of the release for the same reason
  // the binaries are: it authenticates deployments, supplies the SDK module to
  // every user worker, and sets each worker's permissions. Reaching the host
  // only through the provisioning role meant a release could be deployed and
  // the runtime keep running whatever worker had been copied there last --
  // two fixes shipped in a release were absent from the host it went to, and
  // no digest could say so.
  for (const worker of ["index.ts", "supervisor.ts", "edge-sdk-source.ts"])
    assert(
      artifactPaths.includes(`runtime-main/${worker}`),
      `release manifest omits the edge runtime worker ${worker}`,
    );
}

function validateHostedConsole(index, bundle) {
  assert(
    index.includes('name="mako-console-management-endpoint" content="same-origin"'),
    "built console omits its same-origin management configuration",
  );
  for (const required of [
    "Short-lived developer session token",
    "mako.console.developer-session.v1",
    "The developer session token is invalid or expired.",
  ]) {
    assert(bundle.includes(required), `built console omits hosted authentication: ${required}`);
  }
}

function validateDeveloperRegistrationInfrastructure({
  groupVariables,
  configurationTasks,
  guestFirewall,
  serviceUnit,
  serviceConfigTemplate,
  caddyTemplate,
  alertRules,
  secretGenerator,
  secretRedaction,
  releaseManifest,
}) {
  for (const required of [
    "mako_developer_registration_enabled: false",
    "mako_developer_smtp_relay_hostname: smtp.resend.com",
    "mako_developer_smtp_port: 587",
    "mako_developer_smtp_tls_mode: starttls",
    "mako_developer_smtp_username: resend",
    "mako_developer_smtp_sender: no-reply@mail.makodb.com",
    "developer-mail-encryption",
    "developer-smtp-password",
    "mako_operator_password_auth_enabled: false",
    "mako_operator_session_lifetime_seconds: 3600",
    "mako_operator_mutation_freshness_seconds: 300",
    "mako_operator_break_glass_bearer_enabled: true",
  ])
    assert(groupVariables.includes(required), `developer registration variables omit ${required}`);

  for (const required of [
    "Inspect protected developer registration mail credentials",
    "Require protected developer registration mail credentials",
    "Install protected developer registration mail credentials",
    "mako_developer_registration_enabled | bool",
    'item.stat.mode == "0600"',
    "no_log: true",
  ])
    assert(configurationTasks.includes(required), `mail credential installation omits ${required}`);

  for (const required of [
    "ConditionPathExists=/etc/mako/credentials/developer-mail-encryption",
    "ConditionPathExists=/etc/mako/credentials/developer-smtp-password",
    "LoadCredential=developer-mail-encryption:/etc/mako/credentials/developer-mail-encryption",
    "LoadCredential=developer-smtp-password:/etc/mako/credentials/developer-smtp-password",
    "item.name == 'mako-control-plane' and mako_developer_registration_enabled | bool",
    "IPAddressAllow=any",
  ])
    assert(serviceUnit.includes(required), `control-plane credential boundary omits ${required}`);

  for (const required of ["chain output", "policy drop", "tcp dport { 465, 587 } accept"])
    assert(guestFirewall.includes(required), `mail egress boundary omits ${required}`);

  for (const required of [
    '"enabled": {{ mako_developer_registration_enabled | bool | lower }}',
    '"smtp_relay_hostname": "{{ mako_developer_smtp_relay_hostname }}"',
    '"smtp_tls_mode": "{{ mako_developer_smtp_tls_mode }}"',
    '"smtp_username": "{{ mako_developer_smtp_username }}"',
    '"developer_smtp_password": "file:/run/credentials/mako-control-plane.service/developer-smtp-password"',
    '"operator_authentication": {',
    '"enabled": {{ mako_operator_password_auth_enabled | bool | lower }}',
    '"break_glass_bearer_enabled": {{ mako_operator_break_glass_bearer_enabled | bool | lower }}',
  ])
    assert(
      serviceConfigTemplate.includes(required),
      `service mail configuration omits ${required}`,
    );

  for (const required of [
    "^/_internal(?:/|$)",
    "/v1/developer-auth/registrations",
    "/v1/developer-auth/wait-list-status",
    "/v1/operator-auth/sessions",
    "/v1/operator-auth/sessions/current/actions/verify-password",
    "/v1/operator/developer-waitlist",
    'header @developer_auth Cache-Control "no-store"',
    'header @operator_auth Cache-Control "no-store"',
  ])
    assert(caddyTemplate.includes(required), `public route boundary omits ${required}`);

  for (const alert of [
    "MakoDeveloperMailNotReady",
    "MakoDeveloperOutboxBacklog",
    "MakoDeveloperMailDeadLetter",
    "MakoDeveloperMailWorkerFailure",
    "MakoDeveloperAuthenticationThrottlingHigh",
    "MakoDeveloperWaitlistReviewStale",
  ])
    assert(alertRules.includes(`alert: ${alert}`), `developer alert inventory omits ${alert}`);

  assert(
    secretGenerator.includes("generate_secret developer-mail-encryption"),
    "secret generator omits the developer mail-encryption key",
  );
  for (const name of ["developer-mail-encryption", "developer-smtp-password"])
    assert(secretRedaction.includes(name), `deployment log redaction omits ${name}`);

  const sourcePaths = new Set(releaseManifest.source.files.map((entry) => entry.path));
  for (const path of [
    "crates/mako-control-plane/src/developer_identity.rs",
    "crates/mako-control-plane/src/developer_registration.rs",
    "crates/mako-control-plane/src/developer_workflow.rs",
    "services/mako-control-plane/src/developer_auth_http.rs",
    "services/mako-control-plane/src/developer_metrics.rs",
    "services/mako-control-plane/src/smtp.rs",
    "services/mako-control-plane/src/bin/mako-operator-session.rs",
    "services/mako-control-plane/src/bin/mako-operator-admin.rs",
    "apps/console/src/developer-auth-views.tsx",
    "apps/console/src/operator-waitlist.tsx",
    "scripts/generate-public-beta-secrets.sh",
    "scripts/verify-public-beta-secret-redaction.sh",
  ])
    assert(sourcePaths.has(path), `release manifest omits developer registration source ${path}`);
}

function validateOperatorAlertInfrastructure({
  groupVariables,
  guestFirewall,
  alertRules,
  alertmanagerTemplate,
  observabilityTasks,
  alertmanagerHardening,
}) {
  for (const alert of [
    "MakoOperatorBreakGlassEnabled",
    "MakoOperatorAuthenticationThrottlingHigh",
    "MakoOperatorAuthenticationFailuresHigh",
    "MakoOperatorBootstrapFailure",
    "MakoDeveloperRoleRepairFailure",
    "MakoOperatorSessionGrowth",
  ])
    assert(alertRules.includes(`alert: ${alert}`), `operator auth alert inventory omits ${alert}`);
  for (const required of [
    "mako_alert_smtp_host: smtp.resend.com",
    "mako_alert_smtp_port: 587",
    "mako_alert_smtp_username: resend",
    "mako_alert_smtp_sender: alerts@mail.makodb.com",
    "mako_alert_delivery_enabled: true",
  ])
    assert(groupVariables.includes(required), `operator alert variables omit ${required}`);

  for (const required of [
    'smtp_from: "{{ mako_alert_smtp_sender }}"',
    'smtp_auth_username: "{{ mako_alert_smtp_username }}"',
    "smtp_auth_password_file: /run/credentials/prometheus-alertmanager.service/alert-smtp-password",
    "smtp_require_tls: true",
  ])
    assert(
      alertmanagerTemplate.includes(required),
      `Alertmanager authenticated SMTP configuration omits ${required}`,
    );
  assert(
    !alertmanagerTemplate.includes("smtp_auth_password:") &&
      !alertmanagerTemplate.includes("smtp_auth_secret:"),
    "Alertmanager template contains an inline SMTP secret",
  );

  for (const required of [
    "Inspect the protected operator-alert SMTP credential source",
    "Require the protected operator-alert SMTP credential source",
    "Install the protected operator-alert SMTP credential",
    "dest: /etc/mako/credentials/alert-smtp-password",
    "no_log: true",
  ])
    assert(observabilityTasks.includes(required), `operator alert tasks omit ${required}`);

  for (const required of [
    "ConditionPathExists=/etc/mako/credentials/alert-smtp-password",
    "LoadCredential=alert-smtp-password:/etc/mako/credentials/alert-smtp-password",
    "IPAddressAllow=any",
  ])
    assert(alertmanagerHardening.includes(required), `Alertmanager unit omits ${required}`);
  assert(
    guestFirewall.includes("tcp dport { 465, 587 } accept") &&
      !guestFirewall.includes("tcp dport 25 accept"),
    "operator alert egress is not restricted to authenticated TLS SMTP ports",
  );
}

function validateCustomDomainInfrastructure({ caddyTemplate, guestFirewall, serviceUnit }) {
  // Certificates for developers' domains are issued on demand and only after
  // the control plane confirms the hostname is verified; the catch-all site
  // never serves the console or the management API.
  const siteStart = caddyTemplate.indexOf("\nhttps:// {");
  assert(siteStart > 0, "Caddy template omits the custom-domain catch-all site");
  const platformSite = caddyTemplate.slice(
    caddyTemplate.indexOf("{{ mako_public_fqdn }} {"),
    siteStart,
  );
  const customDomainSite = caddyTemplate.slice(siteStart);
  for (const required of [
    "on_demand_tls {",
    "ask http://127.0.0.1:8081/_internal/v1/custom-domains/ask",
  ])
    assert(caddyTemplate.includes(required), `on-demand TLS gate omits ${required}`);
  for (const required of [
    "\ttls {\n\t\ton_demand\n\t}",
    "respond @internal_rpc 404",
    "respond @service_credential_api 404",
    "@custom_domain_function path_regexp custom_domain_function ^/functions/v1/[^/]+$",
    "header_up X-Mako-Custom-Domain {http.request.host}",
    "\trespond 404\n",
  ])
    assert(customDomainSite.includes(required), `custom-domain site omits ${required}`);
  for (const forbidden of [
    "127.0.0.1:8081",
    "@console_route",
    "@control_api",
    "@operator_control_api",
    "@developer_workspace_api",
    "file_server",
  ])
    assert(!customDomainSite.includes(forbidden), `custom-domain site serves ${forbidden}`);
  assert(
    platformSite.includes("header_up -X-Mako-Custom-Domain") &&
      !platformSite.includes("header_up X-Mako-Custom-Domain {") &&
      !platformSite.includes("on_demand"),
    "platform hostname site does not refuse a client-supplied custom-domain assertion",
  );
  // The services decide cross-origin access from each domain's allowlist, so
  // the proxy neither emits a cross-origin header nor filters the methods a
  // preflight needs to reach them.
  assert(
    !/access-control-/iu.test(caddyTemplate),
    "Caddy emits its own Access-Control header; CORS is the services' decision",
  );
  assert(
    !/^\t*(?:@\w+ )?(?:not )?method\s/mu.test(caddyTemplate),
    "a site filters by method, which would drop OPTIONS preflights",
  );
  // The verifier's resolver is the host's stub resolver on loopback by
  // default; the guest may reach the configured name servers on port 53.
  assert(
    guestFirewall.includes("udp dport 53 accept"),
    "guest firewall does not allow DNS lookups for domain verification",
  );
  assert(
    serviceUnit.includes("IPAddressAllow=localhost"),
    "control plane cannot reach the loopback stub resolver",
  );
}

function validateEvidence({
  plan,
  releaseManifest,
  deploymentEvidence,
  admissionEvidence,
  observabilityEvidence,
  teardownEvidence,
  groupVariables,
}) {
  for (const [name, evidence] of [
    ["deployment", deploymentEvidence],
    ["admission stop", admissionEvidence],
    ["observability", observabilityEvidence],
    ["teardown", teardownEvidence],
  ])
    assert(evidence.schemaVersion === 1, `${name} evidence has the wrong schemaVersion`);
  assert(deploymentEvidence.planHash === plan.planHash, "deployment evidence uses another plan");
  const vmId = deploymentEvidence.deployment?.vmId ?? deploymentEvidence.vmId;
  const address = deploymentEvidence.deployment?.address ?? deploymentEvidence.address;
  const origin =
    deploymentEvidence.publicOrigin ??
    (deploymentEvidence.deployment?.fqdn
      ? `https://${deploymentEvidence.deployment.fqdn}`
      : undefined);
  assert(vmId === 124, "deployment evidence does not identify VM 124");
  assert(address === "130.245.173.11", "deployment evidence has the wrong address");
  assert(origin === "https://cloud-test.makodb.com", "deployment evidence has the wrong origin");
  assert(
    deploymentEvidence.release?.digest === releaseManifest.releaseDigest,
    "deployed release differs from manifest",
  );
  assert(
    ["disabled", "pre_gate", "restricted-pre-gate", "risk_accepted_preview"].includes(
      deploymentEvidence.networkAdmission?.mode,
    ),
    "deployment evidence has an unsupported admission mode",
  );
  assert(
    deploymentEvidence.networkAdmission?.unrestrictedPublicAdmissionApproved === false,
    "deployment evidence claims public approval",
  );
  assert(admissionEvidence.status === "passed", "emergency admission stop has not passed");
  assert(admissionEvidence.proxmoxGate?.active === true, "Proxmox admission stop is inactive");
  assert(
    admissionEvidence.proxmoxGate?.enabled === true,
    "Proxmox admission stop is not persistent",
  );
  assert(
    observabilityEvidence.publicAdmissionEnabled === false,
    "observability evidence reports public admission",
  );
  assert(
    observabilityEvidence.allFourServicesReady === true,
    "service readiness evidence is incomplete",
  );
  assert(teardownEvidence.executionSupported === false, "ordinary tooling can execute teardown");
  const rebuilt = buildTeardownPlan(
    {
      vm: teardownEvidence.vm,
      admission: teardownEvidence.admission,
      backups: teardownEvidence.backups,
      guest: teardownEvidence.guest,
      dns: teardownEvidence.dns,
      retainedEvidence: teardownEvidence.retainedEvidence,
    },
    teardownEvidence.generatedAt,
  );
  assert(rebuilt.planHash === teardownEvidence.planHash, "teardown plan hash is invalid");
  assert(
    groupVariables.includes("mako_public_admission_mode: disabled"),
    "default admission is not disabled",
  );
  assert(groupVariables.includes("mako_caddy_enabled: false"), "Caddy is enabled by default");
}

function sameSet(left, right) {
  return left.size === right.size && [...left].every((value) => right.has(value));
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function assert(condition, message) {
  if (!condition) throw new Error(message);
}
