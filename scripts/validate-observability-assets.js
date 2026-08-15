import { access, readdir, readFile } from "node:fs/promises";
import { resolve } from "node:path";

const root = resolve(import.meta.dirname, "..");
const dashboardDirectory = resolve(root, "infra/local/grafana/dashboards");
const expected = new Set([
  "control-plane-sqlite.json",
  "developer-registration.json",
  "error-rates.json",
  "function-workers.json",
  "index-builds.json",
  "live-streams.json",
  "replication-lag.json",
  "production-rocksdb.json",
  "public-beta-operations.json",
  "revocation-freshness.json",
  "saturation.json",
  "sequencer-gaps.json",
  "service-health.json",
]);
const files = (await readdir(dashboardDirectory)).filter((file) => file.endsWith(".json"));
if (files.length !== expected.size || files.some((file) => !expected.has(file))) {
  throw new Error(`unexpected dashboard set: ${files.sort().join(", ")}`);
}

const uids = new Set();
for (const file of files) {
  const dashboard = JSON.parse(await readFile(resolve(dashboardDirectory, file), "utf8"));
  if (
    typeof dashboard.uid !== "string" ||
    uids.has(dashboard.uid) ||
    typeof dashboard.title !== "string" ||
    !Array.isArray(dashboard.panels) ||
    dashboard.panels.length === 0 ||
    dashboard.schemaVersion < 40
  ) {
    throw new Error(`${file} is not a complete, uniquely identified dashboard`);
  }
  uids.add(dashboard.uid);
  const panelIds = new Set();
  for (const panel of dashboard.panels) {
    if (
      !Number.isInteger(panel.id) ||
      panelIds.has(panel.id) ||
      panel.datasource?.uid !== "prometheus" ||
      !Array.isArray(panel.targets) ||
      panel.targets.some(
        (target) =>
          typeof target.expr !== "string" ||
          (!target.expr.includes("mako_") && !target.expr.includes("node_")),
      )
    ) {
      throw new Error(`${file} contains an invalid panel`);
    }
    panelIds.add(panel.id);
  }
  const encoded = JSON.stringify(dashboard);
  for (const forbiddenLabel of ["actor_id=", "request_id=", "trace_id=", "document_id="]) {
    if (encoded.includes(forbiddenLabel)) {
      throw new Error(`${file} uses forbidden high-cardinality label ${forbiddenLabel}`);
    }
  }
}

const provider = await readFile(
  resolve(root, "infra/local/grafana/provisioning/dashboards/mako-cloud.yaml"),
  "utf8",
);
const compose = await readFile(resolve(root, "infra/local/compose.yaml"), "utf8");
if (
  !provider.includes("path: /etc/grafana/dashboards") ||
  !compose.includes("./grafana/dashboards:/etc/grafana/dashboards:ro")
) {
  throw new Error("Grafana dashboard provisioning is not mounted consistently");
}

