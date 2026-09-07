import {
  Alert,
  AlertDescription,
  Badge,
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  Checkbox,
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
  Textarea,
  cn,
} from "@mako-cloud/ui";
import { TriangleAlert } from "lucide-react";
import { type FormEvent, type ReactNode, useCallback, useEffect, useId, useState } from "react";

import type {
  FunctionSchedule,
  FunctionScheduleCreate,
  FunctionScheduleRequest,
  FunctionScheduleRun,
  FunctionScheduleRunOutcome,
  FunctionScheduleRunPage,
} from "@mako-cloud/management-sdk";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useManagementClient } from "./management.js";
import { confirmDestructiveAction } from "./safety.js";

type ScheduleMethod = FunctionScheduleRequest["method"];

const METHODS: readonly ScheduleMethod[] = ["GET", "POST", "PUT", "PATCH", "DELETE"];
const OUTCOMES: readonly FunctionScheduleRunOutcome[] = [
  "succeeded",
  "failed",
  "error",
  "skipped_overlap",
];
const RUN_PAGE_SIZE = 50;
/** Headers the platform sets itself; the API refuses them, the form says so first. */
const RESERVED_HEADERS: ReadonlySet<string> = new Set(["authorization", "host", "content-length"]);
const CRON_EXAMPLES: readonly { readonly expression: string; readonly meaning: string }[] = [
  { expression: "0 2 * * *", meaning: "every night at 02:00" },
  { expression: "*/15 * * * *", meaning: "every 15 minutes" },
  { expression: "0 9 * * 1", meaning: "Mondays at 09:00" },
  { expression: "0 0 1 * *", meaning: "the first of each month" },
];

/** A code snippet inline in prose or a cell: an identifier, an expression, an error. */
const CODE = "rounded bg-muted px-1 py-0.5 font-mono text-[0.85em]";
/** The quieter second line of a cell: an id, a request, a duration. */
const SUBLINE = "mt-0.5 block text-xs font-normal text-muted-foreground";

