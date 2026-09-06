import { type FormEvent, useMemo, useState } from "react";

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
import { BarChart, LineChart, Sankey, Treemap } from "./charts/index.js";
import { useQuery } from "./hooks.js";
import { type Route, routeHash, transactionsHash } from "./router.js";
import "./styles/cash-flow.css";

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

  return (
    <section className="cash-flow" aria-labelledby="cash-flow-title" data-testid="cash-flow-screen">
      <div className="heading">
        <div>
          <h1 id="cash-flow-title">Cash Flow</h1>
          <p className="muted stamp" data-testid="reports-stamp">
            {household === null || household.syncedAt === null
              ? "Not synced yet — these are your local records."
              : `As of the last sync, ${new Date(household.syncedAt).toLocaleTimeString()}.`}
          </p>
        </div>
        <button
          type="button"
          className="secondary"
          disabled={countedInRange.length === 0}
          onClick={exportTransactions}
        >
          Export transactions CSV
        </button>
      </div>

      <RangeBar
        rangeKey={rangeKey}
        range={range}
        today={today}
        onChange={(next) => navigate({ range: next })}
      />

      <SummaryTiles summaries={summaries} fallbackCurrency={currency} />

      <section className="flow-card" aria-labelledby="months-title">
        <div className="section-heading">
          <h2 id="months-title">Income and spending by month</h2>
        </div>
        <BarChart
          groups={months.map((month) => ({
            key: month.month,
            label: monthLabel(month.month),
            values: [
              { series: "income", value: month.income },
              { series: "spending", value: month.spending },
            ],
          }))}
          series={[
            { key: "income", label: "Income" },
            { key: "spending", label: "Spending" },
          ]}
          currency={reportCurrency}
          height={220}
          ariaLabel="Income and spending by month"
          emptyMessage="Nothing counted in this range yet."
        />
        {months.length === 0 ? null : (
          <details className="flow-twin">
            <summary>Month by month</summary>
            <div className="flow-scroll">
              <table className="data-table" aria-label="Income and spending by month">
                <thead>
                  <tr>
                    <th scope="col">Month</th>
                    <th scope="col" className="amount">
                      Income
                    </th>
                    <th scope="col" className="amount">
                      Spending
                    </th>
                    <th scope="col" className="amount">
                      Net
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {months.map((month) => (
                    <tr key={month.month} data-testid={`month-${month.month}`}>
                      <th scope="row">{monthLabel(month.month)}</th>
                      <td className="amount" data-testid="income">
                        {formatMinorUnits(month.income, month.currency)}
                      </td>
                      <td className="amount" data-testid="spending">
                        {formatMinorUnits(month.spending, month.currency)}
                      </td>
                      <td className="amount" data-testid="net">
                        {formatMinorUnits(month.net, month.currency)}
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </details>
        )}
      </section>

      {nothingCounted ? null : (
        <section className="flow-card" aria-labelledby="flow-title" data-testid="flow-card">
          <div className="section-heading">
            <div>
              <h2 id="flow-title">Where the money went</h2>
              <p className="muted">
                Income on the left, the groups it paid for in the middle, their categories and what
                was left on the right.
              </p>
            </div>
          </div>
          <Sankey
            graph={flows}
            currency={reportCurrency}
            height={440}
            ariaLabel="Where the money went"
          />
        </section>
      )}

      <section className="flow-card" aria-labelledby="categories-title">
        <div className="section-heading">
          <div>
            <h2 id="categories-title">Spending by category</h2>
            <p className="muted">Pick a category to trend it below.</p>
          </div>
        </div>
        <div className="flow-split">
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
          <div className="flow-scroll">
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
          </div>
        </div>
      </section>

      <section className="flow-card" aria-labelledby="breakdown-title">
        <div className="section-heading">
          <h2 id="breakdown-title">
            Spending by {BREAKDOWNS.find((entry) => entry.key === view)?.label.toLowerCase()}
          </h2>
          <button
            type="button"
            className="secondary"
            disabled={breakdown.length === 0}
            onClick={exportBreakdown}
          >
            Export breakdown CSV
          </button>
        </div>
        <div className="flow-tabs" role="tablist" aria-label="Breakdown">
          {BREAKDOWNS.map((entry) => (
            <button
              key={entry.key}
              type="button"
              role="tab"
              id={`breakdown-tab-${entry.key}`}
              aria-selected={entry.key === view}
              data-testid={`breakdown-${entry.key}`}
              onClick={() => navigate({ view: entry.key })}
            >
              {entry.label}
            </button>
          ))}
        </div>
        <div role="tabpanel" aria-labelledby={`breakdown-tab-${view}`}>
          {view === "tag" ? (
            <p className="hint">
              A transaction with several tags counts once under each, so tags need not add up to the
              total.
            </p>
          ) : null}
          <div className="flow-scroll">
            <BreakdownTable
              view={view}
              label={BREAKDOWNS.find((entry) => entry.key === view)?.label ?? view}
              slices={breakdown}
              currency={reportCurrency}
              range={range}
              merchantIds={merchantIds}
            />
          </div>
        </div>
      </section>

      <section className="flow-card" aria-labelledby="trend-title">
        <div className="section-heading">
          <div>
            <h2 id="trend-title" data-testid="trend-title">
              {trendName === null ? "Trend" : `Trend: ${trendName}`}
            </h2>
            <p className="muted">
              {trendMonths.length === 0
                ? "Pick a category above to see it month by month."
                : `Net spending by month, ${monthLabel(trendMonths[0] ?? "")} to ${monthLabel(
                    trendMonths[trendMonths.length - 1] ?? "",
                  )}; a range shorter than a year is shown with the months before it.`}
            </p>
          </div>
          {trendCategory === null ? null : (
            <a
              href={transactionsHash({
                ...(range === null ? {} : { from: range.start, to: range.end }),
                ...(trendCategory === "" ? {} : { category: trendCategory }),
              })}
              data-testid="trend-transactions"
            >
              See transactions
            </a>
          )}
        </div>
        <LineChart
          points={trend.map((point) => ({ x: point.month, y: point.amount }))}
          currency={reportCurrency}
          height={200}
          showArea
          ariaLabel={trendName === null ? "Category trend" : `Monthly spending in ${trendName}`}
          {...(trendName === null ? {} : { label: trendName })}
          emptyMessage="Nothing spent in this range yet."
        />
      </section>

      <p className="hint footnote" data-testid="exclusions-note">
        Transfers between your own accounts, hidden transactions, and balance updates of tracked
        assets are left out of every figure on this page, as they are on the budget and the
        dashboard. The exports carry the same transactions the page counts.
      </p>
    </section>
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
    <div className="range-bar" data-testid="range-bar">
      <fieldset className="range-nav">
        <legend className="visually-hidden">Period</legend>
        <button
          type="button"
          className="secondary"
          aria-label="Previous period"
          disabled={previous === rangeKey}
          onClick={() => onChange(previous)}
        >
          ‹
        </button>
        <span className="range-label" data-testid="range-label">
          {rangeLabel(rangeKey)}
        </span>
        <button
          type="button"
          className="secondary"
          aria-label="Next period"
          disabled={next === rangeKey}
          onClick={() => onChange(next)}
        >
          ›
        </button>
      </fieldset>
      <fieldset className="range-presets">
        <legend className="visually-hidden">Range presets</legend>
        {presets.map((preset) => (
          <button
            key={preset.key}
            type="button"
            className="secondary"
            aria-pressed={preset.key === rangeKey}
            onClick={() => {
              setCustomOpen(false);
              onChange(preset.key);
            }}
          >
            {preset.label}
          </button>
        ))}
        <button
          type="button"
          className="secondary"
          aria-pressed={isCustom || customOpen}
          onClick={() => setCustomOpen(!customOpen)}
        >
          Custom
        </button>
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
    <form className="range-custom" aria-label="Custom range" onSubmit={submit}>
      <label>
        From
        <input
          type="date"
          name="from"
          value={start}
          onChange={(event) => setStart(event.target.value)}
        />
      </label>
      <label>
        To
        <input type="date" name="to" value={end} onChange={(event) => setEnd(event.target.value)} />
      </label>
      <button type="submit" disabled={!valid}>
        Apply
      </button>
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
          className="totals"
          data-testid={`cash-flow-summary-${summary.currency}`}
        >
          <div className="total">
            <span>Income{shown.length > 1 ? ` (${summary.currency})` : ""}</span>
            <strong data-testid="income">
              {formatMinorUnits(summary.income, summary.currency)}
            </strong>
          </div>
          <div className="total">
            <span>Spending{shown.length > 1 ? ` (${summary.currency})` : ""}</span>
            <strong data-testid="spending">
              {formatMinorUnits(summary.spending, summary.currency)}
            </strong>
          </div>
          <div className="total">
            <span>Savings{shown.length > 1 ? ` (${summary.currency})` : ""}</span>
            <strong data-testid="savings" className={summary.savings < 0 ? "negative" : undefined}>
              {formatMinorUnits(summary.savings, summary.currency)}
            </strong>
          </div>
          <div className="total">
            <span>Savings rate{shown.length > 1 ? ` (${summary.currency})` : ""}</span>
            <strong data-testid="savings-rate">
              {summary.income > 0 ? `${Math.round(summary.savingsRate * 100)}%` : "—"}
            </strong>
            <small>of income</small>
          </div>
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
    <table
      className="data-table flow-table"
      aria-label={`Spending by ${label.toLowerCase()}`}
      data-testid={`breakdown-table-${view}`}
    >
      <thead>
        <tr>
          <th scope="col">{label}</th>
          {detail === undefined ? null : <th scope="col">Group</th>}
          <th scope="col" className="amount">
            Spent
          </th>
          <th scope="col" className="amount">
            Share
          </th>
          <th scope="col">
            <span className="visually-hidden">Transactions</span>
          </th>
        </tr>
      </thead>
      <tbody>
        {slices.length === 0 ? (
          <tr>
            <td colSpan={columns} className="empty">
              Nothing spent in this range yet.
            </td>
          </tr>
        ) : null}
        {slices.map((slice) => {
          const link = transactionsLink(view, slice, range, merchantIds);
          return (
            <tr
              key={slice.key}
              data-testid={`${view}-${slotKey(slice.key)}`}
              className={selected !== undefined && selected === slice.key ? "selected" : undefined}
            >
              <th scope="row">
                {onPick === undefined ? (
                  slice.name
                ) : (
                  <button type="button" className="link" onClick={() => onPick(slice.key)}>
                    {slice.name}
                  </button>
                )}
              </th>
              {detail === undefined ? null : <td className="muted">{detail(slice)}</td>}
              <td className="amount" data-testid="amount">
                {formatMinorUnits(slice.amount, slice.currency)}
              </td>
              <td className="amount muted">{percent(slice.amount, total)}</td>
              <td className="actions">{link === null ? null : <a href={link}>Transactions</a>}</td>
            </tr>
          );
        })}
      </tbody>
      {slices.length === 0 ? null : (
        <tfoot>
          <tr>
            <th scope="row">Total</th>
            {detail === undefined ? null : <td />}
            <td className="amount" data-testid="total">
              {formatMinorUnits(total, currency)}
            </td>
            <td className="amount">{total > 0 ? "100%" : "—"}</td>
            <td />
          </tr>
        </tfoot>
      )}
    </table>
  );
}
