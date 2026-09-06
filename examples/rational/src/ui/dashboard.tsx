import { type ReactNode, useMemo, useState } from "react";

import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import {
  type BudgetMode,
  type Category,
  type Household,
  type HouseholdCollectionId,
  isCategory,
  type Recurrence,
  type Transaction,
} from "../model/types.js";
import {
  type MonthBudgetModel,
  monthLabel,
  previousMonth,
  selectBudgetMonth,
} from "../selectors/budget-month.js";
import {
  type DashboardInput,
  type DashboardSummary,
  selectDashboard,
} from "../selectors/dashboard.js";
import type { GoalRow } from "../selectors/goals.js";
import { selectNetWorthHistoryInRange } from "../selectors/history.js";
import { type MerchantResolver, merchantResolver } from "../selectors/merchants.js";
import { formatMinorUnits } from "../selectors/money.js";
import {
  addDays,
  type BillState,
  type ResolvedBill,
  recurrenceName,
} from "../selectors/recurrences.js";
import { needsReview } from "../selectors/review.js";
import { monthKey } from "../selectors/transactions.js";
import {
  axisLabel,
  Meter,
  ProgressBar,
  readoutLabel,
  Sparkline,
  shortMonthLabel,
} from "./charts/index.js";
import { useQuery } from "./hooks.js";
import { routeHash, transactionsHash } from "./router.js";
import "./styles/dashboard.css";

/**
 * The landing page: the household at a glance, every number borrowed.
 *
 * Nothing on this screen is computed here. Each card shows a figure that some
 * other screen owns -- the accounts page's net worth, the cash-flow page's
 * spending, the budget page's headroom -- read from the same selector that
 * screen renders from, and each card is a door to that screen. A dashboard
 * that disagreed with the page it points at, even by a transfer leg, would
 * teach the household to trust neither; `selectors/dashboard.ts` is where
 * that promise is kept, and this file only decides how the cards look and
 * which of them a person wants to see.
 */

/** How many days of nightly snapshots the net-worth line covers. */
const HISTORY_DAYS = 90;

const WIDGET_IDS = [
  "net-worth",
  "spending",
  "budget",
  "bills",
  "goals",
  "review",
  "notifications",
  "recent",
  "investments",
] as const;
type WidgetId = (typeof WIDGET_IDS)[number];

/** How much of the grid a card takes: two columns for the hero, three for the list, one otherwise. */
type WidgetSize = "wide" | "single" | "full";

interface WidgetDefinition {
  readonly title: string;
  readonly size: WidgetSize;
  /**
   * The words on the card's link. They are the card's own, never the
   * sidebar's labels: the browser tests find the sidebar's "Accounts" by its
   * name, and a second link whose name contained the same word would make
   * that lookup ambiguous on the one screen everybody starts from.
   */
  readonly linkLabel: string;
}

const WIDGETS: Readonly<Record<WidgetId, WidgetDefinition>> = {
  "net-worth": { title: "Net worth", size: "wide", linkLabel: "See all balances" },
  spending: { title: "Spending", size: "single", linkLabel: "Compare the months" },
  budget: { title: "Budget", size: "single", linkLabel: "Plan the month" },
  bills: { title: "Upcoming bills", size: "single", linkLabel: "See every bill" },
  goals: { title: "Goals", size: "single", linkLabel: "See progress" },
  review: { title: "Review", size: "single", linkLabel: "Review them" },
  notifications: { title: "Notifications", size: "single", linkLabel: "Read them" },
  recent: { title: "Recent transactions", size: "full", linkLabel: "See the full list" },
  investments: { title: "Investments", size: "single", linkLabel: "See holdings" },
};

/** Where a card leads: the screen that owns its number, opened on what the card shows. */
function destinationOf(id: WidgetId, month: string): string {
  switch (id) {
    case "net-worth":
      return routeHash({ name: "accounts" });
    case "spending":
      return routeHash({ name: "cash-flow", range: month });
    case "budget":
      return routeHash({ name: "budget", month });
    case "bills":
      return routeHash({ name: "recurring" });
    case "goals":
      return routeHash({ name: "goals" });
    case "review":
      return transactionsHash({ review: "needs" });
    case "notifications":
      return routeHash({ name: "settings", page: "notifications" });
    case "recent":
      return transactionsHash({});
    case "investments":
      return routeHash({ name: "investments" });
  }
}

/* --- The layout: which cards, in what order, remembered on this device ---- */

interface Layout {
  readonly order: readonly WidgetId[];
  readonly hidden: readonly WidgetId[];
}