// A function's cron schedules, on the function's own page: the list with
// next and last run, pause/resume, run now, and delete per schedule, a form
// to attach one, and each schedule's run history on request. Every schedule
// targets the function's active deployment; the API refuses one otherwise.
export function FunctionSchedulesPanel({
  projectId,
  environmentId,
  functionName,
  activeVersion,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly functionName: string;
  readonly activeVersion: number | null;
}) {
  const client = useManagementClient();
  const formId = useId();
  const [schedules, setSchedules] = useState<FunctionSchedule[] | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [acting, setActing] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [expanded, setExpanded] = useState<string | null>(null);
  const [queuedRun, setQueuedRun] = useState<FunctionScheduleRun | null>(null);

  const reload = useCallback(async () => {
    try {
      setSchedules(await client.listFunctionSchedules(projectId, environmentId, functionName));
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  }, [client, environmentId, functionName, projectId]);
  useEffect(() => {
    void reload();
  }, [reload]);

  const replace = (next: FunctionSchedule) =>
    setSchedules((current) =>
      current === null ? [next] : current.map((item) => (item.id === next.id ? next : item)),
    );

  const create = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const form = event.currentTarget;
    setCreating(true);
    setStatus(null);
    try {
      const input = scheduleInputFrom(new FormData(form));
      const created = await client.createFunctionSchedule(
        projectId,
        environmentId,
        functionName,
        input,
        idempotencyKey(),
      );
      setFailure(null);
      setStatus(
        created.nextRunAt === null
          ? `Schedule ${created.id} created, paused.`
          : `Schedule ${created.id} created; next run ${formatTime(created.nextRunAt)}.`,
      );
      form.reset();
      await reload();
    } catch (error) {
      setFailure(failureFrom(error));
    } finally {
      setCreating(false);
    }
  };

  const setEnabled = async (schedule: FunctionSchedule, enabled: boolean) => {
    setActing(schedule.id);
    setStatus(null);
    try {
      const next = await client.updateFunctionSchedule(
        projectId,
        environmentId,
        functionName,
        schedule.id,
        { enabled },
        idempotencyKey(),
      );
      replace(next);
      setFailure(null);
      setStatus(
        next.nextRunAt === null
          ? `Schedule ${next.name} paused; it keeps its history and runs again once resumed.`
          : `Schedule ${next.name} resumed; next run ${formatTime(next.nextRunAt)}.`,
      );
    } catch (error) {
      setFailure(failureFrom(error));
    } finally {
      setActing(null);
    }
  };

  const runNow = async (schedule: FunctionSchedule) => {
    setActing(schedule.id);
    setStatus(null);
    try {
      const run = await client.runFunctionScheduleNow(
        projectId,
        environmentId,
        functionName,
        schedule.id,
        idempotencyKey(),
      );
      setFailure(null);
      setStatus(`Run ${run.id} queued for schedule ${schedule.name}; it is recorded as manual.`);
      setQueuedRun(run);
    } catch (error) {
      setFailure(failureFrom(error));
    } finally {
      setActing(null);
    }
  };

  const remove = async (schedule: FunctionSchedule) => {
    if (
      !confirmDestructiveAction({
        action: "Delete",
        target: `schedule ${schedule.name} (${schedule.id})`,
        consequence: "Its run history is removed with it and no further run is queued.",
      })
    ) {
      return;
    }
    setActing(schedule.id);
    setStatus(null);
    try {
      await client.deleteFunctionSchedule(
        projectId,
        environmentId,
        functionName,
        schedule.id,
        idempotencyKey(),
      );
      setSchedules((current) => current?.filter((item) => item.id !== schedule.id) ?? null);
      if (expanded === schedule.id) {
        setExpanded(null);
      }
      setFailure(null);
      setStatus(`Schedule ${schedule.name} deleted.`);
    } catch (error) {
      setFailure(failureFrom(error));
    } finally {
      setActing(null);
    }
  };

  return (
    <Card aria-labelledby="schedules-title">
      <CardHeader>
        <CardTitle id="schedules-title">Schedules</CardTitle>
        <CardDescription>
          Each due time invokes the active deployment with the configured request, through the same
          gateway as any invocation. Expressions are five-field cron evaluated in UTC. A run still
          executing at the next due time causes that one to be skipped, never run alongside.
        </CardDescription>
      </CardHeader>
      <CardContent className="grid gap-5">
        {activeVersion === null ? (
          <Alert variant="warning" role="note">
            <TriangleAlert aria-hidden="true" />
            <AlertDescription className="block">
              This function has no active deployment; a schedule is refused until a version is
              promoted.
            </AlertDescription>
          </Alert>
        ) : null}
        <ApiFailureNotice failure={failure} />
        {status === null ? null : (
          <Alert variant="positive" role="status">
            <AlertDescription className="block text-foreground">{status}</AlertDescription>
          </Alert>
        )}
        {schedules === null ? (
          <p className="m-0 text-sm text-muted-foreground">Loading schedules…</p>
        ) : schedules.length === 0 ? (
          <p className="m-0 text-sm text-muted-foreground">
            No schedules are attached to this function.
          </p>
        ) : (
          <Table>
            <TableHeader>
              <TableRow className="hover:bg-transparent">
                <TableHead scope="col">Name</TableHead>
                <TableHead scope="col">Cron (UTC)</TableHead>
                <TableHead scope="col">State</TableHead>
                <TableHead scope="col">Next run</TableHead>
                <TableHead scope="col">Last run</TableHead>
                <TableHead scope="col" className="text-right">
                  <span className="sr-only">Actions</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {schedules.map((schedule) => (
                <ScheduleRows
                  key={schedule.id}
                  schedule={schedule}
                  busy={acting !== null}
                  acting={acting === schedule.id}
                  expanded={expanded === schedule.id}
                  onToggleHistory={() =>
                    setExpanded((current) => (current === schedule.id ? null : schedule.id))
                  }
                  onSetEnabled={(enabled) => void setEnabled(schedule, enabled)}
                  onRunNow={() => void runNow(schedule)}
                  onDelete={() => void remove(schedule)}
                >
                  <ScheduleRunHistory
                    projectId={projectId}
                    environmentId={environmentId}
                    functionName={functionName}
                    schedule={schedule}
                    latestRun={queuedRun}
                    onFailure={setFailure}
                  />
                </ScheduleRows>
              ))}
            </TableBody>
          </Table>
        )}
        <section className="grid gap-4 border-t pt-5" aria-labelledby="create-schedule-title">
          <h3 id="create-schedule-title" className="text-sm font-semibold">
            Add schedule
          </h3>
          <form className="grid gap-4" onSubmit={(event) => void create(event)}>
            <div className="grid gap-4 md:grid-cols-2">
              <Field label="Name" htmlFor={`${formId}-name`}>
                <Input
                  id={`${formId}-name`}
                  name="name"
                  maxLength={128}
                  placeholder={`${functionName} nightly`}
                />
              </Field>
              <Field label="Cron expression" htmlFor={`${formId}-cron`}>
                <Input
                  id={`${formId}-cron`}
                  name="cron"
                  required
                  maxLength={128}
                  placeholder="0 2 * * *"
                  spellCheck={false}
                  autoComplete="off"
                  aria-describedby="cron-hint"
                  className="font-mono"
                />
              </Field>
            </div>
            <p id="cron-hint" className="m-0 text-sm text-muted-foreground">
              Five fields in UTC — minute, hour, day of month, month, day of week. Examples:{" "}
              {CRON_EXAMPLES.map((example, index) => (
                <span key={example.expression}>
                  {index === 0 ? "" : "; "}
                  <code className={cn(CODE, "whitespace-nowrap")}>{example.expression}</code>{" "}
                  {example.meaning}
                </span>
              ))}
              . An invalid expression is refused when saved.
            </p>
            <div className="grid gap-4 md:grid-cols-[minmax(6rem,8rem)_minmax(0,1fr)_minmax(0,1fr)]">
              <Field label="Method" htmlFor={`${formId}-method`}>
                <NativeSelect id={`${formId}-method`} name="method" defaultValue="POST">
                  {METHODS.map((method) => (
                    <option value={method} key={method}>
                      {method}
                    </option>
                  ))}
                </NativeSelect>
              </Field>
              <Field label="Path" htmlFor={`${formId}-path`}>
                <Input
                  id={`${formId}-path`}
                  name="path"
                  defaultValue="/"
                  maxLength={1024}
                  spellCheck={false}
                  className="font-mono"
                />
              </Field>
              <Field label="Content type" htmlFor={`${formId}-content-type`}>
                <Input
                  id={`${formId}-content-type`}
                  name="contentType"
                  defaultValue="application/json"
                  maxLength={128}
                  spellCheck={false}
                  className="font-mono"
                />
              </Field>
            </div>
            <Field label="Headers (one name=value per line)" htmlFor={`${formId}-headers`}>
              <Textarea
                id={`${formId}-headers`}
                name="headers"
                rows={3}
                spellCheck={false}
                className="font-mono text-xs"
              />
            </Field>
            <Field label="Body (sent as text; omitted for GET)" htmlFor={`${formId}-body`}>
              <Textarea
                id={`${formId}-body`}
                name="body"
                rows={4}
                maxLength={65_536}
                spellCheck={false}
                className="font-mono text-xs"
              />
            </Field>
            <div className="flex items-center gap-2">
              <Checkbox id={`${formId}-enabled`} name="enabled" defaultChecked />
              <Label htmlFor={`${formId}-enabled`} className="font-normal">
                Enabled — start running at the next due time
              </Label>
            </div>
            <div>
              <Button type="submit" disabled={creating}>
                {creating ? "Saving…" : "Create schedule"}
              </Button>
            </div>
          </form>
        </section>
      </CardContent>
    </Card>
  );
}

