import {
  Alert,
  AlertDescription,
  Badge,
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  Field,
  Input,
  LineChart,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  cn,
} from "@mako-cloud/ui";
import { ChevronLeft } from "lucide-react";
import { type FormEvent, useId, useMemo, useState } from "react";

import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import { BALANCE_UPDATE_DESCRIPTION, ValidationError } from "../data/writes.js";
import {
  ACCOUNT_TYPE_LABELS,
  type Account,
  HOLDING_TYPES,
  type HouseholdCollectionId,
  isCategory,
  TRACKED_TYPES,
  type Transaction,
} from "../model/types.js";
import { accountBalance, holdingsValue } from "../selectors/balances.js";
import {
  type HistoryRangeKey,
  historyRange,
  selectAccountBalanceHistory,
} from "../selectors/history.js";
import { type HoldingRow, holdingGain, holdingValue } from "../selectors/holdings.js";
import { merchantResolver } from "../selectors/merchants.js";
import { amountToText, formatMinorUnits, parseAmount } from "../selectors/money.js";
import { sortTransactions } from "../selectors/transactions.js";
import {
  AccountForm,
  EditorDialog,
  HistoryRangePicker,
  Stat,
  compactMoney,
  memberLabel,
  shortDate,
  todayIso,
  useHouseholdMembers,
} from "./account-form.js";
import { HoldingForm, HoldingsTable } from "./holdings.js";
import { useBehavior, useQuery } from "./hooks.js";
import { routeHash, transactionsHash } from "./router.js";

/** How many of an account's transactions the page lists before pointing at the full list. */
const TRANSACTION_LIMIT = 100;

/** The table's cells sit flush with the card that holds it. */
const TABLE_IN_CARD =
  "[&_td:first-child]:pl-5 [&_td:last-child]:pr-5 [&_th:first-child]:pl-5 [&_th:last-child]:pr-5";

/**
 * One account: what it is, what it holds, how its balance has moved night by
 * night, and what happened in it. The balance is derived here exactly as the
 * accounts list derives it; the history is what the nightly snapshots
 * recorded of this account, never recomputed from today's transactions, so a
 * transaction edited last week does not rewrite last week.
 */