const LAYOUT_KEY = "rational.dashboard.layout";
const DEFAULT_LAYOUT: Layout = { order: WIDGET_IDS, hidden: [] };

function isWidgetId(value: unknown): value is WidgetId {
  return typeof value === "string" && (WIDGET_IDS as readonly string[]).includes(value);
}

/**
 * A stored layout made whole. A card this version no longer has is dropped;
 * a card the stored layout never heard of -- added since the layout was saved
 * -- is appended in its default place rather than lost, so a person who
 * customized last year still sees what was added this year.
 */
function normalizeLayout(candidate: unknown): Layout {
  const record =
    typeof candidate === "object" && candidate !== null
      ? (candidate as Record<string, unknown>)
      : {};
  const storedOrder = Array.isArray(record.order) ? record.order.filter(isWidgetId) : [];
  const storedHidden = Array.isArray(record.hidden) ? record.hidden.filter(isWidgetId) : [];
  return {
    order: [...new Set<WidgetId>([...storedOrder, ...WIDGET_IDS])],
    hidden: [...new Set<WidgetId>(storedHidden)],
  };
}

function readLayout(): Layout {
  try {
    const stored = window.localStorage.getItem(LAYOUT_KEY);
    return stored === null ? DEFAULT_LAYOUT : normalizeLayout(JSON.parse(stored));
  } catch {
    return DEFAULT_LAYOUT;
  }
}

function writeLayout(layout: Layout | null): void {
  try {
    if (layout === null) window.localStorage.removeItem(LAYOUT_KEY);
    else window.localStorage.setItem(LAYOUT_KEY, JSON.stringify(layout));
  } catch {
    // Not remembering is fine: the default layout is a good dashboard too.
  }
}

/**
 * The layout is a preference of the device, like the theme, not a document
 * of the household: two members of one household want different things at
 * the top, and neither should be able to rearrange the other's morning.
 */
function useLayout(): {
  readonly layout: Layout;
  readonly move: (id: WidgetId, direction: -1 | 1) => void;
  readonly toggle: (id: WidgetId) => void;
  readonly reset: () => void;
} {
  const [layout, setLayout] = useState<Layout>(readLayout);
  const change = (next: Layout): void => {
    setLayout(next);
    writeLayout(next);
  };
  return {
    layout,
    move: (id, direction) => {
      const index = layout.order.indexOf(id);
      const target = index + direction;
      const other = layout.order[target];
      if (index < 0 || other === undefined) return;
      const order = [...layout.order];
      order[index] = other;
      order[target] = id;
      change({ ...layout, order });
    },
    toggle: (id) => {
      change({
        ...layout,
        hidden: layout.hidden.includes(id)
          ? layout.hidden.filter((candidate) => candidate !== id)
          : [...layout.hidden, id],
      });
    },
    reset: () => {
      setLayout(DEFAULT_LAYOUT);
      writeLayout(null);
    },
  };
}

/* --- The screen ---------------------------------------------------------- */