/** One schedule's row, and beneath it the history when opened. */
function ScheduleRows({
  schedule,
  busy,
  acting,
  expanded,
  onToggleHistory,
  onSetEnabled,
  onRunNow,
  onDelete,
  children,
}: {
  readonly schedule: FunctionSchedule;
  readonly busy: boolean;
  readonly acting: boolean;
  readonly expanded: boolean;
  readonly onToggleHistory: () => void;
  readonly onSetEnabled: (enabled: boolean) => void;
  readonly onRunNow: () => void;
  readonly onDelete: () => void;
  readonly children: ReactNode;
}) {
  const historyId = `schedule-history-${schedule.id}`;
  return (
    <>
      <TableRow data-schedule-id={schedule.id} data-state={schedule.state}>
        <TableHead scope="row" className="align-top">
          {schedule.name}
          <small className={cn(SUBLINE, "font-mono")}>{schedule.id}</small>
        </TableHead>
        <TableCell className="align-top">
          <code className={cn(CODE, "schedule-cron whitespace-nowrap")}>{schedule.cron}</code>
          <small className={cn(SUBLINE, "font-mono")}>
            {schedule.request.method} {schedule.request.path}
          </small>
        </TableCell>
        <TableCell className="align-top">
          <Badge
            className="schedule-state capitalize"
            variant={schedule.state === "active" ? "positive" : "warning"}
          >
            {schedule.state}
          </Badge>
        </TableCell>
        <TableCell className="schedule-next-run align-top">
          {schedule.nextRunAt === null ? (
            <em className="text-muted-foreground">none while paused</em>
          ) : (
            <Timestamp value={schedule.nextRunAt} />
          )}
        </TableCell>
        <TableCell className="schedule-last-run align-top whitespace-normal">
          {schedule.lastRun === null ? (
            <em className="text-muted-foreground">never</em>
          ) : (
            <>
              <OutcomeBadge outcome={schedule.lastRun.outcome} />
              <small className={cn(SUBLINE, "tabular-nums")}>
                {schedule.lastRun.responseStatus === null
                  ? null
                  : `HTTP ${schedule.lastRun.responseStatus} · `}
                {formatDuration(schedule.lastRun.durationMilliseconds)} · due{" "}
                <Timestamp value={schedule.lastRun.dueAt} />
              </small>
            </>
          )}
        </TableCell>
        <TableCell className="align-top">
          <div className="flex justify-end gap-1">
            {schedule.enabled ? (
              <Button
                variant="outline"
                size="sm"
                aria-label={`Pause ${schedule.name}`}
                disabled={busy}
                onClick={() => onSetEnabled(false)}
              >
                {acting ? "Working…" : "Pause"}
              </Button>
            ) : (
              <Button
                variant="outline"
                size="sm"
                aria-label={`Resume ${schedule.name}`}
                disabled={busy}
                onClick={() => onSetEnabled(true)}
              >
                {acting ? "Working…" : "Resume"}
              </Button>
            )}
            <Button
              variant="outline"
              size="sm"
              aria-label={`Run ${schedule.name} now`}
              disabled={busy}
              onClick={onRunNow}
            >
              Run now
            </Button>
            <Button
              variant="ghost"
              size="sm"
              aria-label={`${expanded ? "Hide" : "Show"} history of ${schedule.name}`}
              aria-expanded={expanded}
              aria-controls={historyId}
              onClick={onToggleHistory}
            >
              {expanded ? "Hide history" : "History"}
            </Button>
            <Button
              variant="ghost"
              size="sm"
              className="text-destructive hover:bg-destructive/10 hover:text-destructive"
              aria-label={`Delete ${schedule.name}`}
              disabled={busy}
              onClick={onDelete}
            >
              Delete
            </Button>
          </div>
        </TableCell>
      </TableRow>
      {expanded ? (
        <TableRow id={historyId} className="hover:bg-transparent">
          <TableCell colSpan={6} className="bg-muted/40 p-4 whitespace-normal">
            {children}
          </TableCell>
        </TableRow>
      ) : null}
    </>
  );
}