export function AccountScreen({
  app,
  session,
  currency,
  accountId,
}: {
  app: RationalApp;
  session: ScopeSession<HouseholdCollectionId>;
  currency: string;
  accountId: string;
}) {
  const state = useBehavior(app.state$);
  const accounts = useQuery(
    session.collection("accounts")?.find({ sort: [{ name: "asc" }] }) ?? null,
  );
  const account = accounts.find((candidate) => candidate.id === accountId);
  const transactions = useQuery(
    session.collection("transactions")?.find({ selector: { account_id: accountId } }) ?? null,
  );
  const snapshots = useQuery(session.collection("net_worth_snapshots")?.find() ?? null);
  const taxonomy = useQuery(session.collection("taxonomy")?.find() ?? null);
  const connections = useQuery(
    session.collection("connections")?.find({ selector: { account_id: accountId } }) ?? null,
  );
  const members = useHouseholdMembers(app);
  const [rangeKey, setRangeKey] = useState<HistoryRangeKey>("3M");
  const today = todayIso();
  const range = useMemo(() => historyRange(rangeKey, today), [rangeKey, today]);
  const history = selectAccountBalanceHistory(snapshots, accountId, range);
  const sorted = useMemo(() => sortTransactions(transactions), [transactions]);
  const resolveMerchant = useMemo(() => merchantResolver(taxonomy), [taxonomy]);
  const categoryNames = useMemo(
    () => new Map(taxonomy.filter(isCategory).map((category) => [category.id, category.name])),
    [taxonomy],
  );
  const balance = useMemo(
    () => (account === undefined ? 0 : accountBalance(account, transactions)),
    [account, transactions],
  );
  const holdingRows = useMemo<readonly HoldingRow[]>(
    () =>
      account === undefined
        ? []
        : (account.holdings ?? []).map((holding) => ({
            account,
            holding,
            value: holdingValue(holding),
            gain: holdingGain(holding),
          })),
    [account],
  );
  const [panel, setPanel] = useState<"edit" | "value" | "holding" | null>(null);
  const [editingHolding, setEditingHolding] = useState<HoldingRow | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const role = app.roleIn(state.currentHouseholdId);
  const canEdit = role === "owner" || role === "editor";

  if (account === undefined) {
    return (
      <section aria-labelledby="account-title" data-testid="account-screen" className="grid gap-4">
        <Breadcrumb />
        <h1 id="account-title" className="m-0 text-2xl font-semibold tracking-tight">
          Account
        </h1>
        {accounts.length === 0 ? (
          <p role="status" className="m-0 text-sm text-muted-foreground">
            Opening the account…
          </p>
        ) : (
          <p className="m-0 text-sm text-muted-foreground" data-testid="account-missing">
            There is no account with this id on this device. It may have been deleted, or belong to
            another space.
          </p>
        )}
      </section>
    );
  }

  const tracked = TRACKED_TYPES.includes(account.type);
  const holdsPositions = HOLDING_TYPES.includes(account.type);
  const hidden = account.hide_from_net_worth === true;
  const closed = account.closed_at !== undefined;
  const positionsValue = holdingsValue(account.holdings);
  const owner =
    account.owner_id === undefined
      ? null
      : (members.find((membership) => membership.user_id === account.owner_id) ?? null);
  const institutionConnections = connections.filter(
    (connection) => connection.kind === "institution" || connection.kind === "plaid",
  );
  const listed = sorted.slice(0, TRANSACTION_LIMIT);

  const act = async (action: () => Promise<unknown> | undefined) => {
    setProblem(null);
    try {
      await action();
    } catch (caught) {
      setProblem(caught instanceof Error ? caught.message : "That could not be saved.");
    }
  };

  return (
    <section aria-labelledby="account-title" data-testid="account-screen" className="grid gap-6">
      <div className="grid gap-3">
        <Breadcrumb />
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div className="grid gap-1">
            <h1 id="account-title" className="m-0 text-2xl font-semibold tracking-tight">
              {account.name}
            </h1>
            <p
              className="m-0 flex flex-wrap items-center text-sm text-muted-foreground [&>*+*]:before:mx-2 [&>*+*]:before:content-['·']"
              data-testid="account-meta"
            >
              <span data-testid="type">{ACCOUNT_TYPE_LABELS[account.type]}</span>
              {account.institution === undefined ? null : <span>{account.institution}</span>}
              <span>{account.currency}</span>
              {account.owner_id === undefined ? null : (
                <span data-testid="owner">
                  {owner === null ? account.owner_id : memberLabel(owner)}&apos;s
                </span>
              )}
              {hidden ? (
                <span>
                  <Badge variant="secondary">hidden from net worth</Badge>
                </span>
              ) : null}
              {closed ? (
                <span>
                  <Badge variant="outline">closed</Badge>
                </span>
              ) : null}
            </p>
          </div>
          {canEdit ? (
            <div className="flex flex-wrap items-center justify-end gap-2">
              <Button variant="outline" onClick={() => setPanel("edit")}>
                Edit
              </Button>
              <Button
                variant="outline"
                onClick={() => void act(() => app.writes?.setHideFromNetWorth(account.id, !hidden))}
              >
                {hidden ? "Unhide from net worth" : "Hide from net worth"}
              </Button>
              {tracked ? <Button onClick={() => setPanel("value")}>Update value</Button> : null}
              {holdsPositions ? (
                <Button
                  onClick={() => {
                    setEditingHolding(null);
                    setPanel("holding");
                  }}
                >
                  Add holding
                </Button>
              ) : null}
              <Button
                variant="outline"
                onClick={() =>
                  void act(() =>
                    closed
                      ? app.writes?.reopenAccount(account.id)
                      : app.writes?.closeAccount(account.id),
                  )
                }
              >
                {closed ? "Reopen account" : "Close account"}
              </Button>
            </div>
          ) : null}
        </div>
      </div>
      {problem === null ? null : (
        <Alert variant="destructive" role="alert">
          <AlertDescription>{problem}</AlertDescription>
        </Alert>
      )}

      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        <Stat
          data-testid="account-balance"
          label="Balance"
          value={formatMinorUnits(balance, account.currency)}
          valueTestId="balance"
          note={
            tracked
              ? "Tracked value, updated in place"
              : `Opened ${account.opening_date} at ${formatMinorUnits(
                  account.opening_balance,
                  account.currency,
                )}`
          }
        />
        {holdsPositions ? (
          <>
            <Stat
              label="Cash"
              value={formatMinorUnits(balance - positionsValue, account.currency)}
              valueTestId="cash"
            />
            <Stat
              label="Holdings"
              value={formatMinorUnits(positionsValue, account.currency)}
              valueTestId="holdings-value"
              note={`${holdingRows.length} ${holdingRows.length === 1 ? "position" : "positions"}`}
            />
          </>
        ) : null}
      </div>

      {panel === "edit" ? (
        <AccountForm
          app={app}
          account={account}
          defaultCurrency={currency}
          onDone={() => setPanel(null)}
        />
      ) : null}
      {panel === "value" && tracked ? (
        <UpdateValueForm
          app={app}
          account={account}
          current={balance}
          onDone={() => setPanel(null)}
        />
      ) : null}
      {panel === "holding" && holdsPositions ? (
        <HoldingForm
          key={editingHolding?.holding.id ?? "new"}
          app={app}
          accounts={[account]}
          accountId={account.id}
          holding={editingHolding}
          onDone={() => setPanel(null)}
        />
      ) : null}

      <Card>
        <CardHeader className="flex flex-wrap items-center justify-between gap-3">
          <CardTitle>Balance history</CardTitle>
          <HistoryRangePicker value={rangeKey} onChange={setRangeKey} />
        </CardHeader>
        <CardContent>
          {history.length === 0 ? (
            <p className="m-0 text-sm text-muted-foreground" data-testid="balance-history-empty">
              No snapshots for this account in this range. The nightly job records one a night, so
              the chart fills in from tomorrow.
            </p>
          ) : (
            <div className="grid gap-2" data-testid="balance-history">
              <LineChart
                data={history.map((point) => ({ date: point.date, balance: point.balance }))}
                x="date"
                series={[{ key: "balance", label: "Balance" }]}
                title={`Balance of ${account.name} over time`}
                height={200}
                formatValue={(value) => compactMoney(value, account.currency)}
                formatX={shortDate}
              />
              <p
                className="m-0 text-sm text-muted-foreground tabular-nums"
                data-testid="balance-history-change"
              >
                {balanceCaption(history, account.currency)}
              </p>
            </div>
          )}
        </CardContent>
      </Card>

      {holdsPositions ? (
        <Card className="gap-4 pb-0">
          <CardHeader>
            <CardTitle>Holdings</CardTitle>
          </CardHeader>
          <CardContent className="px-0">
            <HoldingsTable
              app={app}
              rows={holdingRows}
              showAccount={false}
              canEdit={canEdit}
              onEdit={(row) => {
                setEditingHolding(row);
                setPanel("holding");
              }}
              emptyMessage="No holdings yet. Add one and its value joins the balance."
            />
          </CardContent>
        </Card>
      ) : null}

      {institutionConnections.length === 0 ? null : (
        <Card className="gap-3">
          <CardHeader>
            <CardTitle>Connection</CardTitle>
          </CardHeader>
          <CardContent className="grid gap-2">
            {institutionConnections.map((connection) => (
              <p
                key={connection.id}
                className="m-0 text-sm text-muted-foreground"
                data-testid="connection-status"
              >
                Connected to {connection.institution ?? "an institution"} —{" "}
                <span data-testid="status">{connection.status ?? "connected"}</span>; last sync{" "}
                {connection.last_sync_at === undefined
                  ? "never"
                  : new Date(connection.last_sync_at).toLocaleString()}
                {connection.last_sync_outcome === undefined
                  ? ""
                  : ` (${connection.last_sync_outcome})`}
                .{" "}
                <a
                  className="text-primary"
                  href={routeHash({ name: "settings", page: "connections" })}
                >
                  Connections
                </a>
              </p>
            ))}
          </CardContent>
        </Card>
      )}

      <Card className="gap-4 pb-0">
        <CardHeader className="flex flex-wrap items-center justify-between gap-3">
          <CardTitle>{tracked ? "Balance updates" : "Transactions"}</CardTitle>
          <a className="text-sm text-primary" href={transactionsHash({ account: account.id })}>
            All transactions
          </a>
        </CardHeader>
        <CardContent className="px-0">
          <Table
            aria-label={tracked ? "Balance updates" : "Transactions"}
            className={TABLE_IN_CARD}
          >
            <TableHeader>
              <TableRow>
                <TableHead scope="col">Date</TableHead>
                <TableHead scope="col">Description</TableHead>
                <TableHead scope="col">Category</TableHead>
                <TableHead scope="col" className="money text-right">
                  Amount
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {listed.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={4} className="py-8 text-center text-muted-foreground">
                    {tracked ? "No balance updates yet." : "No transactions yet."}
                  </TableCell>
                </TableRow>
              ) : null}
              {listed.map((transaction) => (
                <TransactionRow
                  key={transaction.id}
                  transaction={transaction}
                  merchant={
                    transaction.adjustment === true
                      ? BALANCE_UPDATE_DESCRIPTION
                      : resolveMerchant(transaction).name
                  }
                  categoryName={categoryNames}
                />
              ))}
            </TableBody>
          </Table>
          {sorted.length > listed.length ? (
            <p className="m-0 px-5 py-3 text-sm text-muted-foreground">
              Showing the latest {listed.length} of {sorted.length}.{" "}
              <a className="text-primary" href={transactionsHash({ account: account.id })}>
                All transactions
              </a>
            </p>
          ) : null}
        </CardContent>
      </Card>
    </section>
  );
}

