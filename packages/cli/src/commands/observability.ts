import type {
  MakoManagementClient,
  ObservabilityPage,
  ObservabilityPayload,
  ObservabilityQuery,
  ObservabilityRecord,
} from "@mako-cloud/management-sdk";

import type { CommandContext, Page } from "../cli/context.js";
import { usageError } from "../cli/errors.js";
import type { TableColumn } from "../cli/output.js";
import type { Command, CommandArgs, OptionSpec } from "../cli/registry.js";
import { TENANT_OPTIONS, tenantFrom } from "./shared.js";

// Retained project signals: every command here reads one observability
// endpoint for a tenant, inside a window the API bounds by retention. The
// wire takes absolute ISO instants; `--since` accepts a duration for humans.
// `--json` prints the page as served (a filter, where one applies, narrows
// its items and nothing else); human output is newest first where the
// console is, and the page's retention block goes to stderr.

type Retention = ObservabilityPage["retention"];
type Kind = ObservabilityPayload["kind"];
type PayloadOf<K extends Kind> = Extract<ObservabilityPayload, { readonly kind: K }>;
interface Signal<K extends Kind> {
  readonly timestamp: string;
  readonly payload: PayloadOf<K>;
}

type SignalQuery = (
  client: MakoManagementClient,
  projectId: string,
  environmentId: string,
  query: ObservabilityQuery,
) => Promise<ObservabilityPage>;

/** Options every retained-signal query takes. */
export const QUERY_OPTIONS: Readonly<Record<string, OptionSpec>> = {
  since: {
    type: "string",
    description: "Start of the window: a duration ago (30m, 1h, 24h, 7d) or an ISO date-time",
    placeholder: "<duration|iso>",
  },
  until: {
    type: "string",
    description: "End of the window as an ISO date-time or a duration ago (default: now)",
    placeholder: "<iso|duration>",
  },
  limit: {
    type: "string",
    description: "Records per page, 1-1000 (default: 100)",
    placeholder: "<n>",
  },
  cursor: {
    type: "string",
    description: "Continue from a previous page's next cursor",
    placeholder: "<cursor>",
  },
};

const DURATION = /^(\d+)\s*([smhdw])$/u;
const UNIT_MILLISECONDS: Readonly<Record<string, number>> = {
  s: 1_000,
  m: 60_000,
  h: 3_600_000,
  d: 86_400_000,
  w: 604_800_000,
};

/** An ISO instant from a duration ago (`1h`, `7d`), `now`, or an ISO date-time. */
export function parseInstant(value: string, name: string, now = Date.now()): string {
  const trimmed = value.trim();
  if (trimmed === "now") return new Date(now).toISOString();
  const match = DURATION.exec(trimmed);
  if (match !== null) {
    const amount = Number(match[1]);
    const unit = UNIT_MILLISECONDS[match[2] ?? ""] ?? 0;
    return new Date(now - amount * unit).toISOString();
  }
  const parsed = Date.parse(trimmed);
  if (Number.isNaN(parsed)) {
    throw usageError(
      `--${name} must be a duration like 30m, 1h, 24h, 7d or an ISO date-time, got "${value}"`,
    );
  }
  return new Date(parsed).toISOString();
}

/** The window and page size from the common options; absent means the API's default. */
export function windowFrom(args: CommandArgs, now = Date.now()): ObservabilityQuery {
  const since = args.string("since");
  const until = args.string("until");
  const from = since === undefined ? undefined : parseInstant(since, "since", now);
  const to = until === undefined ? undefined : parseInstant(until, "until", now);
  if (from !== undefined && to !== undefined && from > to) {
    throw usageError("--since must be before --until");
  }
  const limit = args.integer("limit");
  if (limit !== undefined && (limit < 1 || limit > 1000)) {
    throw usageError("--limit must be between 1 and 1000");
  }
  return {
    ...(from !== undefined ? { from } : {}),
    ...(to !== undefined ? { until: to } : {}),
    ...(limit !== undefined ? { limit } : {}),
  };
}

function formatRetention(seconds: number): string {
  const days = seconds / 86_400;
  if (days >= 1) return `${Number.isInteger(days) ? days : days.toFixed(1)} days`;
  return `${seconds} seconds`;
}

/** The retention notice the console shows above every retained-signal view. */
export function retentionNotice(retention: Retention): string {
  return `retained from ${retention.retainedFrom}; observed at ${retention.observedAt} (${formatRetention(retention.retentionSeconds)} retention)`;
}

