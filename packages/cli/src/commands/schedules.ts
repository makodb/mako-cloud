import type {
  FunctionSchedule,
  FunctionScheduleCreate,
  FunctionScheduleRequest,
  FunctionScheduleRunOutcome,
  FunctionScheduleUpdate,
} from "@mako-cloud/management-sdk";

import type { CommandContext } from "../cli/context.js";
import { usageError } from "../cli/errors.js";
import type { TableColumn } from "../cli/output.js";
import type { Command, CommandArgs, OptionSpec, PositionalSpec } from "../cli/registry.js";
import { pairsToObject, TENANT_OPTIONS, tenantFrom } from "./shared.js";

type ScheduleMethod = FunctionScheduleRequest["method"];

const METHODS: readonly ScheduleMethod[] = ["GET", "POST", "PUT", "PATCH", "DELETE"];
const OUTCOMES: readonly FunctionScheduleRunOutcome[] = [
  "succeeded",
  "failed",
  "error",
  "skipped_overlap",
];
/** Headers the platform sets itself; the API refuses them and so does the CLI, earlier. */
const RESERVED_HEADERS: ReadonlySet<string> = new Set(["authorization", "host", "content-length"]);
/** The API's ceiling on one page of runs. */
const MAX_LIST_LIMIT = 200;
const DEFAULT_REQUEST: FunctionScheduleRequest = {
  method: "POST",
  path: "/",
  contentType: "application/json",
};

const SCHEDULE_COLUMNS: readonly TableColumn[] = [
  { key: "id" },
  { key: "name" },
  { key: "cron" },
  { key: "state" },
  { key: "nextRunAt" },
  { key: "lastRun" },
];

const RUN_COLUMNS: readonly TableColumn[] = [
  { key: "id" },
  { key: "dueAt" },
  { key: "startedAt" },
  { key: "durationMilliseconds", label: "ms" },
  { key: "outcome" },
  { key: "responseStatus", label: "status" },
  { key: "error" },
  { key: "manual" },
];

const SCHEDULE_ID: PositionalSpec = {
  name: "schedule-id",
  description: "Schedule id (sch_…)",
  required: true,
};

/** Every schedules command is scoped to one deployed function. */
const FUNCTION_OPTION: Readonly<Record<string, OptionSpec>> = {
  function: {
    type: "string",
    short: "f",
    description: "Name of the function the schedule belongs to",
    placeholder: "<name>",
    required: true,
  },
};

/** What `schedules create` and `schedules update` share: the schedule and its request. */
const SCHEDULE_OPTIONS: Readonly<Record<string, OptionSpec>> = {
  cron: {
    type: "string",
    description:
      'Five-field cron expression in UTC (minute hour day-of-month month day-of-week), e.g. "0 2 * * *"',
    placeholder: "<expr>",
  },
  name: {
    type: "string",
    description: "A name for people; not part of any request",
    placeholder: "<text>",
  },
  method: {
    type: "string",
    description: "HTTP method of the request each due time sends (default: POST)",
    placeholder: `<${METHODS.join("|")}>`,
  },
  path: {
    type: "string",
    description:
      "Path under the function, beginning with /; a query string is allowed (default: /)",
    placeholder: "<path>",
  },
  header: {
    type: "string",
    multiple: true,
    description:
      "Extra request header; repeat for several (authorization, host, and content-length are refused)",
    placeholder: "<name=value>",
  },
  "content-type": {
    type: "string",
    description: "Content type of the body (default: application/json)",
    placeholder: "<type>",
  },
  body: {
    type: "string",
    description: "Request body as text: @path, - for stdin, or inline text; omitted for GET",
    placeholder: "<@file|-|text>",
  },
};

function functionOf(args: CommandArgs): string {
  return args.requireString("function");
}

function scheduleOf(args: CommandArgs): string {
  return args.requirePositional(0, "schedule-id");
}

function methodFrom(args: CommandArgs): ScheduleMethod | undefined {
  const method = args.string("method");
  if (method === undefined) return undefined;
  const upper = method.toUpperCase();
  if (!METHODS.includes(upper as ScheduleMethod)) {
    throw usageError(`--method must be one of ${METHODS.join(", ")}`);
  }
  return upper as ScheduleMethod;
}

function pathFrom(args: CommandArgs): string | undefined {
  const path = args.string("path");
  if (path === undefined) return undefined;
  if (!path.startsWith("/")) throw usageError("--path must begin with /");
  return path;
}

function headersFrom(args: CommandArgs): Record<string, string> | undefined {
  if (args.values.header === undefined) return undefined;
  const headers = pairsToObject(args.strings("header"), "--header");
  for (const name of Object.keys(headers)) {
    if (RESERVED_HEADERS.has(name.toLowerCase())) {
      throw usageError(
        `--header ${name} is set by the platform; ${[...RESERVED_HEADERS].join(", ")} are refused`,
      );
    }
  }
  return headers;
}

