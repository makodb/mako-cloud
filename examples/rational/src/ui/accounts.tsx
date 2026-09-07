import {
  AreaChart,
  Badge,
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  Table,
  TableBody,
  TableCaption,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@mako-cloud/ui";
import { Plus } from "lucide-react";
import { type ReactNode, useMemo, useState } from "react";

import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import {
  ACCOUNT_TYPE_LABELS,
  type Account,
  type HouseholdCollectionId,
  type NetWorthSnapshot,
} from "../model/types.js";
import {
  type AccountClassGroup,
  type CurrencyTotal,
  selectAccountBalances,
  selectAccountsByClass,
  selectNetWorth,
  totalsByCurrency,
} from "../selectors/balances.js";
import type { DateRange } from "../selectors/cashflow.js";
import {
  type HistoryRangeKey,
  historyRange,
  selectNetWorthHistoryInRange,
} from "../selectors/history.js";
import { formatMinorUnits } from "../selectors/money.js";
import type { NetWorthHistory } from "../selectors/reports.js";
import {
  AccountForm,
  HistoryRangePicker,
  PageHeader,
  Stat,
  compactMoney,
  shortDate,
  todayIso,
} from "./account-form.js";
import { useBehavior, useQuery } from "./hooks.js";
import { routeHash } from "./router.js";

/**
 * Every account the household has, grouped by class with what each class adds
 * up to, under the net worth they make and how it has moved. Balances are
 * derived on the device from the accounts and their transactions -- the same
 * derivation the nightly snapshot uses -- so the number at the top and the
 * rows below it cannot disagree; the history is the snapshots' own.
 */
export function AccountsScreen({
  app,
  session,
  currency,
}: {
  app: RationalApp;
  session: ScopeSession<HouseholdCollectionId>;
  currency: string;
}) {
  const state = useBehavior(app.state$);
  const accounts = useQuery(
    session.collection("accounts")?.find({ sort: [{ name: "asc" }] }) ?? null,
  );
  const transactions = useQuery(session.collection("transactions")?.find() ?? null);
  const snapshots = useQuery(session.collection("net_worth_snapshots")?.find() ?? null);
  const balances = selectAccountBalances(accounts, transactions);
  const netWorth = selectNetWorth(accounts, balances);
  const open = useMemo(
    () => accounts.filter((account) => account.closed_at === undefined),
    [accounts],
  );
  const closed = useMemo(
    () => accounts.filter((account) => account.closed_at !== undefined),
    [accounts],
  );
  const groups = selectAccountsByClass(open, balances);
  // The class subtotal is what the class contributes to net worth, so an
  // account hidden from net worth is left out of it as the spec asks; the
  // selector's own subtotal counts every member, which the caption notes.
  const shown = useMemo(
    () =>
      groups.map((group) => ({
        group,
        counted: totalsByCurrency(
          group.accounts.filter((account) => account.hide_from_net_worth !== true),
          balances,
        ),
      })),
    [groups, balances],
  );
  const [rangeKey, setRangeKey] = useState<HistoryRangeKey>("3M");
  const today = todayIso();
  const range = useMemo(() => historyRange(rangeKey, today), [rangeKey, today]);
  const currencies = useMemo(
    () => [...new Set(snapshots.map((snapshot) => snapshot.currency))].sort(),
    [snapshots],
  );
  const [editing, setEditing] = useState<Account | "new" | null>(null);
  const role = app.roleIn(state.currentHouseholdId);
  const canEdit = role === "owner" || role === "editor";

  return (
    <section aria-labelledby="accounts-title" className="grid gap-6">
      <PageHeader
        id="accounts-title"
        title="Accounts"
        subtitle="Every account the household has, by class, and the net worth they make together."
        actions={
          canEdit ? (
            <Button onClick={() => setEditing("new")}>
              <Plus aria-hidden="true" />
              New account
            </Button>
          ) : undefined
        }
      />
      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3" data-testid="net-worth">
        {netWorth.length === 0 ? (
          <Stat
            data-testid={`net-worth-${currency}`}
            label={`Net worth (${currency})`}
            value={formatMinorUnits(0, currency)}
            valueTestId="net"
            note="No open accounts count towards it yet."
          />
        ) : (
          netWorth.map((total) => (
            <Stat
              key={total.currency}
              data-testid={`net-worth-${total.currency}`}
              label={`Net worth (${total.currency})`}
              value={formatMinorUnits(total.netWorth, total.currency)}
              valueTestId="net"
              note={
                <>
                  assets {formatMinorUnits(total.assets, total.currency)} · liabilities{" "}
                  {formatMinorUnits(total.liabilities, total.currency)}
                </>
              }
            />
          ))
        )}
      </div>

      <Card>
        <CardHeader className="flex flex-wrap items-center justify-between gap-3">
          <CardTitle>Net worth over time</CardTitle>
          <HistoryRangePicker value={rangeKey} onChange={setRangeKey} />
        </CardHeader>
        <CardContent className="grid gap-6">
          {snapshots.length === 0 ? (
            <p className="m-0 text-sm text-muted-foreground" data-testid="net-worth-history-empty">
              No snapshots yet. The nightly job records one a night, so this fills in from tomorrow.
            </p>
          ) : (
            currencies.map((snapshotCurrency) => (
              <NetWorthHistoryChart
                key={snapshotCurrency}
                snapshots={snapshots}
                currency={snapshotCurrency}
                range={range}
              />
            ))
          )}
        </CardContent>
      </Card>

      {editing === null ? null : (
        <AccountForm
          key={editing === "new" ? "new" : editing.id}
          app={app}
          account={editing === "new" ? null : editing}
          defaultCurrency={currency}
          onDone={() => setEditing(null)}
        />
      )}

      {open.length === 0 ? (
        <p
          className="m-0 rounded-xl border border-dashed px-6 py-10 text-center text-sm text-muted-foreground"
          data-testid="accounts-empty"
        >
          No accounts yet. Add one with &ldquo;New account&rdquo;, or{" "}
          <a className="text-primary" href={routeHash({ name: "settings", page: "import" })}>
            import a statement
          </a>{" "}
          or{" "}
          <a className="text-primary" href={routeHash({ name: "settings", page: "connections" })}>
            connect an institution
          </a>
          .
        </p>
      ) : null}
      {shown.map(({ group, counted }) => (
        <AccountTable
          key={group.class.id}
          label={group.class.label}
          caption={
            <ClassCaption
              group={group}
              counted={counted}
              hiddenCount={
                group.accounts.filter((account) => account.hide_from_net_worth === true).length
              }
            />
          }
          accounts={group.accounts}
          balances={balances}
          canEdit={canEdit}
          onEdit={setEditing}
          onClose={(account) => void app.writes?.closeAccount(account.id)}
        />
      ))}
      {closed.length === 0 ? null : (
        <AccountTable
          label="Closed"
          caption={<span className="font-semibold text-foreground">Closed</span>}
          accounts={closed}
          balances={balances}
          canEdit={canEdit}
          onEdit={setEditing}
          onReopen={(account) => void app.writes?.reopenAccount(account.id)}
        />
      )}
    </section>
  );
}

/**
 * One currency's net worth over the chosen range, with the change spelled out
 * under the line -- the caption is what a screen reader and the nightly test
 * read, so it names the count, the span, and the direction in words.
 */
function NetWorthHistoryChart({
  snapshots,
  currency,
  range,
}: {
  snapshots: readonly NetWorthSnapshot[];
  currency: string;
  range: DateRange | null;
}) {
  const history = selectNetWorthHistoryInRange(snapshots, currency, range);
  if (history === null) {
    return (
      <p
        className="m-0 text-sm text-muted-foreground"
        data-testid={`net-worth-history-${currency}`}
      >
        <span data-testid="net-worth-history-out-of-range">
          No snapshots in this range for {currency}. Choose a longer range.
        </span>
      </p>
    );
  }
  return (
    <div className="grid gap-2" data-testid={`net-worth-history-${currency}`}>
      <AreaChart
        data={history.points.map((point) => ({ date: point.date, netWorth: point.netWorth }))}
        x="date"
        series={[{ key: "netWorth", label: `Net worth (${currency})` }]}
        title={`Net worth in ${currency} over time`}
        height={200}
        formatValue={(value) => compactMoney(value, currency)}
        formatX={shortDate}
      />
      <p className="m-0 text-sm text-muted-foreground tabular-nums" data-testid="net-worth-change">
        {historyCaption(history)}
      </p>
    </div>
  );
}

/** "3 snapshots, 2026-08-28 to 2026-08-30: up $200.00." -- or the one-snapshot form. */
export function historyCaption(history: NetWorthHistory): string {
  const first = history.points[0];
  const last = history.points[history.points.length - 1];
  if (history.points.length === 1 || first === undefined || last === undefined) {
    return `One snapshot, ${formatMinorUnits(first?.netWorth ?? 0, history.currency)}.`;
  }
  return `${history.points.length} snapshots, ${first.date} to ${last.date}: ${
    history.change >= 0 ? "up" : "down"
  } ${formatMinorUnits(Math.abs(history.change), history.currency)}.`;
}

function ClassCaption({
  group,
  counted,
  hiddenCount,
}: {
  group: AccountClassGroup;
  counted: readonly CurrencyTotal[];
  hiddenCount: number;
}) {
  const fallbackCurrency = group.accounts[0]?.currency ?? "USD";
  const text =
    counted.length === 0
      ? formatMinorUnits(0, fallbackCurrency)
      : counted.map((total) => formatMinorUnits(total.total, total.currency)).join(" · ");
  return (
    <>
      <span className="font-semibold text-foreground">{group.class.label}</span>
      <span
        className="money font-medium text-muted-foreground"
        data-testid="subtotal"
        title="What this class adds to net worth"
      >
        {text}
        {hiddenCount === 0 ? null : (
          <small className="text-xs font-normal"> · {hiddenCount} hidden</small>
        )}
      </span>
    </>
  );
}

/** The table's cells sit flush with the card that holds it. */
const TABLE_IN_CARD =
  "caption-top [&_td:first-child]:pl-5 [&_td:last-child]:pr-5 [&_th:first-child]:pl-5 [&_th:last-child]:pr-5";

function AccountTable({
  label,
  caption,
  accounts,
  balances,
  canEdit,
  onEdit,
  onClose,
  onReopen,
}: {
  label: string;
  caption: ReactNode;
  accounts: readonly Account[];
  balances: ReadonlyMap<string, number>;
  canEdit: boolean;
  onEdit: (account: Account) => void;
  onClose?: (account: Account) => void;
  onReopen?: (account: Account) => void;
}) {
  return (
    <Card className="gap-0 overflow-hidden py-0">
      <Table aria-label={`${label} accounts`} className={TABLE_IN_CARD}>
        <TableCaption className="mt-0 px-5 pt-4 pb-3 text-left text-sm">
          <div className="flex items-baseline justify-between gap-4">{caption}</div>
        </TableCaption>
        <TableHeader>
          <TableRow>
            <TableHead scope="col">Name</TableHead>
            <TableHead scope="col">Type</TableHead>
            <TableHead scope="col">Institution</TableHead>
            <TableHead scope="col" className="money text-right">
              Balance
            </TableHead>
            {canEdit ? (
              <TableHead scope="col" className="text-right">
                <span className="sr-only">Actions</span>
              </TableHead>
            ) : null}
          </TableRow>
        </TableHeader>
        <TableBody>
          {accounts.map((account) => (
            <TableRow
              key={account.id}
              data-testid={`account-${account.id}`}
              data-name={account.name}
              className={account.hide_from_net_worth === true ? "text-muted-foreground" : undefined}
            >
              <TableCell>
                <span className="inline-flex items-center gap-2">
                  <a
                    href={routeHash({ name: "account", accountId: account.id })}
                    className={
                      account.hide_from_net_worth === true
                        ? "text-muted-foreground"
                        : "font-medium text-primary"
                    }
                  >
                    {account.name}
                  </a>
                  {account.hide_from_net_worth === true ? (
                    <Badge variant="secondary" title="Hidden from net worth">
                      hidden
                    </Badge>
                  ) : null}
                </span>
              </TableCell>
              <TableCell>{ACCOUNT_TYPE_LABELS[account.type]}</TableCell>
              <TableCell>
                {account.institution ?? <span className="text-muted-foreground">—</span>}
              </TableCell>
              <TableCell className="money font-medium" data-testid="balance">
                {formatMinorUnits(
                  balances.get(account.id) ?? account.opening_balance,
                  account.currency,
                )}
              </TableCell>
              {canEdit ? (
                <TableCell className="text-right">
                  <span className="inline-flex items-center justify-end gap-1">
                    <Button
                      variant="link"
                      size="sm"
                      className="h-auto px-1"
                      onClick={() => onEdit(account)}
                    >
                      Edit
                    </Button>
                    {onClose === undefined ? null : (
                      <Button
                        variant="link"
                        size="sm"
                        className="h-auto px-1"
                        onClick={() => onClose(account)}
                      >
                        Close
                      </Button>
                    )}
                    {onReopen === undefined ? null : (
                      <Button
                        variant="link"
                        size="sm"
                        className="h-auto px-1"
                        onClick={() => onReopen(account)}
                      >
                        Reopen
                      </Button>
                    )}
                  </span>
                </TableCell>
              ) : null}
            </TableRow>
          ))}
        </TableBody>
      </Table>
    </Card>
  );
}