/** One page, or every page with --all, plus the retention block of the first page read. */
async function readSignals(
  context: CommandContext,
  args: CommandArgs,
  query: SignalQuery,
): Promise<{
  readonly page: Page<ObservabilityRecord>;
  readonly retention: Retention | undefined;
}> {
  const tenant = tenantFrom(context, args);
  const window = windowFrom(args);
  const start = args.string("cursor");
  const client = await context.management();
  let retention: Retention | undefined;
  const page = await context.collect(async (cursor) => {
    const next = cursor ?? start;
    const result = await query(client, tenant.projectId, tenant.environmentId, {
      ...window,
      ...(next !== undefined ? { cursor: next } : {}),
    });
    if (retention === undefined) retention = result.retention;
    return result;
  });
  return { page, retention };
}

function signalsOf<K extends Kind>(items: readonly ObservabilityRecord[], kind: K): Signal<K>[] {
  const signals: Signal<K>[] = [];
  for (const record of items) {
    if (record.payload.kind === kind) {
      signals.push({ timestamp: record.timestamp, payload: record.payload as PayloadOf<K> });
    }
  }
  return signals;
}

/** Stable descending sort by timestamp: equal instants keep their served order. */
function newestFirst<T extends { readonly timestamp: string }>(items: readonly T[]): T[] {
  return items
    .map((item, index) => ({ item, index, at: Date.parse(item.timestamp) }))
    .sort((left, right) => right.at - left.at || left.index - right.index)
    .map((entry) => entry.item);
}

/** Text written by customers or operators, kept to one terminal line with no control codes. */
function plain(text: string | null | undefined): string {
  if (text === null || text === undefined) return "—";
  return Array.from(text, (character) => {
    const code = character.charCodeAt(0);
    return (code < 32 && code !== 9) || code === 127 ? " " : character;
  }).join("");
}

function columns(...keys: readonly string[]): readonly TableColumn[] {
  return keys.map((key) => ({ key }));
}

interface SignalView<K extends Kind> {
  readonly kind: K;
  /** Human output sorts newest first, as the console does for this signal. */
  readonly newestFirst?: boolean;
  /** A flat table row per signal, rendered under `columns`. */
  readonly row?: (signal: Signal<K>) => Record<string, unknown>;
  readonly columns?: readonly TableColumn[];
  /** One human line per signal instead of a table. */
  readonly line?: (signal: Signal<K>) => string;
}

function emit<K extends Kind>(
  context: CommandContext,
  page: Page<ObservabilityRecord>,
  retention: Retention | undefined,
  view: SignalView<K>,
  filter: ((signal: Signal<K>) => boolean) | undefined,
): void {
  if (retention !== undefined) context.info(retentionNotice(retention));
  const signals = signalsOf(page.items, view.kind);
  const kept = filter === undefined ? signals : signals.filter(filter);
  if (context.json) {
    context.out(filter === undefined ? page : { ...page, items: kept });
    return;
  }
  const ordered = view.newestFirst ? newestFirst(kept) : kept;
  const human = {
    items: view.row === undefined ? ordered : ordered.map(view.row),
    nextCursor: page.nextCursor ?? null,
  };
  const line = view.line;
  context.out(
    human,
    line === undefined
      ? { columns: view.columns ?? [] }
      : { line: (item) => line(item as Signal<K>) },
  );
}

interface SignalCommandSpec<K extends Kind> {
  readonly path: readonly string[];
  readonly summary: string;
  readonly operation: string;
  readonly query: SignalQuery;
  readonly view: SignalView<K>;
  readonly options?: Readonly<Record<string, OptionSpec>>;
  readonly filter?: (args: CommandArgs) => ((signal: Signal<K>) => boolean) | undefined;
}

function signalCommand<K extends Kind>(spec: SignalCommandSpec<K>): Command {
  return {
    path: spec.path,
    summary: spec.summary,
    operations: [spec.operation],
    options: { ...TENANT_OPTIONS, ...QUERY_OPTIONS, ...(spec.options ?? {}) },
    run: async (context, args) => {
      const filter = spec.filter?.(args);
      const { page, retention } = await readSignals(context, args, spec.query);
      emit(context, page, retention, spec.view, filter);
    },
  };
}

// ---- usage: flows sum their records, levels average their samples ---------

/** Resources whose records are summed over a period. */
const FLOW_RESOURCES: readonly string[] = [
  "replication_requests_per_minute",
  "replication_bytes_per_month",
  "edge_invocations_per_month",
  "edge_compute_milliseconds_per_month",
  "log_bytes_per_month",
];

/** Resources sampled as a height; a period's figure is the sample average. */
const LEVEL_RESOURCES: readonly string[] = [
  "storage_bytes",
  "application_users",
  "environments",
  "collections_per_environment",
  "edge_functions",
];

