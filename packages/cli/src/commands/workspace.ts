import type {
  ConnectionCheckRequest,
  DeveloperRestore,
  DeveloperRestoreRequest,
  SyncSummary,
} from "@mako-cloud/management-sdk";

import type { CommandContext } from "../cli/context.js";
import { CliError, EXIT, usageError } from "../cli/errors.js";
import type { TableColumn } from "../cli/output.js";
import type { Command, CommandArgs, OptionSpec } from "../cli/registry.js";
import { parseInstant } from "./observability.js";
import { PROJECT_OPTION, projectFrom, TENANT_OPTIONS, tenantFrom } from "./shared.js";

// The developer workspace: the console's overview, navigation, connect page,
// sync diagnostics, and backup inventory, read for one tenant. These APIs
// report instants as Unix seconds; humans see them as ISO date-times, and
// `--json` prints the response as served.

function iso(unixSeconds: number | null | undefined): string | null {
  if (unixSeconds === null || unixSeconds === undefined) return null;
  return new Date(unixSeconds * 1000).toISOString();
}

function unixSeconds(instant: string): number {
  return Math.floor(Date.parse(instant) / 1000);
}

function columns(...keys: readonly string[]): readonly TableColumn[] {
  return keys.map((key) => ({ key }));
}

// ---- workspace ----------------------------------------------------------------

async function workspaceSummary(context: CommandContext, args: CommandArgs): Promise<void> {
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  const summary = await client.getWorkspaceSummary(projectId, environmentId);
  if (context.json) {
    context.out(summary);
    return;
  }
  const rows = Object.entries(summary.sections).map(([section, item]) => ({
    section,
    status: item.status,
    observedAt: iso(item.observedAtUnixSeconds),
    freshUntil: iso(item.freshUntilUnixSeconds),
    retainedSince: iso(item.retainedSinceUnixSeconds),
    remediation: item.remediationCode ?? null,
  }));
  context.out(rows, {
    columns: columns(
      "section",
      "status",
      "observedAt",
      "freshUntil",
      "retainedSince",
      "remediation",
    ),
  });
}

async function workspaceNavigation(context: CommandContext, args: CommandArgs): Promise<void> {
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  const destinations = await client.getWorkspaceNavigation(projectId, environmentId);
  context.out(destinations, { columns: columns("id", "label", "path", "permitted") });
}

/**
 * Public connection metadata only: the endpoint, the active public key id,
 * and compatibility. Key material is never part of this response and is
 * never printed here; a key's value is shown once, when it is issued.
 */
async function workspaceConnect(context: CommandContext, args: CommandArgs): Promise<void> {
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  const metadata = await client.getConnectMetadata(projectId, environmentId);
  if (context.json) {
    context.out(metadata);
    return;
  }
  context.out({
    projectId: metadata.tenant.projectId,
    environmentId: metadata.tenant.environmentId,
    publicEndpoint: metadata.publicEndpoint,
    publicKeyId: metadata.publicKeyId,
    collections:
      metadata.collections
        .map((item) => `${item.collectionId} (schema v${item.activeSchemaVersion})`)
        .join(", ") || "(none)",
    rxdbClientRange: metadata.rxdbClientRange,
    templateVersion: metadata.templateVersion,
  });
}

const CHECK_OPTIONS: Readonly<Record<string, OptionSpec>> = {
  "public-key-id": {
    type: "string",
    description: "Public key id the application will present",
    placeholder: "<key-id>",
  },
  collection: {
    type: "string",
    description: "Collection whose schema and replication route to probe",
    placeholder: "<collection-id>",
  },
  "schema-version": {
    type: "string",
    description: "Schema version the application expects for --collection",
    placeholder: "<n>",
  },
  "rxdb-version": {
    type: "string",
    description: "RxDB client version to check compatibility for",
    placeholder: "<version>",
  },
};