async function bodyFrom(context: CommandContext, args: CommandArgs): Promise<string | undefined> {
  const source = args.string("body");
  if (source === undefined) return undefined;
  return context.readInput(source);
}

function outcomeFrom(args: CommandArgs): FunctionScheduleRunOutcome | undefined {
  const outcome = args.string("outcome");
  if (outcome === undefined) return undefined;
  if (!OUTCOMES.includes(outcome as FunctionScheduleRunOutcome)) {
    throw usageError(`--outcome must be one of ${OUTCOMES.join(", ")}`);
  }
  return outcome as FunctionScheduleRunOutcome;
}

/**
 * The request options as given, or undefined when none was. The API stores a
 * whole request, so the caller lays the given fields over the current ones.
 */
async function requestChangesFrom(
  context: CommandContext,
  args: CommandArgs,
): Promise<Partial<FunctionScheduleRequest> | undefined> {
  const method = methodFrom(args);
  const path = pathFrom(args);
  const headers = headersFrom(args);
  const contentType = args.string("content-type");
  const body = await bodyFrom(context, args);
  const changes: Partial<FunctionScheduleRequest> = {
    ...(method !== undefined ? { method } : {}),
    ...(path !== undefined ? { path } : {}),
    ...(headers !== undefined ? { headers } : {}),
    ...(contentType !== undefined ? { contentType } : {}),
    ...(body !== undefined ? { body } : {}),
  };
  return Object.keys(changes).length === 0 ? undefined : changes;
}

/** The last run as one cell for a table; --json keeps the whole record. */
function lastRunCell(schedule: FunctionSchedule): string | null {
  const run = schedule.lastRun;
  if (run === null) return null;
  const parts: string[] = [run.outcome];
  if (run.responseStatus !== null) parts.push(String(run.responseStatus));
  if (run.durationMilliseconds !== null) parts.push(`${run.durationMilliseconds} ms`);
  return `${parts.join(" ")} at ${run.dueAt}`;
}

async function listSchedules(context: CommandContext, args: CommandArgs): Promise<void> {
  const functionName = functionOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  const schedules = await client.listFunctionSchedules(projectId, environmentId, functionName);
  if (context.json) {
    context.out(schedules);
    return;
  }
  context.out(
    schedules.map((schedule) => ({ ...schedule, lastRun: lastRunCell(schedule) })),
    { columns: SCHEDULE_COLUMNS },
  );
}

async function getSchedule(context: CommandContext, args: CommandArgs): Promise<void> {
  const functionName = functionOf(args);
  const scheduleId = scheduleOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(await client.getFunctionSchedule(projectId, environmentId, functionName, scheduleId));
}

async function createSchedule(context: CommandContext, args: CommandArgs): Promise<void> {
  const functionName = functionOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const cron = args.string("cron");
  if (cron === undefined || cron.trim() === "") throw usageError("--cron <expr> is required");
  const name = args.string("name");
  const changes = await requestChangesFrom(context, args);
  const request: FunctionScheduleCreate = {
    cron,
    ...(name !== undefined ? { name } : {}),
    ...(changes !== undefined ? { request: { ...DEFAULT_REQUEST, ...changes } } : {}),
    enabled: !args.boolean("disabled"),
  };
  const client = await context.management();
  const schedule = await client.createFunctionSchedule(
    projectId,
    environmentId,
    functionName,
    request,
    context.idempotencyKey(),
  );
  context.out(schedule);
}

async function updateSchedule(context: CommandContext, args: CommandArgs): Promise<void> {
  const functionName = functionOf(args);
  const scheduleId = scheduleOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const enable = args.boolean("enable");
  const disable = args.boolean("disable");
  if (enable && disable) throw usageError("--enable and --disable exclude each other");
  const cron = args.string("cron");
  const name = args.string("name");
  const changes = await requestChangesFrom(context, args);
  if (cron === undefined && name === undefined && changes === undefined && !enable && !disable) {
    throw usageError(
      "nothing to update: pass at least one of --cron, --name, --method, --path, --header, --content-type, --body, --enable, --disable",
    );
  }
  const client = await context.management();
  // The request is stored whole, so a changed field is laid over the current
  // request rather than sent alone; everything else is sent only when given.
  const current =
    changes === undefined
      ? undefined
      : await client.getFunctionSchedule(projectId, environmentId, functionName, scheduleId);
  const request: FunctionScheduleUpdate = {
    ...(cron !== undefined ? { cron } : {}),
    ...(name !== undefined ? { name } : {}),
    ...(current !== undefined && changes !== undefined
      ? { request: { ...current.request, ...changes } }
      : {}),
    ...(enable || disable ? { enabled: enable } : {}),
  };
  const schedule = await client.updateFunctionSchedule(
    projectId,
    environmentId,
    functionName,
    scheduleId,
    request,
    context.idempotencyKey(),
  );
  context.out(schedule);
}