/** The way back to the list this account is one of. */
function Breadcrumb() {
  return (
    <nav aria-label="Breadcrumb" className="text-sm">
      <a
        className="inline-flex items-center gap-1 text-muted-foreground hover:text-foreground"
        href={routeHash({ name: "accounts" })}
      >
        <ChevronLeft aria-hidden="true" className="size-4" />
        Accounts
      </a>
    </nav>
  );
}

/** "12 snapshots, 2026-06-06 to 2026-08-15: up $1,200.00." -- or the one-snapshot form. */
function balanceCaption(
  history: ReadonlyArray<{ readonly date: string; readonly balance: number }>,
  currency: string,
): string {
  const first = history[0];
  const last = history[history.length - 1];
  if (first === undefined || last === undefined) return "";
  if (history.length === 1) return `One snapshot, ${formatMinorUnits(first.balance, currency)}.`;
  const change = last.balance - first.balance;
  return `${history.length} snapshots, ${first.date} to ${last.date}: ${
    change >= 0 ? "up" : "down"
  } ${formatMinorUnits(Math.abs(change), currency)}.`;
}

/**
 * The same row the transactions list draws: when, with whom, filed where,
 * how much. A balance update, a transfer leg, and a hidden transaction say
 * what they are, since none of them is money the household spent.
 */