/** How the bill aggregates a resource's records; unknown resources count as flows. */
export function resourceKind(resource: string): "flow" | "level" {
  if (LEVEL_RESOURCES.includes(resource)) return "level";
  if (FLOW_RESOURCES.includes(resource)) return "flow";
  return "flow";
}

const usageView: SignalView<"usage"> = {
  kind: "usage",
  row: ({ timestamp, payload }) => ({
    time: timestamp,
    resource: payload.resource,
    kind: resourceKind(payload.resource),
    quantity: payload.quantity,
    unit: payload.unit,
  }),
  columns: columns("time", "resource", "kind", "quantity", "unit"),
};

const quotaView: SignalView<"quota"> = {
  kind: "quota",
  row: ({ timestamp, payload }) => ({
    time: timestamp,
    resource: payload.resource,
    consumed: payload.consumed,
    limit: payload.limit,
    used:
      payload.limit > 0
        ? `${Math.min(100, (payload.consumed / payload.limit) * 100).toFixed(1)}%`
        : "—",
    retryAfter: payload.retryAfter ?? "hard limit / no retry time",
  }),
  columns: columns("time", "resource", "consumed", "limit", "used", "retryAfter"),
};

const healthView: SignalView<"health"> = {
  kind: "health",
  row: ({ timestamp, payload }) => ({
    time: timestamp,
    service: payload.service,
    region: payload.region,
    status: payload.status,
    diagnostic: plain(payload.diagnostic),
  }),
  columns: columns("time", "service", "region", "status", "diagnostic"),
};

const replicationErrorView: SignalView<"replication_error"> = {
  kind: "replication_error",
  row: ({ timestamp, payload }) => ({
    time: timestamp,
    collection: payload.collectionId,
    category: payload.category,
    retryable: payload.retryable,
    message: plain(payload.message),
    correlation: payload.correlationId,
  }),
  columns: columns("time", "collection", "category", "retryable", "message", "correlation"),
};

const authenticationEventView: SignalView<"authentication_event"> = {
  kind: "authentication_event",
  row: ({ timestamp, payload }) => ({
    time: timestamp,
    category: payload.category,
    outcome: payload.outcome,
    applicationUser: payload.applicationUserId,
    message: plain(payload.message),
    correlation: payload.correlationId,
  }),
  columns: columns("time", "category", "outcome", "applicationUser", "message", "correlation"),
};

const functionMetricView: SignalView<"function_metric"> = {
  kind: "function_metric",
  row: ({ timestamp, payload }) => ({
    time: timestamp,
    function: payload.functionName,
    version: payload.version,
    region: payload.region,
    invocations: payload.invocationCount,
    errors: payload.errorCount,
    latencyMs: payload.latencyMilliseconds,
    computeMs: payload.computeMilliseconds,
  }),
  columns: columns(
    "time",
    "function",
    "version",
    "region",
    "invocations",
    "errors",
    "latencyMs",
    "computeMs",
  ),
};

const indexStateView: SignalView<"index_state"> = {
  kind: "index_state",
  newestFirst: true,
  row: ({ timestamp, payload }) => ({
    time: timestamp,
    collection: payload.collectionId,
    index: payload.indexName,
    version: payload.indexVersion,
    state: payload.state,
    progress: `${payload.progressPercent}%`,
    message: plain(payload.message),
  }),
  columns: columns("time", "collection", "index", "version", "state", "progress", "message"),
};

const auditView: SignalView<"audit"> = {
  kind: "audit",
  newestFirst: true,
  line: ({ timestamp, payload }) => {
    const head = `${timestamp}  ${payload.actorId}  ${payload.action}  ${plain(payload.target)}  ${payload.outcome}`;
    return payload.details === null || payload.details === ""
      ? head
      : `${head}  ${plain(payload.details)} (request ${payload.requestId})`;
  },
};

// ---- logs -------------------------------------------------------------------

const LOG_LEVELS = ["debug", "info", "warn", "error"] as const;
type LogLevel = (typeof LOG_LEVELS)[number];

/** Levels the filter understands, as the console normalizes them; others stay under "all". */
export function normalizeLevel(level: string): LogLevel | null {
  const lowered = level.trim().toLowerCase();
  if (lowered === "warning") return "warn";
  if (lowered === "err" || lowered === "fatal" || lowered === "critical") return "error";
  if (lowered === "trace" || lowered === "verbose") return "debug";
  return (LOG_LEVELS as readonly string[]).includes(lowered) ? (lowered as LogLevel) : null;
}

