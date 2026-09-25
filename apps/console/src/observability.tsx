import {
  Alert,
  AlertDescription,
  Badge,
  Button,
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  Eyebrow,
  Field,
  Input,
  Progress,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  Tabs,
  TabsContent,
  TabsLine,
  TabsTrigger,
  cn,
} from "@mako-cloud/ui";
import { Download, Info } from "lucide-react";
import { type FormEvent, useCallback, useEffect, useId, useMemo, useState } from "react";

import type {
  ObservabilityPage,
  ObservabilityPayload,
  ObservabilityQuery,
  ObservabilityRecord,
} from "@mako-cloud/management-sdk";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { withOccurrenceKeys } from "./list-keys.js";
import { useManagementClient } from "./management.js";

const VIEW_IDS = ["usage", "quotas", "health", "replication", "auth", "audit"] as const;
type ViewId = (typeof VIEW_IDS)[number];
type ViewPages = Partial<Record<ViewId, ObservabilityPage>>;

const VIEW_LABELS: Record<ViewId, string> = {
  usage: "Usage",
  quotas: "Quotas",
  health: "Service health",
  replication: "Replication errors",
  auth: "Auth events",
  audit: "Audit history",
};

/** Columns whose values are identifiers, set in monospace. */
const IDENTIFIER_COLUMNS: ReadonlySet<string> = new Set([
  "Collection",
  "Correlation",
  "Request ID",
  "Application user",
  "Actor",
  "Target",
]);

/** Columns whose values are quantities, set right-aligned in tabular figures so magnitudes line up. */
const MEASURED_COLUMNS: ReadonlySet<string> = new Set([
  "Quantity",
  "Consumed",
  "Limit",
  "Utilization",
]);

/** A code snippet inline in a cell: an identifier. */
const CODE = "rounded bg-muted px-1 py-0.5 font-mono text-[0.85em]";