export function DashboardScreen({
  session,
  currency,
  household,
}: {
  app: RationalApp;
  session: ScopeSession<HouseholdCollectionId>;
  currency: string;
  household: Household | null;
}) {
  const accounts = useQuery(session.collection("accounts")?.find() ?? null);
  const transactions = useQuery(session.collection("transactions")?.find() ?? null);
  const taxonomy = useQuery(session.collection("taxonomy")?.find() ?? null);
  const budgets = useQuery(session.collection("budgets")?.find() ?? null);
  const recurrences = useQuery(session.collection("recurrences")?.find() ?? null);
  const goals = useQuery(session.collection("goals")?.find() ?? null);
  const alerts = useQuery(session.collection("alerts")?.find() ?? null);
  const snapshots = useQuery(session.collection("net_worth_snapshots")?.find() ?? null);

  const today = new Date().toISOString().slice(0, 10);
  const thisMonth = monthKey(today);
  const budgetMode: BudgetMode = household?.budget_mode ?? "category";

  // The selectors remember their last arguments by identity, and the query
  // arrays only change when documents do; the input object has to hold still
  // too, or every render would recompute the whole page.
  const input = useMemo<DashboardInput>(
    () => ({
      accounts,
      transactions,
      taxonomy,
      budgets,
      recurrences,
      goals,
      alerts,
      snapshots,
      today,
      currency,
      budgetMode,
    }),
    [
      accounts,
      transactions,
      taxonomy,
      budgets,
      recurrences,
      goals,
      alerts,
      snapshots,
      today,
      currency,
      budgetMode,
    ],
  );
  const summary = selectDashboard(input);
  const budget = selectBudgetMonth(budgets, transactions, taxonomy, thisMonth, currency, goals);
  const historyRange = useMemo(
    () => ({ start: addDays(today, -HISTORY_DAYS), end: today }),
    [today],
  );
  const history = selectNetWorthHistoryInRange(snapshots, currency, historyRange);
  const resolveMerchant = useMemo(() => merchantResolver(taxonomy), [taxonomy]);
  const categories = useMemo(
    () => new Map(taxonomy.filter(isCategory).map((category) => [category.id, category])),
    [taxonomy],
  );

  const { layout, move, toggle, reset } = useLayout();
  const [customizing, setCustomizing] = useState(false);
  const shown = layout.order.filter((id) => customizing || !layout.hidden.includes(id));

  const body = (id: WidgetId): ReactNode => {
    switch (id) {
      case "net-worth":
        return <NetWorthCard summary={summary} history={history} currency={currency} />;
      case "spending":
        return <SpendingCard summary={summary} month={thisMonth} currency={currency} />;
      case "budget":
        return (
          <BudgetCard
            summary={summary}
            model={budget}
            mode={budgetMode}
            month={thisMonth}
            currency={currency}
            href={destinationOf("budget", thisMonth)}
          />
        );
      case "bills":
        return <BillsCard bills={summary.nextBills} />;
      case "goals":
        return <GoalsCard goals={summary.goals} href={destinationOf("goals", thisMonth)} />;
      case "review":
        return (
          <StatCard
            figure={String(summary.reviewCount)}
            figureTestId="review-count"
            caption={summary.reviewCount === 1 ? "transaction to review" : "transactions to review"}
            note={
              summary.reviewCount === 0
                ? "Everything that arrived by sync or import has been looked at."
                : "Arrived by sync or import; nobody has looked at them yet."
            }
          />
        );
      case "notifications":
        return (
          <StatCard
            figure={String(summary.unreadAlerts)}
            figureTestId="unread-alerts"
            caption={summary.unreadAlerts === 1 ? "unread notification" : "unread notifications"}
            note="Bills coming due, goals reached, budgets gone over, connections that stopped syncing."
          />
        );
      case "recent":
        return (
          <RecentCard
            transactions={summary.recentTransactions}
            resolveMerchant={resolveMerchant}
            categories={categories}
          />
        );
      case "investments":
        return (
          <StatCard
            figure={formatMinorUnits(summary.investmentsTotal, currency)}
            figureTestId="investments-total"
            caption="in investment and crypto accounts"
            note="Positions at their last price, plus the cash beside them."
          />
        );
    }
  };

  return (
    <section aria-labelledby="dashboard-title" data-testid="dashboard-screen" className="dashboard">
      <div className="heading">
        <div>
          <h1 id="dashboard-title">Dashboard</h1>
          <p className="muted dashboard-subtitle">
            {household?.name ?? "Your space"} · {readoutLabel(today)}
          </p>
        </div>
        <div className="dashboard-toolbar">
          {customizing ? (
            <button type="button" className="link" onClick={reset}>
              Reset layout
            </button>
          ) : null}
          <button
            type="button"
            className="secondary"
            aria-pressed={customizing}
            data-testid="customize-toggle"
            onClick={() => setCustomizing(!customizing)}
          >
            {customizing ? "Done" : "Customize"}
          </button>
        </div>
      </div>
      {customizing ? (
        <p className="hint" role="status">
          Hide the cards you do not need and move the rest. The layout is remembered on this device
          only.
        </p>
      ) : null}
      <div className="dashboard-grid" data-testid="dashboard-grid">
        {shown.map((id, index) => {
          const definition = WIDGETS[id];
          return (
            <Widget
              key={id}
              id={id}
              definition={definition}
              href={destinationOf(id, thisMonth)}
              hidden={layout.hidden.includes(id)}
              customizing={customizing}
              first={index === 0}
              last={index === shown.length - 1}
              onMove={(direction) => move(id, direction)}
              onToggle={() => toggle(id)}
            >
              {body(id)}
            </Widget>
          );
        })}
      </div>
    </section>
  );
}

/* --- The card ------------------------------------------------------------ */