/**
 * One schedule's runs, newest first, filtered by outcome and paged from the
 * API; every due time is here, including the ones skipped for overlap.
 */
function ScheduleRunHistory({
  projectId,
  environmentId,
  functionName,
  schedule,
  latestRun,
  onFailure,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly functionName: string;
  readonly schedule: FunctionSchedule;
  /** A run the page just queued with run-now; it goes on top without a round trip. */
  readonly latestRun: FunctionScheduleRun | null;
  readonly onFailure: (failure: ConsoleApiFailure | null) => void;
}) {
  const client = useManagementClient();
  const [outcome, setOutcome] = useState<FunctionScheduleRunOutcome | "">("");
  const [page, setPage] = useState<FunctionScheduleRunPage | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    client
      .listFunctionScheduleRuns(
        projectId,
        environmentId,
        functionName,
        schedule.id,
        runQuery(outcome),
      )
      .then((next) => {
        if (!cancelled) {
          setPage(next);
          onFailure(null);
        }
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          onFailure(failureFrom(error));
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
  }, [client, environmentId, functionName, onFailure, outcome, projectId, schedule.id]);

  useEffect(() => {
    if (latestRun === null || latestRun.scheduleId !== schedule.id) {
      return;
    }
    if (outcome !== "" && latestRun.outcome !== outcome) {
      return;
    }
    setPage((current) =>
      current === null || current.items.some((run) => run.id === latestRun.id)
        ? current
        : { items: [latestRun, ...current.items], nextCursor: current.nextCursor },
    );
  }, [latestRun, outcome, schedule.id]);

  const loadMore = async () => {
    if (page === null || page.nextCursor === null) {
      return;
    }
    setLoadingMore(true);
    try {
      const next = await client.listFunctionScheduleRuns(
        projectId,
        environmentId,
        functionName,
        schedule.id,
        runQuery(outcome, page.nextCursor),
      );
      setPage({ items: [...page.items, ...next.items], nextCursor: next.nextCursor });
      onFailure(null);
    } catch (error) {
      onFailure(failureFrom(error));
    } finally {
      setLoadingMore(false);
    }
  };

  const filterId = `run-outcome-${schedule.id}`;
  return (
    <section className="grid gap-3" aria-label={`Run history of ${schedule.name}`}>
      <Field label="Outcome" htmlFor={filterId} className="w-56">
        <NativeSelect
          id={filterId}
          size="sm"
          value={outcome}
          onChange={(event) =>
            setOutcome(event.currentTarget.value as FunctionScheduleRunOutcome | "")
          }
        >
          <option value="">Every outcome</option>
          {OUTCOMES.map((item) => (
            <option value={item} key={item}>
              {outcomeLabel(item)}
            </option>
          ))}
        </NativeSelect>
      </Field>
      {page === null || (loading && page.items.length === 0) ? (
        <p className="m-0 text-sm text-muted-foreground">Loading runs…</p>
      ) : page.items.length === 0 ? (
        <p className="m-0 text-sm text-muted-foreground">
          {outcome === ""
            ? "No runs are retained for this schedule."
            : `No retained run ${outcomeLabel(outcome)}.`}
        </p>
      ) : (
        <RunTable runs={page.items} />
      )}
      <div className="flex flex-wrap items-center gap-3">
        <Button
          variant="outline"
          size="sm"
          disabled={loading || loadingMore || page?.nextCursor == null}
          onClick={() => void loadMore()}
        >
          {loadingMore ? "Loading…" : "Load more"}
        </Button>
        {page !== null && page.nextCursor === null && page.items.length > 0 ? (
          <small className="text-xs text-muted-foreground">Every retained run is listed.</small>
        ) : null}
      </div>
    </section>
  );
}

