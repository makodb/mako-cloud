// Developer activity feed: the audit trail, read by the developer.
//
// The feed is the observability audit-events signal, newest first, with
// actor, action, target, outcome, and time rendered. A project's feed unions
// its environments' feeds client-side and is bounded; an environment's feed
// pages through its cursor. Record details are rendered as text only.
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
  Checkbox,
  Eyebrow,
  Field,
  Input,
  Label,
  NativeSelect,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@mako-cloud/ui";
import { useCallback, useEffect, useId, useMemo, useState } from "react";

import type {
  Environment,
  MakoManagementClient,
  ObservabilityPage,
  ObservabilityPayload,
} from "@mako-cloud/management-sdk";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useManagementClient } from "./management.js";

type AuditPayload = Extract<ObservabilityPayload, { readonly kind: "audit" }>;
type Outcome = AuditPayload["outcome"];
type OutcomeFilter = Outcome | "all";

const OUTCOMES: readonly Outcome[] = ["allowed", "denied", "failed"];
const PAGE_LIMIT = 100;
/** A project's union is bounded to this many rows; the bound is stated. */
const PROJECT_FEED_BOUND = 100;

/** How an outcome is coloured: quiet when allowed, amber when refused, red when it broke. */
const OUTCOME_VARIANTS: Readonly<Record<Outcome, "outline" | "warning" | "destructive">> = {
  allowed: "outline",
  denied: "warning",
  failed: "destructive",
};

export interface ActivityRow {
  readonly id: string;
  readonly timestamp: string;
  readonly environmentId: string;
  readonly environmentName: string;
  readonly payload: AuditPayload;
}

interface EnvironmentFeedFailure {
  readonly environmentId: string;
  readonly environmentName: string;
  readonly failure: ConsoleApiFailure;
}

interface Feed {
  readonly rows: readonly ActivityRow[];
  /** Newest observation across the pages the feed was read from. */
  readonly observedAt: string | null;
  readonly retainedFrom: string | null;
  /** Environment mode only: the cursor for older records. */
  readonly nextCursor: string | null;
  /** Project mode: environments whose feed could not be read. */
  readonly partial: readonly EnvironmentFeedFailure[];
  readonly environmentCount: number;
}

type FeedState =
  | { readonly status: "loading" }
  | { readonly status: "ready"; readonly feed: Feed }
  | { readonly status: "unavailable"; readonly failure: ConsoleApiFailure };