function Widget({
  id,
  definition,
  href,
  hidden,
  customizing,
  first,
  last,
  onMove,
  onToggle,
  children,
}: {
  id: WidgetId;
  definition: WidgetDefinition;
  href: string;
  hidden: boolean;
  customizing: boolean;
  first: boolean;
  last: boolean;
  onMove: (direction: -1 | 1) => void;
  onToggle: () => void;
  children: ReactNode;
}) {
  const titleId = `widget-${id}-title`;
  return (
    <article
      className="widget"
      data-testid={`widget-${id}`}
      data-widget={id}
      data-size={definition.size}
      data-hidden={hidden ? "true" : undefined}
      aria-labelledby={titleId}
    >
      <header className="widget-head">
        <h2 id={titleId}>{definition.title}</h2>
        {customizing ? (
          <div className="widget-controls">
            <button
              type="button"
              className="icon"
              aria-label={`Move ${definition.title} up`}
              disabled={first}
              onClick={() => onMove(-1)}
            >
              ↑
            </button>
            <button
              type="button"
              className="icon"
              aria-label={`Move ${definition.title} down`}
              disabled={last}
              onClick={() => onMove(1)}
            >
              ↓
            </button>
            <button
              type="button"
              className="link"
              aria-label={`${hidden ? "Show" : "Hide"} ${definition.title}`}
              aria-pressed={hidden}
              onClick={onToggle}
            >
              {hidden ? "Show" : "Hide"}
            </button>
          </div>
        ) : (
          <a className="widget-link" href={href} data-testid="widget-link">
            {definition.linkLabel} <span aria-hidden="true">→</span>
          </a>
        )}
      </header>
      {hidden ? (
        <p className="muted widget-hidden-note">Hidden. It comes back with “Show”.</p>
      ) : (
        <div className="widget-body">{children}</div>
      )}
    </article>
  );
}

/* --- What goes in each card --------------------------------------------- */

type Direction = "up" | "down" | "flat";

function directionOf(value: number): Direction {
  return value > 0 ? "up" : value < 0 ? "down" : "flat";
}

/**
 * An arrow and a phrase, colored by whether the move is welcome: net worth
 * going up is, spending going up is not, so the card says which way is good.
 */
function Trend({
  value,
  goodWhen,
  testId,
  children,
}: {
  value: number;
  goodWhen: "up" | "down";
  testId: string;
  children: ReactNode;
}) {
  const direction = directionOf(value);
  const tone = direction === "flat" ? "flat" : direction === goodWhen ? "good" : "bad";
  return (
    <span className={`trend trend-${tone}`} data-testid={testId} data-direction={direction}>
      {direction === "flat" ? null : (
        <span aria-hidden="true" className="trend-arrow">
          {direction === "up" ? "▲" : "▼"}
        </span>
      )}
      {children}
    </span>
  );
}

function NetWorthCard({
  summary,
  history,
  currency,
}: {
  summary: DashboardSummary;
  history: ReturnType<typeof selectNetWorthHistoryInRange>;
  currency: string;
}) {
  const { netWorth, netWorthChange30d } = summary;
  const points = (history?.points ?? []).map((point) => ({ x: point.date, y: point.netWorth }));
  const first = points[0];
  const last = points[points.length - 1];
  return (
    <>
      <p className="widget-figure hero" data-testid="net-worth-figure">
        {formatMinorUnits(netWorth.netWorth, currency)}
      </p>
      <p className="widget-meta">
        {netWorthChange30d === null ? (
          <span className="trend trend-flat muted" data-testid="net-worth-change">
            No history yet; the nightly snapshot starts the trend.
          </span>
        ) : (
          <Trend value={netWorthChange30d} goodWhen="up" testId="net-worth-change">
            {netWorthChange30d === 0
              ? "Unchanged over 30 days"
              : `${formatMinorUnits(Math.abs(netWorthChange30d), currency)} over 30 days`}
          </Trend>
        )}
      </p>
      {first !== undefined && last !== undefined ? (
        <div className="widget-sparkline">
          <Sparkline
            points={points}
            height={40}
            ariaLabel={`Net worth over the last ${HISTORY_DAYS} days, from ${formatMinorUnits(
              first.y,
              currency,
            )} on ${readoutLabel(first.x)} to ${formatMinorUnits(last.y, currency)} on ${readoutLabel(
              last.x,
            )}`}
          />
        </div>
      ) : null}
      <p className="widget-meta muted tabular">
        assets {formatMinorUnits(netWorth.assets, currency)} · liabilities{" "}
        {formatMinorUnits(netWorth.liabilities, currency)}
      </p>
    </>
  );
}