const LOG_OPTIONS: Readonly<Record<string, OptionSpec>> = {
  level: {
    type: "string",
    description: "Keep only lines at this level: debug, info, warn, or error",
    placeholder: "<level>",
  },
  source: {
    type: "string",
    description: "Keep only lines from this source (as the log line names it)",
    placeholder: "<source>",
  },
};

function logFilter(args: CommandArgs): ((signal: Signal<"project_log">) => boolean) | undefined {
  const rawLevel = args.string("level");
  const level = rawLevel === undefined ? undefined : normalizeLevel(rawLevel);
  if (rawLevel !== undefined && level === null) {
    throw usageError(`--level must be one of ${LOG_LEVELS.join(", ")}, got "${rawLevel}"`);
  }
  const source = args.string("source");
  if (level === undefined && source === undefined) return undefined;
  return ({ payload }) =>
    (level === undefined || normalizeLevel(payload.level) === level) &&
    (source === undefined || payload.source === source);
}

const logView: SignalView<"project_log"> = {
  kind: "project_log",
  newestFirst: true,
  line: ({ timestamp, payload }) =>
    `${timestamp}  ${payload.level}  ${payload.source}  ${plain(payload.message)}`,
};

// ---- the commands -----------------------------------------------------------

function logsCommand(path: readonly string[]): Command {
  return signalCommand({
    path,
    summary: "Retained, scrubbed log lines from functions, the data plane, and sync (newest first)",
    operation: "queryProjectLogs",
    query: (client, projectId, environmentId, query) =>
      client.queryProjectLogs(projectId, environmentId, query),
    view: logView,
    options: LOG_OPTIONS,
    filter: logFilter,
  });
}

function auditCommand(path: readonly string[], summary: string): Command {
  return signalCommand({
    path,
    summary,
    operation: "queryAuditEvents",
    query: (client, projectId, environmentId, query) =>
      client.queryAuditEvents(projectId, environmentId, query),
    view: auditView,
  });
}

function usageCommand(path: readonly string[]): Command {
  return signalCommand({
    path,
    summary:
      "Retained usage samples per resource (flows sum their records; levels average their samples)",
    operation: "queryProjectUsage",
    query: (client, projectId, environmentId, query) =>
      client.queryProjectUsage(projectId, environmentId, query),
    view: usageView,
  });
}

export const observabilityCommands: readonly Command[] = [
  logsCommand(["logs"]),
  auditCommand(["activity"], "Audited actions in this environment, newest first"),
  usageCommand(["usage"]),
  logsCommand(["observability", "logs"]),
  auditCommand(
    ["observability", "audit"],
    "Append-only administration history for this environment, newest first",
  ),
  usageCommand(["observability", "usage"]),
  signalCommand({
    path: ["observability", "quotas"],
    summary: "Consumption against enforced limits, with the retry time when work was throttled",
    operation: "queryProjectQuotas",
    query: (client, projectId, environmentId, query) =>
      client.queryProjectQuotas(projectId, environmentId, query),
    view: quotaView,
  }),
  signalCommand({
    path: ["observability", "health"],
    summary: "Regional data-plane service status and sanitized diagnostics",
    operation: "queryProjectHealth",
    query: (client, projectId, environmentId, query) =>
      client.queryProjectHealth(projectId, environmentId, query),
    view: healthView,
  }),
  signalCommand({
    path: ["observability", "replication-errors"],
    summary: "RxDB replication failures with retry guidance and correlation identifiers",
    operation: "queryReplicationErrors",
    query: (client, projectId, environmentId, query) =>
      client.queryReplicationErrors(projectId, environmentId, query),
    view: replicationErrorView,
  }),
  signalCommand({
    path: ["observability", "auth-events"],
    summary: "Sanitized application authentication outcomes, without credentials",
    operation: "queryAuthenticationEvents",
    query: (client, projectId, environmentId, query) =>
      client.queryAuthenticationEvents(projectId, environmentId, query),
    view: authenticationEventView,
  }),
  signalCommand({
    path: ["observability", "function-metrics"],
    summary: "Invocation, error, latency, and compute counts per function version and region",
    operation: "queryFunctionMetrics",
    query: (client, projectId, environmentId, query) =>
      client.queryFunctionMetrics(projectId, environmentId, query),
    view: functionMetricView,
  }),
  signalCommand({
    path: ["observability", "index-state"],
    summary: "Index build state events per collection index, newest first",
    operation: "queryIndexStateEvents",
    query: (client, projectId, environmentId, query) =>
      client.queryIndexStateEvents(projectId, environmentId, query),
    view: indexStateView,
  }),
];