export function ActivityScreen({
  projectId,
  environmentId,
}: {
  readonly projectId: string;
  readonly environmentId?: string;
}) {
  const client = useManagementClient();
  const [state, setState] = useState<FeedState>({ status: "loading" });
  const [loadingMore, setLoadingMore] = useState(false);
  const [moreFailure, setMoreFailure] = useState<ConsoleApiFailure | null>(null);
  const [outcome, setOutcome] = useState<OutcomeFilter>("all");
  const [actionText, setActionText] = useState("");
  // Reads are left out unless asked for: every visit to this page records
  // reads of its own, which used to push the changes off the first page.
  const [includeReads, setIncludeReads] = useState(false);
  const filterId = useId();

  useEffect(() => {
    let active = true;
    setState({ status: "loading" });
    setMoreFailure(null);
    const read =
      environmentId === undefined
        ? readProjectFeed(client, projectId, !includeReads)
        : readEnvironmentFeed(client, projectId, environmentId, !includeReads);
    void read.then(
      (feed) => active && setState({ status: "ready", feed }),
      (error: unknown) =>
        active && setState({ status: "unavailable", failure: toConsoleApiFailure(error) }),
    );
    return () => {
      active = false;
    };
  }, [client, environmentId, includeReads, projectId]);

  const loadMore = useCallback(async () => {
    if (environmentId === undefined || state.status !== "ready" || state.feed.nextCursor === null) {
      return;
    }
    const current = state.feed;
    const cursor = state.feed.nextCursor;
    setLoadingMore(true);
    setMoreFailure(null);
    try {
      const page = await client.queryAuditEvents(projectId, environmentId, {
        limit: PAGE_LIMIT,
        order: "newest",
        cursor,
        changes: !includeReads,
      });
      const older = rowsFromPage(page, environmentId, environmentId, current.rows.length);
      setState({
        status: "ready",
        feed: {
          ...current,
          rows: newestFirst([...current.rows, ...older]),
          nextCursor: page.nextCursor,
        },
      });
    } catch (error) {
      setMoreFailure(toConsoleApiFailure(error));
    } finally {
      setLoadingMore(false);
    }
  }, [client, environmentId, includeReads, projectId, state]);

  const rows = state.status === "ready" ? state.feed.rows : [];
  const visibleRows = useMemo(
    () => filterActivityRows(rows, outcome, actionText),
    [rows, outcome, actionText],
  );

  return (
    <Card className="activity-screen" data-state={state.status} aria-labelledby="activity-title">
      <CardHeader>
        <Eyebrow>
          {environmentId === undefined ? "Project " : "Environment "}
          <span className="font-mono tracking-normal normal-case">
            {environmentId === undefined ? projectId : environmentId}
          </span>
        </Eyebrow>
        <CardTitle as="h1" id="activity-title" className="text-2xl">
          Recent activity
        </CardTitle>
        <CardDescription>
          Audited actions, newest first, as the audit trail retains them for what your memberships
          allow you to read.
        </CardDescription>
        <CardAction>
          <FeedStateBadge state={state.status} />
        </CardAction>
      </CardHeader>
      <CardContent className="grid gap-4">
        {state.status === "unavailable" ? <ApiFailureNotice failure={state.failure} /> : null}
        {state.status === "ready" ? (
          <FeedProvenance feed={state.feed} project={environmentId === undefined} />
        ) : null}
        <form
          className="grid gap-3 sm:grid-cols-[minmax(0,12rem)_minmax(0,20rem)]"
          aria-label="Activity filters"
          onSubmit={(event) => event.preventDefault()}
        >
          <Field label="Outcome" htmlFor={`${filterId}-outcome`}>
            <NativeSelect
              id={`${filterId}-outcome`}
              name="outcome"
              value={outcome}
              onChange={(event) => setOutcome(event.currentTarget.value as OutcomeFilter)}
            >
              <option value="all">All outcomes</option>
              {OUTCOMES.map((value) => (
                <option key={value} value={value}>
                  {value}
                </option>
              ))}
            </NativeSelect>
          </Field>
          <Field label="Action" htmlFor={`${filterId}-action`}>
            <Input
              id={`${filterId}-action`}
              name="action"
              type="search"
              value={actionText}
              placeholder="e.g. policy.activate"
              onChange={(event) => setActionText(event.currentTarget.value)}
            />
          </Field>
          <div className="flex items-center gap-2 sm:col-span-2">
            <Checkbox
              id={`${filterId}-reads`}
              checked={includeReads}
              onCheckedChange={(checked) => setIncludeReads(checked === true)}
            />
            <Label htmlFor={`${filterId}-reads`}>Include reads</Label>
          </div>
        </form>
        {state.status === "loading" ? (
          <p className="m-0 text-sm text-muted-foreground" aria-live="polite">
            Loading recent activity…
          </p>
        ) : state.status === "ready" ? (
          <ActivityTable rows={visibleRows} showEnvironment={environmentId === undefined} />
        ) : null}
        <div className="flex flex-wrap items-center justify-between gap-3">
          <small className="text-xs text-muted-foreground" aria-live="polite">
            Showing {visibleRows.length} of {rows.length} loaded events
            {environmentId === undefined && state.status === "ready"
              ? ` (the ${PROJECT_FEED_BOUND} most recent across ${state.feed.environmentCount} environments)`
              : ""}
            .
          </small>
          {environmentId === undefined ? null : (
            <Button
              variant="outline"
              size="sm"
              disabled={state.status !== "ready" || state.feed.nextCursor === null || loadingMore}
              onClick={() => void loadMore()}
            >
              {loadingMore ? "Loading…" : "Load more"}
            </Button>
          )}
        </div>
        <ApiFailureNotice failure={moreFailure} />
      </CardContent>
    </Card>
  );
}