/** Spending against last month as a percentage; null when last month had none to compare with. */
function percentChange(current: number, previous: number): number | null {
  if (previous <= 0) return null;
  return Math.round(((current - previous) / previous) * 100);
}

function SpendingCard({
  summary,
  month,
  currency,
}: {
  summary: DashboardSummary;
  month: string;
  currency: string;
}) {
  const current = summary.thisMonthSpending;
  const previous = summary.lastMonthSpending;
  const lastMonth = previousMonth(month);
  const change = percentChange(current, previous);
  const scale = Math.max(current, previous, 1);
  return (
    <>
      <p className="widget-figure" data-testid="spending-this-month">
        {formatMinorUnits(current, currency)}
      </p>
      <p className="widget-meta">
        {change === null ? (
          <span className="trend trend-flat muted" data-testid="spending-change">
            Nothing spent last month to compare with.
          </span>
        ) : current === 0 ? (
          // "100% less" is true of a month that has not started spending, and
          // says nothing a person wants to hear on the first of the month.
          <span className="trend trend-flat muted" data-testid="spending-change">
            Nothing spent yet this month.
          </span>
        ) : (
          <Trend value={change} goodWhen="down" testId="spending-change">
            {change === 0
              ? "The same as last month"
              : `${Math.abs(change)}% ${change > 0 ? "more" : "less"} than last month`}
          </Trend>
        )}
      </p>
      <div className="compare">
        <div className="compare-row">
          <span className="compare-label">
            This month <small>{shortMonthLabel(month)}</small>
          </span>
          <ProgressBar value={current} max={scale} label={`Spent in ${monthLabel(month)}`} />
          <span className="compare-value tabular">{formatMinorUnits(current, currency)}</span>
        </div>
        <div className="compare-row previous">
          <span className="compare-label">
            Last month <small>{shortMonthLabel(lastMonth)}</small>
          </span>
          <ProgressBar value={previous} max={scale} label={`Spent in ${monthLabel(lastMonth)}`} />
          <span className="compare-value tabular" data-testid="spending-last-month">
            {formatMinorUnits(previous, currency)}
          </span>
        </div>
      </div>
    </>
  );
}

function BudgetCard({
  summary,
  model,
  mode,
  month,
  currency,
  href,
}: {
  summary: DashboardSummary;
  model: MonthBudgetModel;
  mode: BudgetMode;
  month: string;
  currency: string;
  href: string;
}) {
  // The remaining figure is the summary's; the bar it sits over is the same
  // model's spent and allowance, read the way the budget page reads them in
  // each mode -- the flexible bucket in flex mode, every budget otherwise.
  const remaining = summary.budgetRemaining;
  const spent = mode === "flex" ? model.flex.flexible.spent : model.totals.spent;
  const allowance = mode === "flex" ? model.flex.flexible.allowance : model.totals.budgeted;
  const nothingPlanned = allowance <= 0 && spent === 0;
  return (
    <>
      <p
        className={`widget-figure${remaining < 0 ? " negative" : ""}`}
        data-testid="budget-remaining"
      >
        {formatMinorUnits(remaining, currency)}
      </p>
      <p className="widget-meta muted">
        {remaining < 0 ? "over" : "left"} {mode === "flex" ? "of the flexible budget" : "to spend"}{" "}
        in {monthLabel(month)}
      </p>
      {nothingPlanned ? (
        <p className="widget-empty">
          Nothing budgeted for {monthLabel(month)} yet. <a href={href}>Set one up</a>, or copy last
          month's.
        </p>
      ) : (
        <>
          <Meter
            value={spent}
            max={allowance}
            label={`${formatMinorUnits(spent, currency)} spent of ${formatMinorUnits(allowance, currency)}`}
          />
          <p className="widget-meta muted tabular" data-testid="budget-spent">
            {formatMinorUnits(spent, currency)} spent · {formatMinorUnits(allowance, currency)}{" "}
            {mode === "flex" ? "flexible" : "budgeted"}
          </p>
        </>
      )}
    </>
  );
}

const BILL_STATE_LABELS: Readonly<Record<BillState, string>> = {
  upcoming: "Upcoming",
  due: "Due",
  paid: "Paid",
  late: "Late",
};

/** "due in 3 days", "due today", "2 days late": how far a bill is from its date, in words. */
function dueText(bill: ResolvedBill<Recurrence>): string {
  const days = bill.daysAway;
  const plural = (count: number): string => (count === 1 ? "day" : "days");
  if (bill.state === "late") return `${-days} ${plural(-days)} late`;
  if (days === 0) return "due today";
  if (days < 0) return `due ${-days} ${plural(-days)} ago`;
  if (days === 1) return "due tomorrow";
  return `due in ${days} days`;
}