const prometheus = await readFile(resolve(root, "infra/local/prometheus.yaml"), "utf8");
const rules = await readFile(
  resolve(root, "infra/local/prometheus-rules/mako-cloud-alerts.yaml"),
  "utf8",
);
const alertmanager = await readFile(
  resolve(root, "infra/ansible/roles/observability/templates/alertmanager.yml.j2"),
  "utf8",
);
const expectedAlerts = new Set([
  "MakoControlSqliteUnavailable",
  "MakoControlSqliteContentionHigh",
  "MakoControlSqliteWalPressure",
  "MakoControlSqliteCapacityCritical",
  "MakoControlSqliteBackupOrRestoreFailure",
  "MakoOperatorBreakGlassEnabled",
  "MakoOperatorAuthenticationThrottlingHigh",
  "MakoOperatorAuthenticationFailuresHigh",
  "MakoOperatorBootstrapFailure",
  "MakoDeveloperRoleRepairFailure",
  "MakoOperatorSessionGrowth",
  "MakoDeveloperMailNotReady",
  "MakoDeveloperOutboxBacklog",
  "MakoDeveloperMailDeadLetter",
  "MakoDeveloperMailWorkerFailure",
  "MakoDeveloperAuthenticationThrottlingHigh",
  "MakoDeveloperWaitlistReviewStale",
  "MakoAuthRefreshReplayDetected",
  "MakoEdgeSandboxIncident",
  "MakoPolicyEvaluationFailureRatioHigh",
  "MakoSequencerGapBlocking",
  "MakoStorageContractFailed",
  "MakoTenantIsolationSignal",
  "MakoRocksDbUnavailable",
  "MakoRocksDbDiskCritical",
  "MakoRocksDbWriteStalled",
  "MakoRocksDbCorruptionSignal",
  "MakoRocksDbBackupStale",
  "MakoRocksDbRestoreFailed",
  "MakoRocksDbRecoverySlow",
  "MakoServiceUnavailable",
  "MakoCertificateMetricMissing",
  "MakoCertificateExpiringSoon",
  "MakoFilesystemReserveLow",
  "MakoRecoveryEvidenceMissing",
  "MakoRequestLatencyHigh",
  "MakoHttpErrorRateHigh",
  "MakoAuditWriteFailure",
  "MakoServiceRestartLoop",
  "MakoPodmanDependencyDown",
  "MakoHostCpuSaturation",
  "MakoHostMemoryLow",
  "MakoHostNetworkErrors",
  "MakoOperatorControlCenterSourceUnavailable",
  "MakoOperatorControlCenterFailureRatioHigh",
  "MakoOperatorControlCenterLatencyHigh",
]);
const alertBlocks = rules
  .split(/^ {6}- alert: /m)
  .slice(1)
  .map((section) => {
    const newline = section.indexOf("\n");
    return [section.slice(0, newline), section.slice(newline + 1)];
  });
if (
  alertBlocks.length !== expectedAlerts.size ||
  alertBlocks.some(([name]) => !expectedAlerts.has(name)) ||
  !prometheus.includes("/etc/prometheus/rules/*.yaml") ||
  !prometheus.includes('targets: ["127.0.0.1:9093"]') ||
  !prometheus.includes('targets: ["127.0.0.1:8081"]') ||
  !compose.includes("./prometheus-rules:/etc/prometheus/rules:ro")
) {
  throw new Error("Prometheus alert inventory or provisioning is incomplete");
}
if (
  !alertmanager.includes('to: "{{ mako_alert_email }}"') ||
  !alertmanager.includes(
    'smtp_smarthost: "{{ mako_alert_smtp_host }}:{{ mako_alert_smtp_port }}"',
  ) ||
  !alertmanager.includes('smtp_from: "{{ mako_alert_smtp_sender }}"') ||
  !alertmanager.includes('smtp_auth_username: "{{ mako_alert_smtp_username }}"') ||
  !alertmanager.includes(
    "smtp_auth_password_file: /run/credentials/prometheus-alertmanager.service/alert-smtp-password",
  ) ||
  !alertmanager.includes("smtp_require_tls: true") ||
  alertmanager.includes("smtp_auth_password:") ||
  alertmanager.includes("smtp_auth_secret:")
) {
  throw new Error(
    "Alertmanager email routing is missing authenticated TLS or contains inline SMTP secrets",
  );
}
for (const [name, block] of alertBlocks) {
  const runbook = block.match(/^ {10}runbook: (\S+)$/m)?.[1];
  if (
    !block.includes("        expr:") ||
    !block.includes("        for:") ||
    !block.includes("          severity:") ||
    runbook === undefined
  ) {
    throw new Error(`${name} is missing an expression, hold duration, severity, or runbook`);
  }
  await access(resolve(root, runbook));
}

console.log(
  `validated ${files.length} Mako Cloud Grafana dashboards and ${alertBlocks.length} alert runbooks`,
);
