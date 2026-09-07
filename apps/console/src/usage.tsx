// Usage against the plan's allowance, per environment and per project.
//
// The figures are the same ones the bill is rated from: the retained usage
// signals for the current calendar month, aggregated the way `mako-billing`
// aggregates them -- flows sum their records, levels average their samples.
// The allowance comes from the team's live bill, whose non-payable notice is
// rendered verbatim ahead of any number it produced.
import {
  Alert,
  AlertDescription,
  Badge,
  Card,
  CardAction,
  CardContent,
  CardHeader,
  CardTitle,
  Eyebrow,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  cn,
} from "@mako-cloud/ui";
import { Info } from "lucide-react";
import { type ReactNode, useEffect, useState } from "react";

import type {
  Environment,
  MakoManagementClient,
  ObservabilityRecord,
} from "@mako-cloud/management-sdk";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useManagementClient } from "./management.js";

type TeamBill = Awaited<ReturnType<MakoManagementClient["getTeamBill"]>>;
type BillLineItem = TeamBill["lineItems"][number];

/** A small label over a figure. `dt` keeps the definition list intact, so this
 * is the kit's Eyebrow styling on the term rather than the component. */
const FIGURE_LABEL = "text-xs font-semibold uppercase tracking-wider text-muted-foreground";

/** Resources whose records are summed over the period. */
const FLOW_RESOURCES = [
  "replication_requests_per_minute",
  "replication_bytes_per_month",
  "edge_invocations_per_month",
  "edge_compute_milliseconds_per_month",
  "log_bytes_per_month",
  "object_egress_bytes_per_month",
] as const;

/** Resources sampled as a height; the period's figure is the sample average. */
const LEVEL_RESOURCES = [
  "storage_bytes",
  "object_storage_bytes",
  "application_users",
  "environments",
  "collections_per_environment",
  "edge_functions",
] as const;

const RESOURCE_KINDS: ReadonlyMap<string, ResourceKind> = new Map([
  ...FLOW_RESOURCES.map((resource) => [resource, "flow"] as const),
  ...LEVEL_RESOURCES.map((resource) => [resource, "level"] as const),
]);

/** Pages of 1,000 records read for one environment's month before the read
 * stops and says so. */
const MAX_USAGE_PAGES = 10;
const USAGE_PAGE_LIMIT = 1000;

type ResourceKind = "flow" | "level";

export interface ResourceUsage {
  readonly resource: string;
  readonly kind: ResourceKind;
  /** Null for a level with no sample in the period: an average of nothing
   * is not zero. */
  readonly quantity: number | null;
  readonly samples: number;
}

interface MonthUsage {
  readonly resources: readonly ResourceUsage[];
  readonly records: number;
  readonly observedAt: string;
  readonly retainedFrom: string;
  /** True when the page cap stopped the read before the last page. */
  readonly truncated: boolean;
}

type SectionState<T> =
  | { readonly status: "loading" }
  | { readonly status: "ready"; readonly value: T }
  | { readonly status: "unavailable"; readonly failure: ConsoleApiFailure };

/** Aggregate one environment's usage records the way the bill rates them. */
export function aggregateUsage(records: readonly ObservabilityRecord[]): ResourceUsage[] {
  const totals = new Map<string, { total: number; samples: number }>();
  for (const record of records) {
    if (record.payload.kind !== "usage") {
      continue;
    }
    const entry = totals.get(record.payload.resource) ?? { total: 0, samples: 0 };
    entry.total += record.payload.quantity;
    entry.samples += 1;
    totals.set(record.payload.resource, entry);
  }
  const catalog: string[] = [...FLOW_RESOURCES, ...LEVEL_RESOURCES];
  const extra = Array.from(totals.keys())
    .filter((resource) => !RESOURCE_KINDS.has(resource))
    .sort();
  return [...catalog, ...extra].map((resource) => {
    const kind = RESOURCE_KINDS.get(resource) ?? "flow";
    const entry = totals.get(resource);
    if (kind === "flow") {
      return { resource, kind, quantity: entry?.total ?? 0, samples: entry?.samples ?? 0 };
    }
    return {
      resource,
      kind,
      quantity: entry === undefined ? null : entry.total / entry.samples,
      samples: entry?.samples ?? 0,
    };
  });
}

