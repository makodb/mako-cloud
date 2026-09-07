import {
  Alert,
  AlertDescription,
  Badge,
  Button,
  Card,
  CardContent,
  CardFooter,
  CardHeader,
  type ChartDatum,
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
  LineChart,
  NativeSelect,
  Progress,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@mako-cloud/ui";
import {
  Archive,
  ArrowDown,
  ArrowUp,
  Check,
  ChevronRight,
  Pencil,
  Plus,
  RotateCcw,
  Target,
} from "lucide-react";
import { type FormEvent, type ReactNode, useState } from "react";

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
import {
  addMonths,
  type GoalRow,
  plannedMonthlyTotal,
  selectGoalProgress,
} from "../selectors/goals.js";
import { amountToText, formatMinorUnits, parseAmount } from "../selectors/money.js";
import { compactMoney, readoutLabel, shortMonthLabel } from "./charts/layout.js";
import { useQuery } from "./hooks.js";

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

/** The small heading over a figure. */
const EYEBROW = "text-xs font-semibold uppercase tracking-wider text-muted-foreground";

/** A projection is drawn no further than this, however small the payment. */
const PROJECTION_MONTHS = 600;

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
    <section aria-labelledby="goals-title" data-testid="goals-screen" className="grid gap-6">
      <header className="flex flex-wrap items-start justify-between gap-4">
        <div className="grid gap-1">
          <h1 id="goals-title" className="text-2xl">
            Goals
          </h1>
          <p className="m-0 text-sm text-muted-foreground">
            What the household is saving for and paying down, in the order it ranks them.
          </p>
        </div>
        <Button onClick={() => setCreating(!creating)} aria-expanded={creating}>
          <Plus />
          New goal
        </Button>
      </header>
      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        <Card className="gap-1 py-4">
          <CardContent className="grid gap-1 px-4">
            <span className={EYEBROW}>Planned a month ({currency})</span>
            <strong
              className="money justify-self-start text-2xl font-semibold"
              data-testid="planned-monthly"
            >
              {formatMinorUnits(planned, currency)}
            </strong>
            <small className="text-xs text-muted-foreground">
              {active.length === 1 ? "1 active goal" : `${active.length} active goals`}
              {finished.length === 0 ? "" : ` · ${finished.length} finished`}
            </small>
          </CardContent>
        </Card>
      </div>
      {problem === null ? null : (
        <Alert variant="destructive" role="alert">
          <AlertDescription>{problem}</AlertDescription>
        </Alert>
      )}

      <Dialog open={creating} onOpenChange={setCreating}>
        <DialogContent className="sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>New goal</DialogTitle>
            <DialogDescription>
              Something to save up for, or a debt to pay down. Progress is measured from today.
            </DialogDescription>
          </DialogHeader>
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
        </DialogContent>
      </Dialog>

      {rows.length === 0 ? (
        <EmptyState
          icon={<Target />}
          title="No goals yet."
          description="Name something to save for, or a debt to pay down."
        />
      ) : null}

      {active.length === 0 ? null : (
        <div className="grid gap-4" data-testid="active-goals">
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
        <div className="grid gap-4">
          <h2 className="text-lg">Finished</h2>
          <div className="grid gap-4" data-testid="finished-goals">
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
        </div>
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

/**
 * What a pay-down goal still owes at the start of each month, from today
 * until the planned payment clears it: the same arithmetic as `payoffMonth`,
 * drawn out. Empty without a plan or with nothing left to pay.
 */
function payoffProjection(
  remaining: number,
  planned: number | undefined,
  today: string,
): ChartDatum[] {
  if (planned === undefined || planned <= 0 || remaining <= 0) return [];
  const points: ChartDatum[] = [];
  let owed = remaining;
  let month = today.slice(0, 7);
  points.push({ month, owed });
  while (owed > 0 && points.length < PROJECTION_MONTHS) {
    month = addMonths(month, 1);
    owed = Math.max(0, owed - planned);
    points.push({ month, owed });
  }
  return points;
}

/** `2027-02` on an axis: `Feb 27`, short and unambiguous across a long projection. */
function monthTick(value: string | number): string {
  const month = String(value);
  return `${shortMonthLabel(month)} ${month.slice(2, 4)}`;
}

/** One figure under the bar: a small heading and the number it names. */
function Figure({ label, children }: { label: string; children: ReactNode }) {
  return (
    <div className="grid content-start gap-0.5">
      <dt className={EYEBROW}>{label}</dt>
      {children}
    </div>
  );
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
  const projection =
    kind === "pay_down" && goal.status === "active"
      ? payoffProjection(math.remaining, goal.planned_monthly, today)
      : [];
  const finished = goal.status !== "active";
  return (
    // The kit's Card is a `<section>`; a goal is an article, which is what
    // the suite and a screen reader look for, so the card's dress is worn here.
    <article
      className={cn(
        "flex flex-col gap-5 rounded-xl border bg-card py-5 text-card-foreground shadow-xs",
        // A finished goal stays on the page, quieter, so the history is legible.
        finished && "bg-muted/40 text-muted-foreground",
      )}
      data-testid={`goal-${goal.id}`}
      data-percent={math.percent}
      data-kind={kind}
      data-status={goal.status}
      aria-label={goal.name}
    >
      <CardHeader className="flex flex-wrap items-start justify-between gap-3">
        <div className="grid gap-2">
          <h2 className="text-lg">{goal.name}</h2>
          <div className="flex flex-wrap gap-1.5">
            <Badge variant="secondary" data-testid="kind">
              {kind === "pay_down" ? "pay-down" : "save-up"}
            </Badge>
            <Badge variant="outline" data-testid="source">
              {source === "balance" ? "follows the balance" : "by contributions"}
            </Badge>
            {goal.status === "active" ? null : <Badge variant="outline">{goal.status}</Badge>}
          </div>
        </div>
        {goal.status === "active" ? (
          <div className="flex gap-1">
            <Button
              variant="outline"
              size="sm"
              disabled={position === 0}
              onClick={() => onMove(-1)}
            >
              <ArrowUp />
              Move up
            </Button>
            <Button
              variant="outline"
              size="sm"
              disabled={position >= count - 1}
              onClick={() => onMove(1)}
            >
              <ArrowDown />
              Move down
            </Button>
          </div>
        ) : null}
      </CardHeader>
      <CardContent className="grid gap-5">
        <div className="grid gap-1.5">
          <div className="flex items-baseline justify-between gap-3 text-sm">
            <span className="font-medium">{math.percent}% of the way</span>
            <span className="text-muted-foreground">
              {money(math.saved)} of {money(goal.target_amount)}
            </span>
          </div>
          <Progress
            value={math.percent}
            tone={math.percent >= 100 ? "positive" : "primary"}
            aria-label={`${goal.name}: ${math.percent}% of the way`}
          />
        </div>
        <dl className="m-0 grid grid-cols-[repeat(auto-fit,minmax(8rem,1fr))] gap-x-4 gap-y-3">
          <Figure label={kind === "pay_down" ? "Paid down" : "Saved"}>
            <dd className="m-0 font-medium tabular-nums" data-testid="saved">
              {money(math.saved)}
            </dd>
          </Figure>
          <Figure label={kind === "pay_down" ? "Owed" : "Remaining"}>
            <dd className="m-0 font-medium tabular-nums" data-testid="remaining">
              {money(math.remaining)}
            </dd>
          </Figure>
          <Figure label="Target">
            <dd className="m-0 font-medium tabular-nums" data-testid="target">
              {money(goal.target_amount)}
            </dd>
          </Figure>
          <Figure label="By">
            <dd className="m-0 font-medium tabular-nums" data-testid="target-date">
              {goal.target_date ?? "—"}
            </dd>
          </Figure>
          <Figure label={goal.target_date === undefined ? "Planned a month" : "Needed a month"}>
            <dd className="m-0 grid font-medium tabular-nums">
              <span data-testid="monthly">{monthly === null ? "—" : money(monthly)}</span>
              {goal.target_date !== undefined && goal.planned_monthly !== undefined ? (
                <small className="text-xs font-normal text-muted-foreground">
                  planned {money(goal.planned_monthly)}
                </small>
              ) : null}
            </dd>
          </Figure>
          <Figure label="Plan">
            {/* On track is good news, behind is a warning; neither shouts. */}
            <dd className="m-0 grid justify-items-start gap-1" data-track={track}>
              <Badge
                variant={
                  track === "on" ? "positive" : track === "behind" ? "destructive" : "secondary"
                }
                data-testid="on-track"
              >
                {track === "none" ? "no plan" : track === "on" ? "on track" : "behind"}
              </Badge>
              {math.onTrack === null ? null : (
                <small className="text-xs text-muted-foreground">
                  {money(math.expectedByNow)} expected by now
                </small>
              )}
            </dd>
          </Figure>
          {kind === "pay_down" ? (
            <Figure label="Paid off">
              <dd className="m-0 grid font-medium tabular-nums">
                {math.payoffMonth === null ? (
                  <span data-testid="payoff">—</span>
                ) : (
                  <time data-testid="payoff" dateTime={math.payoffMonth}>
                    {readoutLabel(math.payoffMonth)}
                  </time>
                )}
                {goal.planned_monthly === undefined ? (
                  <small className="text-xs font-normal text-muted-foreground">
                    set a planned payment to project it
                  </small>
                ) : null}
              </dd>
            </Figure>
          ) : null}
        </dl>
        {projection.length < 2 ? null : (
          <LineChart
            data={projection}
            x="month"
            series={[{ key: "owed", label: "Still owed" }]}
            title={`${goal.name}: what is still owed each month until the planned payment clears it`}
            height={160}
            legend
            formatValue={(value) => compactMoney(value, goal.currency)}
            formatX={monthTick}
          />
        )}
        {linked.length === 0 ? null : (
          <p className="m-0 text-sm text-muted-foreground" data-testid="linked">
            {kind === "pay_down" ? "Paying down" : "Linked to"} {linked.join(", ")}
          </p>
        )}

        <details className="group rounded-lg border">
          <summary className="flex cursor-pointer list-none items-center gap-2 px-4 py-2.5 text-sm font-medium text-muted-foreground select-none hover:text-foreground [&::-webkit-details-marker]:hidden">
            <ChevronRight
              aria-hidden="true"
              className="size-4 shrink-0 transition-transform group-open:rotate-90"
            />
            Contributions ({contributions.length}
            {contributions.length === 0
              ? ""
              : `, ${money(contributions.reduce((total, entry) => total + entry.amount, 0))}`}
            )
          </summary>
          <div className="grid gap-3 border-t px-4 py-3">
            {source === "balance" ? (
              <p className="m-0 text-sm text-muted-foreground">
                Progress follows the linked balance; contributions here are a record of what was put
                in, not what moves the bar.
              </p>
            ) : null}
            {contributions.length === 0 ? null : (
              <Table aria-label={`Contributions to ${goal.name}`}>
                <TableHeader>
                  <TableRow>
                    <TableHead scope="col" className="w-36">
                      Date
                    </TableHead>
                    <TableHead scope="col" className="w-32 text-right">
                      Amount
                    </TableHead>
                    <TableHead scope="col">Note</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {contributions.map((entry) => (
                    <TableRow key={entry.id} data-testid={`contribution-${entry.id}`}>
                      <TableCell className="tabular-nums">{entry.date}</TableCell>
                      <TableCell className="money">{money(entry.amount)}</TableCell>
                      <TableCell className="whitespace-normal">{entry.note ?? ""}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            )}
            <ContributionForm app={app} goal={goal} today={today} onProblem={onProblem} />
          </div>
        </details>
      </CardContent>

      <CardFooter className="flex-wrap gap-2">
        <Button variant="outline" size="sm" onClick={() => onEdit(!editing)}>
          <Pencil />
          {editing ? "Close" : "Edit"}
        </Button>
        {goal.status === "active" ? (
          <Button variant="ghost" size="sm" onClick={() => onStatus("completed")}>
            <Check />
            Complete
          </Button>
        ) : (
          <Button variant="ghost" size="sm" onClick={() => onStatus("active")}>
            <RotateCcw />
            Reopen
          </Button>
        )}
        {goal.status === "archived" ? null : (
          <Button variant="ghost" size="sm" onClick={() => onStatus("archived")}>
            <Archive />
            Archive
          </Button>
        )}
      </CardFooter>
      <Dialog open={editing} onOpenChange={onEdit}>
        <DialogContent className="sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>Edit {goal.name}</DialogTitle>
            <DialogDescription>
              Everything but its kind, which would change what its numbers mean.
            </DialogDescription>
          </DialogHeader>
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
        </DialogContent>
      </Dialog>
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

  const ids = {
    amount: `contribution-amount-${goal.id}`,
    date: `contribution-date-${goal.id}`,
    note: `contribution-note-${goal.id}`,
  };

  return (
    <form
      className="flex flex-wrap items-end gap-3"
      aria-label={`Add contribution to ${goal.name}`}
      onSubmit={(event) => void submit(event)}
    >
      <Field label={`Amount (${goal.currency})`} htmlFor={ids.amount} className="w-36">
        <Input
          id={ids.amount}
          name="amount"
          inputMode="decimal"
          required
          className="money"
          placeholder={amountToText(10_000, goal.currency)}
          value={amount}
          onChange={(event) => setAmount(event.target.value)}
        />
      </Field>
      <Field label="Date" htmlFor={ids.date} className="w-40">
        <Input
          id={ids.date}
          name="date"
          type="date"
          required
          value={date}
          onChange={(event) => setDate(event.target.value)}
        />
      </Field>
      <Field label="Note" htmlFor={ids.note} className="min-w-48 flex-1">
        <Input
          id={ids.note}
          name="note"
          maxLength={200}
          value={note}
          onChange={(event) => setNote(event.target.value)}
        />
      </Field>
      <Button type="submit" variant="secondary">
        Add contribution
      </Button>
    </form>
  );
}

/** The accounts a goal of this kind may follow: a debt is paid down on a liability. */
function eligibleAccounts(kind: GoalKind, accounts: readonly Account[]): readonly Account[] {
  return kind === "pay_down"
    ? accounts.filter((account) => LIABILITY_TYPES.includes(account.type))
    : accounts;
}

/**
 * A list of accounts to link, tall enough to see several at once. The kit's
 * select is dressed for one choice -- its chevron says "open me" -- so the
 * chevron is hidden here, where the whole list is already open.
 */
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
    <div className="[&_svg]:hidden">
      <NativeSelect
        id={id}
        name="account_ids"
        multiple
        className="h-auto min-h-28 py-1 pr-3 [&_option]:rounded-sm [&_option]:px-2 [&_option]:py-1"
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
      </NativeSelect>
    </div>
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
    <form className="grid gap-4" aria-label="New goal" onSubmit={(event) => void submit(event)}>
      <div className="grid gap-4 sm:grid-cols-2">
        <Field label="Kind" htmlFor="new-goal-kind">
          <NativeSelect
            id="new-goal-kind"
            name="kind"
            value={kind}
            onChange={(event) => setKind(event.target.value as GoalKind)}
          >
            <option value="save">Save up</option>
            <option value="pay_down">Pay down</option>
          </NativeSelect>
        </Field>
        <Field label="Name" htmlFor="new-goal-name">
          <Input
            id="new-goal-name"
            name="name"
            required
            maxLength={200}
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </Field>
        <Field label={`Target (${currency})`} htmlFor="new-goal-target">
          <Input
            id="new-goal-target"
            name="target_amount"
            inputMode="decimal"
            required={kind !== "pay_down"}
            className="money"
            placeholder={
              kind === "pay_down" && owed > 0
                ? amountToText(owed, currency)
                : amountToText(200_000, currency)
            }
            value={target}
            onChange={(event) => setTarget(event.target.value)}
          />
        </Field>
        <Field label="By" htmlFor="new-goal-target-date">
          <Input
            id="new-goal-target-date"
            name="target_date"
            type="date"
            value={targetDate}
            onChange={(event) => setTargetDate(event.target.value)}
          />
        </Field>
        <Field label={`Planned monthly (${currency})`} htmlFor="new-goal-planned">
          <Input
            id="new-goal-planned"
            name="planned_monthly"
            inputMode="decimal"
            className="money"
            placeholder={amountToText(25_000, currency)}
            value={plannedMonthly}
            onChange={(event) => setPlannedMonthly(event.target.value)}
          />
        </Field>
        <Field label="Accounts" htmlFor="new-goal-accounts">
          <AccountsSelect
            id="new-goal-accounts"
            accounts={eligible}
            value={linked}
            onChange={setAccountIds}
          />
        </Field>
        {kind === "pay_down" ? null : (
          <Field label="Progress" htmlFor="new-goal-progress">
            <NativeSelect
              id="new-goal-progress"
              name="progress_source"
              value={source}
              onChange={(event) => setSource(event.target.value as GoalProgressSource)}
            >
              <option value="contributions">By contributions I record</option>
              <option value="balance">Follows the linked balance</option>
            </NativeSelect>
          </Field>
        )}
      </div>
      <p className="m-0 text-sm text-muted-foreground">
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
      <DialogFooter>
        <Button variant="outline" onClick={onDone}>
          Cancel
        </Button>
        <Button type="submit">Add goal</Button>
      </DialogFooter>
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
  const ids = {
    name: `edit-goal-name-${goal.id}`,
    target: `edit-goal-target-${goal.id}`,
    targetDate: `edit-goal-target-date-${goal.id}`,
    planned: `edit-goal-planned-${goal.id}`,
    priority: `edit-goal-priority-${goal.id}`,
    accounts: `edit-goal-accounts-${goal.id}`,
    progress: `edit-goal-progress-${goal.id}`,
  };

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
      className="grid gap-4"
      aria-label={`Edit ${goal.name}`}
      onSubmit={(event) => void submit(event)}
    >
      <div className="grid gap-4 sm:grid-cols-2">
        <Field label="Name" htmlFor={ids.name}>
          <Input
            id={ids.name}
            name="name"
            required
            maxLength={200}
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </Field>
        <Field label={`Target (${goal.currency})`} htmlFor={ids.target}>
          <Input
            id={ids.target}
            name="target_amount"
            inputMode="decimal"
            required
            className="money"
            value={target}
            onChange={(event) => setTarget(event.target.value)}
          />
        </Field>
        <Field label="By" htmlFor={ids.targetDate}>
          <Input
            id={ids.targetDate}
            name="target_date"
            type="date"
            value={targetDate}
            onChange={(event) => setTargetDate(event.target.value)}
          />
        </Field>
        <Field label={`Planned monthly (${goal.currency})`} htmlFor={ids.planned}>
          <Input
            id={ids.planned}
            name="planned_monthly"
            inputMode="decimal"
            className="money"
            value={plannedMonthly}
            onChange={(event) => setPlannedMonthly(event.target.value)}
          />
        </Field>
        <Field label="Priority" htmlFor={ids.priority}>
          <Input
            id={ids.priority}
            name="priority"
            inputMode="numeric"
            className="tabular-nums"
            value={priority}
            onChange={(event) => setPriority(event.target.value)}
          />
        </Field>
        <Field label="Accounts" htmlFor={ids.accounts}>
          <AccountsSelect
            id={ids.accounts}
            accounts={eligible}
            value={linked}
            onChange={setAccountIds}
          />
        </Field>
        {kind === "pay_down" ? null : (
          <Field label="Progress" htmlFor={ids.progress}>
            <NativeSelect
              id={ids.progress}
              name="progress_source"
              value={source}
              onChange={(event) => setSource(event.target.value as GoalProgressSource)}
            >
              <option value="contributions">By contributions I record</option>
              <option value="balance">Follows the linked balance</option>
            </NativeSelect>
          </Field>
        )}
      </div>
      <DialogFooter>
        <Button variant="outline" onClick={onDone}>
          Cancel
        </Button>
        <Button type="submit">Save goal</Button>
      </DialogFooter>
    </form>
  );
}
