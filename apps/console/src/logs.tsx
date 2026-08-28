import { type FormEvent, useEffect, useMemo, useState } from "react";

import type {
  ObservabilityPage,
  ObservabilityQuery,
  ObservabilityRecord,
} from "@mako-cloud/management-sdk";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useManagementClient } from "./management.js";
import { RetentionNotice } from "./observability.js";

const PAGE_SIZE = 200;
const LOG_LEVELS = ["debug", "info", "warn", "error"] as const;
type LogLevel = (typeof LOG_LEVELS)[number];
const RANGE_PRESETS = {
  "1h": { label: "Last hour", milliseconds: 3_600_000 },
  "24h": { label: "Last 24 hours", milliseconds: 86_400_000 },
  "7d": { label: "Last 7 days", milliseconds: 604_800_000 },
} as const;
type RangePreset = keyof typeof RANGE_PRESETS;
type RangeChoice = RangePreset | "custom";

interface TimeRange {
  readonly from: string;
  readonly until: string | null;
}

export interface LogLine {
  readonly id: number;
  readonly timestamp: string;
  readonly source: string;
  readonly level: string;
  readonly message: string;
  readonly correlationId: string;
}

// Retained project logs: scrubbed server-side, listed newest first, filtered
// by level and source over the loaded lines, paged through the retention
// window with the API's cursor.
export function LogsScreen({
  projectId,
  environmentId,
}: {
  readonly projectId: string;
  readonly environmentId: string;
}) {
  const client = useManagementClient();
  const [choice, setChoice] = useState<RangeChoice>("1h");
  const [range, setRange] = useState<TimeRange>(() => presetRange("1h"));
  const [level, setLevel] = useState<"all" | LogLevel>("all");
  const [source, setSource] = useState("all");
  const [page, setPage] = useState<ObservabilityPage | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    client
      .queryProjectLogs(projectId, environmentId, logQuery(range))
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
  }, [client, environmentId, projectId, range]);

  const lines = useMemo(() => projectLogLines(page?.items ?? []), [page?.items]);
  const sources = useMemo(
    () => Array.from(new Set(lines.map((line) => line.source))).sort(),
    [lines],
  );
  const visible = useMemo(
    () =>
      lines.filter(
        (line) =>
          (level === "all" || normalizeLevel(line.level) === level) &&
          (source === "all" || line.source === source),
      ),
    [level, lines, source],
  );

  const changeRange = (next: RangeChoice) => {
    setChoice(next);
    if (next !== "custom") {
      setRange(presetRange(next));
      setFailure(null);
    }
  };

  const applyCustomRange = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    try {
      const data = new FormData(event.currentTarget);
      const from = optionalDate(data, "from");
      const until = optionalDate(data, "until");
      if (from === undefined) {
        throw new Error("Choose where the custom range starts.");
      }
      if (until !== undefined && from > until) {
        throw new Error("The start of the time range must be before its end.");
      }
      setRange({ from, until: until ?? null });
      setFailure(null);
    } catch (error) {
      setFailure({
        message: error instanceof Error ? error.message : "Invalid time range.",
        requestId: null,
      });
    }
  };

  const loadMore = async () => {
    if (page === null || page.nextCursor === null) {
      return;
    }
    setLoadingMore(true);
    try {
      const next = await client.queryProjectLogs(
        projectId,
        environmentId,
        logQuery(range, page.nextCursor),
      );
      setPage({ ...next, items: [...page.items, ...next.items] });
      setFailure(null);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    } finally {
      setLoadingMore(false);
    }
  };

  return (
    <section aria-labelledby="logs-title">
      <div className="section-heading">
        <div>
          <p className="eyebrow">Environment {environmentId}</p>
          <h1 id="logs-title">Retained logs</h1>
          <p>
            Scrubbed log lines from the data plane, edge functions, and sync for project{" "}
            <code>{projectId}</code>, newest first.
          </p>
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      <section className="panel full-span" aria-labelledby="log-filters-title">
        <h2 id="log-filters-title">Filters</h2>
        <form className="log-filters" onSubmit={applyCustomRange}>
          <label>
            Time range
            <select
              name="range"
              value={choice}
              onChange={(event) => changeRange(event.currentTarget.value as RangeChoice)}
            >
              {(Object.keys(RANGE_PRESETS) as RangePreset[]).map((preset) => (
                <option key={preset} value={preset}>
                  {RANGE_PRESETS[preset].label}
                </option>
              ))}
              <option value="custom">Custom range</option>
            </select>
          </label>
          {choice === "custom" ? (
            <>
              <label>
                From
                <input name="from" type="datetime-local" required />
              </label>
              <label>
                Until
                <input name="until" type="datetime-local" />
              </label>
              <button type="submit">Apply time range</button>
            </>
          ) : null}
          <label>
            Level
            <select
              name="level"
              value={level}
              onChange={(event) => setLevel(event.currentTarget.value as "all" | LogLevel)}
            >
              <option value="all">All levels</option>
              {LOG_LEVELS.map((item) => (
                <option key={item} value={item}>
                  {item}
                </option>
              ))}
            </select>
          </label>
          <label>
            Source
            <select
              name="source"
              value={sources.includes(source) ? source : "all"}
              onChange={(event) => setSource(event.currentTarget.value)}
            >
              <option value="all">All sources</option>
              {sources.map((item) => (
                <option key={item} value={item}>
                  {item}
                </option>
              ))}
            </select>
          </label>
        </form>
        <p className="log-window">
          Window: {new Date(range.from).toLocaleString()} –{" "}
          {range.until === null ? "now" : new Date(range.until).toLocaleString()}
        </p>
        {page === null ? null : <RetentionNotice page={page} />}
      </section>
      <section className="panel full-span" aria-labelledby="log-lines-title">
        <div className="button-row spread">
          <h2 id="log-lines-title">Log lines</h2>
          <small>
            Showing {visible.length} of {lines.length} loaded lines.
          </small>
        </div>
        {loading && page === null ? (
          <p aria-live="polite">Loading retained logs…</p>
        ) : lines.length === 0 ? (
          <div className="log-empty" role="status">
            <strong>No log lines are retained for this window.</strong>
            <p>Widen the time range, or check that the environment has served traffic.</p>
          </div>
        ) : visible.length === 0 ? (
          <div className="log-empty" role="status">
            <strong>No loaded lines match the level and source filters.</strong>
            <p>Clear a filter or load more lines from the retention window.</p>
          </div>
        ) : (
          <LogTable lines={visible} />
        )}
        <div className="button-row spread">
          <small>
            {page === null || page.nextCursor === null
              ? "Every retained line in this window is loaded."
              : "Older lines remain in the retention window."}
          </small>
          <button
            type="button"
            className="secondary"
            disabled={page === null || page.nextCursor === null || loading || loadingMore}
            onClick={() => void loadMore()}
          >
            {loadingMore ? "Loading…" : "Load more"}
          </button>
        </div>
      </section>
    </section>
  );
}