function TransactionRow({
  transaction,
  merchant,
  categoryName,
}: {
  transaction: Transaction;
  merchant: string;
  categoryName: ReadonlyMap<string, string>;
}) {
  const needsReview =
    transaction.reviewed !== true &&
    (transaction.external_id !== undefined || transaction.import_batch_id !== undefined);
  const quiet = transaction.hidden === true || transaction.adjustment === true;
  return (
    <TableRow
      data-testid={`transaction-${transaction.id}`}
      data-description={transaction.description}
      className={quiet ? "text-muted-foreground" : undefined}
    >
      <TableCell className="tabular-nums">{transaction.date}</TableCell>
      <TableCell className="whitespace-normal">
        <span data-testid="description" className={cn(!quiet && "font-medium")}>
          {merchant}
        </span>
        {merchant === transaction.description ? null : (
          <small className="text-xs text-muted-foreground"> {transaction.description}</small>
        )}
        {transaction.notes === undefined ? null : (
          <small className="text-xs text-muted-foreground"> {transaction.notes}</small>
        )}
      </TableCell>
      <TableCell className="whitespace-normal">
        <span className="inline-flex flex-wrap items-center gap-1.5">
          {transaction.splits.length > 0
            ? "split"
            : transaction.category_id === undefined
              ? ""
              : (categoryName.get(transaction.category_id) ?? transaction.category_id)}
          {transaction.adjustment === true ? (
            <Badge variant="secondary">balance update</Badge>
          ) : null}
          {transaction.transfer_id === undefined ? null : (
            <Badge variant="secondary">transfer</Badge>
          )}
          {transaction.hidden === true && transaction.adjustment !== true ? (
            <Badge variant="secondary">hidden</Badge>
          ) : null}
          {needsReview ? <Badge variant="warning">needs review</Badge> : null}
        </span>
      </TableCell>
      <TableCell className="money" data-testid="amount">
        {formatMinorUnits(transaction.amount, transaction.currency)}
      </TableCell>
    </TableRow>
  );
}

