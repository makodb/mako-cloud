import {
  BarChart,
  Button,
  type ButtonProps,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  type ChartSeries,
  DonutChart,
  type DonutSlice,
  EmptyState,
  Field,
  Input,
  LineChart,
  Table,
  TableBody,
  TableCell,
  TableFooter,
  TableHead,
  TableHeader,
  TableRow,
  Tabs,
  TabsContent,
  TabsLine,
  TabsTrigger,
  cn,
} from "@mako-cloud/ui";
import { ChevronLeft, ChevronRight, Download } from "lucide-react";
import {
  type ComponentProps,
  type FormEvent,
  memo,
  type ReactNode,
  useCallback,
  useMemo,
  useState,
} from "react";

import { isCounted } from "../../functions/shared/exclusions.js";
import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import { type HouseholdCollectionId, isCategory, isGroup, isMerchant } from "../model/types.js";
import {
  addMonths,
  type CashFlowSummary,
  customRangeKey,
  type DateRange,
  inRange,
  type MonthFlow,
  monthLabel,
  monthsBetween,
  type RangeKey,
  rangeFor,
  rangeLabel,
  rangePresets,
  type SpendingContext,
  type SpendingKey,
  type SpendingSlice,
  selectCashFlowSummary,
  selectCategoryTrend,
  selectMonthlySeries,
  selectSankeyFlows,
  shiftRange,
  spendingBy,
  summaryFor,
} from "../selectors/cashflow.js";
import { csvDocument, transactionsCsv } from "../selectors/export.js";
import { amountToText, formatMinorUnits } from "../selectors/money.js";
import { isIsoDate, monthKey, sortTransactions } from "../selectors/transactions.js";
import { compactMoney, shortMonthLabel } from "./charts/layout.js";
import { Sankey } from "./charts/sankey.jsx";
import { Treemap } from "./charts/treemap.jsx";
import { useQuery } from "./hooks.js";
import { type Route, routeHash, transactionsHash } from "./router.js";

/**
 * Cash flow: what came in, what went out, and where it went, over a range the
 * person chooses.
 *
 * Every number here is computed on the device from the documents replication
 * already delivered, by the same selectors the dashboard and the budget read,
 * so "spent this month" is one figure wherever it appears. The range and the
 * breakdown tab live in the address, so a link to this page is a link to
 * exactly what was on it. Nothing is drawn without a table beside or beneath
 * it: the charts are the fast way to read the numbers, the tables are the
 * accessible one, and the tests read the tables.
 */

/** The tabs of the breakdown card; the category view has a card of its own. */
type BreakdownKey = Exclude<SpendingKey, "category">;

const BREAKDOWNS: ReadonlyArray<{ readonly key: BreakdownKey; readonly label: string }> = [
  { key: "group", label: "Group" },
  { key: "merchant", label: "Merchant" },
  { key: "tag", label: "Tag" },
  { key: "account", label: "Account" },
];

/** A trend of one point says nothing, so a short range is shown with the months before it. */
const TREND_MIN_MONTHS = 12;

/** The empty key is "Uncategorized"; the DOM wants a word where a test can find it. */
const UNCATEGORIZED_KEY = "uncategorized";

/** How many slices a donut shows on its own before the rest fold into "Other". */
const DONUT_SLICES = 7;

/**
 * The kit's charts copy their data on every render, and Recharts keys its
 * entry animation on that copy; a screen that re-renders on every sync tick
 * would restart the reveal each time and a line or a donut would never finish
 * appearing. Memoised, with props that keep their identity, they redraw only
 * when what they show has changed.
 */
const StillBarChart = memo(BarChart);
const StillLineChart = memo(LineChart);
const StillDonutChart = memo(DonutChart);

const MONTH_SERIES: ReadonlyArray<ChartSeries> = [
  { key: "income", label: "Income", color: "var(--chart-2)" },
  { key: "spending", label: "Spending", color: "var(--chart-1)" },
];