/** Runs the bounded connection probes; exits 5 when any step failed. */
async function workspaceCheck(context: CommandContext, args: CommandArgs): Promise<void> {
  const { projectId, environmentId } = tenantFrom(context, args);
  const publicKeyId = args.string("public-key-id");
  const collectionId = args.string("collection");
  const schemaVersion = args.integer("schema-version");
  const rxdbVersion = args.string("rxdb-version");
  if (schemaVersion !== undefined && schemaVersion < 1) {
    throw usageError("--schema-version must be at least 1");
  }
  if (schemaVersion !== undefined && collectionId === undefined) {
    throw usageError("--schema-version needs --collection");
  }
  const input: ConnectionCheckRequest = {
    ...(publicKeyId !== undefined ? { publicKeyId } : {}),
    ...(collectionId !== undefined ? { collectionId } : {}),
    ...(schemaVersion !== undefined ? { schemaVersion } : {}),
    ...(rxdbVersion !== undefined ? { rxdbVersion } : {}),
  };
  const client = await context.management();
  const check = await client.checkConnection(projectId, environmentId, input);
  if (context.json) {
    context.out(check);
  } else {
    context.info(`checked at ${iso(check.checkedAtUnixSeconds)}`);
    context.out(check.steps, {
      line: (item) => {
        const step = item as (typeof check.steps)[number];
        const remediation =
          step.remediationCode === null || step.remediationCode === undefined
            ? ""
            : `  remediation: ${step.remediationCode} (${step.retryable ? "retryable" : "configuration change required"})`;
        return `${step.state.padEnd(7)}  ${step.id}${remediation}`;
      },
    });
  }
  const failed = check.steps.filter((step) => step.state === "failed");
  if (failed.length > 0) {
    throw new CliError(
      `${failed.length} of ${check.steps.length} connection check steps failed: ${failed.map((step) => step.id).join(", ")}`,
      EXIT.refused,
      "CLI_CONNECTION_CHECK_FAILED",
    );
  }
}

// ---- sync ------------------------------------------------------------------------

const SYNC_OPTIONS: Readonly<Record<string, OptionSpec>> = {
  collection: {
    type: "string",
    description: "Limit the aggregates to one collection",
    placeholder: "<collection-id>",
  },
  from: {
    type: "string",
    description: "Start of the window: a duration ago (1h, 24h) or an ISO date-time (default: 1h)",
    placeholder: "<duration|iso>",
  },
  until: {
    type: "string",
    description: "End of the window as an ISO date-time or a duration ago (default: now)",
    placeholder: "<iso|duration>",
  },
};

/** The console's recommended actions for the counters that carry one. */
export function syncRemediation(summary: SyncSummary): string[] {
  const issues: readonly (readonly [number | undefined, string])[] = [
    [summary.throttled, "throttling is retryable after the server-provided delay"],
    [summary.checkpointExpired, "expired checkpoints require a confirmed full resync"],
    [summary.streamGaps, "stream gaps require a confirmed full resync"],
    [summary.schemaMismatches, "schema mismatches require an RxDB migration before retrying"],
    [
      summary.policyDenials,
      "policy denials are non-retryable until policy or user authorization changes",
    ],
  ];
  return issues.filter(([count]) => (count ?? 0) > 0).map(([, text]) => text);
}

