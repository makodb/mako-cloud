import {
  AreaChart,
  Badge,
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  cn,
  Progress,
  Sheet,
  SheetBody,
  SheetClose,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
  Table,
  TableBody,
  TableCell,
  TableRow,
} from "@mako-cloud/ui";
import {
  ArrowDown,
  ArrowRight,
  ArrowUp,
  ChevronDown,
  ChevronUp,
  SlidersHorizontal,
} from "lucide-react";
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
import { formatMinorUnits, minorUnitDigits } from "../selectors/money.js";
import {
  addDays,
  type BillState,
  type ResolvedBill,
  recurrenceName,
} from "../selectors/recurrences.js";
import { needsReview } from "../selectors/review.js";
import { monthKey } from "../selectors/transactions.js";
import { useQuery } from "./hooks.js";
import { routeHash, transactionsHash } from "./router.js";

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

/**
 * The grid: one column on a phone, two, three, then four as the viewport
 * widens; the hero spans two of them and the list spans a row's worth. Dense
 * packing lets a one-column card fill the hole a wide card leaves at the end
 * of a row, so a customized order never shows a gap.
 */
const WIDGET_SPAN: Readonly<Record<WidgetSize, string>> = {
  single: "",
  wide: "sm:col-span-2",
  full: "sm:col-span-2 lg:col-span-3",
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

/* --- Dates and amounts, in words ------------------------------------------ */

const SHORT_MONTHS = [
  "Jan",
  "Feb",
  "Mar",
  "Apr",
  "May",
  "Jun",
  "Jul",
  "Aug",
  "Sep",
  "Oct",
  "Nov",
  "Dec",
] as const;

const MONTH_KEY = /^(\d{4})-(0[1-9]|1[0-2])$/u;
const DATE_KEY = /^(\d{4})-(0[1-9]|1[0-2])-(0[1-9]|[12]\d|3[01])$/u;

/** `2026-09-07` as `Sep 7`; anything that is not a date as it came. */
function shortDateLabel(key: string): string {
  const match = DATE_KEY.exec(key);
  if (match === null) return key;
  const [, , month = "01", day = "01"] = match;
  return `${SHORT_MONTHS[Number(month) - 1] ?? month} ${Number(day)}`;
}

/** `2026-09-07` as `Sep 7, 2026`, for a readout rather than an axis. */
function longDateLabel(key: string): string {
  const match = DATE_KEY.exec(key);
  if (match === null) return key;
  const [, year = "", month = "01", day = "01"] = match;
  return `${SHORT_MONTHS[Number(month) - 1] ?? month} ${Number(day)}, ${year}`;
}

/** `2026-09` as `Sep`. */
function shortMonthName(key: string): string {
  const match = MONTH_KEY.exec(key);
  if (match === null) return key;
  const [, , month = "01"] = match;
  return SHORT_MONTHS[Number(month) - 1] ?? month;
}

/**
 * How the trend writes an amount on its axis and in its readout: to the whole
 * unit while every value in the series fits a tick, and compact (`$1.2M`)
 * once one would not. One chart keeps one notation, so its ticks agree.
 */
function trendMoneyFormatter(
  values: readonly number[],
  currency: string,
): (minor: number) => string {
  const scale = 10 ** minorUnitDigits(currency);
  const widest = values.reduce((max, value) => Math.max(max, Math.abs(value)), 0) / scale;
  const options: Intl.NumberFormatOptions =
    widest < 100_000
      ? { style: "currency", currency, minimumFractionDigits: 0, maximumFractionDigits: 0 }
      : { style: "currency", currency, notation: "compact", maximumFractionDigits: 1 };
  let formatter: Intl.NumberFormat | null;
  try {
    formatter = new Intl.NumberFormat("en-US", options);
  } catch {
    formatter = null;
  }
  return (minor) =>
    formatter === null
      ? `${Math.round(minor / scale)} ${currency}`
      : formatter.format(minor / scale);
}

/** How full a bar is, as the percentage the kit's `Progress` takes. */
function percentOf(value: number, max: number): number {
  if (!(max > 0) || !Number.isFinite(value)) return 0;
  return Math.min(100, Math.max(0, (value / max) * 100));
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
    <section
      aria-labelledby="dashboard-title"
      data-testid="dashboard-screen"
      className="grid gap-6"
    >
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div className="grid gap-1">
          <h1 id="dashboard-title" className="text-2xl">
            Dashboard
          </h1>
          <p className="text-sm text-muted-foreground">
            {household?.name ?? "Your space"} · {longDateLabel(today)}
          </p>
        </div>
        <Button
          variant="secondary"
          aria-pressed={customizing}
          data-testid="customize-toggle"
          onClick={() => setCustomizing(true)}
        >
          <SlidersHorizontal aria-hidden="true" />
          Customize
        </Button>
      </div>
      {/* The `dashboard-grid` and `widget` class names carry no styling; the
          browser suite addresses the cards by them. */}
      <div
        className="dashboard-grid grid grid-flow-dense grid-cols-1 gap-4 sm:grid-cols-2 lg:grid-cols-3 xl:grid-cols-4"
        data-testid="dashboard-grid"
      >
        {shown.map((id) => (
          <Widget
            key={id}
            id={id}
            definition={WIDGETS[id]}
            href={destinationOf(id, thisMonth)}
            hidden={layout.hidden.includes(id)}
          >
            {body(id)}
          </Widget>
        ))}
      </div>
      <Sheet open={customizing} onOpenChange={setCustomizing}>
        <SheetContent>
          <SheetHeader>
            <SheetTitle>Customize the dashboard</SheetTitle>
            <SheetDescription role="status">
              Hide the cards you do not need and move the rest. The layout is remembered on this
              device only.
            </SheetDescription>
          </SheetHeader>
          <SheetBody>
            <ol className="m-0 grid list-none gap-1 p-0" aria-label="Cards">
              {layout.order.map((id, index) => {
                const definition = WIDGETS[id];
                const hidden = layout.hidden.includes(id);
                return (
                  <li
                    key={id}
                    className="flex items-center gap-1 rounded-md py-1 pl-2 text-sm"
                    data-widget={id}
                    data-hidden={hidden ? "true" : undefined}
                  >
                    <span
                      className={cn(
                        "min-w-0 flex-1 truncate font-medium",
                        hidden && "text-muted-foreground",
                      )}
                    >
                      {definition.title}
                    </span>
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      aria-label={`Move ${definition.title} up`}
                      disabled={index === 0}
                      onClick={() => move(id, -1)}
                    >
                      <ChevronUp />
                    </Button>
                    <Button
                      variant="ghost"
                      size="icon-sm"
                      aria-label={`Move ${definition.title} down`}
                      disabled={index === layout.order.length - 1}
                      onClick={() => move(id, 1)}
                    >
                      <ChevronDown />
                    </Button>
                    <Button
                      variant="ghost"
                      size="sm"
                      className="w-14"
                      aria-label={`${hidden ? "Show" : "Hide"} ${definition.title}`}
                      aria-pressed={hidden}
                      onClick={() => toggle(id)}
                    >
                      {hidden ? "Show" : "Hide"}
                    </Button>
                  </li>
                );
              })}
            </ol>
          </SheetBody>
          <SheetFooter className="flex-row items-center justify-between">
            <Button variant="ghost" onClick={reset}>
              Reset layout
            </Button>
            <SheetClose asChild>
              <Button>Done</Button>
            </SheetClose>
          </SheetFooter>
        </SheetContent>
      </Sheet>
    </section>
  );
}

