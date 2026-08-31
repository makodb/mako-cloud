import { type FormEvent, useState } from "react";

import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import { ValidationError } from "../data/writes.js";
import {
  ACCOUNT_TYPES,
  type Account,
  type AccountType,
  type HouseholdCollectionId,
} from "../model/types.js";
import { selectAccountBalances, selectNetWorth } from "../selectors/balances.js";
import { amountToText, formatMinorUnits, parseAmount } from "../selectors/money.js";
import { useQuery } from "./hooks.js";
import { routeHash } from "./router.js";

export function AccountsScreen({
  app,
  session,
  currency,
}: {
  app: RationalApp;
  session: ScopeSession<HouseholdCollectionId>;
  currency: string;
}) {
  const accounts = useQuery(
    session.collection("accounts")?.find({ sort: [{ name: "asc" }] }) ?? null,
  );
  const transactions = useQuery(session.collection("transactions")?.find() ?? null);
  const balances = selectAccountBalances(accounts, transactions);
  const netWorth = selectNetWorth(accounts, balances);
  const [editing, setEditing] = useState<Account | "new" | null>(null);
  const open = accounts.filter((account) => account.closed_at === undefined);
  const closed = accounts.filter((account) => account.closed_at !== undefined);

  return (
    <section aria-labelledby="accounts-title">
      <div className="heading">
        <h1 id="accounts-title">Accounts</h1>
        <button type="button" onClick={() => setEditing("new")}>
          New account
        </button>
      </div>
      <div className="totals" data-testid="net-worth">
        {netWorth.map((total) => (
          <div key={total.currency} className="total">
            <span>Net worth ({total.currency})</span>
            <strong>{formatMinorUnits(total.netWorth, total.currency)}</strong>
            <small>
              assets {formatMinorUnits(total.assets, total.currency)} · liabilities{" "}
              {formatMinorUnits(total.liabilities, total.currency)}
            </small>
          </div>
        ))}
      </div>
      {editing === null ? null : (
        <AccountForm
          app={app}
          account={editing === "new" ? null : editing}
          defaultCurrency={currency}
          onDone={() => setEditing(null)}
        />
      )}
      <AccountTable
        title="Open"
        accounts={open}
        balances={balances}
        onEdit={setEditing}
        onClose={(account) => void app.writes?.closeAccount(account.id)}
      />
      {closed.length === 0 ? null : (
        <AccountTable
          title="Closed"
          accounts={closed}
          balances={balances}
          onEdit={setEditing}
          onReopen={(account) => void app.writes?.reopenAccount(account.id)}
        />
      )}
    </section>
  );
}

function AccountTable({
  title,
  accounts,
  balances,
  onEdit,
  onClose,
  onReopen,
}: {
  title: string;
  accounts: readonly Account[];
  balances: ReadonlyMap<string, number>;
  onEdit: (account: Account) => void;
  onClose?: (account: Account) => void;
  onReopen?: (account: Account) => void;
}) {
  return (
    <table className="list" aria-label={`${title} accounts`}>
      <caption>{title}</caption>
      <thead>
        <tr>
          <th scope="col">Name</th>
          <th scope="col">Type</th>
          <th scope="col">Currency</th>
          <th scope="col" className="amount">
            Balance
          </th>
          <th scope="col">
            <span className="visually-hidden">Actions</span>
          </th>
        </tr>
      </thead>
      <tbody>
        {accounts.length === 0 ? (
          <tr>
            <td colSpan={5} className="empty">
              No accounts yet.
            </td>
          </tr>
        ) : null}
        {accounts.map((account) => (
          <tr key={account.id} data-testid={`account-${account.id}`} data-name={account.name}>
            <td>
              <a href={routeHash({ name: "transactions", accountId: account.id })}>
                {account.name}
              </a>
              {account.institution === undefined ? null : <small> {account.institution}</small>}
            </td>
            <td>{account.type}</td>
            <td>{account.currency}</td>
            <td className="amount" data-testid="balance">
              {formatMinorUnits(
                balances.get(account.id) ?? account.opening_balance,
                account.currency,
              )}
            </td>
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
          </tr>
        ))}
      </tbody>
    </table>
  );
}

function AccountForm({
  app,
  account,
  defaultCurrency,
  onDone,
}: {
  app: RationalApp;
  account: Account | null;
  defaultCurrency: string;
  onDone: () => void;
}) {
  const [name, setName] = useState(account?.name ?? "");
  const [type, setType] = useState<AccountType>(account?.type ?? "checking");
  const [currency, setCurrency] = useState(account?.currency ?? defaultCurrency);
  const [openingBalance, setOpeningBalance] = useState(
    account === null ? "0.00" : amountToText(account.opening_balance, account.currency),
  );
  const [openingDate, setOpeningDate] = useState(
    account?.opening_date ?? new Date().toISOString().slice(0, 10),
  );
  const [institution, setInstitution] = useState(account?.institution ?? "");
  const [error, setError] = useState<string | null>(null);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    const writes = app.writes;
    if (writes === null) return;
    try {
      const upperCurrency = currency.trim().toUpperCase();
      const balance = parseAmount(openingBalance, upperCurrency);
      if (account === null) {
        await writes.createAccount({
          name,
          type,
          currency: upperCurrency,
          opening_balance: balance,
          opening_date: openingDate,
          institution,
        });
      } else {
        await writes.updateAccount(account.id, {
          name,
          type,
          currency: upperCurrency,
          opening_balance: balance,
          opening_date: openingDate,
          institution: institution.trim() === "" ? null : institution.trim(),
        });
      }
      onDone();
    } catch (caught) {
      setError(
        caught instanceof ValidationError || caught instanceof RangeError
          ? caught.message
          : "The account could not be saved.",
      );
    }
  };

  return (
    <form className="editor" onSubmit={(event) => void submit(event)} aria-label="Account editor">
      <h2>{account === null ? "New account" : `Edit ${account.name}`}</h2>
      <div className="grid">
        <label>
          Name
          <input
            name="name"
            required
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </label>
        <label>
          Type
          <select
            name="type"
            value={type}
            onChange={(event) => setType(event.target.value as AccountType)}
          >
            {ACCOUNT_TYPES.map((candidate) => (
              <option key={candidate} value={candidate}>
                {candidate}
              </option>
            ))}
          </select>
        </label>
        <label>
          Currency
          <input
            name="currency"
            required
            maxLength={3}
            value={currency}
            onChange={(event) => setCurrency(event.target.value)}
          />
        </label>
        <label>
          Opening balance
          <input
            name="opening_balance"
            inputMode="decimal"
            value={openingBalance}
            onChange={(event) => setOpeningBalance(event.target.value)}
          />
        </label>
        <label>
          Opening date
          <input
            name="opening_date"
            type="date"
            required
            value={openingDate}
            onChange={(event) => setOpeningDate(event.target.value)}
          />
        </label>
        <label>
          Institution
          <input
            name="institution"
            value={institution}
            onChange={(event) => setInstitution(event.target.value)}
          />
        </label>
      </div>
      {error === null ? null : (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      <div className="actions">
        <button type="submit">Save account</button>
        <button type="button" className="secondary" onClick={onDone}>
          Cancel
        </button>
      </div>
    </form>
  );
}