// Messages are rendered as text nodes only: they are scrubbed server-side and
// must never be interpreted as markup here.
function LogTable({ lines }: { readonly lines: readonly LogLine[] }) {
  return (
    <div className="table-scroll">
      <table className="log-table">
        <thead>
          <tr>
            <th scope="col">Time</th>
            <th scope="col">Level</th>
            <th scope="col">Source</th>
            <th scope="col">Message</th>
            <th scope="col">Correlation</th>
          </tr>
        </thead>
        <tbody>
          {lines.map((line) => (
            <tr key={line.id}>
              <td>
                <time dateTime={line.timestamp}>{new Date(line.timestamp).toLocaleString()}</time>
              </td>
              <td>
                <span className={`log-level log-level-${normalizeLevel(line.level) ?? "other"}`}>
                  {line.level}
                </span>
              </td>
              <td>{line.source}</td>
              <td className="log-message">{line.message}</td>
              <td>
                <code>{line.correlationId}</code>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** The project-log lines among retained records, newest first. */
export function projectLogLines(records: readonly ObservabilityRecord[]): LogLine[] {
  const lines: LogLine[] = [];
  records.forEach((record, index) => {
    if (record.payload.kind === "project_log") {
      lines.push({
        id: index,
        timestamp: record.timestamp,
        source: record.payload.source,
        level: record.payload.level,
        message: record.payload.message,
        correlationId: record.payload.correlationId,
      });
    }
  });
  return lines.sort((left, right) => Date.parse(right.timestamp) - Date.parse(left.timestamp));
}

/** Levels the filter understands; other levels stay visible under "all". */
export function normalizeLevel(level: string): LogLevel | null {
  const lowered = level.trim().toLowerCase();
  if (lowered === "warning") {
    return "warn";
  }
  if (lowered === "err" || lowered === "fatal" || lowered === "critical") {
    return "error";
  }
  if (lowered === "trace" || lowered === "verbose") {
    return "debug";
  }
  return (LOG_LEVELS as readonly string[]).includes(lowered) ? (lowered as LogLevel) : null;
}

function presetRange(preset: RangePreset, now = Date.now()): TimeRange {
  return { from: new Date(now - RANGE_PRESETS[preset].milliseconds).toISOString(), until: null };
}

function logQuery(range: TimeRange, cursor?: string): ObservabilityQuery {
  return {
    limit: PAGE_SIZE,
    from: range.from,
    ...(range.until === null ? {} : { until: range.until }),
    ...(cursor === undefined ? {} : { cursor }),
  };
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