/* --- The card ------------------------------------------------------------ */

function Widget({
  id,
  definition,
  href,
  hidden,
  children,
}: {
  id: WidgetId;
  definition: WidgetDefinition;
  href: string;
  hidden: boolean;
  children: ReactNode;
}) {
  const titleId = `widget-${id}-title`;
  return (
    <Card
      className={cn(
        "widget gap-3 py-4",
        WIDGET_SPAN[definition.size],
        // While customizing, a hidden card stays on the grid, dimmed and
        // dashed, so the person can see what "Show" would bring back.
        hidden && "border-dashed opacity-60 shadow-none",
      )}
      data-testid={`widget-${id}`}
      data-widget={id}
      data-size={definition.size}
      data-hidden={hidden ? "true" : undefined}
      aria-labelledby={titleId}
    >
      <CardHeader className="flex flex-row items-baseline justify-between gap-3">
        <CardTitle
          id={titleId}
          className="text-xs font-semibold uppercase tracking-wider text-muted-foreground whitespace-nowrap"
        >
          {definition.title}
        </CardTitle>
        <a
          className="inline-flex items-center gap-1 text-sm font-medium whitespace-nowrap text-primary"
          href={href}
          data-testid="widget-link"
        >
          {definition.linkLabel} <ArrowRight aria-hidden="true" className="size-3.5" />
        </a>
      </CardHeader>
      <CardContent className="flex min-w-0 flex-col gap-2.5">
        {hidden ? (
          <p className="text-sm text-muted-foreground">Hidden. It comes back with “Show”.</p>
        ) : (
          children
        )}
      </CardContent>
    </Card>
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
  const Arrow = direction === "up" ? ArrowUp : ArrowDown;
  return (
    <span
      className={cn(
        "inline-flex items-center gap-1 text-sm font-medium tabular-nums",
        tone === "good" && "text-positive",
        tone === "bad" && "text-destructive",
        tone === "flat" && "text-muted-foreground",
      )}
      data-testid={testId}
      data-direction={direction}
    >
      {direction === "flat" ? null : <Arrow aria-hidden="true" className="size-3.5" />}
      {children}
    </span>
  );
}

/** A card's big number: tabular, and left-aligned like the label above it. */
function Figure({
  children,
  testId,
  hero = false,
  negative = false,
}: {
  children: ReactNode;
  testId: string;
  hero?: boolean;
  negative?: boolean;
}) {
  return (
    <p
      className={cn(
        "money text-left font-semibold tracking-tight",
        hero ? "text-4xl" : "text-2xl",
        negative && "text-destructive",
      )}
      data-testid={testId}
    >
      {children}
    </p>
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
  const points = (history?.points ?? []).map((point) => ({
    date: point.date,
    netWorth: point.netWorth,
  }));
  const first = points[0];
  const last = points[points.length - 1];
  const formatTick = trendMoneyFormatter(
    points.map((point) => point.netWorth),
    currency,
  );
  return (
    <>
      <Figure testId="net-worth-figure" hero>
        {formatMinorUnits(netWorth.netWorth, currency)}
      </Figure>
      <p className="text-sm">
        {netWorthChange30d === null ? (
          <span className="text-muted-foreground" data-testid="net-worth-change">
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
        <AreaChart
          data={points}
          x="date"
          series={[{ key: "netWorth", label: "Net worth" }]}
          title={`Net worth over the last ${HISTORY_DAYS} days, from ${formatMinorUnits(
            first.netWorth,
            currency,
          )} on ${longDateLabel(first.date)} to ${formatMinorUnits(last.netWorth, currency)} on ${longDateLabel(
            last.date,
          )}`}
          height={150}
          formatValue={formatTick}
          formatX={(value) => shortDateLabel(String(value))}
        />
      ) : null}
      <p className="text-sm text-muted-foreground tabular-nums">
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
      <Figure testId="spending-this-month">{formatMinorUnits(current, currency)}</Figure>
      <p className="text-sm">
        {change === null ? (
          <span className="text-muted-foreground" data-testid="spending-change">
            Nothing spent last month to compare with.
          </span>
        ) : current === 0 ? (
          // "100% less" is true of a month that has not started spending, and
          // says nothing a person wants to hear on the first of the month.
          <span className="text-muted-foreground" data-testid="spending-change">
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
      {/* This month against last, two bars on one scale. */}
      <div className="grid gap-3 text-sm">
        <div className="grid gap-1.5">
          <div className="flex items-baseline justify-between gap-3">
            <span className="whitespace-nowrap text-muted-foreground">
              This month <small className="text-xs">{shortMonthName(month)}</small>
            </span>
            <span className="money">{formatMinorUnits(current, currency)}</span>
          </div>
          <Progress
            value={percentOf(current, scale)}
            aria-label={`Spent in ${monthLabel(month)}`}
          />
        </div>
        <div className="grid gap-1.5">
          <div className="flex items-baseline justify-between gap-3">
            <span className="whitespace-nowrap text-muted-foreground">
              Last month <small className="text-xs">{shortMonthName(lastMonth)}</small>
            </span>
            <span className="money" data-testid="spending-last-month">
              {formatMinorUnits(previous, currency)}
            </span>
          </div>
          <Progress
            value={percentOf(previous, scale)}
            aria-label={`Spent in ${monthLabel(lastMonth)}`}
            className="[&_[data-slot=progress-indicator]]:opacity-40"
          />
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
  // A limit's bar is calm under it, a warning near it, and a problem past it.
  const ratio = allowance > 0 ? spent / allowance : spent > 0 ? Number.POSITIVE_INFINITY : 0;
  const tone = ratio > 1 ? "destructive" : ratio >= 0.85 ? "warning" : "primary";
  return (
    <>
      <Figure testId="budget-remaining" negative={remaining < 0}>
        {formatMinorUnits(remaining, currency)}
      </Figure>
      <p className="text-sm text-muted-foreground">
        {remaining < 0 ? "over" : "left"} {mode === "flex" ? "of the flexible budget" : "to spend"}{" "}
        in {monthLabel(month)}
      </p>
      {nothingPlanned ? (
        <p className="text-sm text-muted-foreground">
          Nothing budgeted for {monthLabel(month)} yet.{" "}
          <a className="text-primary" href={href}>
            Set one up
          </a>
          , or copy last month's.
        </p>
      ) : (
        <>
          <Progress
            value={allowance > 0 ? percentOf(spent, allowance) : 100}
            tone={tone}
            aria-label={`${formatMinorUnits(spent, currency)} spent of ${formatMinorUnits(allowance, currency)}`}
          />
          <p className="text-sm text-muted-foreground tabular-nums" data-testid="budget-spent">
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

/** A bill's state wears a tone: late is a problem, due is a nudge, upcoming is just information. */
const BILL_STATE_VARIANTS: Readonly<
  Record<BillState, "secondary" | "warning" | "positive" | "destructive">
> = {
  upcoming: "secondary",
  due: "warning",
  paid: "positive",
  late: "destructive",
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

/**
 * A row of a card's list: what and how much on the first line, when and its
 * state on the second; a goal's bar takes a line of its own between them.
 */
const LIST_ROW =
  "grid grid-cols-[minmax(0,1fr)_auto] items-baseline gap-x-3 gap-y-1 border-t pt-3 text-sm first:border-t-0 first:pt-0";

function BillsCard({ bills }: { bills: readonly ResolvedBill<Recurrence>[] }) {
  if (bills.length === 0) {
    return (
      <p className="text-sm text-muted-foreground">
        No bills coming up. Confirmed recurrences show here.
      </p>
    );
  }
  return (
    <ul className="m-0 grid list-none gap-3 p-0" aria-label="Next bills">
      {bills.map((bill) => (
        <li
          key={bill.recurrence.id}
          className={LIST_ROW}
          data-testid={`bill-${bill.recurrence.id}`}
          data-state={bill.state}
        >
          <span className="truncate font-medium">{recurrenceName(bill.recurrence)}</span>
          <span className="money">
            {formatMinorUnits(bill.recurrence.expected_amount, bill.recurrence.currency)}
          </span>
          <span className="text-xs text-muted-foreground">
            <time dateTime={bill.dueDate}>{shortDateLabel(bill.dueDate)}</time> · {dueText(bill)}
          </span>
          <Badge variant={BILL_STATE_VARIANTS[bill.state]} className="self-center justify-self-end">
            {BILL_STATE_LABELS[bill.state]}
          </Badge>
        </li>
      ))}
    </ul>
  );
}

function GoalsCard({ goals, href }: { goals: readonly GoalRow[]; href: string }) {
  if (goals.length === 0) {
    return (
      <p className="text-sm text-muted-foreground">
        No goals yet.{" "}
        <a className="text-primary" href={href}>
          Start one
        </a>{" "}
        to save up or pay something down.
      </p>
    );
  }
  return (
    <ul className="m-0 grid list-none gap-3 p-0" aria-label="Top goals">
      {goals.map(({ goal, math }) => (
        <li
          key={goal.id}
          className={LIST_ROW}
          data-testid={`goal-${goal.id}`}
          data-percent={math.percent}
        >
          <span className="truncate font-medium">{goal.name}</span>
          <span className="money">{math.percent}%</span>
          <Progress
            className="col-span-2 my-0.5"
            value={math.percent}
            tone={math.percent >= 100 ? "positive" : "primary"}
            aria-label={`${goal.name}: ${math.percent}% of ${formatMinorUnits(goal.target_amount, goal.currency)}`}
          />
          <span
            className={cn("text-xs text-muted-foreground", math.onTrack === null && "col-span-2")}
          >
            {goal.kind === "pay_down"
              ? `${formatMinorUnits(math.saved, goal.currency)} paid down · ${formatMinorUnits(math.remaining, goal.currency)} owed`
              : `${formatMinorUnits(math.saved, goal.currency)} of ${formatMinorUnits(goal.target_amount, goal.currency)} saved`}
          </span>
          {math.onTrack === null ? null : (
            <Badge
              variant={math.onTrack ? "positive" : "destructive"}
              className="self-center justify-self-end"
            >
              {math.onTrack ? "on track" : "behind"}
            </Badge>
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
      <Figure testId={figureTestId}>{figure}</Figure>
      <p className="text-sm">{caption}</p>
      <p className="text-sm text-muted-foreground">{note}</p>
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
    return <p className="text-sm text-muted-foreground">No transactions yet.</p>;
  }
  const categoryLabel = (transaction: Transaction): string => {
    if (transaction.splits.length > 0) return "Split";
    const category = categories.get(transaction.category_id ?? "");
    if (category === undefined) return "Uncategorized";
    return category.icon === undefined ? category.name : `${category.icon} ${category.name}`;
  };
  // The date, category, and amount columns take their own measure and the
  // merchant column is what is left -- so a long statement line ellipsizes
  // there instead of squeezing the others, however narrow the card gets.
  return (
    <Table aria-label="Recent transactions">
      <TableBody>
        {transactions.map((transaction) => (
          <TableRow key={transaction.id} data-testid={`recent-${transaction.id}`}>
            <TableCell className="w-px pl-0 text-muted-foreground tabular-nums">
              <time dateTime={transaction.date}>{shortDateLabel(transaction.date)}</time>
            </TableCell>
            <TableCell className="w-full max-w-0 truncate font-medium">
              <span data-testid="merchant">{resolveMerchant(transaction).name}</span>
              {needsReview(transaction) ? (
                <Badge variant="warning" className="ml-2">
                  review
                </Badge>
              ) : null}
            </TableCell>
            <TableCell className="hidden w-px max-w-48 truncate text-muted-foreground sm:table-cell">
              {categoryLabel(transaction)}
            </TableCell>
            <TableCell className="money w-px pr-0" data-testid="amount">
              {formatMinorUnits(transaction.amount, transaction.currency)}
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}