async function syncSummary(context: CommandContext, args: CommandArgs): Promise<void> {
  const { projectId, environmentId } = tenantFrom(context, args);
  const now = Date.now();
  const collectionId = args.string("collection");
  const fromArg = args.string("from");
  const untilArg = args.string("until");
  const from = fromArg === undefined ? undefined : unixSeconds(parseInstant(fromArg, "from", now));
  const until =
    untilArg === undefined ? undefined : unixSeconds(parseInstant(untilArg, "until", now));
  if (from !== undefined && until !== undefined && from > until) {
    throw usageError("--from must be before --until");
  }
  const client = await context.management();
  const summary = await client.getSyncSummary(projectId, environmentId, {
    ...(collectionId !== undefined ? { collectionId } : {}),
    ...(from !== undefined ? { from } : {}),
    ...(until !== undefined ? { until } : {}),
  });
  if (context.json) {
    context.out(summary);
  } else {
    context.out({
      projectId: summary.tenant.projectId,
      environmentId: summary.tenant.environmentId,
      collection: summary.collectionId ?? "(all)",
      windowStart: iso(summary.windowStartUnixSeconds),
      windowEnd: iso(summary.windowEndUnixSeconds),
      observedAt: iso(summary.observedAtUnixSeconds),
      retainedSince: iso(summary.retainedSinceUnixSeconds),
      pulls: summary.pullCount ?? null,
      pushes: summary.pushCount ?? null,
      liveStreams: summary.liveStreams ?? null,
      lagP95Ms: summary.lagP95Milliseconds ?? null,
      conflicts: summary.conflicts ?? null,
      policyDenials: summary.policyDenials ?? null,
      throttled: summary.throttled ?? null,
      checkpointExpired: summary.checkpointExpired ?? null,
      streamGaps: summary.streamGaps ?? null,
      resyncs: summary.resyncs ?? null,
      schemaMismatches: summary.schemaMismatches ?? null,
      clientVersionClasses: summary.clientVersionClasses ?? {},
    });
  }
  for (const issue of syncRemediation(summary)) context.info(`recommended: ${issue}`);
}

// ---- backups and restore requests -----------------------------------------------

async function backupsList(context: CommandContext, args: CommandArgs): Promise<void> {
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  const backups = await client.listDeveloperBackups(projectId, environmentId);
  if (context.json) {
    context.out(backups);
    return;
  }
  context.out(
    backups.map((backup) => ({
      backupId: backup.backupId,
      recoveryPoint: iso(backup.recoveryPointUnixSeconds),
      verifiedAt: iso(backup.verifiedAtUnixSeconds),
      retainedUntil: iso(backup.retainedUntilUnixSeconds),
      lastRestoreDrill: iso(backup.lastRestoreDrillUnixSeconds) ?? "no recorded drill",
      objective: backup.recoveryObjectiveStatus,
    })),
    {
      columns: columns(
        "backupId",
        "recoveryPoint",
        "verifiedAt",
        "retainedUntil",
        "lastRestoreDrill",
        "objective",
      ),
    },
  );
}

function restoreRow(restore: DeveloperRestore): Record<string, unknown> {
  return {
    requestId: restore.requestId,
    backupId: restore.backupId,
    targetProjectId: restore.target.projectId,
    targetEnvironmentId: restore.target.environmentId,
    state: restore.state,
    accessible: restore.accessible,
    requestedAt: iso(restore.requestedAtUnixSeconds),
    updatedAt: iso(restore.updatedAtUnixSeconds),
  };
}

const RESTORE_COLUMNS = columns(
  "requestId",
  "backupId",
  "targetEnvironmentId",
  "state",
  "accessible",
  "requestedAt",
  "updatedAt",
);

async function restoreRequestsList(context: CommandContext, args: CommandArgs): Promise<void> {
  const projectId = projectFrom(context, args);
  const client = await context.management();
  const restores = await client.listDeveloperRestoreRequests(projectId);
  if (context.json) {
    context.out(restores);
    return;
  }
  context.out(restores.map(restoreRow), { columns: RESTORE_COLUMNS });
}

function requiredField(input: Record<string, unknown>, name: string, hint = ""): string {
  const value = input[name];
  if (typeof value !== "string" || value.trim() === "") {
    throw usageError(`--input needs a non-empty "${name}"${hint}`);
  }
  return value.trim();
}

/**
 * Requests a restore of a backup into a new, isolated environment. The API
 * requires a step-up grant: taken from the input when present, otherwise
 * obtained by prompting for the password (or MAKO_STEP_UP_PASSWORD_FILE).
 */