/** A month key on an axis: "Jun", and "Jan 26" where the year turns. */
function axisMonth(value: string | number): string {
  return shortMonthLabel(String(value));
}

function breakdownKey(view: string | undefined): BreakdownKey {
  return BREAKDOWNS.find((entry) => entry.key === view)?.key ?? "group";
}

/** The default tab stays out of the address so the ordinary link stays short. */
function cashFlowHash(range: RangeKey, view: BreakdownKey): string {
  return routeHash({ name: "cash-flow", range, ...(view === "group" ? {} : { view }) });
}

function slotKey(key: string): string {
  return key === "" ? UNCATEGORIZED_KEY : key;
}

function percent(part: number, whole: number): string {
  return whole > 0 ? `${Math.round((part / whole) * 100)}%` : "—";
}

/**
 * The months a category is trended over: the range's own when it spans a
 * year, otherwise the twelve ending at the range's last month. An unbounded
 * range trends over the months anything happened in.
 */
function trendSpan(range: DateRange | null, series: readonly MonthFlow[], today: string): string[] {
  const last =
    range === null ? (series[series.length - 1]?.month ?? monthKey(today)) : monthKey(range.end);
  const first = range === null ? (series[0]?.month ?? last) : monthKey(range.start);
  const span = monthsBetween(first, last);
  return span.length >= TREND_MIN_MONTHS
    ? span
    : monthsBetween(addMonths(last, 1 - TREND_MIN_MONTHS), last);
}

/** The range as a filename can carry it: `custom:a:b` has colons a filesystem may refuse. */
function fileStem(key: RangeKey): string {
  return key.replaceAll(":", "_");
}

/** Hand the browser a file. The link is not kept: the object URL is the file, and it is released once the click has taken it. */
function download(filename: string, csv: string): void {
  const url = URL.createObjectURL(new Blob([csv], { type: "text/csv;charset=utf-8" }));
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  anchor.rel = "noopener";
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1_000);
}

/** The transactions page narrowed to a slice and the range, for the row links. */
function transactionsLink(
  view: SpendingKey,
  slice: SpendingSlice,
  range: DateRange | null,
  merchantIds: ReadonlySet<string>,
): string | null {
  const bounds = range === null ? {} : { from: range.start, to: range.end };
  switch (view) {
    case "category":
      // A blank category cannot ride in a link -- the router drops empty
      // values -- so "Uncategorized" links to the range alone.
      return slice.key === ""
        ? transactionsHash(bounds)
        : transactionsHash({ ...bounds, category: slice.key });
    case "group":
      return slice.key === ""
        ? transactionsHash(bounds)
        : transactionsHash({ ...bounds, group: slice.key });
    case "merchant":
      // A merchant that is only a cleaned description has no id to filter by.
      return merchantIds.has(slice.key)
        ? transactionsHash({ ...bounds, merchant: slice.key })
        : null;
    case "tag":
      return transactionsHash({ ...bounds, tag: slice.key });
    case "account":
      return transactionsHash({ ...bounds, account: slice.key });
  }
}

/**
 * A breakdown as a donut: the largest slices on their own and the tail as
 * "Other", so a merchant list of forty does not become forty slivers. Only
 * outflows are drawn; a net refund has no share of a whole.
 */
function donutSlices(slices: readonly SpendingSlice[]): DonutSlice[] {
  const positive = slices.filter((slice) => slice.amount > 0);
  const shown = positive
    .slice(0, DONUT_SLICES)
    .map((slice) => ({ name: slice.name, value: slice.amount }));
  const rest = positive.slice(DONUT_SLICES).reduce((sum, slice) => sum + slice.amount, 0);
  return rest > 0
    ? [...shown, { name: "Other", value: rest, color: "var(--muted-foreground)" }]
    : shown;
}

