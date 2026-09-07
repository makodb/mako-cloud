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
  cn,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  EmptyState,
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
import { CalendarDays, List, type LucideIcon, Pause, Pencil, Play, Repeat, X } from "lucide-react";
import { type FormEvent, type ReactNode, useEffect, useRef, useState } from "react";

import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import type {
  HouseholdCollectionId,
  Recurrence,
  RecurrenceInterval,
  TaxonomyEntry,
  Transaction,
} from "../model/types.js";
import { calendarTotals, monthlyTotal, selectBillsByDate } from "../selectors/calendar.js";
import { addMonths } from "../selectors/goals.js";
import { amountToText, formatMinorUnits, parseAmount } from "../selectors/money.js";
import {
  advanceAfterPayment,
  type DetectedRecurrence,
  type ResolvedBill,
  recurrenceName,
  selectBills,
  selectDetectedRecurrences,
  storedDetection,
} from "../selectors/recurrences.js";
import { MonthCalendar } from "./charts/calendar.js";
import { readoutLabel } from "./charts/layout.js";
import { useQuery } from "./hooks.js";
import { type Route, routeHash } from "./router.js";

/**
 * What repeats: the charges Rational has noticed, the bills that are coming,
 * and every recurrence the household has confirmed -- as a list, or laid out
 * over a month.
 *
 * A detected recurrence is a suggestion. Rational writes nothing about one
 * until the person confirms or dismisses it, because a wrong guess that
 * quietly becomes an upcoming bill is worse than no guess at all. A confirmed
 * one lives on: paid when its transaction lands, late when the date passes
 * with nothing, paused when the household says so.
 */
const INTERVALS: readonly RecurrenceInterval[] = [
  "weekly",
  "biweekly",
  "monthly",
  "quarterly",
  "yearly",
];

/** Bills further off than this are the calendar's business rather than the list's. */
const UPCOMING_WITHIN_DAYS = 45;

/** How long a row found from the calendar stays marked, so the eye lands on it. */
const HIGHLIGHT_MS = 4_000;

const MONTH_KEY = /^\d{4}-(0[1-9]|1[0-2])$/u;

/** The small heading over a figure or a table column. */
const EYEBROW = "text-xs font-semibold uppercase tracking-wider text-muted-foreground";

interface Suggestion {
  readonly detection: DetectedRecurrence;
  /** The nightly job's own document, when it was the one that noticed. */
  readonly storedId: string | null;
}