export function ObservabilityScreen({
  projectId,
  environmentId,
  onBack,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly onBack: () => void;
}) {
  const client = useManagementClient();
  const id = useId();
  const [activeView, setActiveView] = useState<ViewId>("usage");
  const [pages, setPages] = useState<ViewPages>({});
  const [query, setQuery] = useState<ObservabilityQuery>({ limit: 250 });
  const [search, setSearch] = useState("");
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [loading, setLoading] = useState(true);

  const queryView = useCallback(
    async (view: ViewId, input: ObservabilityQuery) => {
      switch (view) {
        case "usage":
          return client.queryProjectUsage(projectId, environmentId, input);
        case "quotas":
          return client.queryProjectQuotas(projectId, environmentId, input);
        case "health":
          return client.queryProjectHealth(projectId, environmentId, input);
        case "replication":
          return client.queryReplicationErrors(projectId, environmentId, input);
        case "auth":
          return client.queryAuthenticationEvents(projectId, environmentId, input);
        case "audit":
          return client.queryAuditEvents(projectId, environmentId, input);
      }
    },
    [client, environmentId, projectId],
  );

  const reload = useCallback(
    async (input: ObservabilityQuery) => {
      setLoading(true);
      try {
        const results = await Promise.all(VIEW_IDS.map((view) => queryView(view, input)));
        setPages(Object.fromEntries(VIEW_IDS.map((view, index) => [view, results[index]])));
        setFailure(null);
      } catch (error) {
        setFailure(toConsoleApiFailure(error));
      } finally {
        setLoading(false);
      }
    },
    [queryView],
  );

  useEffect(() => {
    void reload(query);
  }, [query, reload]);

  const applyRange = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    try {
      const data = new FormData(event.currentTarget);
      const from = optionalDate(data, "from");
      const until = optionalDate(data, "until");
      if (from !== undefined && until !== undefined && from > until) {
        throw new Error("The start of the time range must be before its end.");
      }
      setQuery({
        limit: 250,
        ...(from === undefined ? {} : { from }),
        ...(until === undefined ? {} : { until }),
      });
      setFailure(null);
    } catch (error) {
      setFailure({
        message: error instanceof Error ? error.message : "Invalid time range.",
        requestId: null,
      });
    }
  };

  const loadMore = async () => {
    const page = pages[activeView];
    if (page?.nextCursor == null) {
      return;
    }
    try {
      const next = await queryView(activeView, { ...query, cursor: page.nextCursor });
      setPages((current) => ({
        ...current,
        [activeView]: { ...next, items: [...page.items, ...next.items] },
      }));
      setFailure(null);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  };

  const page = pages[activeView];
  const visibleRecords = useMemo(
    () => filterObservabilityRecords(page?.items ?? [], search),
    [page?.items, search],
  );

  return (
    <section aria-labelledby="observability-title" className="grid gap-6">
      <div className="grid gap-2">
        <Button
          variant="ghost"
          size="sm"
          className="-ml-2 w-fit text-muted-foreground hover:text-foreground"
          onClick={onBack}
        >
          ← Project
        </Button>
        <div className="grid gap-1">
          <Eyebrow>Environment {environmentId}</Eyebrow>
          <h1 id="observability-title" className="text-2xl">
            Usage and observability
          </h1>
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      <Card aria-labelledby="observability-filters-title">
        <CardHeader>
          <CardTitle id="observability-filters-title">Retained data window</CardTitle>
        </CardHeader>
        <CardContent className="grid gap-4">
          <form className="flex flex-wrap items-end gap-3" onSubmit={applyRange}>
            <Field label="From" htmlFor={`${id}-from`} className="w-56">
              <Input id={`${id}-from`} name="from" type="datetime-local" />
            </Field>
            <Field label="Until" htmlFor={`${id}-until`} className="w-56">
              <Input id={`${id}-until`} name="until" type="datetime-local" />
            </Field>
            <Button type="submit">Apply time range</Button>
          </form>
          {page === undefined ? null : <RetentionNotice page={page} />}
        </CardContent>
      </Card>
      <Tabs
        value={activeView}
        onValueChange={(value) => {
          if ((VIEW_IDS as readonly string[]).includes(value)) {
            setActiveView(value as ViewId);
            setSearch("");
          }
        }}
        className="gap-6"
      >
        <TabsLine aria-label="Observability views" className="overflow-x-auto">
          {VIEW_IDS.map((view) => (
            <TabsTrigger key={view} value={view}>
              {VIEW_LABELS[view]}
            </TabsTrigger>
          ))}
        </TabsLine>
        <TabsContent value={activeView}>
          <Card aria-labelledby="observability-view-title">
            <CardHeader>
              <CardTitle id="observability-view-title">{VIEW_LABELS[activeView]}</CardTitle>
              <CardDescription>{viewDescription(activeView)}</CardDescription>
              <CardAction className="flex flex-wrap gap-2">
                <Button
                  variant="outline"
                  size="sm"
                  disabled={visibleRecords.length === 0}
                  onClick={() => downloadExport(activeView, "json", visibleRecords)}
                >
                  <Download aria-hidden="true" />
                  Export JSON
                </Button>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={visibleRecords.length === 0}
                  onClick={() => downloadExport(activeView, "csv", visibleRecords)}
                >
                  <Download aria-hidden="true" />
                  Export CSV
                </Button>
              </CardAction>
            </CardHeader>
            <CardContent className="grid gap-4">
              <Field label="Search loaded records" htmlFor={`${id}-search`} className="max-w-xl">
                <Input
                  id={`${id}-search`}
                  type="search"
                  value={search}
                  onChange={(event) => setSearch(event.currentTarget.value)}
                  placeholder={
                    activeView === "audit"
                      ? "Actor, action, target, outcome, or request ID"
                      : "Search any visible field"
                  }
                />
              </Field>
              {loading && page === undefined ? (
                <p className="m-0 text-sm text-muted-foreground" aria-live="polite">
                  Loading retained signals…
                </p>
              ) : (
                <ObservabilityTable view={activeView} records={visibleRecords} />
              )}
              <div className="flex flex-wrap items-center justify-between gap-3">
                <small className="text-xs text-muted-foreground tabular-nums">
                  Showing {visibleRecords.length} of {page?.items.length ?? 0} loaded records.
                </small>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={page?.nextCursor == null || loading}
                  onClick={() => void loadMore()}
                >
                  Load more retained records
                </Button>
              </div>
            </CardContent>
          </Card>
        </TabsContent>
      </Tabs>
      <IndexStatePanel projectId={projectId} environmentId={environmentId} query={query} />
    </section>
  );
}

