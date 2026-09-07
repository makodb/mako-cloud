import {
  Badge,
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  EmptyState,
  Eyebrow,
  Field,
  Input,
  NativeSelect,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@mako-cloud/ui";
import { ScrollText } from "lucide-react";
import { type FormEvent, useEffect, useId, useMemo, useState } from "react";

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

/** A code snippet inline in prose or a cell: an identifier. */
const CODE = "rounded bg-muted px-1 py-0.5 font-mono text-[0.85em]";

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
  const id = useId();
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
    <section aria-labelledby="logs-title" className="grid gap-6">
      <div className="grid gap-1">
        <Eyebrow>Environment {environmentId}</Eyebrow>
        <h1 id="logs-title" className="text-2xl">
          Retained logs
        </h1>
        <p className="m-0 max-w-3xl text-sm text-muted-foreground">
          Scrubbed log lines from the data plane, edge functions, and sync for project{" "}
          <code className={CODE}>{projectId}</code>, newest first.
        </p>
      </div>
      <ApiFailureNotice failure={failure} />
      <Card aria-labelledby="log-filters-title">
        <CardHeader>
          <CardTitle id="log-filters-title">Filters</CardTitle>
        </CardHeader>
        <CardContent className="grid gap-4">
          <form
            className="grid items-end gap-3 sm:grid-cols-2 xl:grid-cols-[repeat(auto-fit,minmax(11rem,1fr))]"
            onSubmit={applyCustomRange}
          >
            <Field label="Time range" htmlFor={`${id}-range`}>
              <NativeSelect
                id={`${id}-range`}
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
              </NativeSelect>
            </Field>
            {choice === "custom" ? (
              <>
                <Field label="From" htmlFor={`${id}-from`}>
                  <Input id={`${id}-from`} name="from" type="datetime-local" required />
                </Field>
                <Field label="Until" htmlFor={`${id}-until`}>
                  <Input id={`${id}-until`} name="until" type="datetime-local" />
                </Field>
                <div>
                  <Button type="submit">Apply time range</Button>
                </div>
              </>
            ) : null}
            <Field label="Level" htmlFor={`${id}-level`}>
              <NativeSelect
                id={`${id}-level`}
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
              </NativeSelect>
            </Field>
            <Field label="Source" htmlFor={`${id}-source`}>
              <NativeSelect
                id={`${id}-source`}
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
              </NativeSelect>
            </Field>
          </form>
          <p className="m-0 text-sm text-muted-foreground tabular-nums">
            Window: {new Date(range.from).toLocaleString()} –{" "}
            {range.until === null ? "now" : new Date(range.until).toLocaleString()}
          </p>
          {page === null ? null : <RetentionNotice page={page} />}
        </CardContent>
      </Card>
      <Card aria-labelledby="log-lines-title">
        <CardHeader>
          <CardTitle id="log-lines-title">Log lines</CardTitle>
          <CardDescription className="tabular-nums">
            Showing {visible.length} of {lines.length} loaded lines.
          </CardDescription>
        </CardHeader>
        <CardContent className="grid gap-4">
          {loading && page === null ? (
            <p className="m-0 text-sm text-muted-foreground" aria-live="polite">
              Loading retained logs…
            </p>
          ) : lines.length === 0 ? (
            <EmptyState
              role="status"
              icon={<ScrollText aria-hidden="true" />}
              title="No log lines are retained for this window."
              description="Widen the time range, or check that the environment has served traffic."
            />
          ) : visible.length === 0 ? (
            <EmptyState
              role="status"
              icon={<ScrollText aria-hidden="true" />}
              title="No loaded lines match the level and source filters."
              description="Clear a filter or load more lines from the retention window."
            />
          ) : (
            <LogTable lines={visible} />
          )}
          <div className="flex flex-wrap items-center justify-between gap-3">
            <small className="text-xs text-muted-foreground">
              {page === null || page.nextCursor === null
                ? "Every retained line in this window is loaded."
                : "Older lines remain in the retention window."}
            </small>
            <Button
              variant="outline"
              size="sm"
              disabled={page === null || page.nextCursor === null || loading || loadingMore}
              onClick={() => void loadMore()}
            >
              {loadingMore ? "Loading…" : "Load more"}
            </Button>
          </div>
        </CardContent>
      </Card>
    </section>
  );
}

// Messages are rendered as text nodes only: they are scrubbed server-side and
// must never be interpreted as markup here.
function LogTable({ lines }: { readonly lines: readonly LogLine[] }) {
  return (
    <Table className="log-table">
      <TableHeader>
        <TableRow className="hover:bg-transparent">
          <TableHead scope="col">Time</TableHead>
          <TableHead scope="col">Level</TableHead>
          <TableHead scope="col">Source</TableHead>
          <TableHead scope="col">Message</TableHead>
          <TableHead scope="col">Correlation</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {lines.map((line) => (
          <TableRow key={line.id}>
            <TableCell className="align-top text-muted-foreground tabular-nums">
              <time dateTime={line.timestamp}>{new Date(line.timestamp).toLocaleString()}</time>
            </TableCell>
            <TableCell className="align-top">
              <LevelBadge level={line.level} />
            </TableCell>
            <TableCell className="align-top font-mono text-xs">{line.source}</TableCell>
            <TableCell className="min-w-72 align-top font-mono text-xs whitespace-pre-wrap break-words">
              {line.message}
            </TableCell>
            <TableCell className="align-top">
              <code className={CODE}>{line.correlationId}</code>
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

/** The level as a badge; warn and error carry colour, the rest stay quiet. */
function LevelBadge({ level }: { readonly level: string }) {
  const normalized = normalizeLevel(level);
  return (
    <Badge
      variant={
        normalized === "error"
          ? "destructive"
          : normalized === "warn"
            ? "warning"
            : normalized === "info"
              ? "secondary"
              : "outline"
      }
      className="font-mono uppercase"
    >
      {level}
    </Badge>
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