/** Whether the feed is current, still loading, or could not be read. */
function FeedStateBadge({ state }: { readonly state: FeedState["status"] }) {
  return (
    <Badge
      variant={state === "ready" ? "positive" : state === "loading" ? "secondary" : "destructive"}
    >
      {state === "ready" ? "Current" : state === "loading" ? "Loading" : "Unavailable"}
    </Badge>
  );
}

function FeedProvenance({ feed, project }: { readonly feed: Feed; readonly project: boolean }) {
  return (
    <>
      <p className="m-0 text-sm text-muted-foreground">
        {feed.observedAt === null ? (
          "No retained audit records were observed."
        ) : (
          <>
            Observed at <time dateTime={feed.observedAt}>{localTime(feed.observedAt)}</time>
            {feed.retainedFrom === null ? null : (
              <>
                {" "}
                · retained from{" "}
                <time dateTime={feed.retainedFrom}>{localTime(feed.retainedFrom)}</time>
              </>
            )}
          </>
        )}
      </p>
      {project && feed.partial.length > 0 ? (
        <Alert variant="warning" role="status">
          <AlertDescription className="block">
            <p className="m-0">
              Activity from some environments could not be read; the feed below is partial.
            </p>
            <ul className="m-0 mt-1 list-disc pl-5">
              {feed.partial.map((entry) => (
                <li key={entry.environmentId}>
                  {entry.environmentName}: {entry.failure.message}
                </li>
              ))}
            </ul>
          </AlertDescription>
        </Alert>
      ) : null}
    </>
  );
}

