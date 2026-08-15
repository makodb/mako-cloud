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
    </section>
  );
}

function RetentionNotice({ page }: { readonly page: ObservabilityPage }) {
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
