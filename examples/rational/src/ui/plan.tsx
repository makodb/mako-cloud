import { type FormEvent, useState } from "react";

import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import type { HouseholdCollectionId } from "../model/types.js";
import { goalsByUrgency } from "../selectors/goals.js";
import { amountToText, formatMinorUnits, parseAmount } from "../selectors/money.js";
import {
  type DetectedRecurrence,
  detectRecurrences,
  storedDetection,
  upcomingBills,
} from "../selectors/recurrences.js";
import { useQuery } from "./hooks.js";

/**
 * The two forward-looking screens: what repeats, and what is being saved for.
 *
 * A detected recurrence is a suggestion. Rational writes nothing about one
 * until the person confirms or dismisses it, because a wrong guess that
 * quietly becomes an upcoming bill is worse than no guess at all.
 */
export function PlanScreen({
  app,
  session,
  currency,
}: {
  app: RationalApp;
  session: ScopeSession<HouseholdCollectionId>;
  currency: string;
}) {
  const transactions = useQuery(session.collection("transactions")?.find() ?? null);
  const recurrences = useQuery(session.collection("recurrences")?.find() ?? null);
  const goals = useQuery(session.collection("goals")?.find() ?? null);
  const accounts = useQuery(
    session.collection("accounts")?.find({ sort: [{ name: "asc" }] }) ?? null,
  );
  const [problem, setProblem] = useState<string | null>(null);
  const today = new Date().toISOString().slice(0, 10);
  // The nightly job notices repeating charges too, and writes down what it
  // finds. Its detections come first: they were noticed before this device
  // opened the app, and a household that has not visited in a month should
  // find them waiting rather than have them re-derived only while it looks.
  const stored = recurrences.filter((recurrence) => recurrence.status === "detected");
  const detected: ReadonlyArray<{ detection: DetectedRecurrence; storedId: string | null }> = [
    ...stored.map((recurrence) => ({
      detection: storedDetection(recurrence),
      storedId: recurrence.id,
    })),
    ...detectRecurrences(transactions, recurrences).map((detection) => ({
      detection,
      storedId: null,
    })),
  ];
  const bills = upcomingBills(recurrences, today);
  const progress = goalsByUrgency(goals, today);
  const accountName = (id: string | undefined) =>
    id === undefined ? "—" : (accounts.find((account) => account.id === id)?.name ?? id);

  const decide = async (
    suggestion: (typeof detected)[number],
    status: "confirmed" | "dismissed",
  ) => {
    const { detection, storedId } = suggestion;
    try {
      if (storedId !== null) {
        await app.writes?.updateRecurrence(storedId, { status });
        setProblem(null);
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
      });
      setProblem(null);
    } catch (error) {
      setProblem(error instanceof Error ? error.message : "the recurrence could not be saved");
    }
  };

  const createGoal = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    try {
      const targetDate = String(data.get("target_date") ?? "").trim();
      await app.writes?.createGoal({
        name: String(data.get("name") ?? ""),
        target_amount: parseAmount(String(data.get("target_amount") ?? ""), currency),
        currency,
        ...(targetDate === "" ? {} : { target_date: targetDate }),
        account_id: String(data.get("account_id") ?? ""),
      });
      form.reset();
      setProblem(null);
    } catch (error) {
      setProblem(error instanceof Error ? error.message : "the goal could not be saved");
    }
  };

  const contribute = async (goalId: string) => {
    const typed = window.prompt("How much are you putting in?");
    if (typed === null) return;
    try {
      await app.writes?.contributeToGoal(goalId, {
        date: today,
        amount: parseAmount(typed, currency),
      });
      setProblem(null);
    } catch (error) {
      setProblem(error instanceof Error ? error.message : "the contribution could not be saved");
    }
  };

  return (
    <section aria-labelledby="plan-title" data-testid="plan-screen">
      <div className="section-heading">
        <h2 id="plan-title">Plan</h2>
      </div>
      {problem === null ? null : (
        <p className="notice error" role="alert">
          {problem}
        </p>
      )}

      <h3>Suggested recurring charges</h3>
      {detected.length === 0 ? (
        <p className="muted">Nothing looks like it repeats yet.</p>
      ) : (
        <table className="data-table" aria-label="Suggested recurring charges">
          <thead>
            <tr>
              <th scope="col">Description</th>
              <th scope="col">Account</th>
              <th scope="col">Every</th>
              <th scope="col">Usually</th>
              <th scope="col">Next</th>
              <th scope="col">
                <span className="visually-hidden">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {detected.map(({ detection, storedId }) => (
              <tr
                key={storedId ?? `${detection.accountId}:${detection.normalizedDescription}`}
                data-testid={`detected-${detection.normalizedDescription.replaceAll(" ", "-")}`}
                data-noticed-by={storedId === null ? "this device" : "the nightly job"}
              >
                <th scope="row">{detection.description}</th>
                <td>{accountName(detection.accountId)}</td>
                <td data-testid="interval">{detection.interval}</td>
                <td>{formatMinorUnits(detection.expectedAmount, detection.currency)}</td>
                <td data-testid="next">{detection.nextDate}</td>
                <td className="actions">
                  <button
                    type="button"
                    className="link"
                    onClick={() => void decide({ detection, storedId }, "confirmed")}
                  >
                    Confirm
                  </button>
                  <button
                    type="button"
                    className="link"
                    onClick={() => void decide({ detection, storedId }, "dismissed")}
                  >
                    Dismiss
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      <h3>Upcoming bills</h3>
      {bills.length === 0 ? (
        <p className="muted">Nothing due in the next six weeks.</p>
      ) : (
        <ul aria-label="Upcoming bills">
          {bills.map((bill) => (
            <li key={bill.recurrence.id} data-testid={`bill-${bill.recurrence.id}`}>
              {bill.recurrence.normalized_description} —{" "}
              {formatMinorUnits(bill.expectedAmount, bill.currency)} on {bill.dueDate}{" "}
              <small data-testid="days">
                {bill.daysAway < 0
                  ? `${-bill.daysAway} days late`
                  : bill.daysAway === 0
                    ? "today"
                    : `in ${bill.daysAway} days`}
              </small>
            </li>
          ))}
        </ul>
      )}

      <h3>Goals</h3>
      {progress.length === 0 ? (
        <p className="muted">No goals yet.</p>
      ) : (
        <table className="data-table" aria-label="Goals">
          <thead>
            <tr>
              <th scope="col">Goal</th>
              <th scope="col">Saved</th>
              <th scope="col">Target</th>
              <th scope="col">By</th>
              <th scope="col">A month</th>
              <th scope="col">
                <span className="visually-hidden">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {progress.map((entry) => (
              <tr
                key={entry.goal.id}
                data-testid={`goal-${entry.goal.id}`}
                data-percent={entry.percent}
              >
                <th scope="row">
                  {entry.goal.name}
                  {entry.goal.status === "completed" ? <small> done</small> : null}
                </th>
                <td data-testid="saved">{formatMinorUnits(entry.saved, entry.goal.currency)}</td>
                <td>{formatMinorUnits(entry.goal.target_amount, entry.goal.currency)}</td>
                <td>{entry.goal.target_date ?? "—"}</td>
                <td data-testid="monthly">
                  {entry.monthlyContribution === null
                    ? "—"
                    : formatMinorUnits(entry.monthlyContribution, entry.goal.currency)}
                </td>
                <td className="actions">
                  <button
                    type="button"
                    className="link"
                    onClick={() => void contribute(entry.goal.id)}
                  >
                    Add
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      <form aria-label="New goal" onSubmit={(event) => void createGoal(event)}>
        <h3>New goal</h3>
        <label>
          Name
          <input name="name" required maxLength={200} />
        </label>
        <label>
          Target
          <input
            name="target_amount"
            inputMode="decimal"
            required
            placeholder={amountToText(200_000, currency)}
          />
        </label>
        <label>
          By
          <input name="target_date" type="date" />
        </label>
        <label>
          Saved in
          <select name="account_id" defaultValue="">
            <option value="">No particular account</option>
            {accounts.map((account) => (
              <option key={account.id} value={account.id}>
                {account.name}
              </option>
            ))}
          </select>
        </label>
        <button type="submit">Add goal</button>
      </form>
    </section>
  );
}