/** The first instant of the calendar month containing `now`, in UTC -- the
 * period the live bill rates. */
export function monthStartUtc(now: Date): Date {
  return new Date(Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), 1));
}

export function formatQuantity(resource: string, value: number): string {
  if (resource.includes("bytes")) {
    if (value >= 1024 * 1024 * 1024) {
      return `${(value / (1024 * 1024 * 1024)).toFixed(2)} GiB`;
    }
    if (value >= 1024 * 1024) {
      return `${(value / (1024 * 1024)).toFixed(1)} MiB`;
    }
    return `${Math.round(value).toLocaleString("en-US")} B`;
  }
  return value.toLocaleString("en-US", { maximumFractionDigits: 1 });
}

/** Micro-dollars as a dollar string. Integer arithmetic end to end; only the
 * display divides. */
function dollars(microDollars: number): string {
  const sign = microDollars < 0 ? "-" : "";
  const absolute = Math.abs(microDollars);
  const whole = Math.floor(absolute / 1_000_000);
  const cents = Math.floor((absolute % 1_000_000) / 10_000);
  return `${sign}$${whole}.${String(cents).padStart(2, "0")}`;
}

function humanize(value: string): string {
  return value.replaceAll("_", " ");
}

function monthLabel(monthStart: Date): string {
  return new Intl.DateTimeFormat("en-US", {
    month: "long",
    year: "numeric",
    timeZone: "UTC",
  }).format(monthStart);
}

function localTime(iso: string): string {
  return new Date(iso).toLocaleString();
}

async function readMonthUsage(
  client: MakoManagementClient,
  projectId: string,
  environmentId: string,
  from: string,
): Promise<MonthUsage> {
  const records: ObservabilityRecord[] = [];
  let cursor: string | undefined;
  let observedAt = "";
  let retainedFrom = "";
  for (let pageIndex = 0; pageIndex < MAX_USAGE_PAGES; pageIndex += 1) {
    const page = await client.queryProjectUsage(projectId, environmentId, {
      limit: USAGE_PAGE_LIMIT,
      from,
      ...(cursor === undefined ? {} : { cursor }),
    });
    records.push(...page.items);
    observedAt = page.retention.observedAt;
    retainedFrom = page.retention.retainedFrom;
    if (page.nextCursor === null) {
      return {
        resources: aggregateUsage(records),
        records: records.length,
        observedAt,
        retainedFrom,
        truncated: false,
      };
    }
    cursor = page.nextCursor;
  }
  return {
    resources: aggregateUsage(records),
    records: records.length,
    observedAt,
    retainedFrom,
    truncated: true,
  };
}

function useTeamBill(projectId: string): SectionState<TeamBill> {
  const client = useManagementClient();
  const [state, setState] = useState<SectionState<TeamBill>>({ status: "loading" });
  useEffect(() => {
    let active = true;
    setState({ status: "loading" });
    void client
      .getProject(projectId)
      .then((project) => client.getTeamBill(project.teamId))
      .then(
        (bill) => active && setState({ status: "ready", value: bill }),
        (error: unknown) =>
          active && setState({ status: "unavailable", failure: toConsoleApiFailure(error) }),
      );
    return () => {
      active = false;
    };
  }, [client, projectId]);
  return state;
}

function useMonthUsage(
  projectId: string,
  environmentId: string,
  from: string,
): SectionState<MonthUsage> {
  const client = useManagementClient();
  const [state, setState] = useState<SectionState<MonthUsage>>({ status: "loading" });
  useEffect(() => {
    let active = true;
    setState({ status: "loading" });
    void readMonthUsage(client, projectId, environmentId, from).then(
      (usage) => active && setState({ status: "ready", value: usage }),
      (error: unknown) =>
        active && setState({ status: "unavailable", failure: toConsoleApiFailure(error) }),
    );
    return () => {
      active = false;
    };
  }, [client, environmentId, from, projectId]);
  return state;
}

function useEnvironments(projectId: string): SectionState<Environment[]> {
  const client = useManagementClient();
  const [state, setState] = useState<SectionState<Environment[]>>({ status: "loading" });
  useEffect(() => {
    let active = true;
    setState({ status: "loading" });
    void client.listEnvironments(projectId).then(
      (environments) => active && setState({ status: "ready", value: environments }),
      (error: unknown) =>
        active && setState({ status: "unavailable", failure: toConsoleApiFailure(error) }),
    );
    return () => {
      active = false;
    };
  }, [client, projectId]);
  return state;
}