export function RecurringScreen({
  app,
  session,
  route,
  currency,
}: {
  app: RationalApp;
  session: ScopeSession<HouseholdCollectionId>;
  route: Extract<Route, { name: "recurring" }>;
  currency: string;
}) {
  const transactions = useQuery(session.collection("transactions")?.find() ?? null);
  const recurrences = useQuery(session.collection("recurrences")?.find() ?? null);
  const accounts = useQuery(
    session.collection("accounts")?.find({ sort: [{ name: "asc" }] }) ?? null,
  );
  const categories = useQuery(
    session
      .collection("taxonomy")
      ?.find({ selector: { kind: "category" }, sort: [{ name: "asc" }] }) ?? null,
  );
  const merchants = useQuery(
    session
      .collection("taxonomy")
      ?.find({ selector: { kind: "merchant" }, sort: [{ name: "asc" }] }) ?? null,
  );
  const [problem, setProblem] = useState<string | null>(null);
  const [editing, setEditing] = useState<string | null>(null);
  const [highlighted, setHighlighted] = useState<string | null>(null);
  const today = new Date().toISOString().slice(0, 10);
  const thisMonth = today.slice(0, 7);
  // The address decides the month and the view, so a link to the calendar of
  // a month is a link to exactly that; anything malformed falls back to now.
  const month = route.month !== undefined && MONTH_KEY.test(route.month) ? route.month : thisMonth;
  const view = route.view ?? "list";
  const role = app.roleIn(app.state.currentHouseholdId);
  const canWrite = role === "owner" || role === "editor";

  // The nightly job notices repeating charges too, and writes down what it
  // finds. Its detections come first: they were noticed before this device
  // opened the app, and a household that has not visited in a month should
  // find them waiting rather than have them re-derived only while it looks.
  const suggestions: readonly Suggestion[] = [
    ...recurrences
      .filter((recurrence) => recurrence.status === "detected")
      .map((recurrence) => ({ detection: storedDetection(recurrence), storedId: recurrence.id })),
    ...selectDetectedRecurrences(transactions, recurrences).map((detection) => ({
      detection,
      storedId: null,
    })),
  ];
  const bills = selectBills(recurrences, transactions, today);
  const upcoming = bills.filter(
    (bill) => bill.state !== "upcoming" || bill.daysAway <= UPCOMING_WITHIN_DAYS,
  );
  const perMonth = monthlyTotal(recurrences);
  const confirmedOrPaused = recurrences
    .filter((recurrence) => recurrence.status === "confirmed" || recurrence.status === "paused")
    .sort(
      (left, right) =>
        left.next_date.localeCompare(right.next_date) || left.id.localeCompare(right.id),
    );
  const byDate = selectBillsByDate(recurrences, month);
  const monthTotals = calendarTotals(byDate);
  const editingRecurrence =
    editing === null
      ? null
      : (confirmedOrPaused.find((recurrence) => recurrence.id === editing) ?? null);

  // A bill whose transaction has landed is advanced here as the nightly job
  // would advance it: the engine decided the payment, this device records it
  // and the next date moves one interval on from the payment. Only a member
  // who may write does so -- a viewer's write would only be refused -- and
  // each payment is applied once, however many times the lists re-render
  // before the recurrence comes back changed.
  const advanced = useRef(new Set<string>());
  useEffect(() => {
    const writes = app.writes;
    if (writes === null || !canWrite) return;
    for (const bill of bills) {
      if (bill.state !== "paid" || bill.paidBy === undefined) continue;
      const key = `${bill.recurrence.id}:${bill.paidBy}`;
      if (advanced.current.has(key)) continue;
      const transaction = transactions.find((candidate) => candidate.id === bill.paidBy);
      if (transaction === undefined) continue;
      advanced.current.add(key);
      void writes
        .recordBillPaid(
          bill.recurrence.id,
          transaction,
          advanceAfterPayment(bill.recurrence, transaction, bill.recurrence.interval),
        )
        .catch(() => undefined);
    }
  }, [app, bills, canWrite, transactions]);

  // A bill clicked on the calendar is shown in the list, marked, and scrolled to.
  useEffect(() => {
    if (highlighted === null || view !== "list") return undefined;
    document
      .querySelector(`[data-testid="recurrence-${highlighted}"]`)
      ?.scrollIntoView({ block: "center" });
    const timer = window.setTimeout(() => setHighlighted(null), HIGHLIGHT_MS);
    return () => window.clearTimeout(timer);
  }, [highlighted, view]);

  const hashFor = (nextView: "list" | "calendar", nextMonth = month): string =>
    routeHash({
      name: "recurring",
      ...(nextMonth === thisMonth ? {} : { month: nextMonth }),
      ...(nextView === "calendar" ? { view: "calendar" as const } : {}),
    });

  const reveal = (recurrenceId: string) => {
    setHighlighted(recurrenceId);
    window.location.hash = hashFor("list");
  };

  const accountName = (id: string) => accounts.find((account) => account.id === id)?.name ?? id;
  const categoryName = (id: string | undefined) =>
    id === undefined ? "—" : (categories.find((category) => category.id === id)?.name ?? id);

  const attempt = async (work: () => Promise<unknown>, fallback: string) => {
    try {
      await work();
      setProblem(null);
    } catch (error) {
      setProblem(error instanceof Error ? error.message : fallback);
    }
  };

  const decide = (suggestion: Suggestion, status: "confirmed" | "dismissed") =>
    attempt(async () => {
      const { detection, storedId } = suggestion;
      // The job's own document is settled rather than a second one written.
      if (storedId !== null) {
        await app.writes?.updateRecurrence(storedId, { status });
        return;
      }
      await app.writes?.saveRecurrence({
        account_id: detection.accountId,
        normalized_description: detection.normalizedDescription,
        interval: detection.interval,
        expected_amount: detection.expectedAmount,
        currency: detection.currency,
        next_date: detection.nextDate,
        last_date: detection.lastDate,
        status,
        matched_count: detection.occurrences,
        source: "detected",
      });
    }, "the recurrence could not be saved");

  return (
    <section
      aria-labelledby="recurring-title"
      data-testid="recurring-screen"
      className="grid gap-6"
    >
      <header className="flex flex-wrap items-start justify-between gap-4">
        <div className="grid gap-1">
          <h1 id="recurring-title" className="text-2xl">
            Recurring
          </h1>
          <p className="m-0 text-sm text-muted-foreground">
            The charges that repeat, the bills that are coming, and what the household has confirmed
            — as a list, or laid over a month.
          </p>
        </div>
        {/* Two views of one thing, side by side like a segmented control. The
            address carries which one is showing, so these are links rather
            than tabs: a link to the calendar of a month is a link to exactly
            that. */}
        <nav
          aria-label="View"
          className="inline-flex h-9 items-center rounded-lg bg-muted p-[3px] text-muted-foreground"
        >
          <ViewLink href={hashFor("list")} active={view === "list"} icon={List}>
            List
          </ViewLink>
          <ViewLink href={hashFor("calendar")} active={view === "calendar"} icon={CalendarDays}>
            Calendar
          </ViewLink>
        </nav>
      </header>
      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        {(perMonth.length === 0 ? [{ currency, total: 0 }] : perMonth).map((total) => (
          <Card key={total.currency} className="gap-1 py-4">
            <CardContent className="grid gap-1 px-4">
              <span className={EYEBROW}>Recurring per month ({total.currency})</span>
              <strong
                className="money justify-self-start text-2xl font-semibold"
                data-testid={`monthly-total-${total.currency}`}
              >
                {formatMinorUnits(total.total, total.currency)}
              </strong>
              <small className="text-xs text-muted-foreground">
                every confirmed recurrence, at its monthly equivalent
              </small>
            </CardContent>
          </Card>
        ))}
      </div>
      {problem === null ? null : (
        <Alert variant="destructive" role="alert">
          <AlertDescription>{problem}</AlertDescription>
        </Alert>
      )}

      {view === "calendar" ? (
        <CalendarView
          month={month}
          today={today}
          byDate={byDate}
          totals={monthTotals}
          currency={currency}
          onMonth={(next) => {
            window.location.hash = hashFor("calendar", next);
          }}
          onReveal={reveal}
        />
      ) : (
        <>
          <Card>
            <CardHeader>
              <CardTitle>Suggested</CardTitle>
              <CardDescription>
                Charges that look like they repeat. Nothing is written down until you decide.
              </CardDescription>
            </CardHeader>
            <CardContent>
              {suggestions.length === 0 ? (
                <p className="m-0 text-sm text-muted-foreground">
                  Nothing looks like it repeats yet.
                </p>
              ) : (
                <Table aria-label="Suggested recurring charges">
                  <TableHeader>
                    <TableRow>
                      <TableHead scope="col">Description</TableHead>
                      <TableHead scope="col">Account</TableHead>
                      <TableHead scope="col">Every</TableHead>
                      <TableHead scope="col" className="text-right">
                        Usually
                      </TableHead>
                      <TableHead scope="col">Next</TableHead>
                      <TableHead scope="col">
                        <span className="sr-only">Actions</span>
                      </TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {suggestions.map((suggestion) => {
                      const { detection, storedId } = suggestion;
                      return (
                        <TableRow
                          key={
                            storedId ?? `${detection.accountId}:${detection.normalizedDescription}`
                          }
                          data-testid={`detected-${detection.normalizedDescription.replaceAll(" ", "-")}`}
                          data-noticed-by={storedId === null ? "this device" : "the nightly job"}
                        >
                          <TableHead
                            scope="row"
                            className="h-auto py-2 font-medium whitespace-normal text-current"
                          >
                            {detection.description}
                            <small className="block text-xs font-normal text-muted-foreground">
                              {storedId === null
                                ? `${detection.occurrences} times on this device`
                                : "noticed by the nightly job"}
                            </small>
                          </TableHead>
                          <TableCell>{accountName(detection.accountId)}</TableCell>
                          <TableCell data-testid="interval">{detection.interval}</TableCell>
                          <TableCell className="money">
                            {formatMinorUnits(detection.expectedAmount, detection.currency)}
                          </TableCell>
                          <TableCell data-testid="next">{detection.nextDate}</TableCell>
                          <TableCell>
                            <div className="flex justify-end gap-1">
                              <Button
                                variant="outline"
                                size="sm"
                                onClick={() => void decide(suggestion, "confirmed")}
                              >
                                Confirm
                              </Button>
                              <Button
                                variant="ghost"
                                size="sm"
                                onClick={() => void decide(suggestion, "dismissed")}
                              >
                                Dismiss
                              </Button>
                            </div>
                          </TableCell>
                        </TableRow>
                      );
                    })}
                  </TableBody>
                </Table>
              )}
            </CardContent>
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>Upcoming</CardTitle>
              <CardDescription>
                Confirmed bills due in the next six weeks, and how each one stands.
              </CardDescription>
            </CardHeader>
            {upcoming.length === 0 ? (
              <CardContent>
                <p className="m-0 text-sm text-muted-foreground">
                  Nothing due in the next six weeks.
                </p>
              </CardContent>
            ) : (
              <ul className="m-0 list-none divide-y border-t p-0" aria-label="Upcoming bills">
                {upcoming.map((bill) => (
                  <BillRow key={bill.recurrence.id} bill={bill} transactions={transactions} />
                ))}
              </ul>
            )}
          </Card>

          <Card>
            <CardHeader>
              <CardTitle>Confirmed and paused</CardTitle>
              <CardDescription>
                Every recurrence the household keeps. A paused one is neither due nor counted.
              </CardDescription>
            </CardHeader>
            <CardContent>
              {confirmedOrPaused.length === 0 ? (
                <EmptyState
                  icon={<Repeat />}
                  title="Nothing recurring yet."
                  description="Confirm a suggestion above, or mark a transaction as recurring from the transactions screen."
                />
              ) : (
                <Table aria-label="All recurring">
                  <TableHeader>
                    <TableRow>
                      <TableHead scope="col">Name</TableHead>
                      <TableHead scope="col">Account</TableHead>
                      <TableHead scope="col">Every</TableHead>
                      <TableHead scope="col" className="text-right">
                        Amount
                      </TableHead>
                      <TableHead scope="col">Next</TableHead>
                      <TableHead scope="col">Category</TableHead>
                      <TableHead scope="col">
                        <span className="sr-only">Actions</span>
                      </TableHead>
                    </TableRow>
                  </TableHeader>
                  <TableBody>
                    {confirmedOrPaused.map((recurrence) => (
                      <TableRow
                        key={recurrence.id}
                        data-testid={`recurrence-${recurrence.id}`}
                        data-status={recurrence.status}
                        className={cn(
                          recurrence.status === "paused" && "text-muted-foreground",
                          // The class name is what marks a row found from the
                          // calendar; the colour is what the eye lands on.
                          highlighted === recurrence.id && "highlighted bg-accent/60",
                        )}
                      >
                        <TableHead
                          scope="row"
                          className="h-auto py-2 font-medium whitespace-normal text-current"
                        >
                          <span className="inline-flex flex-wrap items-center gap-1.5">
                            <span data-testid="name">{recurrenceName(recurrence)}</span>
                            {recurrence.source === "manual" ? (
                              <Badge variant="outline">manual</Badge>
                            ) : null}
                            {recurrence.status === "paused" ? (
                              <Badge variant="secondary">paused</Badge>
                            ) : null}
                          </span>
                          {recurrenceName(recurrence) ===
                          recurrence.normalized_description ? null : (
                            <small className="block text-xs font-normal text-muted-foreground">
                              {recurrence.normalized_description}
                            </small>
                          )}
                        </TableHead>
                        <TableCell>{accountName(recurrence.account_id)}</TableCell>
                        <TableCell data-testid="interval">{recurrence.interval}</TableCell>
                        <TableCell className="money" data-testid="amount">
                          {formatMinorUnits(recurrence.expected_amount, recurrence.currency)}
                        </TableCell>
                        <TableCell data-testid="next">{recurrence.next_date}</TableCell>
                        <TableCell data-testid="category">
                          {categoryName(recurrence.category_id)}
                        </TableCell>
                        <TableCell>
                          <div className="flex justify-end gap-1">
                            <Button
                              variant="outline"
                              size="sm"
                              onClick={() => setEditing(recurrence.id)}
                            >
                              <Pencil />
                              Edit
                            </Button>
                            <Button
                              variant="ghost"
                              size="sm"
                              onClick={() =>
                                void attempt(
                                  () =>
                                    app.writes?.pauseRecurrence(
                                      recurrence.id,
                                      recurrence.status !== "paused",
                                    ) ?? Promise.resolve(),
                                  "the recurrence could not be changed",
                                )
                              }
                            >
                              {recurrence.status === "paused" ? <Play /> : <Pause />}
                              {recurrence.status === "paused" ? "Resume" : "Pause"}
                            </Button>
                            <Button
                              variant="ghost"
                              size="sm"
                              onClick={() =>
                                void attempt(
                                  () =>
                                    app.writes?.updateRecurrence(recurrence.id, {
                                      status: "dismissed",
                                    }) ?? Promise.resolve(),
                                  "the recurrence could not be dismissed",
                                )
                              }
                            >
                              <X />
                              Dismiss
                            </Button>
                          </div>
                        </TableCell>
                      </TableRow>
                    ))}
                  </TableBody>
                </Table>
              )}
            </CardContent>
          </Card>

          <Dialog
            open={editingRecurrence !== null}
            onOpenChange={(open) => {
              if (!open) setEditing(null);
            }}
          >
            <DialogContent className="sm:max-w-xl">
              <DialogHeader>
                <DialogTitle>Edit recurrence</DialogTitle>
                <DialogDescription>
                  What it is called, how often, how much, and where it files. Its dates are the
                  engine's to move: a payment advances them.
                </DialogDescription>
              </DialogHeader>
              {editingRecurrence === null ? null : (
                <RecurrenceEditor
                  key={editingRecurrence.id}
                  app={app}
                  recurrence={editingRecurrence}
                  categories={categories}
                  merchants={merchants}
                  onDone={() => setEditing(null)}
                  onProblem={setProblem}
                />
              )}
            </DialogContent>
          </Dialog>
        </>
      )}
    </section>
  );
}

