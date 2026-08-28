import { type FormEvent, useCallback, useEffect, useMemo, useState } from "react";

import type {
  ObservabilityPage,
  ObservabilityPayload,
  ObservabilityQuery,
  ObservabilityRecord,
} from "@mako-cloud/management-sdk";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
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
    <section aria-labelledby="observability-title">
      <button type="button" className="back-link" onClick={onBack}>
        ← Project
      </button>
      <div className="section-heading">
        <div>
          <p className="eyebrow">Environment {environmentId}</p>
          <h1 id="observability-title">Usage and observability</h1>
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      <section className="panel full-span" aria-labelledby="observability-filters-title">
        <h2 id="observability-filters-title">Retained data window</h2>
        <form className="filter-grid" onSubmit={applyRange}>
          <label>
            From
            <input name="from" type="datetime-local" />
          </label>
          <label>
            Until
            <input name="until" type="datetime-local" />
          </label>
          <button type="submit">Apply time range</button>
        </form>
        {page === undefined ? null : <RetentionNotice page={page} />}
      </section>
      <div className="tab-list" role="tablist" aria-label="Observability views">
        {VIEW_IDS.map((view) => (
          <button
            key={view}
            type="button"
            role="tab"
            aria-selected={activeView === view}
            className={activeView === view ? "active" : "secondary"}
            onClick={() => {
              setActiveView(view);
              setSearch("");
            }}
          >
            {VIEW_LABELS[view]}
          </button>
        ))}
      </div>
      <section className="panel full-span" aria-labelledby="observability-view-title">
        <div className="button-row spread">
          <div>
            <h2 id="observability-view-title">{VIEW_LABELS[activeView]}</h2>
            <p>{viewDescription(activeView)}</p>
          </div>
          <div className="button-row">
            <button
              type="button"
              className="secondary"
              disabled={visibleRecords.length === 0}
              onClick={() => downloadExport(activeView, "json", visibleRecords)}
            >
              Export JSON
            </button>
            <button
              type="button"
              className="secondary"
              disabled={visibleRecords.length === 0}
              onClick={() => downloadExport(activeView, "csv", visibleRecords)}
            >
              Export CSV
            </button>
          </div>
        </div>
        <label className="search-field">
          Search loaded records
          <input
            type="search"
            value={search}
            onChange={(event) => setSearch(event.currentTarget.value)}
            placeholder={
              activeView === "audit"
                ? "Actor, action, target, outcome, or request ID"
                : "Search any visible field"
            }
          />
        </label>
        {loading && page === undefined ? (
          <p aria-live="polite">Loading retained signals…</p>
        ) : (
          <ObservabilityTable view={activeView} records={visibleRecords} />
        )}
        <div className="button-row spread">
          <small>
            Showing {visibleRecords.length} of {page?.items.length ?? 0} loaded records.
          </small>
          <button
            type="button"
            className="secondary"
            disabled={page?.nextCursor == null || loading}
            onClick={() => void loadMore()}
          >
            Load more retained records
          </button>
        </div>
      </section>
      <IndexStatePanel projectId={projectId} environmentId={environmentId} query={query} />
    </section>
  );
}

export function RetentionNotice({ page }: { readonly page: ObservabilityPage }) {
  return (
    <p className="notice" role="status">
      Retained from {new Date(page.retention.retainedFrom).toLocaleString()}; observed at{" "}
      {new Date(page.retention.observedAt).toLocaleString()} (
      {formatDuration(page.retention.retentionSeconds)} retention).
    </p>
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
    return <p>No retained records match this view and search.</p>;
  }
  return (
    <div className="table-scroll">
      <table>
        <thead>
          <tr>
            <th scope="col">Time</th>
            {columns(view).map((column) => (
              <th scope="col" key={column}>
                {column}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {records.map((record) => (
            <tr key={`${record.timestamp}:${payloadIdentity(record.payload)}`}>
              <td>{new Date(record.timestamp).toLocaleString()}</td>
              {columns(view).map((column, cellIndex) => (
                <td key={column}>{cells(view, record.payload)[cellIndex]}</td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
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

function payloadIdentity(payload: ObservabilityPayload): string {
  return flattenPayload(payload).slice(0, 128);
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
    <section className="panel full-span" aria-labelledby="index-state-title">
      <div className="button-row spread">
        <div>
          <h2 id="index-state-title">Indexes</h2>
          <p>
            Build state per collection index from the retained index-state events, newest first.
          </p>
        </div>
        <small>
          {groups.length} {groups.length === 1 ? "index" : "indexes"} · {eventCount}{" "}
          {eventCount === 1 ? "event" : "events"}
        </small>
      </div>
      <ApiFailureNotice failure={failure} />
      {loading && page === null ? (
        <p aria-live="polite">Loading index states…</p>
      ) : groups.length === 0 ? (
        <p>No index build events are retained for this window.</p>
      ) : (
        <div className="table-scroll">
          <table className="index-state-table">
            <thead>
              <tr>
                <th scope="col">Collection</th>
                <th scope="col">Index</th>
                <th scope="col">Version</th>
                <th scope="col">State</th>
                <th scope="col">Progress</th>
                <th scope="col">Message</th>
                <th scope="col">Observed</th>
                <th scope="col">History</th>
              </tr>
            </thead>
            <tbody>
              {groups.map((group) => (
                <tr key={group.key}>
                  <td>
                    <code>{group.collectionId}</code>
                  </td>
                  <td>
                    <strong>{group.indexName}</strong>
                  </td>
                  <td>v{group.latest.payload.indexVersion}</td>
                  <td>
                    <span className={`status-pill ${indexStateClass(group.latest.payload.state)}`}>
                      {humanize(group.latest.payload.state)}
                    </span>
                  </td>
                  <td>
                    <IndexProgress
                      percent={group.latest.payload.progressPercent}
                      label={`${group.collectionId} ${group.indexName} build progress`}
                    />
                  </td>
                  <td>{group.latest.payload.message ?? "—"}</td>
                  <td>{new Date(group.latest.timestamp).toLocaleString()}</td>
                  <td>
                    {group.history.length < 2 ? (
                      "—"
                    ) : (
                      <details>
                        <summary>{group.history.length} events</summary>
                        <ol className="index-history">
                          {group.history.map((record) => (
                            <li key={record.timestamp}>
                              {new Date(record.timestamp).toLocaleString()}:{" "}
                              {humanize(record.payload.state)} v{record.payload.indexVersion} (
                              {clampPercent(record.payload.progressPercent)}%)
                              {record.payload.message === null
                                ? ""
                                : ` — ${record.payload.message}`}
                            </li>
                          ))}
                        </ol>
                      </details>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

function IndexProgress({ percent, label }: { readonly percent: number; readonly label: string }) {
  const value = clampPercent(percent);
  return (
    <span className="index-progress">
      <progress value={value} max={100} aria-label={label} />
      <small>{value}%</small>
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

function indexStateClass(state: string): string {
  const lowered = state.toLowerCase();
  if (lowered.includes("fail") || lowered.includes("error")) {
    return "index-failed";
  }
  if (["ready", "active", "built", "complete", "completed"].includes(lowered)) {
    return "index-ready";
  }
  return "index-building";
}

function clampPercent(percent: number): number {
  return Number.isFinite(percent) ? Math.max(0, Math.min(100, Math.round(percent))) : 0;
}