/**
 * Move a tracked account to a new value. The difference is booked by the
 * write helper as a balance update -- a hidden, flagged transaction -- so the
 * account and net worth move while cash flow and budgets never see spending.
 */
function UpdateValueForm({
  app,
  account,
  current,
  onDone,
}: {
  app: RationalApp;
  account: Account;
  current: number;
  onDone: () => void;
}) {
  const id = useId();
  const [value, setValue] = useState(amountToText(current, account.currency));
  const [date, setDate] = useState(todayIso());
  const [error, setError] = useState<string | null>(null);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    const writes = app.writes;
    if (writes === null) return;
    try {
      await writes.updateTrackedBalance(account.id, parseAmount(value, account.currency), date);
      onDone();
    } catch (caught) {
      setError(
        caught instanceof ValidationError || caught instanceof RangeError
          ? caught.message
          : "The value could not be saved.",
      );
    }
  };

  return (
    <EditorDialog
      title="Update value"
      description={`The difference from ${formatMinorUnits(current, account.currency)} is booked as a balance update: it moves this account and net worth, and never counts as income or spending.`}
      formLabel="Update value"
      formTestId="update-value"
      submitLabel="Save value"
      onDone={onDone}
      onSubmit={(event) => void submit(event)}
    >
      <div className="grid gap-4 sm:grid-cols-2">
        <Field label={`New value (${account.currency})`} htmlFor={`${id}-value`}>
          <Input
            id={`${id}-value`}
            name="value"
            inputMode="decimal"
            required
            className="tabular-nums"
            value={value}
            onChange={(event) => setValue(event.target.value)}
          />
        </Field>
        <Field label="As of" htmlFor={`${id}-date`}>
          <Input
            id={`${id}-date`}
            name="date"
            type="date"
            required
            value={date}
            onChange={(event) => setDate(event.target.value)}
          />
        </Field>
      </div>
      {error === null ? null : (
        <p className="m-0 text-sm text-destructive" role="alert">
          {error}
        </p>
      )}
    </EditorDialog>
  );
}
