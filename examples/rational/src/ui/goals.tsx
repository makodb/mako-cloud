import { type FormEvent, useState } from "react";

import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import {
  type Account,
  type Goal,
  type GoalKind,
  type GoalProgressSource,
  type HouseholdCollectionId,
  LIABILITY_TYPES,
} from "../model/types.js";
import { selectAccountBalances } from "../selectors/balances.js";
import { type GoalRow, plannedMonthlyTotal, selectGoalProgress } from "../selectors/goals.js";
import { amountToText, formatMinorUnits, parseAmount } from "../selectors/money.js";
import { ProgressBar, readoutLabel } from "./charts/index.js";
import { useQuery } from "./hooks.js";
import "./styles/goals.css";

/**
 * What the household is saving for and paying down, in the order it ranks
 * them.
 *
 * A save-up goal measures money put aside: either what its linked accounts
 * have gained since the goal began, or the contributions a member writes down
 * against it. A pay-down goal measures a liability shrinking, and says when
 * the planned payment clears it. The arithmetic is the shared engine's, so
 * the bars here agree with the alert the nightly job fires when a goal is
 * reached.
 */
export function GoalsScreen({
  app,
  session,
  currency,
}: {
  app: RationalApp;
  session: ScopeSession<HouseholdCollectionId>;
  currency: string;
}) {
  const goals = useQuery(session.collection("goals")?.find() ?? null);
  const accounts = useQuery(
    session.collection("accounts")?.find({ sort: [{ name: "asc" }] }) ?? null,
  );
  const transactions = useQuery(session.collection("transactions")?.find() ?? null);
  const [creating, setCreating] = useState(false);
  const [editing, setEditing] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const today = new Date().toISOString().slice(0, 10);
  const balances = selectAccountBalances(accounts, transactions);
  const rows = selectGoalProgress(goals, balances, today);
  const active = rows.filter((row) => row.goal.status === "active");
  const finished = rows.filter((row) => row.goal.status !== "active");
  const open = accounts.filter((account) => account.closed_at === undefined);
  const planned = plannedMonthlyTotal(goals, currency);

  const attempt = async (work: () => Promise<unknown>, fallback: string) => {
    try {
      await work();
      setProblem(null);
    } catch (error) {
      setProblem(error instanceof Error ? error.message : fallback);
    }
  };

  // Ranking is among the active goals only: a finished goal has no place in
  // the queue, and the order is rewritten as positions so every device agrees.
  const move = (goalId: string, direction: -1 | 1) =>
    attempt(async () => {
      const ordered = active.map((row) => row.goal.id);
      const index = ordered.indexOf(goalId);
      const target = index + direction;
      if (index < 0 || target < 0 || target >= ordered.length) return;
      const swapped = [...ordered];
      swapped[index] = ordered[target] ?? goalId;
      swapped[target] = goalId;
      await app.writes?.reorderGoals(swapped);
    }, "the goals could not be reordered");

  const setStatus = (goalId: string, status: Goal["status"]) =>
    attempt(
      () => app.writes?.updateGoal(goalId, { status }) ?? Promise.resolve(),
      "the goal could not be changed",
    );

  return (
    <section aria-labelledby="goals-title" data-testid="goals-screen">
      <div className="heading">
        <h1 id="goals-title">Goals</h1>
        <button type="button" onClick={() => setCreating(!creating)} aria-expanded={creating}>
          New goal
        </button>
      </div>
      <div className="totals">
        <div className="total">
          <span>Planned a month ({currency})</span>
          <strong data-testid="planned-monthly">{formatMinorUnits(planned, currency)}</strong>
          <small>
            {active.length === 1 ? "1 active goal" : `${active.length} active goals`}
            {finished.length === 0 ? "" : ` · ${finished.length} finished`}
          </small>
        </div>
      </div>
      {problem === null ? null : (
        <p className="notice error" role="alert">
          {problem}
        </p>
      )}
      {creating ? (
        <NewGoalForm
          app={app}
          accounts={open}
          balances={balances}
          currency={currency}
          nextPriority={active.length}
          onDone={() => setCreating(false)}
          onProblem={setProblem}
        />
      ) : null}

      {rows.length === 0 ? (
        <p className="muted">No goals yet. Name something to save for, or a debt to pay down.</p>
      ) : null}

      {active.length === 0 ? null : (
        <div className="goal-list" data-testid="active-goals">
          {active.map((row, index) => (
            <GoalCard
              key={row.goal.id}
              row={row}
              accounts={accounts}
              balances={balances}
              app={app}
              today={today}
              position={index}
              count={active.length}
              editing={editing === row.goal.id}
              onEdit={(open) => setEditing(open ? row.goal.id : null)}
              onMove={(direction) => void move(row.goal.id, direction)}
              onStatus={(status) => void setStatus(row.goal.id, status)}
              onProblem={setProblem}
            />
          ))}
        </div>
      )}

      {finished.length === 0 ? null : (
        <>
          <h2>Finished</h2>
          <div className="goal-list" data-testid="finished-goals">
            {finished.map((row) => (
              <GoalCard
                key={row.goal.id}
                row={row}
                accounts={accounts}
                balances={balances}
                app={app}
                today={today}
                position={0}
                count={1}
                editing={editing === row.goal.id}
                onEdit={(open) => setEditing(open ? row.goal.id : null)}
                onMove={() => undefined}
                onStatus={(status) => void setStatus(row.goal.id, status)}
                onProblem={setProblem}
              />
            ))}
          </div>
        </>
      )}
    </section>
  );
}

