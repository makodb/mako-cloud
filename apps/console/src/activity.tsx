// Developer activity feed: the audit trail, read by the developer.
//
// The feed is the observability audit-events signal, newest first, with
// actor, action, target, outcome, and time rendered. A project's feed unions
// its environments' feeds client-side and is bounded; an environment's feed
// pages through its cursor. Record details are rendered as text only.
import { useCallback, useEffect, useMemo, useState } from "react";

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

  useEffect(() => {
    let active = true;
    setState({ status: "loading" });
    setMoreFailure(null);
    const read =
      environmentId === undefined
        ? readProjectFeed(client, projectId)
        : readEnvironmentFeed(client, projectId, environmentId);
    void read.then(
      (feed) => active && setState({ status: "ready", feed }),
      (error: unknown) =>
        active && setState({ status: "unavailable", failure: toConsoleApiFailure(error) }),
    );
    return () => {
      active = false;
    };
  }, [client, environmentId, projectId]);

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
        cursor,
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
  }, [client, environmentId, projectId, state]);

  const rows = state.status === "ready" ? state.feed.rows : [];
  const visibleRows = useMemo(
    () => filterActivityRows(rows, outcome, actionText),
    [rows, outcome, actionText],
  );

  return (
    <section
      className="activity-screen panel"
      data-state={state.status}
      aria-labelledby="activity-title"
    >
      <div className="button-row spread usage-section-heading">
        <div>
          <p className="eyebrow">
            {environmentId === undefined ? `Project ${projectId}` : `Environment ${environmentId}`}
          </p>
          <h1 id="activity-title">Recent activity</h1>
        </div>
        <span className={`status-pill ${state.status === "ready" ? "current" : state.status}`}>
          {state.status === "ready"
            ? "Current"
            : state.status === "loading"
              ? "Loading"
              : "Unavailable"}
        </span>
      </div>
      <p>
        Audited actions, newest first, as the audit trail retains them for what your memberships
        allow you to read.
      </p>
      {state.status === "unavailable" ? <ApiFailureNotice failure={state.failure} /> : null}
      {state.status === "ready" ? (
        <FeedProvenance feed={state.feed} project={environmentId === undefined} />
      ) : null}
      <form
        className="filter-grid activity-filters"
        aria-label="Activity filters"
        onSubmit={(event) => event.preventDefault()}
      >
        <label>
          Outcome
          <select
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
          </select>
        </label>
        <label>
          Action
          <input
            name="action"
            type="search"
            value={actionText}
            placeholder="e.g. policy.activate"
            onChange={(event) => setActionText(event.currentTarget.value)}
          />
        </label>
      </form>
      {state.status === "loading" ? (
        <p aria-live="polite">Loading recent activity…</p>
      ) : state.status === "ready" ? (
        <ActivityTable rows={visibleRows} showEnvironment={environmentId === undefined} />
      ) : null}
      <div className="button-row spread">
        <small aria-live="polite">
          Showing {visibleRows.length} of {rows.length} loaded events
          {environmentId === undefined && state.status === "ready"
            ? ` (the ${PROJECT_FEED_BOUND} most recent across ${state.feed.environmentCount} environments)`
            : ""}
          .
        </small>
        {environmentId === undefined ? null : (
          <button
            type="button"
            className="secondary"
            disabled={state.status !== "ready" || state.feed.nextCursor === null || loadingMore}
            onClick={() => void loadMore()}
          >
            {loadingMore ? "Loading…" : "Load more"}
          </button>
        )}
      </div>
      <ApiFailureNotice failure={moreFailure} />
    </section>
  );
}

function FeedProvenance({ feed, project }: { readonly feed: Feed; readonly project: boolean }) {
  return (
    <>
      <p className="observed-at">
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
        <div className="notice warning" role="status">
          <p>Activity from some environments could not be read; the feed below is partial.</p>
          <ul>
            {feed.partial.map((entry) => (
              <li key={entry.environmentId}>
                {entry.environmentName}: {entry.failure.message}
              </li>
            ))}
          </ul>
        </div>
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
    return <p>No audited actions match these filters.</p>;
  }
  return (
    <div className="table-scroll">
      <table className="activity-table">
        <thead>
          <tr>
            <th scope="col">Time</th>
            {showEnvironment ? <th scope="col">Environment</th> : null}
            <th scope="col">Actor</th>
            <th scope="col">Action</th>
            <th scope="col">Target</th>
            <th scope="col">Outcome</th>
            <th scope="col">Details</th>
          </tr>
        </thead>
        <tbody>
          {rows.map((row) => (
            <tr key={row.id} data-outcome={row.payload.outcome}>
              <td>
                <time dateTime={row.timestamp}>{localTime(row.timestamp)}</time>
              </td>
              {showEnvironment ? <td>{row.environmentName}</td> : null}
              <td>
                <code>{row.payload.actorId}</code>
              </td>
              <td>{humanizeAction(row.payload.action)}</td>
              <td>
                <code>{row.payload.target}</code>
              </td>
              <td>
                <span className={`outcome-badge outcome-${row.payload.outcome}`}>
                  {row.payload.outcome}
                </span>
              </td>
              <td className="activity-details">
                {/* Details are operator-written free text: rendered as a text
                    node, never as markup. */}
                {row.payload.details === null || row.payload.details === "" ? (
                  "—"
                ) : (
                  <>
                    {row.payload.details}
                    <small>
                      {" "}
                      · request <code>{row.payload.requestId}</code>
                    </small>
                  </>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

async function readEnvironmentFeed(
  client: MakoManagementClient,
  projectId: string,
  environmentId: string,
): Promise<Feed> {
  const page = await client.queryAuditEvents(projectId, environmentId, { limit: PAGE_LIMIT });
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
async function readProjectFeed(client: MakoManagementClient, projectId: string): Promise<Feed> {
  const environments: Environment[] = await client.listEnvironments(projectId);
  const results = await Promise.allSettled(
    environments.map((environment) =>
      client.queryAuditEvents(projectId, environment.id, { limit: PAGE_LIMIT }),
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