async function deleteSchedule(context: CommandContext, args: CommandArgs): Promise<void> {
  const functionName = functionOf(args);
  const scheduleId = scheduleOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  await client.deleteFunctionSchedule(
    projectId,
    environmentId,
    functionName,
    scheduleId,
    context.idempotencyKey(),
  );
  if (context.json) context.out({ id: scheduleId, state: "deleted" });
  else context.info(`schedule ${scheduleId} deleted`);
}

async function runNow(context: CommandContext, args: CommandArgs): Promise<void> {
  const functionName = functionOf(args);
  const scheduleId = scheduleOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const client = await context.management();
  context.out(
    await client.runFunctionScheduleNow(
      projectId,
      environmentId,
      functionName,
      scheduleId,
      context.idempotencyKey(),
    ),
  );
}

async function listRuns(context: CommandContext, args: CommandArgs): Promise<void> {
  const functionName = functionOf(args);
  const scheduleId = scheduleOf(args);
  const { projectId, environmentId } = tenantFrom(context, args);
  const outcome = outcomeFrom(args);
  const limit = args.integer("limit");
  if (limit !== undefined && (limit < 1 || limit > MAX_LIST_LIMIT)) {
    throw usageError(`--limit must be between 1 and ${MAX_LIST_LIMIT}`);
  }
  const start = args.string("cursor");
  const client = await context.management();
  const page = await context.collect((cursor) => {
    const next = cursor ?? start;
    return client.listFunctionScheduleRuns(projectId, environmentId, functionName, scheduleId, {
      ...(outcome !== undefined ? { outcome } : {}),
      ...(limit !== undefined ? { limit } : {}),
      ...(next !== undefined ? { cursor: next } : {}),
    });
  });
  context.out(page, { columns: RUN_COLUMNS });
}

export const schedulesCommands: readonly Command[] = [
  {
    path: ["schedules", "list"],
    summary: "List a function's cron schedules with their state, next run, and last run",
    operations: ["listFunctionSchedules"],
    options: { ...TENANT_OPTIONS, ...FUNCTION_OPTION },
    run: listSchedules,
  },
  {
    path: ["schedules", "get"],
    summary: "Show a schedule, the request it sends, its next run, and its last run",
    operations: ["getFunctionSchedule"],
    positionals: [SCHEDULE_ID],
    options: { ...TENANT_OPTIONS, ...FUNCTION_OPTION },
    run: getSchedule,
  },
  {
    path: ["schedules", "create"],
    summary:
      "Attach a UTC cron schedule to a deployed function; an invalid expression is refused at once",
    operations: ["createFunctionSchedule"],
    options: {
      ...TENANT_OPTIONS,
      ...FUNCTION_OPTION,
      ...SCHEDULE_OPTIONS,
      disabled: {
        type: "boolean",
        description: "Save the schedule paused; it runs once enabled",
      },
    },
    run: createSchedule,
  },
  {
    path: ["schedules", "update"],
    summary:
      "Change a schedule's expression, name, request, or enabled flag; only given options are sent",
    operations: ["updateFunctionSchedule", "getFunctionSchedule"],
    positionals: [SCHEDULE_ID],
    options: {
      ...TENANT_OPTIONS,
      ...FUNCTION_OPTION,
      ...SCHEDULE_OPTIONS,
      enable: { type: "boolean", description: "Resume the schedule; the next run is recomputed" },
      disable: { type: "boolean", description: "Pause the schedule without losing it" },
    },
    run: updateSchedule,
  },
  {
    path: ["schedules", "delete"],
    summary: "Remove a schedule and its run history",
    operations: ["deleteFunctionSchedule"],
    positionals: [SCHEDULE_ID],
    options: { ...TENANT_OPTIONS, ...FUNCTION_OPTION },
    destructive: { action: "delete schedule", resource: scheduleOf },
    run: deleteSchedule,
  },
  {
    path: ["schedules", "run-now"],
    summary:
      "Queue one run of a schedule outside its cron times, recorded as manual; refused while a run is executing",
    operations: ["runFunctionScheduleNow"],
    positionals: [SCHEDULE_ID],
    options: { ...TENANT_OPTIONS, ...FUNCTION_OPTION },
    run: runNow,
  },
  {
    path: ["schedules", "runs"],
    summary:
      "List a schedule's runs, newest first, including ones skipped for overlap; --all follows the cursor to the end",
    operations: ["listFunctionScheduleRuns"],
    positionals: [SCHEDULE_ID],
    options: {
      ...TENANT_OPTIONS,
      ...FUNCTION_OPTION,
      outcome: {
        type: "string",
        description: "Only runs with this outcome",
        placeholder: `<${OUTCOMES.join("|")}>`,
      },
      limit: {
        type: "string",
        description: `Runs per page, 1-${MAX_LIST_LIMIT} (default: 50)`,
        placeholder: "<n>",
      },
      cursor: {
        type: "string",
        description: "Continue from a previous page's next cursor",
        placeholder: "<cursor>",
      },
    },
    run: listRuns,
  },
];