function ActivityTable({
  rows,
  showEnvironment,
}: {
  readonly rows: readonly ActivityRow[];
  readonly showEnvironment: boolean;
}) {
  if (rows.length === 0) {
    return (
      <p className="m-0 text-sm text-muted-foreground">No audited actions match these filters.</p>
    );
  }
  // `activity-table` is the hook the browser suite selects on.
  return (
    <Table className="activity-table">
      <TableHeader>
        <TableRow className="hover:bg-transparent">
          <TableHead scope="col">Time</TableHead>
          {showEnvironment ? <TableHead scope="col">Environment</TableHead> : null}
          <TableHead scope="col">Actor</TableHead>
          <TableHead scope="col">Action</TableHead>
          <TableHead scope="col">Target</TableHead>
          <TableHead scope="col">Outcome</TableHead>
          <TableHead scope="col">Details</TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {rows.map((row) => (
          <TableRow key={row.id} data-outcome={row.payload.outcome} className="align-top">
            <TableCell className="align-top text-muted-foreground tabular-nums">
              <time dateTime={row.timestamp}>{localTime(row.timestamp)}</time>
            </TableCell>
            {showEnvironment ? (
              <TableCell className="align-top">{row.environmentName}</TableCell>
            ) : null}
            <TableCell className="align-top">
              <code className="font-mono text-xs">{row.payload.actorId}</code>
            </TableCell>
            <TableCell className="align-top font-medium">
              {humanizeAction(row.payload.action)}
            </TableCell>
            <TableCell className="align-top">
              <code className="font-mono text-xs break-all whitespace-normal">
                {row.payload.target}
              </code>
            </TableCell>
            <TableCell className="align-top">
              <Badge variant={OUTCOME_VARIANTS[row.payload.outcome]}>{row.payload.outcome}</Badge>
            </TableCell>
            <TableCell className="max-w-96 align-top break-words whitespace-pre-wrap">
              {/* Details are operator-written free text: rendered as a text
                  node, never as markup. */}
              {row.payload.details === null || row.payload.details === "" ? (
                "—"
              ) : (
                <>
                  {row.payload.details}
                  <small className="text-xs text-muted-foreground">
                    {" "}
                    · request <code className="font-mono">{row.payload.requestId}</code>
                  </small>
                </>
              )}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

async function readEnvironmentFeed(
  client: MakoManagementClient,
  projectId: string,
  environmentId: string,
  changes: boolean,
): Promise<Feed> {
  // Newest first: read oldest first, a page is the start of the retention
  // window, days before anything the developer just did.
  const page = await client.queryAuditEvents(projectId, environmentId, {
    limit: PAGE_LIMIT,
    order: "newest",
    changes,
  });
  return {
    rows: newestFirst(rowsFromPage(page, environmentId, environmentId, 0)),
    observedAt: page.retention.observedAt,
    retainedFrom: page.retention.retainedFrom,
    nextCursor: page.nextCursor,
    partial: [],
    environmentCount: 1,
  };
}

/** A project's feed is the union of its environments' feeds, merged newest
 * first and bounded; an environment whose feed fails is reported, not
 * silently dropped. */
async function readProjectFeed(
  client: MakoManagementClient,
  projectId: string,
  changes: boolean,
): Promise<Feed> {
  const environments: Environment[] = await client.listEnvironments(projectId);
  const results = await Promise.allSettled(
    environments.map((environment) =>
      client.queryAuditEvents(projectId, environment.id, {
        limit: PAGE_LIMIT,
        order: "newest",
        changes,
      }),
    ),
  );
  const rows: ActivityRow[] = [];
  const partial: EnvironmentFeedFailure[] = [];
  let observedAt: string | null = null;
  let retainedFrom: string | null = null;
  results.forEach((result, index) => {
    const environment = environments[index];
    if (environment === undefined) {
      return;
    }
    if (result.status === "rejected") {
      partial.push({
        environmentId: environment.id,
        environmentName: environment.name,
        failure: toConsoleApiFailure(result.reason),
      });
      return;
    }
    rows.push(...rowsFromPage(result.value, environment.id, environment.name, 0));
    observedAt = latest(observedAt, result.value.retention.observedAt);
    retainedFrom = earliest(retainedFrom, result.value.retention.retainedFrom);
  });
  return {
    rows: newestFirst(rows).slice(0, PROJECT_FEED_BOUND),
    observedAt,
    retainedFrom,
    nextCursor: null,
    partial,
    environmentCount: environments.length,
  };
}

function rowsFromPage(
  page: ObservabilityPage,
  environmentId: string,
  environmentName: string,
  offset: number,
): ActivityRow[] {
  const rows: ActivityRow[] = [];
  for (const record of page.items) {
    if (record.payload.kind !== "audit") {
      continue;
    }
    rows.push({
      id: `${environmentId}:${offset + rows.length}:${record.timestamp}:${record.payload.requestId}`,
      timestamp: record.timestamp,
      environmentId,
      environmentName,
      payload: record.payload,
    });
  }
  return rows;
}

/** Stable descending sort by timestamp: equal instants keep their served
 * order. */
export function newestFirst(rows: readonly ActivityRow[]): ActivityRow[] {
  return rows
    .map((row, index) => ({ row, index, at: Date.parse(row.timestamp) }))
    .sort((left, right) => right.at - left.at || left.index - right.index)
    .map((entry) => entry.row);
}

export function filterActivityRows(
  rows: readonly ActivityRow[],
  outcome: OutcomeFilter,
  actionText: string,
): ActivityRow[] {
  const needle = actionText.trim().toLocaleLowerCase();
  return rows.filter(
    (row) =>
      (outcome === "all" || row.payload.outcome === outcome) &&
      (needle === "" ||
        row.payload.action.toLocaleLowerCase().includes(needle) ||
        humanizeAction(row.payload.action).toLocaleLowerCase().includes(needle)),
  );
}

function humanizeAction(action: string): string {
  return action.replaceAll("_", " ");
}

function latest(current: string | null, candidate: string): string {
  return current === null || Date.parse(candidate) > Date.parse(current) ? candidate : current;
}

function earliest(current: string | null, candidate: string): string {
  return current === null || Date.parse(candidate) < Date.parse(current) ? candidate : current;
}

function localTime(iso: string): string {
  return new Date(iso).toLocaleString();
}