function BillsCard({ bills }: { bills: readonly ResolvedBill<Recurrence>[] }) {
  if (bills.length === 0) {
    return <p className="widget-empty">No bills coming up. Confirmed recurrences show here.</p>;
  }
  return (
    <ul className="widget-list" aria-label="Next bills">
      {bills.map((bill) => (
        <li
          key={bill.recurrence.id}
          data-testid={`bill-${bill.recurrence.id}`}
          data-state={bill.state}
        >
          <span className="row-title">{recurrenceName(bill.recurrence)}</span>
          <span className="row-amount tabular">
            {formatMinorUnits(bill.recurrence.expected_amount, bill.recurrence.currency)}
          </span>
          <span className="row-meta">
            <time dateTime={bill.dueDate}>{axisLabel(bill.dueDate)}</time> · {dueText(bill)}
          </span>
          <span className={`chip state-${bill.state}`}>{BILL_STATE_LABELS[bill.state]}</span>
        </li>
      ))}
    </ul>
  );
}

function GoalsCard({ goals, href }: { goals: readonly GoalRow[]; href: string }) {
  if (goals.length === 0) {
    return (
      <p className="widget-empty">
        No goals yet. <a href={href}>Start one</a> to save up or pay something down.
      </p>
    );
  }
  return (
    <ul className="widget-list" aria-label="Top goals">
      {goals.map(({ goal, math }) => (
        <li key={goal.id} data-testid={`goal-${goal.id}`} data-percent={math.percent}>
          <span className="row-title">{goal.name}</span>
          <span className="row-amount tabular">{math.percent}%</span>
          <span className="row-bar">
            <ProgressBar
              value={math.percent}
              max={100}
              tone={math.percent >= 100 ? "positive" : "accent"}
              label={`${goal.name}: ${math.percent}% of ${formatMinorUnits(goal.target_amount, goal.currency)}`}
            />
          </span>
          <span className="row-meta">
            {goal.kind === "pay_down"
              ? `${formatMinorUnits(math.saved, goal.currency)} paid down · ${formatMinorUnits(math.remaining, goal.currency)} owed`
              : `${formatMinorUnits(math.saved, goal.currency)} of ${formatMinorUnits(goal.target_amount, goal.currency)} saved`}
          </span>
          {math.onTrack === null ? null : (
            <span className={`chip ${math.onTrack ? "state-upcoming" : "state-late"}`}>
              {math.onTrack ? "on track" : "behind"}
            </span>
          )}
        </li>
      ))}
    </ul>
  );
}

/** One big number and what it counts. */
function StatCard({
  figure,
  figureTestId,
  caption,
  note,
}: {
  figure: string;
  figureTestId: string;
  caption: string;
  note: string;
}) {
  return (
    <>
      <p className="widget-figure" data-testid={figureTestId}>
        {figure}
      </p>
      <p className="widget-meta">{caption}</p>
      <p className="widget-meta muted">{note}</p>
    </>
  );
}

function RecentCard({
  transactions,
  resolveMerchant,
  categories,
}: {
  transactions: readonly Transaction[];
  resolveMerchant: MerchantResolver;
  categories: ReadonlyMap<string, Category>;
}) {
  if (transactions.length === 0) {
    return <p className="widget-empty">No transactions yet.</p>;
  }
  const categoryLabel = (transaction: Transaction): string => {
    if (transaction.splits.length > 0) return "Split";
    const category = categories.get(transaction.category_id ?? "");
    if (category === undefined) return "Uncategorized";
    return category.icon === undefined ? category.name : `${category.icon} ${category.name}`;
  };
  return (
    <table className="widget-table" aria-label="Recent transactions">
      <tbody>
        {transactions.map((transaction) => (
          <tr key={transaction.id} data-testid={`recent-${transaction.id}`}>
            <td className="date">
              <time dateTime={transaction.date}>{axisLabel(transaction.date)}</time>
            </td>
            <td className="merchant">
              <span data-testid="merchant">{resolveMerchant(transaction).name}</span>
              {needsReview(transaction) ? <span className="chip state-due">review</span> : null}
            </td>
            <td className="category muted">{categoryLabel(transaction)}</td>
            <td className="amount tabular" data-testid="amount">
              {formatMinorUnits(transaction.amount, transaction.currency)}
            </td>
          </tr>
        ))}
      </tbody>
    </table>
  );
}