export function UsageScreen({
  projectId,
  environmentId,
}: {
  readonly projectId: string;
  readonly environmentId?: string;
}) {
  // The month is fixed when the screen mounts so every section reads the
  // same period; a month boundary crossing mid-visit shows on reload.
  const [monthStart] = useState(() => monthStartUtc(new Date()));
  const bill = useTeamBill(projectId);
  return (
    <section className="grid gap-5" aria-labelledby="usage-title">
      <div className="grid gap-1">
        <Eyebrow>
          {environmentId === undefined ? "Project " : "Environment "}
          <span className="font-mono tracking-normal normal-case">
            {environmentId === undefined ? projectId : environmentId}
          </span>
        </Eyebrow>
        <h1 id="usage-title" className="text-2xl">
          Usage
        </h1>
        <p className="m-0 text-sm text-muted-foreground">
          {monthLabel(monthStart)} (UTC) so far, aggregated the way the bill rates it: flows sum
          their records and levels average their samples.
        </p>
      </div>
      {environmentId === undefined ? (
        <>
          <TeamBillSection bill={bill} />
          <ProjectEnvironmentsUsage projectId={projectId} bill={bill} monthStart={monthStart} />
        </>
      ) : (
        <>
          <PlanAllowanceSection bill={bill} />
          <EnvironmentUsageSection
            projectId={projectId}
            environmentId={environmentId}
            environmentName={environmentId}
            bill={bill}
            monthStart={monthStart}
          />
        </>
      )}
    </section>
  );
}

function ProjectEnvironmentsUsage({
  projectId,
  bill,
  monthStart,
}: {
  readonly projectId: string;
  readonly bill: SectionState<TeamBill>;
  readonly monthStart: Date;
}) {
  const environments = useEnvironments(projectId);
  if (environments.status === "loading") {
    return (
      <Card data-state="loading" aria-label="Environments">
        <CardContent>
          <p className="m-0 text-sm text-muted-foreground" aria-live="polite">
            Loading environments…
          </p>
        </CardContent>
      </Card>
    );
  }
  if (environments.status === "unavailable") {
    return (
      <Card data-state="unavailable" aria-labelledby="usage-environments-title">
        <SectionHeading id="usage-environments-title" title="Environments" state="unavailable" />
        <CardContent>
          <ApiFailureNotice failure={environments.failure} />
        </CardContent>
      </Card>
    );
  }
  if (environments.value.length === 0) {
    return (
      <Card data-state="ready" aria-labelledby="usage-environments-title">
        <SectionHeading id="usage-environments-title" title="Environments" state="ready" />
        <CardContent>
          <p className="m-0 text-sm text-muted-foreground">
            This project has no environments yet, so nothing has been metered.
          </p>
        </CardContent>
      </Card>
    );
  }
  return (
    <>
      {environments.value.map((environment) => (
        <EnvironmentUsageSection
          key={environment.id}
          projectId={projectId}
          environmentId={environment.id}
          environmentName={`${environment.name} · ${environment.state}`}
          bill={bill}
          monthStart={monthStart}
        />
      ))}
    </>
  );
}

