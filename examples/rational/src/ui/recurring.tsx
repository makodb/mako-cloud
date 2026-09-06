import { type FormEvent, useEffect, useRef, useState } from "react";

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
import { MonthCalendar, readoutLabel } from "./charts/index.js";
import { useQuery } from "./hooks.js";
import { type Route, routeHash } from "./router.js";
import "./styles/recurring.css";

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
    <section aria-labelledby="recurring-title" data-testid="recurring-screen">
      <div className="heading">
        <h1 id="recurring-title">Recurring</h1>
        <nav className="view-toggle" aria-label="View">
          <a href={hashFor("list")} aria-current={view === "list" ? "page" : undefined}>
            List
          </a>
          <a href={hashFor("calendar")} aria-current={view === "calendar" ? "page" : undefined}>
            Calendar
          </a>
        </nav>
      </div>
      <div className="totals">
        {(perMonth.length === 0 ? [{ currency, total: 0 }] : perMonth).map((total) => (
          <div key={total.currency} className="total">
            <span>Recurring per month ({total.currency})</span>
            <strong data-testid={`monthly-total-${total.currency}`}>
              {formatMinorUnits(total.total, total.currency)}
            </strong>
            <small>every confirmed recurrence, at its monthly equivalent</small>
          </div>
        ))}
      </div>
      {problem === null ? null : (
        <p className="notice error" role="alert">
          {problem}
        </p>
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
          <h2>Suggested</h2>
          {suggestions.length === 0 ? (
            <p className="muted">Nothing looks like it repeats yet.</p>
          ) : (
            <table className="data-table" aria-label="Suggested recurring charges">
              <thead>
                <tr>
                  <th scope="col">Description</th>
                  <th scope="col">Account</th>
                  <th scope="col">Every</th>
                  <th scope="col" className="amount">
                    Usually
                  </th>
                  <th scope="col">Next</th>
                  <th scope="col">
                    <span className="visually-hidden">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {suggestions.map((suggestion) => {
                  const { detection, storedId } = suggestion;
                  return (
                    <tr
                      key={storedId ?? `${detection.accountId}:${detection.normalizedDescription}`}
                      data-testid={`detected-${detection.normalizedDescription.replaceAll(" ", "-")}`}
                      data-noticed-by={storedId === null ? "this device" : "the nightly job"}
                    >
                      <th scope="row">
                        {detection.description}
                        <small className="bill-statement">
                          {storedId === null
                            ? `${detection.occurrences} times on this device`
                            : "noticed by the nightly job"}
                        </small>
                      </th>
                      <td>{accountName(detection.accountId)}</td>
                      <td data-testid="interval">{detection.interval}</td>
                      <td className="amount">
                        {formatMinorUnits(detection.expectedAmount, detection.currency)}
                      </td>
                      <td data-testid="next">{detection.nextDate}</td>
                      <td className="actions">
                        <button
                          type="button"
                          className="link"
                          onClick={() => void decide(suggestion, "confirmed")}
                        >
                          Confirm
                        </button>
                        <button
                          type="button"
                          className="link"
                          onClick={() => void decide(suggestion, "dismissed")}
                        >
                          Dismiss
                        </button>
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          )}

          <h2>Upcoming</h2>
          {upcoming.length === 0 ? (
            <p className="muted">Nothing due in the next six weeks.</p>
          ) : (
            <ul className="bills" aria-label="Upcoming bills">
              {upcoming.map((bill) => (
                <BillRow key={bill.recurrence.id} bill={bill} transactions={transactions} />
              ))}
            </ul>
          )}

          <h2>Confirmed and paused</h2>
          {confirmedOrPaused.length === 0 ? (
            <p className="muted">
              Nothing recurring yet. Confirm a suggestion above, or mark a transaction as recurring
              from the transactions screen.
            </p>
          ) : (
            <table className="data-table" aria-label="All recurring">
              <thead>
                <tr>
                  <th scope="col">Name</th>
                  <th scope="col">Account</th>
                  <th scope="col">Every</th>
                  <th scope="col" className="amount">
                    Amount
                  </th>
                  <th scope="col">Next</th>
                  <th scope="col">Category</th>
                  <th scope="col">
                    <span className="visually-hidden">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {confirmedOrPaused.map((recurrence) =>
                  editing === recurrence.id ? (
                    <tr key={recurrence.id} data-testid={`recurrence-${recurrence.id}`}>
                      <td colSpan={7}>
                        <RecurrenceEditor
                          app={app}
                          recurrence={recurrence}
                          categories={categories}
                          merchants={merchants}
                          onDone={() => setEditing(null)}
                          onProblem={setProblem}
                        />
                      </td>
                    </tr>
                  ) : (
                    <tr
                      key={recurrence.id}
                      data-testid={`recurrence-${recurrence.id}`}
                      data-status={recurrence.status}
                      className={[
                        recurrence.status === "paused" ? "muted" : "",
                        highlighted === recurrence.id ? "highlighted" : "",
                      ]
                        .filter((name) => name !== "")
                        .join(" ")}
                    >
                      <th scope="row">
                        <span data-testid="name">{recurrenceName(recurrence)}</span>
                        {recurrence.source === "manual" ? (
                          <span className="chip manual">manual</span>
                        ) : null}
                        {recurrence.status === "paused" ? (
                          <span className="chip paused">paused</span>
                        ) : null}
                        {recurrenceName(recurrence) === recurrence.normalized_description ? null : (
                          <small className="bill-statement">
                            {recurrence.normalized_description}
                          </small>
                        )}
                      </th>
                      <td>{accountName(recurrence.account_id)}</td>
                      <td data-testid="interval">{recurrence.interval}</td>
                      <td className="amount" data-testid="amount">
                        {formatMinorUnits(recurrence.expected_amount, recurrence.currency)}
                      </td>
                      <td data-testid="next">{recurrence.next_date}</td>
                      <td data-testid="category">{categoryName(recurrence.category_id)}</td>
                      <td className="actions">
                        <button
                          type="button"
                          className="link"
                          onClick={() => setEditing(recurrence.id)}
                        >
                          Edit
                        </button>
                        <button
                          type="button"
                          className="link"
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
                          {recurrence.status === "paused" ? "Resume" : "Pause"}
                        </button>
                        <button
                          type="button"
                          className="link"
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
                          Dismiss
                        </button>
                      </td>
                    </tr>
                  ),
                )}
              </tbody>
            </table>
          )}
        </>
      )}
    </section>
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
    <li data-testid={`bill-${recurrence.id}`} data-state={bill.state}>
      <div className="bill-main">
        <span className="bill-name">{name}</span>
        {name === recurrence.normalized_description ? null : (
          <small className="bill-statement">{recurrence.normalized_description}</small>
        )}
      </div>
      <span className="bill-amount" data-testid="amount">
        {formatMinorUnits(recurrence.expected_amount, recurrence.currency)}
      </span>
      <span className="bill-due">
        <time dateTime={bill.dueDate} data-testid="due">
          {bill.dueDate}
        </time>
        <small data-testid="days">{daysText(bill.daysAway)}</small>
      </span>
      <span className="bill-state">
        {bill.state === "paid" ? (
          <span className="chip paid" data-testid="state">
            paid{paidOn === undefined ? "" : ` ${paidOn}`}
          </span>
        ) : bill.state === "late" ? (
          <span className="chip late" data-testid="state">
            late
          </span>
        ) : bill.state === "due" ? (
          <span className="chip due" data-testid="state">
            due
          </span>
        ) : null}
        {bill.state !== "paid" && recurrence.last_date !== undefined ? (
          <small className="bill-last-paid">last paid {recurrence.last_date}</small>
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
    <div data-testid="recurring-calendar">
      <div className="calendar-nav">
        <button
          type="button"
          className="secondary"
          aria-label="Previous month"
          onClick={() => onMonth(addMonths(month, -1))}
        >
          ‹ Previous
        </button>
        <h2 data-testid="calendar-month">{readoutLabel(month)}</h2>
        <button
          type="button"
          className="secondary"
          aria-label="Next month"
          onClick={() => onMonth(addMonths(month, 1))}
        >
          Next ›
        </button>
        <div className="totals calendar-totals">
          {(totals.length === 0 ? [{ currency, total: 0 }] : totals).map((total) => (
            <div key={total.currency} className="total">
              <span>Due in {readoutLabel(month)}</span>
              <strong data-testid={`calendar-total-${total.currency}`}>
                {formatMinorUnits(total.total, total.currency)}
              </strong>
            </div>
          ))}
        </div>
      </div>
      <MonthCalendar
        month={month}
        today={today}
        ariaLabel={`Bills in ${readoutLabel(month)}`}
        renderDay={(cell) =>
          (byDate.get(cell.date) ?? []).map((bill) => (
            <button
              key={bill.recurrence.id}
              type="button"
              className="link calendar-bill"
              data-testid={`calendar-bill-${bill.recurrence.id}`}
              title={`${recurrenceName(bill.recurrence)} · ${formatMinorUnits(
                bill.amount,
                bill.recurrence.currency,
              )}`}
              onClick={() => onReveal(bill.recurrence.id)}
            >
              <span className="calendar-bill-name">{recurrenceName(bill.recurrence)}</span>
              <span className="calendar-bill-amount">
                {formatMinorUnits(bill.amount, bill.recurrence.currency)}
              </span>
            </button>
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
      className="editor row-editor"
      aria-label="Edit recurrence"
      onSubmit={(event) => void submit(event)}
    >
      <div className="grid">
        <label>
          Name
          <input
            name="name"
            maxLength={200}
            placeholder={recurrenceName(recurrence)}
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </label>
        <label>
          Every
          <select
            name="interval"
            value={every}
            onChange={(event) => setEvery(event.target.value as RecurrenceInterval)}
          >
            {INTERVALS.map((interval) => (
              <option key={interval} value={interval}>
                {interval}
              </option>
            ))}
          </select>
        </label>
        <label>
          Expected amount ({recurrence.currency})
          <input
            name="expected_amount"
            inputMode="decimal"
            required
            value={amount}
            onChange={(event) => setAmount(event.target.value)}
          />
        </label>
        <label>
          Category
          <select
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
          </select>
        </label>
        <label>
          Merchant
          <select
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
          </select>
        </label>
      </div>
      <div className="actions">
        <button type="submit">Save recurrence</button>
        <button type="button" className="secondary" onClick={onDone}>
          Cancel
        </button>
      </div>
    </form>
  );
}