async function restoreRequestCreate(context: CommandContext, args: CommandArgs): Promise<void> {
  const projectId = args.requireString("project");
  const raw = await context.readJson<unknown>(args.requireString("input"), "--input");
  if (typeof raw !== "object" || raw === null || Array.isArray(raw)) {
    throw usageError("--input must be a JSON object");
  }
  const fields = raw as Record<string, unknown>;
  const environmentFallback = args.string("env") ?? context.io.env.MAKO_ENVIRONMENT_ID;
  const environmentId =
    typeof fields.environmentId === "string" && fields.environmentId !== ""
      ? fields.environmentId
      : environmentFallback;
  if (environmentId === undefined || environmentId === "") {
    throw usageError('--input needs a non-empty "environmentId" (or pass --env)');
  }
  const stepUpToken =
    typeof fields.stepUpToken === "string" && fields.stepUpToken !== ""
      ? fields.stepUpToken
      : await context.stepUp();
  const input: DeveloperRestoreRequest = {
    environmentId,
    backupId: requiredField(fields, "backupId"),
    targetEnvironmentName: requiredField(fields, "targetEnvironmentName"),
    reason: requiredField(fields, "reason"),
    stepUpToken,
  };
  const client = await context.management();
  const restore = await client.requestDeveloperRestore(projectId, input, context.idempotencyKey());
  context.info(
    `restore request ${restore.requestId} accepted: the new environment stays inaccessible until isolation, storage, service, and recovery checks pass; overwrite and promotion are prohibited`,
  );
  if (context.json) context.out(restore);
  else context.out(restoreRow(restore));
}

export const workspaceCommands: readonly Command[] = [
  {
    path: ["workspace", "summary"],
    summary: "Overview sections for an environment, each with its own freshness",
    operations: ["getWorkspaceSummary"],
    options: TENANT_OPTIONS,
    run: workspaceSummary,
  },
  {
    path: ["workspace", "nav"],
    summary: "Workspace destinations and whether your memberships permit each",
    operations: ["getWorkspaceNavigation"],
    options: TENANT_OPTIONS,
    run: workspaceNavigation,
  },
  {
    path: ["workspace", "connect"],
    summary: "Public RxDB connection metadata: endpoint, active public key id, compatibility",
    operations: ["getConnectMetadata"],
    options: TENANT_OPTIONS,
    run: workspaceConnect,
  },
  {
    path: ["workspace", "check"],
    summary: "Probe DNS, TLS, routes, key, schema, and replication without reading documents",
    operations: ["checkConnection"],
    options: { ...TENANT_OPTIONS, ...CHECK_OPTIONS },
    run: workspaceCheck,
  },
  {
    path: ["sync", "summary"],
    summary: "Aggregate RxDB synchronization diagnostics for a window (default: the last hour)",
    operations: ["getSyncSummary"],
    options: { ...TENANT_OPTIONS, ...SYNC_OPTIONS },
    run: syncSummary,
  },
  {
    path: ["backups", "list"],
    summary: "Verified recovery points for an environment",
    operations: ["listDeveloperBackups"],
    options: TENANT_OPTIONS,
    run: backupsList,
  },
  {
    path: ["backups", "restore-requests", "list"],
    summary: "Restore requests for a project and their verification state",
    operations: ["listDeveloperRestoreRequests"],
    options: PROJECT_OPTION,
    run: restoreRequestsList,
  },
  {
    path: ["backups", "restore-requests", "create"],
    summary: "Restore a verified backup into a new isolated environment (step-up verified)",
    operations: ["requestDeveloperRestore", "verifyCurrentDeveloperPassword"],
    options: {
      project: { ...(PROJECT_OPTION.project as OptionSpec), required: true },
      env: {
        type: "string",
        short: "e",
        description: 'Environment to restore when the input has no "environmentId"',
        placeholder: "<environment-id>",
      },
      input: {
        type: "string",
        required: true,
        description:
          'JSON with "environmentId", "backupId", "targetEnvironmentName", "reason" (and optionally "stepUpToken"): inline, @file, or - for stdin',
        placeholder: "<@file|-|json>",
      },
    },
    destructive: {
      action: "restore a backup of project",
      resource: (args) => args.requireString("project"),
    },
    run: restoreRequestCreate,
  },
];