/** One of the two views, drawn like a tab but carrying an address. */
function ViewLink({
  href,
  active,
  icon: Icon,
  children,
}: {
  href: string;
  active: boolean;
  icon: LucideIcon;
  children: ReactNode;
}) {
  return (
    <a
      href={href}
      aria-current={active ? "page" : undefined}
      className={cn(
        "inline-flex h-full items-center gap-1.5 rounded-md px-3 text-sm font-medium no-underline transition-colors hover:no-underline",
        active ? "bg-card text-foreground shadow-sm" : "text-foreground/70 hover:text-foreground",
      )}
    >
      <Icon aria-hidden="true" className="size-4" />
      {children}
    </a>
  );
}

/** "in 3 days", "today", "2 days late": how far a due date is from today. */
function daysText(daysAway: number): string {
  if (daysAway === 0) return "today";
  const count = Math.abs(daysAway);
  const unit = count === 1 ? "day" : "days";
  return daysAway < 0 ? `${count} ${unit} late` : `in ${count} ${unit}`;
}

/**
 * One bill on the upcoming list. The name is what the household calls it;
 * the statement text sits under it, because that is what the bank will print
 * and what a person searching their statement will look for.
 */
function BillRow({
  bill,
  transactions,
}: {
  bill: ResolvedBill<Recurrence>;
  transactions: readonly Transaction[];
}) {
  const { recurrence } = bill;
  const name = recurrenceName(recurrence);
  const paidOn =
    bill.paidBy === undefined
      ? undefined
      : transactions.find((transaction) => transaction.id === bill.paidBy)?.date;
  return (
    <li
      data-testid={`bill-${recurrence.id}`}
      data-state={bill.state}
      className="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-x-5 gap-y-1 px-5 py-3 text-sm md:grid-cols-[minmax(0,1fr)_auto_auto_minmax(7rem,auto)]"
    >
      <div className="min-w-0">
        <span className="block truncate font-medium">{name}</span>
        {name === recurrence.normalized_description ? null : (
          <small className="block text-xs text-muted-foreground">
            {recurrence.normalized_description}
          </small>
        )}
      </div>
      <span className="money" data-testid="amount">
        {formatMinorUnits(recurrence.expected_amount, recurrence.currency)}
      </span>
      <span className="grid justify-items-start gap-0.5 tabular-nums md:justify-items-end md:text-right">
        <time dateTime={bill.dueDate} data-testid="due">
          {bill.dueDate}
        </time>
        <small className="text-xs text-muted-foreground" data-testid="days">
          {daysText(bill.daysAway)}
        </small>
      </span>
      <span className="grid justify-items-start gap-0.5 md:justify-items-end md:text-right">
        {/* The badges judge: paid is good, due wants attention, late is a problem. */}
        {bill.state === "paid" ? (
          <Badge variant="positive" data-testid="state">
            paid{paidOn === undefined ? "" : ` ${paidOn}`}
          </Badge>
        ) : bill.state === "late" ? (
          <Badge variant="destructive" data-testid="state">
            late
          </Badge>
        ) : bill.state === "due" ? (
          <Badge variant="warning" data-testid="state">
            due
          </Badge>
        ) : null}
        {bill.state !== "paid" && recurrence.last_date !== undefined ? (
          <small className="text-xs text-muted-foreground">last paid {recurrence.last_date}</small>
        ) : null}
      </span>
    </li>
  );
}

