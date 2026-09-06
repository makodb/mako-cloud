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
import { AccountForm, HistoryRangePicker, todayIso } from "./account-form.js";
import { LineChart } from "./charts/index.js";
import { useBehavior, useQuery } from "./hooks.js";
import { routeHash } from "./router.js";
import "./styles/accounts.css";

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
    <section aria-labelledby="accounts-title">
      <div className="heading">
        <h1 id="accounts-title">Accounts</h1>
        {canEdit ? (
          <button type="button" onClick={() => setEditing("new")}>
            New account
          </button>
        ) : null}
      </div>
      <div className="totals" data-testid="net-worth">
        {netWorth.length === 0 ? (
          <div className="total" data-testid={`net-worth-${currency}`}>
            <span>Net worth ({currency})</span>
            <strong data-testid="net">{formatMinorUnits(0, currency)}</strong>
            <small>No open accounts count towards it yet.</small>
          </div>
        ) : (
          netWorth.map((total) => (
            <div key={total.currency} className="total" data-testid={`net-worth-${total.currency}`}>
              <span>Net worth ({total.currency})</span>
              <strong data-testid="net">{formatMinorUnits(total.netWorth, total.currency)}</strong>
              <small>
                assets {formatMinorUnits(total.assets, total.currency)} · liabilities{" "}
                {formatMinorUnits(total.liabilities, total.currency)}
              </small>
            </div>
          ))
        )}
      </div>

      <div className="section-heading">
        <h2>Net worth over time</h2>
        <HistoryRangePicker value={rangeKey} onChange={setRangeKey} />
      </div>
      {snapshots.length === 0 ? (
        <p className="muted" data-testid="net-worth-history-empty">
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
        <p className="empty-state" data-testid="accounts-empty">
          No accounts yet. Add one with &ldquo;New account&rdquo;, or{" "}
          <a href={routeHash({ name: "settings", page: "import" })}>import a statement</a> or{" "}
          <a href={routeHash({ name: "settings", page: "connections" })}>connect an institution</a>.
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
          caption={<div className="class-caption">Closed</div>}
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
      <p className="muted history" data-testid={`net-worth-history-${currency}`}>
        <span data-testid="net-worth-history-out-of-range">
          No snapshots in this range for {currency}. Choose a longer range.
        </span>
      </p>
    );
  }
  return (
    <div className="history" data-testid={`net-worth-history-${currency}`}>
      <LineChart
        points={history.points.map((point) => ({ x: point.date, y: point.netWorth }))}
        currency={currency}
        ariaLabel={`Net worth in ${currency} over time`}
        label={`Net worth (${currency})`}
        showArea
        height={180}
      />
      <p className="muted" data-testid="net-worth-change">
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
    <div className="class-caption">
      <span>{group.class.label}</span>
      <span className="subtotal" data-testid="subtotal" title="What this class adds to net worth">
        {text}
        {hiddenCount === 0 ? null : <small> · {hiddenCount} hidden</small>}
      </span>
    </div>
  );
}

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
    <table className="list" aria-label={`${label} accounts`}>
      <caption>{caption}</caption>
      <thead>
        <tr>
          <th scope="col">Name</th>
          <th scope="col">Type</th>
          <th scope="col">Institution</th>
          <th scope="col" className="amount">
            Balance
          </th>
          {canEdit ? (
            <th scope="col">
              <span className="visually-hidden">Actions</span>
            </th>
          ) : null}
        </tr>
      </thead>
      <tbody>
        {accounts.map((account) => (
          <tr
            key={account.id}
            data-testid={`account-${account.id}`}
            data-name={account.name}
            className={account.hide_from_net_worth === true ? "hidden-account" : undefined}
          >
            <td>
              <a href={routeHash({ name: "account", accountId: account.id })}>{account.name}</a>
              {account.hide_from_net_worth === true ? (
                <span className="chip" title="Hidden from net worth">
                  {" "}
                  hidden
                </span>
              ) : null}
            </td>
            <td>{ACCOUNT_TYPE_LABELS[account.type]}</td>
            <td>{account.institution ?? <span className="muted">—</span>}</td>
            <td className="amount" data-testid="balance">
              {formatMinorUnits(
                balances.get(account.id) ?? account.opening_balance,
                account.currency,
              )}
            </td>
            {canEdit ? (
              <td className="actions">
                <button type="button" className="link" onClick={() => onEdit(account)}>
                  Edit
                </button>
                {onClose === undefined ? null : (
                  <button type="button" className="link" onClick={() => onClose(account)}>
                    Close
                  </button>
                )}
                {onReopen === undefined ? null : (
                  <button type="button" className="link" onClick={() => onReopen(account)}>
                    Reopen
                  </button>
                )}
              </td>
            ) : null}
          </tr>
        ))}
      </tbody>
    </table>
  );
}