export function CashFlowScreen({
  app,
  session,
  route,
  currency,
}: {
  app: RationalApp;
  session: ScopeSession<HouseholdCollectionId>;
  route: Extract<Route, { name: "cash-flow" }>;
  currency: string;
}) {
  const transactions = useQuery(session.collection("transactions")?.find() ?? null);
  const taxonomy = useQuery(session.collection("taxonomy")?.find() ?? null);
  const accounts = useQuery(
    session.collection("accounts")?.find({ sort: [{ name: "asc" }] }) ?? null,
  );

  const today = new Date().toISOString().slice(0, 10);
  const rangeKey = route.range ?? monthKey(today);
  const view = breakdownKey(route.view);
  // The selectors remember their last result by argument identity, so the
  // range object has to be the same one from render to render.
  const range = useMemo(() => rangeFor(rangeKey, today), [rangeKey, today]);
  const navigate = (next: { readonly range?: RangeKey; readonly view?: BreakdownKey }): void => {
    window.location.hash = cashFlowHash(next.range ?? rangeKey, next.view ?? view);
  };

  const summaries = selectCashFlowSummary(transactions, taxonomy, range);
  // Nothing is converted: the charts and tables show one currency, the
  // household's where it occurs, and the tiles show every currency present.
  const reportCurrency = summaries.some((summary) => summary.currency === currency)
    ? currency
    : (summaries[0]?.currency ?? currency);
  const summary = summaryFor(summaries, reportCurrency);
  const months = selectMonthlySeries(transactions, taxonomy, range, reportCurrency);
  const flows = selectSankeyFlows(transactions, taxonomy, taxonomy, range, reportCurrency);
  const context = useMemo<SpendingContext>(
    () => ({
      categories: taxonomy,
      groups: taxonomy,
      merchants: taxonomy,
      tags: taxonomy,
      accounts,
    }),
    [taxonomy, accounts],
  );
  const byCategory = useMemo(
    () =>
      spendingBy(transactions, range, "category", context).filter(
        (slice) => slice.currency === reportCurrency,
      ),
    [transactions, range, context, reportCurrency],
  );
  const breakdown = useMemo(
    () =>
      spendingBy(transactions, range, view, context).filter(
        (slice) => slice.currency === reportCurrency,
      ),
    [transactions, range, view, context, reportCurrency],
  );
  const merchantIds = useMemo(
    () => new Set(taxonomy.filter(isMerchant).map((entry) => entry.id)),
    [taxonomy],
  );
  const groupOf = useMemo(() => {
    const groupNames = new Map(taxonomy.filter(isGroup).map((entry) => [entry.id, entry.name]));
    return new Map(
      taxonomy
        .filter(isCategory)
        .map((entry) => [entry.id, groupNames.get(entry.parent_id ?? "") ?? ""] as const),
    );
  }, [taxonomy]);

  // The trend follows whichever category was picked, and the largest until
  // one is; a pick that the new range no longer holds falls back the same way.
  const [picked, setPicked] = useState<string | null>(null);
  const trendCategory =
    picked !== null && byCategory.some((slice) => slice.key === picked)
      ? picked
      : (byCategory[0]?.key ?? null);
  const trendMonths = useMemo(() => trendSpan(range, months, today), [range, months, today]);
  const trend =
    trendCategory === null
      ? []
      : selectCategoryTrend(transactions, trendCategory, trendMonths, reportCurrency);
  const trendName =
    trendCategory === null
      ? null
      : (byCategory.find((slice) => slice.key === trendCategory)?.name ?? trendCategory);

  const countedInRange = useMemo(
    () =>
      sortTransactions(
        transactions.filter((entry) => isCounted(entry) && inRange(entry.date, range)),
      ),
    [transactions, range],
  );

  const exportTransactions = (): void => {
    download(
      `transactions-${fileStem(rangeKey)}.csv`,
      transactionsCsv(countedInRange, { accounts, taxonomy }),
    );
  };
  const exportBreakdown = (): void => {
    const label = BREAKDOWNS.find((entry) => entry.key === view)?.label ?? view;
    const total = breakdown.reduce((sum, slice) => sum + slice.amount, 0);
    download(
      `spending-by-${view}-${fileStem(rangeKey)}.csv`,
      csvDocument(
        [label.toLowerCase(), "amount", "currency", "share"],
        breakdown.map((slice) => [
          slice.name,
          amountToText(slice.amount, slice.currency),
          slice.currency,
          percent(slice.amount, total),
        ]),
      ),
    );
  };

  const household = app.state.household;
  const nothingCounted = summary.income === 0 && summary.spending === 0;
  const breakdownLabel = BREAKDOWNS.find((entry) => entry.key === view)?.label ?? view;
  const breakdownTotal = breakdown.reduce((sum, slice) => sum + slice.amount, 0);
  const money = useCallback(
    (value: number) => formatMinorUnits(value, reportCurrency),
    [reportCurrency],
  );
  const axisMoney = useCallback(
    (value: number) => compactMoney(value, reportCurrency),
    [reportCurrency],
  );
  // What the charts draw, held steady between renders so they animate once.
  const monthRows = useMemo(
    () =>
      months.map((month) => ({
        month: month.month,
        income: month.income,
        spending: month.spending,
      })),
    [months],
  );
  const trendRows = useMemo(
    () => trend.map((point) => ({ month: point.month, amount: point.amount })),
    [trend],
  );
  const trendSeries = useMemo<ReadonlyArray<ChartSeries>>(
    () => [{ key: "amount", label: trendName ?? "Spending" }],
    [trendName],
  );
  const slices = useMemo(() => donutSlices(breakdown), [breakdown]);
  const donutCenter = useMemo(
    () => ({ label: "spent", value: money(breakdownTotal) }),
    [money, breakdownTotal],
  );

  return (
    <section
      className="grid gap-6"
      aria-labelledby="cash-flow-title"
      data-testid="cash-flow-screen"
    >
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div className="grid gap-1">
          <h1 id="cash-flow-title" className="text-2xl">
            Cash Flow
          </h1>
          <p className="text-sm text-muted-foreground" data-testid="reports-stamp">
            {household === null || household.syncedAt === null
              ? "Not synced yet — these are your local records."
              : `As of the last sync, ${new Date(household.syncedAt).toLocaleTimeString()}.`}
          </p>
        </div>
        <Button
          variant="outline"
          disabled={countedInRange.length === 0}
          onClick={exportTransactions}
        >
          <Download />
          Export transactions CSV
        </Button>
      </div>

      <RangeBar
        rangeKey={rangeKey}
        range={range}
        today={today}
        onChange={(next) => navigate({ range: next })}
      />

      <SummaryTiles summaries={summaries} fallbackCurrency={currency} />

      <Card aria-labelledby="months-title">
        <CardHeader>
          <CardTitle id="months-title">Income and spending by month</CardTitle>
        </CardHeader>
        <CardContent className="grid gap-3">
          {months.length === 0 ? (
            <EmptyState title="Nothing counted in this range yet." className="py-6" />
          ) : (
            <StillBarChart
              data={monthRows}
              x="month"
              series={MONTH_SERIES}
              title="Income and spending by month"
              height={220}
              formatValue={axisMoney}
              formatX={axisMonth}
            />
          )}
          {months.length === 0 ? null : (
            <details className="text-sm">
              <summary className="cursor-pointer font-medium text-muted-foreground">
                Month by month
              </summary>
              <Table aria-label="Income and spending by month" className="mt-3">
                <TableHeader>
                  <TableRow>
                    <TableHead scope="col">Month</TableHead>
                    <MoneyHead scope="col">Income</MoneyHead>
                    <MoneyHead scope="col">Spending</MoneyHead>
                    <MoneyHead scope="col">Net</MoneyHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {months.map((month) => (
                    <TableRow key={month.month} data-testid={`month-${month.month}`}>
                      <RowHead>{monthLabel(month.month)}</RowHead>
                      <TableCell className="money text-positive" data-testid="income">
                        {formatMinorUnits(month.income, month.currency)}
                      </TableCell>
                      <TableCell className="money" data-testid="spending">
                        {formatMinorUnits(month.spending, month.currency)}
                      </TableCell>
                      <TableCell
                        className={cn("money", month.net < 0 && "text-destructive")}
                        data-testid="net"
                      >
                        {formatMinorUnits(month.net, month.currency)}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </details>
          )}
        </CardContent>
      </Card>

      {nothingCounted ? null : (
        <Card aria-labelledby="flow-title" data-testid="flow-card">
          <CardHeader>
            <CardTitle id="flow-title">Where the money went</CardTitle>
            <CardDescription>
              Income on the left, the groups it paid for in the middle, their categories and what
              was left on the right.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <Sankey
              graph={flows}
              currency={reportCurrency}
              height={440}
              ariaLabel="Where the money went"
            />
          </CardContent>
        </Card>
      )}

      <Card aria-labelledby="categories-title">
        <CardHeader>
          <CardTitle id="categories-title">Spending by category</CardTitle>
          <CardDescription>Pick a category to trend it below.</CardDescription>
        </CardHeader>
        <CardContent className="grid gap-5">
          <Treemap
            items={byCategory.map((slice) => ({
              key: slotKey(slice.key),
              label: slice.name,
              value: slice.amount,
            }))}
            currency={reportCurrency}
            width={960}
            height={320}
            onSelect={(key) => setPicked(key === UNCATEGORIZED_KEY ? "" : key)}
            ariaLabel="Spending by category as area"
            emptyMessage="Nothing spent in this range yet."
          />
          <BreakdownTable
            view="category"
            label="Category"
            slices={byCategory}
            currency={reportCurrency}
            range={range}
            merchantIds={merchantIds}
            selected={trendCategory}
            onPick={setPicked}
            detail={(slice) => groupOf.get(slice.key) ?? ""}
          />
        </CardContent>
      </Card>

      <Card aria-labelledby="breakdown-title">
        <CardHeader className="flex flex-wrap items-start justify-between gap-4">
          <CardTitle id="breakdown-title">Spending by {breakdownLabel.toLowerCase()}</CardTitle>
          <Button
            variant="outline"
            size="sm"
            disabled={breakdown.length === 0}
            onClick={exportBreakdown}
          >
            <Download />
            Export breakdown CSV
          </Button>
        </CardHeader>
        <CardContent>
          <Tabs value={view} onValueChange={(value) => navigate({ view: breakdownKey(value) })}>
            <TabsLine aria-label="Breakdown">
              {BREAKDOWNS.map((entry) => (
                <TabsTrigger
                  key={entry.key}
                  value={entry.key}
                  id={`breakdown-tab-${entry.key}`}
                  data-testid={`breakdown-${entry.key}`}
                >
                  {entry.label}
                </TabsTrigger>
              ))}
            </TabsLine>
            <TabsContent
              value={view}
              aria-labelledby={`breakdown-tab-${view}`}
              className="grid gap-4 pt-2"
            >
              {view === "tag" ? (
                <p className="text-sm text-muted-foreground">
                  A transaction with several tags counts once under each, so tags need not add up to
                  the total.
                </p>
              ) : null}
              <div
                className={cn(
                  "grid items-start gap-5",
                  breakdown.length > 0 && "lg:grid-cols-[14rem_minmax(0,1fr)]",
                )}
              >
                {breakdown.length === 0 ? null : (
                  <StillDonutChart
                    data={slices}
                    title={`Spending by ${breakdownLabel.toLowerCase()} as a share`}
                    height={200}
                    formatValue={money}
                    center={donutCenter}
                  />
                )}
                <BreakdownTable
                  view={view}
                  label={breakdownLabel}
                  slices={breakdown}
                  currency={reportCurrency}
                  range={range}
                  merchantIds={merchantIds}
                />
              </div>
            </TabsContent>
          </Tabs>
        </CardContent>
      </Card>

      <Card aria-labelledby="trend-title">
        <CardHeader className="flex flex-wrap items-start justify-between gap-4">
          <div className="grid gap-1">
            <CardTitle id="trend-title" data-testid="trend-title">
              {trendName === null ? "Trend" : `Trend: ${trendName}`}
            </CardTitle>
            <CardDescription>
              {trendMonths.length === 0
                ? "Pick a category above to see it month by month."
                : `Net spending by month, ${monthLabel(trendMonths[0] ?? "")} to ${monthLabel(
                    trendMonths[trendMonths.length - 1] ?? "",
                  )}; a range shorter than a year is shown with the months before it.`}
            </CardDescription>
          </div>
          {trendCategory === null ? null : (
            <a
              className="text-sm"
              href={transactionsHash({
                ...(range === null ? {} : { from: range.start, to: range.end }),
                ...(trendCategory === "" ? {} : { category: trendCategory }),
              })}
              data-testid="trend-transactions"
            >
              See transactions
            </a>
          )}
        </CardHeader>
        <CardContent>
          {trend.length === 0 ? (
            <EmptyState title="Nothing spent in this range yet." className="py-6" />
          ) : (
            <StillLineChart
              data={trendRows}
              x="month"
              series={trendSeries}
              title={trendName === null ? "Category trend" : `Monthly spending in ${trendName}`}
              height={200}
              formatValue={axisMoney}
              formatX={axisMonth}
            />
          )}
        </CardContent>
      </Card>

      <p className="text-sm text-muted-foreground" data-testid="exclusions-note">
        Transfers between your own accounts, hidden transactions, and balance updates of tracked
        assets are left out of every figure on this page, as they are on the budget and the
        dashboard. The exports carry the same transactions the page counts.
      </p>
    </section>
  );
}

/** A column header over money: right-aligned like the figures under it. */
function MoneyHead({ className, ...props }: ComponentProps<"th">) {
  return <TableHead className={cn("money text-right", className)} {...props} />;
}

/** A row's own heading cell: the kit's `th`, in the body's colour and weight. */
function RowHead({ className, ...props }: ComponentProps<"th">) {
  return (
    <TableHead
      scope="row"
      className={cn("h-auto py-2 font-medium text-foreground", className)}
      {...props}
    />
  );
}

/** One figure of the summary: what it is and the number. */
function Tile({ label, children }: { label: string; children: ReactNode }) {
  return (
    <Card className="gap-0 py-4">
      <CardContent className="grid gap-1 px-4">
        <span className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
          {label}
        </span>
        {children}
      </CardContent>
    </Card>
  );
}

/** A range preset: a pill that shows which one the page is on. */
function PresetButton({ pressed, className, ...props }: ButtonProps & { pressed: boolean }) {
  return (
    <Button
      variant="outline"
      size="sm"
      aria-pressed={pressed}
      className={cn(
        "rounded-full",
        pressed && "border-primary bg-accent text-accent-foreground hover:bg-accent",
        className,
      )}
      {...props}
    />
  );
}

/**
 * The range picker. Presets and the arrows write a key into the address; the
 * custom form writes a `custom:from:to` key, which the arrows cannot step
 * because it has no period to step by.
 */
function RangeBar({
  rangeKey,
  range,
  today,
  onChange,
}: {
  rangeKey: RangeKey;
  range: DateRange | null;
  today: string;
  onChange: (key: RangeKey) => void;
}) {
  const presets = rangePresets(today);
  const isCustom = rangeKey.startsWith("custom:");
  const [customOpen, setCustomOpen] = useState(false);
  const previous = shiftRange(rangeKey, -1);
  const next = shiftRange(rangeKey, 1);
  return (
    <div className="flex flex-wrap items-center gap-x-5 gap-y-3" data-testid="range-bar">
      {/* The two control groups are fieldsets for what they mean, not for how
          a fieldset looks. */}
      <fieldset className="m-0 flex min-w-0 items-center gap-1 border-0 p-0">
        <legend className="sr-only">Period</legend>
        <Button
          variant="outline"
          size="icon-sm"
          aria-label="Previous period"
          disabled={previous === rangeKey}
          onClick={() => onChange(previous)}
        >
          <ChevronLeft />
        </Button>
        <span
          className="min-w-[9.5rem] text-center font-semibold tabular-nums"
          data-testid="range-label"
        >
          {rangeLabel(rangeKey)}
        </span>
        <Button
          variant="outline"
          size="icon-sm"
          aria-label="Next period"
          disabled={next === rangeKey}
          onClick={() => onChange(next)}
        >
          <ChevronRight />
        </Button>
      </fieldset>
      <fieldset className="m-0 flex min-w-0 flex-wrap gap-1.5 border-0 p-0">
        <legend className="sr-only">Range presets</legend>
        {presets.map((preset) => (
          <PresetButton
            key={preset.key}
            pressed={preset.key === rangeKey}
            onClick={() => {
              setCustomOpen(false);
              onChange(preset.key);
            }}
          >
            {preset.label}
          </PresetButton>
        ))}
        <PresetButton pressed={isCustom || customOpen} onClick={() => setCustomOpen(!customOpen)}>
          Custom
        </PresetButton>
      </fieldset>
      {isCustom || customOpen ? (
        <CustomRange key={rangeKey} range={range} today={today} onApply={onChange} />
      ) : null}
    </div>
  );
}

function CustomRange({
  range,
  today,
  onApply,
}: {
  range: DateRange | null;
  today: string;
  onApply: (key: RangeKey) => void;
}) {
  const [start, setStart] = useState(range?.start ?? `${monthKey(today)}-01`);
  const [end, setEnd] = useState(range?.end ?? today);
  const valid = isIsoDate(start) && isIsoDate(end);
  const submit = (event: FormEvent): void => {
    event.preventDefault();
    if (!valid) return;
    onApply(customRangeKey(start <= end ? { start, end } : { start: end, end: start }));
  };
  return (
    <form
      className="flex w-full flex-wrap items-end gap-3"
      aria-label="Custom range"
      onSubmit={submit}
    >
      <Field label="From" htmlFor="range-from">
        <Input
          id="range-from"
          type="date"
          name="from"
          className="w-auto"
          value={start}
          onChange={(event) => setStart(event.target.value)}
        />
      </Field>
      <Field label="To" htmlFor="range-to">
        <Input
          id="range-to"
          type="date"
          name="to"
          className="w-auto"
          value={end}
          onChange={(event) => setEnd(event.target.value)}
        />
      </Field>
      <Button type="submit" disabled={!valid}>
        Apply
      </Button>
    </form>
  );
}

/** One set of tiles per currency the range holds; a range with nothing in it still shows zeros. */
function SummaryTiles({
  summaries,
  fallbackCurrency,
}: {
  summaries: readonly CashFlowSummary[];
  fallbackCurrency: string;
}) {
  const shown = summaries.length === 0 ? [summaryFor([], fallbackCurrency)] : summaries;
  return (
    <>
      {shown.map((summary) => (
        <div
          key={summary.currency}
          className="grid grid-cols-2 gap-4 lg:grid-cols-4"
          data-testid={`cash-flow-summary-${summary.currency}`}
        >
          <Tile label={`Income${shown.length > 1 ? ` (${summary.currency})` : ""}`}>
            <strong data-testid="income" className="text-2xl font-semibold tabular-nums">
              {formatMinorUnits(summary.income, summary.currency)}
            </strong>
          </Tile>
          <Tile label={`Spending${shown.length > 1 ? ` (${summary.currency})` : ""}`}>
            <strong data-testid="spending" className="text-2xl font-semibold tabular-nums">
              {formatMinorUnits(summary.spending, summary.currency)}
            </strong>
          </Tile>
          <Tile label={`Savings${shown.length > 1 ? ` (${summary.currency})` : ""}`}>
            <strong
              data-testid="savings"
              className={cn(
                "text-2xl font-semibold tabular-nums",
                summary.savings < 0 && "text-destructive",
              )}
            >
              {formatMinorUnits(summary.savings, summary.currency)}
            </strong>
          </Tile>
          <Tile label={`Savings rate${shown.length > 1 ? ` (${summary.currency})` : ""}`}>
            <strong data-testid="savings-rate" className="text-2xl font-semibold tabular-nums">
              {summary.income > 0 ? `${Math.round(summary.savingsRate * 100)}%` : "—"}
            </strong>
            <small className="text-xs text-muted-foreground">of income</small>
          </Tile>
        </div>
      ))}
    </>
  );
}

/**
 * A breakdown as a table: what, how much, what share, and a way to the
 * transactions behind it. The total row is what a Sankey has to conserve and
 * what the tests compare with the spending tile; tags are the one view whose
 * rows overlap, and the caller says so above the table.
 */
function BreakdownTable({
  view,
  label,
  slices,
  currency,
  range,
  merchantIds,
  selected,
  onPick,
  detail,
}: {
  view: SpendingKey;
  label: string;
  slices: readonly SpendingSlice[];
  currency: string;
  range: DateRange | null;
  merchantIds: ReadonlySet<string>;
  selected?: string | null;
  onPick?: (key: string) => void;
  detail?: (slice: SpendingSlice) => string;
}) {
  const total = slices.reduce((sum, slice) => sum + slice.amount, 0);
  const columns = 4 + (detail === undefined ? 0 : 1);
  return (
    <Table
      aria-label={`Spending by ${label.toLowerCase()}`}
      data-testid={`breakdown-table-${view}`}
    >
      <TableHeader>
        <TableRow>
          <TableHead scope="col">{label}</TableHead>
          {detail === undefined ? null : <TableHead scope="col">Group</TableHead>}
          <MoneyHead scope="col">Spent</MoneyHead>
          <MoneyHead scope="col">Share</MoneyHead>
          <TableHead scope="col">
            <span className="sr-only">Transactions</span>
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {slices.length === 0 ? (
          <TableRow>
            <TableCell colSpan={columns} className="py-6 text-center text-muted-foreground">
              Nothing spent in this range yet.
            </TableCell>
          </TableRow>
        ) : null}
        {slices.map((slice) => {
          const link = transactionsLink(view, slice, range, merchantIds);
          // The picked row is marked both ways: the kit paints `data-state`,
          // and the class is what the browser suite looks for.
          const picked = selected !== undefined && selected === slice.key;
          return (
            <TableRow
              key={slice.key}
              data-testid={`${view}-${slotKey(slice.key)}`}
              className={picked ? "selected" : undefined}
              data-state={picked ? "selected" : undefined}
            >
              <RowHead>
                {onPick === undefined ? (
                  slice.name
                ) : (
                  <Button
                    variant="link"
                    className="h-auto p-0 font-medium"
                    onClick={() => onPick(slice.key)}
                  >
                    {slice.name}
                  </Button>
                )}
              </RowHead>
              {detail === undefined ? null : (
                <TableCell className="text-muted-foreground">{detail(slice)}</TableCell>
              )}
              <TableCell className="money" data-testid="amount">
                {formatMinorUnits(slice.amount, slice.currency)}
              </TableCell>
              <TableCell className="money text-muted-foreground">
                {percent(slice.amount, total)}
              </TableCell>
              <TableCell className="text-right">
                {link === null ? null : <a href={link}>Transactions</a>}
              </TableCell>
            </TableRow>
          );
        })}
      </TableBody>
      {slices.length === 0 ? null : (
        <TableFooter>
          <TableRow>
            <RowHead className="font-semibold">Total</RowHead>
            {detail === undefined ? null : <TableCell />}
            <TableCell className="money font-semibold" data-testid="total">
              {formatMinorUnits(total, currency)}
            </TableCell>
            <TableCell className="money">{total > 0 ? "100%" : "—"}</TableCell>
            <TableCell />
          </TableRow>
        </TableFooter>
      )}
    </Table>
  );
}