export function RetentionNotice({ page }: { readonly page: ObservabilityPage }) {
  return (
    <Alert role="status">
      <Info aria-hidden="true" />
      <AlertDescription className="block tabular-nums">
        Retained from {new Date(page.retention.retainedFrom).toLocaleString()}; observed at{" "}
        {new Date(page.retention.observedAt).toLocaleString()} (
        {formatDuration(page.retention.retentionSeconds)} retention).
      </AlertDescription>
    </Alert>
  );
}

function ObservabilityTable({
  view,
  records,
}: {
  readonly view: ViewId;
  readonly records: readonly ObservabilityRecord[];
}) {
  if (records.length === 0) {
    return (
      <p className="m-0 text-sm text-muted-foreground">
        No retained records match this view and search.
      </p>
    );
  }
  return (
    <Table>
      <TableHeader>
        <TableRow className="hover:bg-transparent">
          <TableHead scope="col">Time</TableHead>
          {columns(view).map((column) => (
            <TableHead
              scope="col"
              key={column}
              className={cn(MEASURED_COLUMNS.has(column) && "text-right")}
            >
              {column}
            </TableHead>
          ))}
        </TableRow>
      </TableHeader>
      <TableBody>
        {withOccurrenceKeys(records, recordIdentity).map(({ item: record, key }) => (
          <TableRow key={key}>
            <TableCell className="align-top text-muted-foreground tabular-nums">
              {new Date(record.timestamp).toLocaleString()}
            </TableCell>
            {columns(view).map((column, cellIndex) => (
              <TableCell
                key={column}
                className={cn(
                  "max-w-md align-top whitespace-normal break-words",
                  IDENTIFIER_COLUMNS.has(column) && "font-mono text-xs",
                  MEASURED_COLUMNS.has(column) && "text-right tabular-nums",
                )}
              >
                {cells(view, record.payload)[cellIndex]}
              </TableCell>
            ))}
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

function columns(view: ViewId): readonly string[] {
  switch (view) {
    case "usage":
      return ["Resource", "Quantity", "Unit"];
    case "quotas":
      return ["Resource", "Consumed", "Limit", "Utilization", "Retry after"];
    case "health":
      return ["Service", "Region", "Status", "Diagnostic"];
    case "replication":
      return ["Collection", "Category", "Retryable", "Message", "Correlation"];
    case "auth":
      return ["Category", "Outcome", "Application user", "Message", "Correlation"];
    case "audit":
      return ["Actor", "Action", "Target", "Outcome", "Request ID", "Details"];
  }
}

function cells(view: ViewId, payload: ObservabilityPayload): readonly string[] {
  switch (view) {
    case "usage":
      return payload.kind === "usage"
        ? [humanize(payload.resource), payload.quantity.toLocaleString(), payload.unit]
        : [];
    case "quotas":
      return payload.kind === "quota"
        ? [
            humanize(payload.resource),
            payload.consumed.toLocaleString(),
            payload.limit.toLocaleString(),
            `${Math.min(100, (payload.consumed / payload.limit) * 100).toFixed(1)}%`,
            payload.retryAfter === null
              ? "Hard limit / no retry time"
              : new Date(payload.retryAfter).toLocaleString(),
          ]
        : [];
    case "health":
      return payload.kind === "health"
        ? [payload.service, payload.region, humanize(payload.status), payload.diagnostic ?? "—"]
        : [];
    case "replication":
      return payload.kind === "replication_error"
        ? [
            payload.collectionId,
            payload.category,
            payload.retryable ? "Yes" : "No",
            payload.message,
            payload.correlationId,
          ]
        : [];
    case "auth":
      return payload.kind === "authentication_event"
        ? [
            payload.category,
            humanize(payload.outcome),
            payload.applicationUserId ?? "—",
            payload.message,
            payload.correlationId,
          ]
        : [];
    case "audit":
      return payload.kind === "audit"
        ? [
            payload.actorId,
            payload.action,
            payload.target,
            humanize(payload.outcome),
            payload.requestId,
            payload.details ?? "—",
          ]
        : [];
  }
}

export function filterObservabilityRecords(
  records: readonly ObservabilityRecord[],
  search: string,
): ObservabilityRecord[] {
  const needle = search.trim().toLocaleLowerCase();
  return records.filter(
    (record) =>
      needle === "" ||
      `${record.timestamp} ${flattenPayload(record.payload)}`.toLocaleLowerCase().includes(needle),
  );
}

export function observabilityRecordsToCsv(records: readonly ObservabilityRecord[]): string {
  const keys = Array.from(
    new Set(
      records.flatMap((record) => Object.keys(record.payload).filter((key) => key !== "kind")),
    ),
  ).sort();
  const rows = [
    ["timestamp", "kind", ...keys],
    ...records.map((record) => [
      record.timestamp,
      record.payload.kind,
      ...keys.map((key) => exportValue(record.payload[key as keyof ObservabilityPayload])),
    ]),
  ];
  return `${rows.map((row) => row.map(csvCell).join(",")).join("\n")}\n`;
}

function downloadExport(
  view: ViewId,
  format: "json" | "csv",
  records: readonly ObservabilityRecord[],
) {
  const content =
    format === "json"
      ? `${JSON.stringify(records, null, 2)}\n`
      : observabilityRecordsToCsv(records);
  const blob = new Blob([content], {
    type: format === "json" ? "application/json" : "text/csv;charset=utf-8",
  });
  const url = URL.createObjectURL(blob);
  const link = document.createElement("a");
  link.href = url;
  link.download = `mako-${view}-${new Date().toISOString().replaceAll(":", "-")}.${format}`;
  link.click();
  URL.revokeObjectURL(url);
}

function optionalDate(data: FormData, name: string): string | undefined {
  const raw = String(data.get(name) ?? "").trim();
  if (raw === "") {
    return undefined;
  }
  const date = new Date(raw);
  if (Number.isNaN(date.getTime())) {
    throw new Error(`${name} must be a valid date and time.`);
  }
  return date.toISOString();
}

function flattenPayload(payload: ObservabilityPayload): string {
  return Object.values(payload)
    .map((value) => exportValue(value))
    .join(" ");
}

function exportValue(value: unknown): string {
  if (value === null || value === undefined) {
    return "";
  }
  return typeof value === "object" ? JSON.stringify(value) : String(value);
}

function csvCell(value: string): string {
  const safe = /^[\t\r ]*[=+\-@]/u.test(value) ? `'${value}` : value;
  return `"${safe.replaceAll('"', '""')}"`;
}

/** A record's time and whole payload: two records share it only as exact copies. */
function recordIdentity(record: {
  readonly timestamp: string;
  readonly payload: ObservabilityPayload;
}): string {
  return `${record.timestamp}:${flattenPayload(record.payload)}`;
}

function humanize(value: string): string {
  return value.replaceAll("_", " ");
}

function formatDuration(seconds: number): string {
  const days = seconds / 86_400;
  return days >= 1
    ? `${days.toLocaleString(undefined, { maximumFractionDigits: 1 })} days`
    : `${seconds.toLocaleString()} seconds`;
}

function viewDescription(view: ViewId): string {
  switch (view) {
    case "usage":
      return "Retained measurements for billable and limited project resources.";
    case "quotas":
      return "Consumption against enforced limits, including whether throttled work can be retried.";
    case "health":
      return "Regional data-plane service status and sanitized diagnostics.";
    case "replication":
      return "RxDB replication failures with retry guidance and correlation identifiers.";
    case "auth":
      return "Sanitized authentication outcomes without credentials or submitted secrets.";
    case "audit":
      return "Append-only administration history. Search and export operate on the retained records loaded below.";
  }
}

type IndexStateEvent = Extract<ObservabilityPayload, { kind: "index_state" }>;

export interface IndexStateGroup {
  readonly key: string;
  readonly collectionId: string;
  readonly indexName: string;
  /** The newest retained event for this index. */
  readonly latest: ObservabilityRecord & { readonly payload: IndexStateEvent };
  /** Every retained event for this index, newest first. */
  readonly history: readonly (ObservabilityRecord & { readonly payload: IndexStateEvent })[];
}

// Index build state: the retained index-state events grouped per collection
// index, newest first, with the latest state leading each group.
function IndexStatePanel({
  projectId,
  environmentId,
  query,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly query: ObservabilityQuery;
}) {
  const client = useManagementClient();
  const [page, setPage] = useState<ObservabilityPage | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    client
      .queryIndexStateEvents(projectId, environmentId, query)
      .then((next) => {
        if (!cancelled) {
          setPage(next);
          setFailure(null);
        }
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          setFailure(toConsoleApiFailure(error));
        }
      })
      .finally(() => {
        if (!cancelled) {
          setLoading(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [client, environmentId, projectId, query]);

  const groups = useMemo(() => groupIndexStates(page?.items ?? []), [page?.items]);
  const eventCount = groups.reduce((total, group) => total + group.history.length, 0);

  return (
    <Card aria-labelledby="index-state-title">
      <CardHeader>
        <CardTitle id="index-state-title">Indexes</CardTitle>
        <CardDescription>
          Build state per collection index from the retained index-state events, newest first.
        </CardDescription>
        <CardAction>
          <span className="text-sm text-muted-foreground tabular-nums">
            {groups.length} {groups.length === 1 ? "index" : "indexes"} · {eventCount}{" "}
            {eventCount === 1 ? "event" : "events"}
          </span>
        </CardAction>
      </CardHeader>
      <CardContent className="grid gap-4">
        <ApiFailureNotice failure={failure} />
        {loading && page === null ? (
          <p className="m-0 text-sm text-muted-foreground" aria-live="polite">
            Loading index states…
          </p>
        ) : groups.length === 0 ? (
          <p className="m-0 text-sm text-muted-foreground">
            No index build events are retained for this window.
          </p>
        ) : (
          <Table className="index-state-table">
            <TableHeader>
              <TableRow className="hover:bg-transparent">
                <TableHead scope="col">Collection</TableHead>
                <TableHead scope="col">Index</TableHead>
                <TableHead scope="col">Version</TableHead>
                <TableHead scope="col">State</TableHead>
                <TableHead scope="col">Progress</TableHead>
                <TableHead scope="col">Message</TableHead>
                <TableHead scope="col">Observed</TableHead>
                <TableHead scope="col">History</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {groups.map((group) => {
                const tone = indexStateTone(group.latest.payload.state);
                return (
                  <TableRow key={group.key}>
                    <TableCell className="align-top">
                      <code className={CODE}>{group.collectionId}</code>
                    </TableCell>
                    <TableCell className="align-top">
                      <strong className="font-mono text-xs font-semibold">{group.indexName}</strong>
                    </TableCell>
                    <TableCell className="align-top font-mono text-xs">
                      v{group.latest.payload.indexVersion}
                    </TableCell>
                    <TableCell className="align-top">
                      <Badge className="status-pill" variant={tone}>
                        {humanize(group.latest.payload.state)}
                      </Badge>
                    </TableCell>
                    <TableCell className="align-top">
                      <IndexProgress
                        percent={group.latest.payload.progressPercent}
                        tone={tone}
                        label={`${group.collectionId} ${group.indexName} build progress`}
                      />
                    </TableCell>
                    <TableCell className="max-w-md align-top whitespace-normal break-words">
                      {group.latest.payload.message ?? "—"}
                    </TableCell>
                    <TableCell className="align-top text-muted-foreground tabular-nums">
                      {new Date(group.latest.timestamp).toLocaleString()}
                    </TableCell>
                    <TableCell className="align-top whitespace-normal">
                      {group.history.length < 2 ? (
                        "—"
                      ) : (
                        <details>
                          <summary className="cursor-pointer text-muted-foreground hover:text-foreground">
                            {group.history.length} events
                          </summary>
                          <ol className="m-0 mt-2 grid gap-1 pl-5 text-xs">
                            {withOccurrenceKeys(group.history, recordIdentity).map(
                              ({ item: record, key }) => (
                                <li key={key}>
                                  {new Date(record.timestamp).toLocaleString()}:{" "}
                                  {humanize(record.payload.state)} v{record.payload.indexVersion} (
                                  {clampPercent(record.payload.progressPercent)}%)
                                  {record.payload.message === null
                                    ? ""
                                    : ` — ${record.payload.message}`}
                                </li>
                              ),
                            )}
                          </ol>
                        </details>
                      )}
                    </TableCell>
                  </TableRow>
                );
              })}
            </TableBody>
          </Table>
        )}
      </CardContent>
    </Card>
  );
}

function IndexProgress({
  percent,
  tone,
  label,
}: {
  readonly percent: number;
  readonly tone: IndexStateTone;
  readonly label: string;
}) {
  const value = clampPercent(percent);
  return (
    <span className="flex min-w-36 items-center gap-2">
      <Progress value={value} tone={tone} aria-label={label} className="h-1.5" />
      <small className="text-xs text-muted-foreground tabular-nums">{value}%</small>
    </span>
  );
}

/** Index-state events grouped per collection index, newest first, latest state leading. */
export function groupIndexStates(records: readonly ObservabilityRecord[]): IndexStateGroup[] {
  const events = records
    .flatMap((record) =>
      record.payload.kind === "index_state" ? [{ ...record, payload: record.payload }] : [],
    )
    .sort((left, right) => Date.parse(right.timestamp) - Date.parse(left.timestamp));
  const groups = new Map<string, IndexStateGroup>();
  for (const event of events) {
    const key = `${event.payload.collectionId}/${event.payload.indexName}`;
    const existing = groups.get(key);
    if (existing === undefined) {
      groups.set(key, {
        key,
        collectionId: event.payload.collectionId,
        indexName: event.payload.indexName,
        latest: event,
        history: [event],
      });
    } else {
      groups.set(key, { ...existing, history: [...existing.history, event] });
    }
  }
  return Array.from(groups.values());
}

type IndexStateTone = "destructive" | "positive" | "warning";

/** The colour an index state carries: failed is destructive, ready is positive, anything in between is a warning. */
function indexStateTone(state: string): IndexStateTone {
  const lowered = state.toLowerCase();
  if (lowered.includes("fail") || lowered.includes("error")) {
    return "destructive";
  }
  if (["ready", "active", "built", "complete", "completed"].includes(lowered)) {
    return "positive";
  }
  return "warning";
}

function clampPercent(percent: number): number {
  return Number.isFinite(percent) ? Math.max(0, Math.min(100, Math.round(percent))) : 0;
}