function EnvironmentUsageSection({
  projectId,
  environmentId,
  environmentName,
  bill,
  monthStart,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly environmentName: string;
  readonly bill: SectionState<TeamBill>;
  readonly monthStart: Date;
}) {
  const usage = useMonthUsage(projectId, environmentId, monthStart.toISOString());
  const headingId = `usage-${environmentId}-title`;
  const allowances = new Map<string, BillLineItem>(
    bill.status === "ready" ? bill.value.lineItems.map((item) => [item.resource, item]) : [],
  );
  return (
    <Card data-state={usage.status} data-environment-id={environmentId} aria-labelledby={headingId}>
      <SectionHeading id={headingId} title={environmentName} state={usage.status} />
      <CardContent className="grid gap-4">
        {usage.status === "loading" ? (
          <p className="m-0 text-sm text-muted-foreground" aria-live="polite">
            Reading this month's usage…
          </p>
        ) : null}
        {usage.status === "unavailable" ? <ApiFailureNotice failure={usage.failure} /> : null}
        {usage.status === "ready" ? (
          <>
            <p className="m-0 text-sm text-muted-foreground">
              {usage.value.records.toLocaleString("en-US")} records observed at{" "}
              <time dateTime={usage.value.observedAt}>{localTime(usage.value.observedAt)}</time>
              {new Date(usage.value.retainedFrom).getTime() > monthStart.getTime() ? (
                <>
                  {" "}
                  · retained evidence begins{" "}
                  <time dateTime={usage.value.retainedFrom}>
                    {localTime(usage.value.retainedFrom)}
                  </time>
                  , after the month started
                </>
              ) : null}
              {bill.status === "unavailable"
                ? " · plan allowance unavailable, so quantities show without one"
                : null}
            </p>
            {usage.value.truncated ? (
              <Alert variant="warning" role="status">
                <AlertDescription className="block">
                  The read stopped after {MAX_USAGE_PAGES * USAGE_PAGE_LIMIT} records; these figures
                  cover only the records read so far.
                </AlertDescription>
              </Alert>
            ) : null}
            <UsageTable resources={usage.value.resources} allowances={allowances} />
          </>
        ) : null}
      </CardContent>
    </Card>
  );
}

function UsageTable({
  resources,
  allowances,
}: {
  readonly resources: readonly ResourceUsage[];
  readonly allowances: ReadonlyMap<string, BillLineItem>;
}) {
  // `usage-table` is the hook the browser suite selects on.
  return (
    <Table className="usage-table">
      <TableHeader>
        <TableRow className="hover:bg-transparent">
          <TableHead scope="col">Resource</TableHead>
          <TableHead scope="col">Aggregation</TableHead>
          <TableHead scope="col" className="text-right">
            This month
          </TableHead>
          <TableHead scope="col" className="text-right">
            Included
          </TableHead>
          <TableHead scope="col" className="text-right">
            Share of allowance
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {resources.map((row) => {
          const allowance = allowances.get(row.resource);
          const over =
            row.quantity !== null && allowance !== undefined && row.quantity > allowance.included;
          return (
            <TableRow key={row.resource} data-resource={row.resource} data-over={over}>
              <TableHead scope="row">{humanize(row.resource)}</TableHead>
              <TableCell className="text-muted-foreground">
                {row.kind === "flow"
                  ? `sum of ${row.samples.toLocaleString("en-US")} records`
                  : `average of ${row.samples.toLocaleString("en-US")} samples`}
              </TableCell>
              <TableCell className={cn("money", over && "font-medium text-destructive")}>
                {row.quantity === null ? (
                  <span className="font-normal text-muted-foreground italic">no samples</span>
                ) : (
                  formatQuantity(row.resource, row.quantity)
                )}
              </TableCell>
              <TableCell className="money text-muted-foreground">
                {allowance === undefined ? "—" : formatQuantity(row.resource, allowance.included)}
              </TableCell>
              <TableCell className={cn("money", over && "font-medium text-destructive")}>
                <AllowanceShare quantity={row.quantity} allowance={allowance} over={over} />
              </TableCell>
            </TableRow>
          );
        })}
      </TableBody>
    </Table>
  );
}

function AllowanceShare({
  quantity,
  allowance,
  over,
}: {
  readonly quantity: number | null;
  readonly allowance: BillLineItem | undefined;
  readonly over: boolean;
}) {
  if (quantity === null || allowance === undefined || allowance.included <= 0) {
    return <>—</>;
  }
  const share = (quantity / allowance.included) * 100;
  return (
    <>
      {share.toLocaleString("en-US", { maximumFractionDigits: share < 10 ? 1 : 0 })}%
      {over ? (
        <span className="ml-1 text-[0.65rem] font-bold tracking-wider uppercase"> over</span>
      ) : null}
    </>
  );
}

/** The plan and period the environment's allowance comes from. */
function PlanAllowanceSection({ bill }: { readonly bill: SectionState<TeamBill> }) {
  return (
    <Card data-state={bill.status} aria-labelledby="usage-plan-title">
      <SectionHeading id="usage-plan-title" title="Plan allowance" state={bill.status} />
      <CardContent className="grid gap-4">
        {bill.status === "loading" ? (
          <p className="m-0 text-sm text-muted-foreground" aria-live="polite">
            Loading the team's bill…
          </p>
        ) : null}
        {bill.status === "unavailable" ? <ApiFailureNotice failure={bill.failure} /> : null}
        {bill.status === "ready" ? (
          <>
            <BillNotice bill={bill.value} />
            <BillPeriod bill={bill.value} />
            <p className="m-0 text-sm">
              Allowances are the team plan's for the whole period; the bill counts every project of
              the team together, so one environment's share is a guide rather than the limit.
            </p>
          </>
        ) : null}
      </CardContent>
    </Card>
  );
}