function linkedIds(goal: Goal): readonly string[] {
  if (goal.account_ids !== undefined && goal.account_ids.length > 0) return goal.account_ids;
  if (goal.account_id !== undefined && goal.account_id !== "") return [goal.account_id];
  return [];
}

function kindOf(goal: Goal): GoalKind {
  return goal.kind ?? "save";
}

/** Balance-based unless the goal says contributions; a pay-down goal always follows the balance. */
function sourceOf(goal: Goal): GoalProgressSource {
  if (kindOf(goal) === "pay_down") return "balance";
  return goal.progress_source ?? "contributions";
}

function GoalCard({
  row,
  accounts,
  balances,
  app,
  today,
  position,
  count,
  editing,
  onEdit,
  onMove,
  onStatus,
  onProblem,
}: {
  row: GoalRow;
  accounts: readonly Account[];
  balances: ReadonlyMap<string, number>;
  app: RationalApp;
  today: string;
  position: number;
  count: number;
  editing: boolean;
  onEdit: (open: boolean) => void;
  onMove: (direction: -1 | 1) => void;
  onStatus: (status: Goal["status"]) => void;
  onProblem: (problem: string | null) => void;
}) {
  const { goal, math } = row;
  const kind = kindOf(goal);
  const source = sourceOf(goal);
  const money = (amount: number) => formatMinorUnits(amount, goal.currency);
  const linked = linkedIds(goal).map(
    (id) => accounts.find((account) => account.id === id)?.name ?? id,
  );
  // With a date, the number that matters is what it takes to arrive on time;
  // without one, the plan is the only monthly figure there is.
  const monthly =
    goal.target_date === undefined ? (goal.planned_monthly ?? null) : math.monthlyNeeded;
  const track = math.onTrack === null ? "none" : math.onTrack ? "on" : "behind";
  const contributions = [...goal.contributions].sort(
    (left, right) => right.date.localeCompare(left.date) || right.id.localeCompare(left.id),
  );
  return (
    <article
      className="goal-card"
      data-testid={`goal-${goal.id}`}
      data-percent={math.percent}
      data-kind={kind}
      data-status={goal.status}
      aria-label={goal.name}
    >
      <header className="goal-head">
        <div>
          <h2>{goal.name}</h2>
          <span className="chip" data-testid="kind">
            {kind === "pay_down" ? "pay-down" : "save-up"}
          </span>
          <span className="chip" data-testid="source">
            {source === "balance" ? "follows the balance" : "by contributions"}
          </span>
          {goal.status === "active" ? null : <span className="chip">{goal.status}</span>}
        </div>
        {goal.status === "active" ? (
          <div className="goal-order">
            <button
              type="button"
              className="secondary"
              disabled={position === 0}
              onClick={() => onMove(-1)}
            >
              Move up
            </button>
            <button
              type="button"
              className="secondary"
              disabled={position >= count - 1}
              onClick={() => onMove(1)}
            >
              Move down
            </button>
          </div>
        ) : null}
      </header>
      <ProgressBar
        value={math.saved}
        max={goal.target_amount}
        label={`${goal.name}: ${math.percent}% of the way`}
        tone={math.percent >= 100 ? "positive" : "accent"}
      />
      <dl className="goal-figures">
        <div>
          <dt>{kind === "pay_down" ? "Paid down" : "Saved"}</dt>
          <dd data-testid="saved">{money(math.saved)}</dd>
        </div>
        <div>
          <dt>{kind === "pay_down" ? "Owed" : "Remaining"}</dt>
          <dd data-testid="remaining">{money(math.remaining)}</dd>
        </div>
        <div>
          <dt>Target</dt>
          <dd data-testid="target">{money(goal.target_amount)}</dd>
        </div>
        <div>
          <dt>By</dt>
          <dd data-testid="target-date">{goal.target_date ?? "—"}</dd>
        </div>
        <div>
          <dt>{goal.target_date === undefined ? "Planned a month" : "Needed a month"}</dt>
          <dd>
            <span data-testid="monthly">{monthly === null ? "—" : money(monthly)}</span>
            {goal.target_date !== undefined && goal.planned_monthly !== undefined ? (
              <small>planned {money(goal.planned_monthly)}</small>
            ) : null}
          </dd>
        </div>
        <div>
          <dt>Plan</dt>
          <dd data-track={track}>
            <span data-testid="on-track">
              {track === "none" ? "no plan" : track === "on" ? "on track" : "behind"}
            </span>
            {math.onTrack === null ? null : (
              <small>{money(math.expectedByNow)} expected by now</small>
            )}
          </dd>
        </div>
        {kind === "pay_down" ? (
          <div>
            <dt>Paid off</dt>
            <dd>
              {math.payoffMonth === null ? (
                <span data-testid="payoff">—</span>
              ) : (
                <time data-testid="payoff" dateTime={math.payoffMonth}>
                  {readoutLabel(math.payoffMonth)}
                </time>
              )}
              {goal.planned_monthly === undefined ? (
                <small>set a planned payment to project it</small>
              ) : null}
            </dd>
          </div>
        ) : null}
      </dl>
      {linked.length === 0 ? null : (
        <p className="goal-linked" data-testid="linked">
          {kind === "pay_down" ? "Paying down" : "Linked to"} {linked.join(", ")}
        </p>
      )}

      <details className="goal-contributions">
        <summary>
          Contributions ({contributions.length}
          {contributions.length === 0
            ? ""
            : `, ${money(contributions.reduce((total, entry) => total + entry.amount, 0))}`}
          )
        </summary>
        {source === "balance" ? (
          <p className="hint">
            Progress follows the linked balance; contributions here are a record of what was put in,
            not what moves the bar.
          </p>
        ) : null}
        {contributions.length === 0 ? null : (
          <table className="data-table" aria-label={`Contributions to ${goal.name}`}>
            <thead>
              <tr>
                <th scope="col">Date</th>
                <th scope="col" className="amount">
                  Amount
                </th>
                <th scope="col">Note</th>
              </tr>
            </thead>
            <tbody>
              {contributions.map((entry) => (
                <tr key={entry.id} data-testid={`contribution-${entry.id}`}>
                  <td>{entry.date}</td>
                  <td className="amount">{money(entry.amount)}</td>
                  <td>{entry.note ?? ""}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        <ContributionForm app={app} goal={goal} today={today} onProblem={onProblem} />
      </details>

      <div className="actions">
        <button type="button" className="link" onClick={() => onEdit(!editing)}>
          {editing ? "Close" : "Edit"}
        </button>
        {goal.status === "active" ? (
          <button type="button" className="link" onClick={() => onStatus("completed")}>
            Complete
          </button>
        ) : (
          <button type="button" className="link" onClick={() => onStatus("active")}>
            Reopen
          </button>
        )}
        {goal.status === "archived" ? null : (
          <button type="button" className="link" onClick={() => onStatus("archived")}>
            Archive
          </button>
        )}
      </div>
      {editing ? (
        <GoalEditor
          app={app}
          goal={goal}
          accounts={accounts}
          balances={balances}
          onDone={() => onEdit(false)}
          onProblem={onProblem}
        />
      ) : null}
    </article>
  );
}

/** A contribution written against the goal: an amount, when, and a word about it. */
function ContributionForm({
  app,
  goal,
  today,
  onProblem,
}: {
  app: RationalApp;
  goal: Goal;
  today: string;
  onProblem: (problem: string | null) => void;
}) {
  const [amount, setAmount] = useState("");
  const [date, setDate] = useState(today);
  const [note, setNote] = useState("");

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    try {
      await app.writes?.contributeToGoal(goal.id, {
        date,
        amount: parseAmount(amount, goal.currency),
        ...(note.trim() === "" ? {} : { note }),
      });
      setAmount("");
      setNote("");
      onProblem(null);
    } catch (error) {
      onProblem(error instanceof Error ? error.message : "the contribution could not be saved");
    }
  };

  return (
    <form
      className="inline"
      aria-label={`Add contribution to ${goal.name}`}
      onSubmit={(event) => void submit(event)}
    >
      <label>
        Amount ({goal.currency})
        <input
          name="amount"
          inputMode="decimal"
          required
          placeholder={amountToText(10_000, goal.currency)}
          value={amount}
          onChange={(event) => setAmount(event.target.value)}
        />
      </label>
      <label>
        Date
        <input
          name="date"
          type="date"
          required
          value={date}
          onChange={(event) => setDate(event.target.value)}
        />
      </label>
      <label>
        Note
        <input
          name="note"
          maxLength={200}
          value={note}
          onChange={(event) => setNote(event.target.value)}
        />
      </label>
      <button type="submit">Add contribution</button>
    </form>
  );
}

/** The accounts a goal of this kind may follow: a debt is paid down on a liability. */
function eligibleAccounts(kind: GoalKind, accounts: readonly Account[]): readonly Account[] {
  return kind === "pay_down"
    ? accounts.filter((account) => LIABILITY_TYPES.includes(account.type))
    : accounts;
}

function AccountsSelect({
  id,
  accounts,
  value,
  onChange,
}: {
  id: string;
  accounts: readonly Account[];
  value: readonly string[];
  onChange: (ids: readonly string[]) => void;
}) {
  return (
    <select
      id={id}
      name="account_ids"
      multiple
      value={[...value]}
      onChange={(event) =>
        onChange(Array.from(event.target.selectedOptions, (option) => option.value))
      }
    >
      {accounts.map((account) => (
        <option key={account.id} value={account.id}>
          {account.name}
        </option>
      ))}
    </select>
  );
}

function sumBalances(ids: readonly string[], balances: ReadonlyMap<string, number>): number {
  let total = 0;
  for (const id of ids) total += balances.get(id) ?? 0;
  return total;
}

function NewGoalForm({
  app,
  accounts,
  balances,
  currency,
  nextPriority,
  onDone,
  onProblem,
}: {
  app: RationalApp;
  accounts: readonly Account[];
  balances: ReadonlyMap<string, number>;
  currency: string;
  nextPriority: number;
  onDone: () => void;
  onProblem: (problem: string | null) => void;
}) {
  const [kind, setKind] = useState<GoalKind>("save");
  const [name, setName] = useState("");
  const [target, setTarget] = useState("");
  const [targetDate, setTargetDate] = useState("");
  const [plannedMonthly, setPlannedMonthly] = useState("");
  const [accountIds, setAccountIds] = useState<readonly string[]>([]);
  const [source, setSource] = useState<GoalProgressSource>("contributions");
  const eligible = eligibleAccounts(kind, accounts);
  // Switching to pay-down drops any asset that was linked: a debt is paid
  // down on a liability, and the select would not offer the asset anyway.
  const linked = accountIds.filter((id) => eligible.some((account) => account.id === id));
  const linkedBalance = sumBalances(linked, balances);
  const owed = Math.abs(linkedBalance);
  const progress: GoalProgressSource = kind === "pay_down" ? "balance" : source;

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const writes = app.writes;
    if (writes === null) return;
    try {
      // A pay-down goal's natural target is what is owed today; typing one
      // is for the household that means to pay part of it.
      const targetAmount =
        kind === "pay_down" && target.trim() === "" ? owed : parseAmount(target, currency);
      const monthly =
        plannedMonthly.trim() === "" ? undefined : parseAmount(plannedMonthly, currency);
      // The linked balance when the goal begins is remembered, so a goal on an
      // account that already holds money starts at zero and measures what was
      // saved since -- and a debt measures what was paid off since.
      const starting = kind === "pay_down" ? owed : linkedBalance;
      await writes.createGoal({
        name,
        target_amount: targetAmount,
        currency,
        ...(targetDate === "" ? {} : { target_date: targetDate }),
        kind,
        account_ids: linked,
        ...(monthly === undefined ? {} : { planned_monthly: monthly }),
        priority: nextPriority,
        progress_source: progress,
        ...(progress === "balance" ? { starting_balance: starting } : {}),
      });
      onProblem(null);
      onDone();
    } catch (error) {
      onProblem(error instanceof Error ? error.message : "the goal could not be saved");
    }
  };

  return (
    <form className="editor" aria-label="New goal" onSubmit={(event) => void submit(event)}>
      <h2>New goal</h2>
      <div className="grid">
        <label>
          Kind
          <select
            name="kind"
            value={kind}
            onChange={(event) => setKind(event.target.value as GoalKind)}
          >
            <option value="save">Save up</option>
            <option value="pay_down">Pay down</option>
          </select>
        </label>
        <label>
          Name
          <input
            name="name"
            required
            maxLength={200}
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </label>
        <label>
          Target ({currency})
          <input
            name="target_amount"
            inputMode="decimal"
            required={kind !== "pay_down"}
            placeholder={
              kind === "pay_down" && owed > 0
                ? amountToText(owed, currency)
                : amountToText(200_000, currency)
            }
            value={target}
            onChange={(event) => setTarget(event.target.value)}
          />
        </label>
        <label>
          By
          <input
            name="target_date"
            type="date"
            value={targetDate}
            onChange={(event) => setTargetDate(event.target.value)}
          />
        </label>
        <label>
          Planned monthly ({currency})
          <input
            name="planned_monthly"
            inputMode="decimal"
            placeholder={amountToText(25_000, currency)}
            value={plannedMonthly}
            onChange={(event) => setPlannedMonthly(event.target.value)}
          />
        </label>
        <label htmlFor="new-goal-accounts">
          Accounts
          <AccountsSelect
            id="new-goal-accounts"
            accounts={eligible}
            value={linked}
            onChange={setAccountIds}
          />
        </label>
        {kind === "pay_down" ? null : (
          <label>
            Progress
            <select
              name="progress_source"
              value={source}
              onChange={(event) => setSource(event.target.value as GoalProgressSource)}
            >
              <option value="contributions">By contributions I record</option>
              <option value="balance">Follows the linked balance</option>
            </select>
          </label>
        )}
      </div>
      <p className="hint">
        {kind === "pay_down"
          ? linked.length === 0
            ? "Choose the loan or card being paid down; progress follows what it still owes."
            : `Owed today: ${formatMinorUnits(owed, currency)}. Leave the target blank to pay it all off.`
          : progress === "balance"
            ? linked.length === 0
              ? "Following a balance needs at least one linked account."
              : `The linked balance is ${formatMinorUnits(linkedBalance, currency)} today; the goal measures what is saved from here.`
            : "Record each contribution on the goal; linked accounts are for reference."}
      </p>
      <div className="actions">
        <button type="submit">Add goal</button>
        <button type="button" className="secondary" onClick={onDone}>
          Cancel
        </button>
      </div>
    </form>
  );
}

/**
 * Editing a goal: everything but its kind, which would change what its
 * numbers mean. Switching a save goal to follow its balance re-records the
 * starting balance, so the change of source does not read as a windfall.
 */
function GoalEditor({
  app,
  goal,
  accounts,
  balances,
  onDone,
  onProblem,
}: {
  app: RationalApp;
  goal: Goal;
  accounts: readonly Account[];
  balances: ReadonlyMap<string, number>;
  onDone: () => void;
  onProblem: (problem: string | null) => void;
}) {
  const kind = kindOf(goal);
  const [name, setName] = useState(goal.name);
  const [target, setTarget] = useState(amountToText(goal.target_amount, goal.currency));
  const [targetDate, setTargetDate] = useState(goal.target_date ?? "");
  const [plannedMonthly, setPlannedMonthly] = useState(
    goal.planned_monthly === undefined ? "" : amountToText(goal.planned_monthly, goal.currency),
  );
  const [priority, setPriority] = useState(
    goal.priority === undefined ? "" : String(goal.priority),
  );
  const [accountIds, setAccountIds] = useState<readonly string[]>(linkedIds(goal));
  const [source, setSource] = useState<GoalProgressSource>(sourceOf(goal));
  const eligible = eligibleAccounts(kind, accounts);
  const linked = accountIds.filter((id) => eligible.some((account) => account.id === id));

  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    try {
      if (name.trim() === "") throw new Error("a goal needs a name");
      const targetAmount = parseAmount(target, goal.currency);
      if (targetAmount <= 0) throw new Error("a goal needs a target above zero");
      const monthly =
        plannedMonthly.trim() === "" ? null : parseAmount(plannedMonthly, goal.currency);
      if (monthly !== null && monthly < 0) {
        throw new Error("the planned monthly amount cannot be negative");
      }
      const rank = priority.trim() === "" ? null : Number(priority);
      if (rank !== null && (!Number.isSafeInteger(rank) || rank < 0)) {
        throw new Error("priority is a whole, non-negative number");
      }
      const progress: GoalProgressSource = kind === "pay_down" ? "balance" : source;
      if (progress === "balance" && linked.length === 0) {
        throw new Error("progress by balance needs a linked account");
      }
      const startsFollowing = progress === "balance" && sourceOf(goal) !== "balance";
      await app.writes?.updateGoal(goal.id, {
        name: name.trim(),
        target_amount: targetAmount,
        target_date: targetDate === "" ? null : targetDate,
        planned_monthly: monthly,
        priority: rank,
        account_ids: linked.length === 0 ? null : linked,
        progress_source: progress,
        ...(startsFollowing ? { starting_balance: sumBalances(linked, balances) } : {}),
      });
      onProblem(null);
      onDone();
    } catch (error) {
      onProblem(error instanceof Error ? error.message : "the goal could not be saved");
    }
  };

  return (
    <form
      className="editor"
      aria-label={`Edit ${goal.name}`}
      onSubmit={(event) => void submit(event)}
    >
      <div className="grid">
        <label>
          Name
          <input
            name="name"
            required
            maxLength={200}
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </label>
        <label>
          Target ({goal.currency})
          <input
            name="target_amount"
            inputMode="decimal"
            required
            value={target}
            onChange={(event) => setTarget(event.target.value)}
          />
        </label>
        <label>
          By
          <input
            name="target_date"
            type="date"
            value={targetDate}
            onChange={(event) => setTargetDate(event.target.value)}
          />
        </label>
        <label>
          Planned monthly ({goal.currency})
          <input
            name="planned_monthly"
            inputMode="decimal"
            value={plannedMonthly}
            onChange={(event) => setPlannedMonthly(event.target.value)}
          />
        </label>
        <label>
          Priority
          <input
            name="priority"
            inputMode="numeric"
            value={priority}
            onChange={(event) => setPriority(event.target.value)}
          />
        </label>
        <label htmlFor={`edit-goal-accounts-${goal.id}`}>
          Accounts
          <AccountsSelect
            id={`edit-goal-accounts-${goal.id}`}
            accounts={eligible}
            value={linked}
            onChange={setAccountIds}
          />
        </label>
        {kind === "pay_down" ? null : (
          <label>
            Progress
            <select
              name="progress_source"
              value={source}
              onChange={(event) => setSource(event.target.value as GoalProgressSource)}
            >
              <option value="contributions">By contributions I record</option>
              <option value="balance">Follows the linked balance</option>
            </select>
          </label>
        )}
      </div>
      <div className="actions">
        <button type="submit">Save goal</button>
        <button type="button" className="secondary" onClick={onDone}>
          Cancel
        </button>
      </div>
    </form>
  );
}