/**
 * The month view: every confirmed recurrence on the days it falls, and what
 * the month adds up to. A bill on the grid leads back to its row in the list.
 */
function CalendarView({
  month,
  today,
  byDate,
  totals,
  currency,
  onMonth,
  onReveal,
}: {
  month: string;
  today: string;
  byDate: ReturnType<typeof selectBillsByDate>;
  totals: ReturnType<typeof calendarTotals>;
  currency: string;
  onMonth: (month: string) => void;
  onReveal: (recurrenceId: string) => void;
}) {
  return (
    <div data-testid="recurring-calendar" className="grid gap-4">
      <div className="flex flex-wrap items-center gap-3">
        <Button
          variant="outline"
          size="sm"
          aria-label="Previous month"
          onClick={() => onMonth(addMonths(month, -1))}
        >
          ‹ Previous
        </Button>
        <h2 data-testid="calendar-month" className="min-w-44 text-center text-lg">
          {readoutLabel(month)}
        </h2>
        <Button
          variant="outline"
          size="sm"
          aria-label="Next month"
          onClick={() => onMonth(addMonths(month, 1))}
        >
          Next ›
        </Button>
        <div className="ml-auto flex flex-wrap gap-3">
          {(totals.length === 0 ? [{ currency, total: 0 }] : totals).map((total) => (
            <Card key={total.currency} className="gap-0 py-2">
              <CardContent className="grid gap-0.5 px-4">
                <span className={EYEBROW}>Due in {readoutLabel(month)}</span>
                <strong
                  className="money justify-self-start text-lg font-semibold"
                  data-testid={`calendar-total-${total.currency}`}
                >
                  {formatMinorUnits(total.total, total.currency)}
                </strong>
              </CardContent>
            </Card>
          ))}
        </div>
      </div>
      <MonthCalendar
        month={month}
        today={today}
        ariaLabel={`Bills in ${readoutLabel(month)}`}
        renderDay={(cell) =>
          (byDate.get(cell.date) ?? []).map((bill) => (
            // A bill on a day: name and amount on one line, the name giving
            // way first. The zero minimum matters: without it the day's grid
            // track grows to the unwrapped text and runs into the next day.
            <Button
              key={bill.recurrence.id}
              variant="link"
              size="sm"
              className="h-auto w-full min-w-0 justify-between gap-1.5 px-1 py-0.5 text-left text-xs font-medium"
              data-testid={`calendar-bill-${bill.recurrence.id}`}
              title={`${recurrenceName(bill.recurrence)} · ${formatMinorUnits(
                bill.amount,
                bill.recurrence.currency,
              )}`}
              onClick={() => onReveal(bill.recurrence.id)}
            >
              <span className="min-w-0 truncate">{recurrenceName(bill.recurrence)}</span>
              <span className="money font-normal text-muted-foreground">
                {formatMinorUnits(bill.amount, bill.recurrence.currency)}
              </span>
            </Button>
          ))
        }
      />
    </div>
  );
}