/** The team's live bill: plan, period, and the balance as it stands. */
function TeamBillSection({ bill }: { readonly bill: SectionState<TeamBill> }) {
  return (
    <Card data-state={bill.status} aria-labelledby="usage-bill-title">
      <SectionHeading id="usage-bill-title" title="Team bill" state={bill.status} />
      <CardContent className="grid gap-4">
        {bill.status === "loading" ? (
          <p className="m-0 text-sm text-muted-foreground" aria-live="polite">
            Loading the team's bill…
          </p>
        ) : null}
        {bill.status === "unavailable" ? <ApiFailureNotice failure={bill.failure} /> : null}
        {bill.status === "ready" ? (
          <>
            <BillNotice bill={bill.value} />
            <BillPeriod bill={bill.value} />
            {/* `bill-summary` is the hook the browser suite selects on. */}
            <dl className="bill-summary m-0 grid grid-cols-2 gap-3 sm:grid-cols-4">
              <Figure label="Plan">{bill.value.planId}</Figure>
              <Figure label="Total">{dollars(bill.value.totalMicroDollars)}</Figure>
              <Figure label="Credits">{dollars(bill.value.creditsMicroDollars)}</Figure>
              <Figure label="Balance" negative={bill.value.balanceMicroDollars < 0}>
                {dollars(bill.value.balanceMicroDollars)}
                {bill.value.balanceMicroDollars < 0 ? (
                  <span className="sr-only"> (negative)</span>
                ) : null}
              </Figure>
            </dl>
          </>
        ) : null}
      </CardContent>
    </Card>
  );
}

/** One figure of the bill: its name and a value, marked when it has gone negative. */
function Figure({
  label,
  negative,
  children,
}: {
  readonly label: string;
  readonly negative?: boolean;
  readonly children: ReactNode;
}) {
  return (
    <div className="grid content-start gap-0.5 rounded-lg border bg-muted/30 px-3 py-2">
      <dt className={FIGURE_LABEL}>{label}</dt>
      <dd
        className={cn("m-0 text-lg font-semibold tabular-nums", negative && "text-destructive")}
        {...(negative === undefined ? {} : { "data-negative": negative })}
      >
        {children}
      </dd>
    </div>
  );
}

/** The notice is the contract of the beta bill: shown verbatim, before any
 * number the bill produced. */
function BillNotice({ bill }: { readonly bill: TeamBill }) {
  return (
    <Alert role="note">
      <Info aria-hidden="true" />
      <AlertDescription className="block text-foreground">{bill.notice}</AlertDescription>
    </Alert>
  );
}

function BillPeriod({ bill }: { readonly bill: TeamBill }) {
  return (
    <p className="m-0 text-sm text-muted-foreground">
      Plan <strong className="text-foreground">{bill.planId}</strong> · period{" "}
      <time dateTime={bill.periodStart}>{bill.periodStart.slice(0, 10)}</time> to{" "}
      <time dateTime={bill.periodEnd}>{bill.periodEnd.slice(0, 10)}</time>
      {bill.finalized ? " (closed)" : " (live)"} · observed at{" "}
      <time dateTime={bill.observedAt}>{localTime(bill.observedAt)}</time>
    </p>
  );
}

function SectionHeading({
  id,
  title,
  state,
}: {
  readonly id: string;
  readonly title: string;
  readonly state: SectionState<unknown>["status"];
}) {
  return (
    <CardHeader>
      <CardTitle id={id}>{title}</CardTitle>
      <CardAction>
        <Badge
          variant={
            state === "ready" ? "positive" : state === "loading" ? "secondary" : "destructive"
          }
        >
          {state === "ready" ? "Current" : state === "loading" ? "Loading" : "Unavailable"}
        </Badge>
      </CardAction>
    </CardHeader>
  );
}
