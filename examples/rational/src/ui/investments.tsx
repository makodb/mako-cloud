import { useMemo, useState } from "react";

import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import {
  ACCOUNT_TYPE_LABELS,
  type Account,
  HOLDING_TYPES,
  type HouseholdCollectionId,
} from "../model/types.js";
import { holdingsValue, selectAccountBalances } from "../selectors/balances.js";
import {
  allocationByClass,
  type HoldingRow,
  selectHouseholdHoldings,
  selectInvestmentsTotal,
} from "../selectors/holdings.js";
import { formatMinorUnits } from "../selectors/money.js";
import { Donut } from "./charts/index.js";
import { ASSET_CLASS_LABELS, HoldingForm, HoldingsTable } from "./holdings.js";
import { useBehavior, useQuery } from "./hooks.js";
import { routeHash } from "./router.js";
import "./styles/accounts.css";

/**
 * Every position the household holds, across its investment and crypto
 * accounts: what they are worth together, how they divide by asset class,
 * and each one's gain against what was paid. The valuation is the shared
 * engine's, so the total here is the sum of the same account balances the
 * accounts page shows, and a price updated here moves both at once.
 */
export function InvestmentsScreen({
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
  const balances = selectAccountBalances(accounts, transactions);
  const totals = selectInvestmentsTotal(accounts, balances);
  const rows = selectHouseholdHoldings(accounts);
  const investmentAccounts = useMemo(
    () =>
      accounts.filter(
        (account) => account.closed_at === undefined && HOLDING_TYPES.includes(account.type),
      ),
    [accounts],
  );
  const [editing, setEditing] = useState<{
    readonly accountId: string;
    readonly holding: HoldingRow | null;
  } | null>(null);
  const role = app.roleIn(state.currentHouseholdId);
  const canEdit = role === "owner" || role === "editor";
  const firstAccount = investmentAccounts[0];

  return (
    <section aria-labelledby="investments-title" data-testid="investments-screen">
      <div className="heading">
        <h1 id="investments-title">Investments</h1>
        {canEdit && firstAccount !== undefined ? (
          <button
            type="button"
            onClick={() => setEditing({ accountId: firstAccount.id, holding: null })}
          >
            Add holding
          </button>
        ) : null}
      </div>

      <div className="totals" data-testid="investments-total">
        {totals.length === 0 ? (
          <div className="total" data-testid={`investments-total-${currency}`}>
            <span>Investments ({currency})</span>
            <strong data-testid="total">{formatMinorUnits(0, currency)}</strong>
            <small>No investment or crypto accounts yet.</small>
          </div>
        ) : (
          totals.map((total) => (
            <div
              key={total.currency}
              className="total"
              data-testid={`investments-total-${total.currency}`}
            >
              <span>Investments ({total.currency})</span>
              <strong data-testid="total">{formatMinorUnits(total.total, total.currency)}</strong>
              <small>
                {countText(
                  investmentAccounts.filter((account) => account.currency === total.currency)
                    .length,
                  "account",
                )}{" "}
                ·{" "}
                {countText(
                  rows.filter((row) => row.account.currency === total.currency).length,
                  "position",
                )}
              </small>
            </div>
          ))
        )}
      </div>

      {editing === null ? null : (
        <HoldingForm
          key={editing.holding?.holding.id ?? `new:${editing.accountId}`}
          app={app}
          accounts={investmentAccounts}
          accountId={editing.accountId}
          holding={editing.holding}
          onDone={() => setEditing(null)}
        />
      )}

      {investmentAccounts.length === 0 ? (
        <p className="empty-state" data-testid="investments-empty">
          No investment or crypto account yet. Add one on the{" "}
          <a href={routeHash({ name: "accounts" })}>Accounts</a> page and its positions show up
          here.
        </p>
      ) : (
        <>
          <h2>Allocation</h2>
          {totals.map((total) => (
            <Allocation key={total.currency} rows={rows} currency={total.currency} />
          ))}

          <h2>Investment accounts</h2>
          <table className="list" aria-label="Investment accounts">
            <thead>
              <tr>
                <th scope="col">Account</th>
                <th scope="col">Type</th>
                <th scope="col">Institution</th>
                <th scope="col" className="amount">
                  Positions
                </th>
                <th scope="col" className="amount">
                  Holdings
                </th>
                <th scope="col" className="amount">
                  Cash
                </th>
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
              {investmentAccounts.map((account) => (
                <InvestmentAccountRow
                  key={account.id}
                  account={account}
                  balance={balances.get(account.id) ?? account.opening_balance}
                  canEdit={canEdit}
                  onAdd={() => setEditing({ accountId: account.id, holding: null })}
                />
              ))}
            </tbody>
          </table>

          <h2>Holdings</h2>
          <HoldingsTable
            app={app}
            rows={rows}
            showAccount
            canEdit={canEdit}
            onEdit={(row) => setEditing({ accountId: row.account.id, holding: row })}
            emptyMessage="No holdings yet. Add one to an investment or crypto account."
          />
        </>
      )}
    </section>
  );
}

/**
 * How one currency's positions divide by asset class. Nothing is converted,
 * so a household with a brokerage in two currencies gets two rings rather
 * than one that adds unlike things.
 */
function Allocation({ rows, currency }: { rows: readonly HoldingRow[]; currency: string }) {
  const slices = useMemo(() => allocationByClass(rows, currency), [rows, currency]);
  return (
    <div className="history allocation" data-testid={`allocation-${currency}`}>
      <Donut
        slices={slices.map((slice) => ({
          key: slice.asset_class,
          label: ASSET_CLASS_LABELS[slice.asset_class],
          value: slice.value,
        }))}
        currency={currency}
        centerLabel="by asset class"
        ariaLabel={`Allocation of ${currency} holdings by asset class: ${slices
          .map(
            (slice) => `${ASSET_CLASS_LABELS[slice.asset_class]} ${Math.round(slice.share * 100)}%`,
          )
          .join(", ")}`}
        emptyMessage={`No positions in ${currency} yet.`}
      />
    </div>
  );
}

function InvestmentAccountRow({
  account,
  balance,
  canEdit,
  onAdd,
}: {
  account: Account;
  balance: number;
  canEdit: boolean;
  onAdd: () => void;
}) {
  const positions = holdingsValue(account.holdings);
  return (
    <tr data-testid={`investment-account-${account.id}`} data-name={account.name}>
      <td>
        <a href={routeHash({ name: "account", accountId: account.id })}>{account.name}</a>
      </td>
      <td>{ACCOUNT_TYPE_LABELS[account.type]}</td>
      <td>{account.institution ?? <span className="muted">—</span>}</td>
      <td className="amount">{account.holdings?.length ?? 0}</td>
      <td className="amount" data-testid="holdings-value">
        {formatMinorUnits(positions, account.currency)}
      </td>
      <td className="amount" data-testid="cash">
        {formatMinorUnits(balance - positions, account.currency)}
      </td>
      <td className="amount" data-testid="balance">
        {formatMinorUnits(balance, account.currency)}
      </td>
      {canEdit ? (
        <td className="actions">
          <button type="button" className="link" onClick={onAdd}>
            Add holding
          </button>
        </td>
      ) : null}
    </tr>
  );
}

function countText(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}