function RunTable({ runs }: { readonly runs: readonly FunctionScheduleRun[] }) {
  return (
    <div className="rounded-lg border bg-card">
      <Table>
        <TableHeader>
          <TableRow className="hover:bg-transparent">
            <TableHead scope="col">Due</TableHead>
            <TableHead scope="col">Started</TableHead>
            <TableHead scope="col" className="text-right">
              Duration
            </TableHead>
            <TableHead scope="col">Outcome</TableHead>
            <TableHead scope="col" className="text-right">
              Status
            </TableHead>
            <TableHead scope="col">Error</TableHead>
            <TableHead scope="col">Trigger</TableHead>
          </TableRow>
        </TableHeader>
        <TableBody>
          {runs.map((run) => (
            <TableRow key={run.id} data-run-id={run.id} data-outcome={run.outcome ?? "pending"}>
              <TableHead scope="row" className="align-top">
                <Timestamp value={run.dueAt} />
                <small className={cn(SUBLINE, "font-mono")}>
                  {run.id}
                  {run.functionVersion === null ? null : ` · v${run.functionVersion}`}
                </small>
              </TableHead>
              <TableCell className="align-top">
                {run.startedAt === null ? (
                  <em className="text-muted-foreground">not started</em>
                ) : (
                  <Timestamp value={run.startedAt} />
                )}
              </TableCell>
              <TableCell className="numeric text-right align-top tabular-nums">
                {formatDuration(run.durationMilliseconds)}
              </TableCell>
              <TableCell className="align-top">
                <OutcomeBadge outcome={run.outcome} started={run.startedAt !== null} />
              </TableCell>
              <TableCell className="numeric text-right align-top tabular-nums">
                {run.responseStatus ?? "—"}
              </TableCell>
              <TableCell className="align-top whitespace-normal">
                {run.error === null ? (
                  <em className="text-muted-foreground">none</em>
                ) : (
                  <code className={cn(CODE, "break-all")}>{run.error}</code>
                )}
              </TableCell>
              <TableCell className="align-top text-muted-foreground">
                {run.manual ? "manual" : "cron"}
              </TableCell>
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </div>
  );
}

/** The outcome as a badge; a run without one yet is queued or running. */
function OutcomeBadge({
  outcome,
  started = true,
}: {
  readonly outcome: FunctionScheduleRunOutcome | null;
  readonly started?: boolean;
}) {
  if (outcome === null) {
    return (
      <Badge className="run-outcome" variant="secondary">
        {started ? "running" : "queued"}
      </Badge>
    );
  }
  return (
    <Badge className="run-outcome" variant={outcomeVariant(outcome)}>
      {outcomeLabel(outcome)}
    </Badge>
  );
}

function outcomeVariant(
  outcome: FunctionScheduleRunOutcome,
): "positive" | "destructive" | "warning" {
  switch (outcome) {
    case "succeeded":
      return "positive";
    case "failed":
    case "error":
      return "destructive";
    case "skipped_overlap":
      return "warning";
  }
}

function Timestamp({ value }: { readonly value: string }) {
  return (
    <time dateTime={value} className="whitespace-nowrap tabular-nums">
      {formatTime(value)}
    </time>
  );
}

function formatTime(value: string): string {
  return new Date(value).toLocaleString();
}

function formatDuration(milliseconds: number | null): string {
  return milliseconds === null ? "—" : `${milliseconds.toLocaleString("en-US")} ms`;
}

function outcomeLabel(outcome: FunctionScheduleRunOutcome): string {
  return outcome === "skipped_overlap" ? "skipped (overlap)" : outcome;
}

function runQuery(
  outcome: FunctionScheduleRunOutcome | "",
  cursor?: string,
): {
  readonly outcome?: FunctionScheduleRunOutcome;
  readonly cursor?: string;
  readonly limit?: number;
} {
  return {
    limit: RUN_PAGE_SIZE,
    ...(outcome === "" ? {} : { outcome }),
    ...(cursor === undefined ? {} : { cursor }),
  };
}

/** The form's fields as the create request; what the form can check, it checks first. */
function scheduleInputFrom(data: FormData): FunctionScheduleCreate {
  const cron = text(data, "cron");
  if (cron === "") {
    throw new FormInputError("A cron expression is required.");
  }
  const name = text(data, "name");
  const method = text(data, "method");
  if (!METHODS.includes(method as ScheduleMethod)) {
    throw new FormInputError(`Method must be one of ${METHODS.join(", ")}.`);
  }
  const path = text(data, "path") || "/";
  if (!path.startsWith("/")) {
    throw new FormInputError("Path must begin with /.");
  }
  const contentType = text(data, "contentType") || "application/json";
  const headers = headersFrom(String(data.get("headers") ?? ""));
  const body = String(data.get("body") ?? "");
  const request: FunctionScheduleRequest = {
    method: method as ScheduleMethod,
    path,
    contentType,
    ...(Object.keys(headers).length === 0 ? {} : { headers }),
    ...(body === "" || method === "GET" ? {} : { body }),
  };
  return {
    cron,
    ...(name === "" ? {} : { name }),
    request,
    enabled: data.get("enabled") === "on",
  };
}

/** `name=value` per line; blank lines are skipped, reserved names are refused. */
function headersFrom(raw: string): Record<string, string> {
  const headers: Record<string, string> = {};
  for (const line of raw.split(/\r?\n/u)) {
    const trimmed = line.trim();
    if (trimmed === "") {
      continue;
    }
    const separator = trimmed.indexOf("=");
    const name = separator === -1 ? "" : trimmed.slice(0, separator).trim();
    if (name === "") {
      throw new FormInputError(`Header lines must be name=value; "${trimmed}" is not.`);
    }
    if (RESERVED_HEADERS.has(name.toLowerCase())) {
      throw new FormInputError(
        `The ${name} header is set by the platform; authorization, host, and content-length are refused.`,
      );
    }
    headers[name] = trimmed.slice(separator + 1).trim();
  }
  return headers;
}

function text(data: FormData, name: string): string {
  return String(data.get(name) ?? "").trim();
}

class FormInputError extends Error {}

function failureFrom(error: unknown): ConsoleApiFailure {
  return error instanceof FormInputError
    ? { message: error.message, requestId: null }
    : toConsoleApiFailure(error);
}

function idempotencyKey(): string {
  return globalThis.crypto.randomUUID();
}