/**
 * Adjusting a recurrence in place: what it is called, how often, how much,
 * and where it files. The dates are the engine's to move -- a payment
 * advances them -- so they are not offered here.
 */
function RecurrenceEditor({
  app,
  recurrence,
  categories,
  merchants,
  onDone,
  onProblem,
}: {
  app: RationalApp;
  recurrence: Recurrence;
  categories: readonly TaxonomyEntry[];
  merchants: readonly TaxonomyEntry[];
  onDone: () => void;
  onProblem: (problem: string | null) => void;
}) {
  const [name, setName] = useState(recurrence.name ?? "");
  const [every, setEvery] = useState<RecurrenceInterval>(recurrence.interval);
  const [amount, setAmount] = useState(
    amountToText(recurrence.expected_amount, recurrence.currency),
  );
  const [categoryId, setCategoryId] = useState(recurrence.category_id ?? "");
  const [merchantId, setMerchantId] = useState(recurrence.merchant_id ?? "");

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    try {
      await app.writes?.updateRecurrence(recurrence.id, {
        name: name.trim() === "" ? null : name.trim(),
        interval: every,
        expected_amount: parseAmount(amount, recurrence.currency),
        category_id: categoryId === "" ? null : categoryId,
        merchant_id: merchantId === "" ? null : merchantId,
      });
      onProblem(null);
      onDone();
    } catch (error) {
      onProblem(error instanceof Error ? error.message : "the recurrence could not be saved");
    }
  };

  return (
    <form
      className="grid gap-4"
      aria-label="Edit recurrence"
      onSubmit={(event) => void submit(event)}
    >
      <div className="grid gap-4 sm:grid-cols-2">
        <Field label="Name" htmlFor="recurrence-name">
          <Input
            id="recurrence-name"
            name="name"
            maxLength={200}
            placeholder={recurrenceName(recurrence)}
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </Field>
        <Field label="Every" htmlFor="recurrence-interval">
          <NativeSelect
            id="recurrence-interval"
            name="interval"
            value={every}
            onChange={(event) => setEvery(event.target.value as RecurrenceInterval)}
          >
            {INTERVALS.map((interval) => (
              <option key={interval} value={interval}>
                {interval}
              </option>
            ))}
          </NativeSelect>
        </Field>
        <Field label={`Expected amount (${recurrence.currency})`} htmlFor="recurrence-amount">
          <Input
            id="recurrence-amount"
            name="expected_amount"
            inputMode="decimal"
            required
            className="money"
            value={amount}
            onChange={(event) => setAmount(event.target.value)}
          />
        </Field>
        <Field label="Category" htmlFor="recurrence-category">
          <NativeSelect
            id="recurrence-category"
            name="category_id"
            value={categoryId}
            onChange={(event) => setCategoryId(event.target.value)}
          >
            <option value="">No category</option>
            {categories.map((category) => (
              <option key={category.id} value={category.id}>
                {category.name}
              </option>
            ))}
          </NativeSelect>
        </Field>
        <Field label="Merchant" htmlFor="recurrence-merchant">
          <NativeSelect
            id="recurrence-merchant"
            name="merchant_id"
            value={merchantId}
            onChange={(event) => setMerchantId(event.target.value)}
          >
            <option value="">No merchant</option>
            {merchants.map((merchant) => (
              <option key={merchant.id} value={merchant.id}>
                {merchant.name}
              </option>
            ))}
          </NativeSelect>
        </Field>
      </div>
      <DialogFooter>
        <Button variant="outline" onClick={onDone}>
          Cancel
        </Button>
        <Button type="submit">Save recurrence</Button>
      </DialogFooter>
    </form>
  );
}
