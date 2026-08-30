import { type ChangeEvent, type FormEvent, useCallback, useEffect, useState } from "react";

import type { RationalApp } from "../data/rational.js";
import type { Receipt } from "../data/receipts.js";
import type { ScopeSession } from "../data/scope.js";
import { ValidationError } from "../data/writes.js";
import type { HouseholdCollectionId, Split, Transaction } from "../model/types.js";
import { amountToText, formatMinorUnits, parseAmount } from "../selectors/money.js";
import { validateSplits } from "../selectors/splits.js";
import {
  selectAvailableMonths,
  selectFilteredTransactions,
  sumAmounts,
} from "../selectors/transactions.js";
import { useQuery } from "./hooks.js";
import { type Route, routeHash } from "./router.js";

export function TransactionsScreen({
  app,
  session,
  route,
  currency,
}: {
  app: RationalApp;
  session: ScopeSession<HouseholdCollectionId>;
  route: Extract<Route, { name: "transactions" }>;
  currency: string;
}) {
  const accounts = useQuery(
    session.collection("accounts")?.find({ sort: [{ name: "asc" }] }) ?? null,
  );
  const categories = useQuery(
    session
      .collection("taxonomy")
      ?.find({ selector: { kind: "category" }, sort: [{ name: "asc" }] }) ?? null,
  );
  const tags = useQuery(
    session.collection("taxonomy")?.find({ selector: { kind: "tag" }, sort: [{ name: "asc" }] }) ??
      null,
  );
  const rules = useQuery(session.collection("rules")?.find() ?? null);
  const all = useQuery(session.collection("transactions")?.find() ?? null);
  const [attaching, setAttaching] = useState<Transaction | null>(null);
  // A receipt outlives its transaction unless something removes it: the
  // bucket knows nothing about the document that referred to it.
  const deleteWithReceipts = async (id: string) => {
    await app.receipts?.removeAll(id).catch(() => undefined);
    await app.writes?.deleteTransaction(id);
  };
  const months = selectAvailableMonths(all);
  const filter = {
    ...(route.accountId === undefined ? {} : { accountId: route.accountId }),
    ...(route.month === undefined ? {} : { month: route.month }),
  };
  const transactions = selectFilteredTransactions(all, filter);
  const [editing, setEditing] = useState<Transaction | "new" | null>(null);
  const categoryName = (id: string | undefined) =>
    id === undefined ? "" : (categories.find((category) => category.id === id)?.name ?? id);
  /**
   * Which rule filed a transaction. A category that appeared without anybody
   * choosing it should say where it came from -- otherwise the household
   * cannot tell an automatic filing from its own, and cannot find the rule to
   * change when the filing is wrong.
   */
  const ruleName = (id: string | undefined) =>
    id === undefined ? null : (rules.find((rule) => rule.id === id)?.name ?? "a deleted rule");
  const tagName = (id: string) => tags.find((tag) => tag.id === id)?.name ?? id;
  const accountName = (id: string) => accounts.find((account) => account.id === id)?.name ?? id;
  const total = sumAmounts(transactions);
  const listCurrency =
    route.accountId === undefined
      ? currency
      : (accounts.find((account) => account.id === route.accountId)?.currency ?? currency);

  return (
    <section aria-labelledby="transactions-title">
      <div className="heading">
        <h1 id="transactions-title">Transactions</h1>
        <button type="button" onClick={() => setEditing("new")} disabled={accounts.length === 0}>
          New transaction
        </button>
      </div>
      <div className="filters">
        <label>
          Account
          <select
            aria-label="Filter by account"
            value={route.accountId ?? ""}
            onChange={(event) => {
              const accountId = event.target.value;
              window.location.hash = routeHash({
                name: "transactions",
                ...(accountId === "" ? {} : { accountId }),
                ...(route.month === undefined ? {} : { month: route.month }),
              });
            }}
          >
            <option value="">All accounts</option>
            {accounts.map((account) => (
              <option key={account.id} value={account.id}>
                {account.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          Month
          <select
            aria-label="Filter by month"
            value={route.month ?? ""}
            onChange={(event) => {
              const month = event.target.value;
              window.location.hash = routeHash({
                name: "transactions",
                ...(route.accountId === undefined ? {} : { accountId: route.accountId }),
                ...(month === "" ? {} : { month }),
              });
            }}
          >
            <option value="">All months</option>
            {months.map((month) => (
              <option key={month} value={month}>
                {month}
              </option>
            ))}
          </select>
        </label>
        <span className="summary" data-testid="transaction-summary">
          {transactions.length} transactions · net {formatMinorUnits(total, listCurrency)}
        </span>
      </div>
      {editing === null ? null : (
        <TransactionForm
          app={app}
          transaction={editing === "new" ? null : editing}
          accounts={accounts.map((account) => ({
            id: account.id,
            name: account.name,
            currency: account.currency,
          }))}
          categories={categories.map((category) => ({ id: category.id, name: category.name }))}
          tags={tags.map((tag) => ({ id: tag.id, name: tag.name }))}
          defaultAccountId={route.accountId ?? accounts[0]?.id ?? ""}
          onDone={() => setEditing(null)}
        />
      )}
      <table className="list" aria-label="Transactions">
        <thead>
          <tr>
            <th scope="col">Date</th>
            <th scope="col">Description</th>
            <th scope="col">Account</th>
            <th scope="col">Category</th>
            <th scope="col">Tags</th>
            <th scope="col" className="amount">
              Amount
            </th>
            <th scope="col">
              <span className="visually-hidden">Actions</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {transactions.length === 0 ? (
            <tr>
              <td colSpan={7} className="empty">
                No transactions match.
              </td>
            </tr>
          ) : null}
          {transactions.map((transaction) => (
            <tr
              key={transaction.id}
              data-testid={`transaction-${transaction.id}`}
              data-description={transaction.description}
            >
              <td>{transaction.date}</td>
              <td>
                <span data-testid="description">{transaction.description}</span>
                {transaction.notes === undefined ? null : <small> {transaction.notes}</small>}
                {transaction.splits.length === 0 ? null : (
                  <ul className="splits" aria-label="Splits">
                    {transaction.splits.map((split) => (
                      <li key={split.id}>
                        {categoryName(split.category_id) || "uncategorized"}{" "}
                        {formatMinorUnits(split.amount, transaction.currency)}
                        {split.note === undefined ? null : <small> {split.note}</small>}
                      </li>
                    ))}
                  </ul>
                )}
              </td>
              <td>{accountName(transaction.account_id)}</td>
              <td>
                {transaction.splits.length > 0 ? "split" : categoryName(transaction.category_id)}
                {transaction.splits.length > 0 || ruleName(transaction.rule_id) === null ? null : (
                  <small className="muted" data-testid="filed-by">
                    {" "}
                    by {ruleName(transaction.rule_id)}
                  </small>
                )}
              </td>
              <td>
                {transaction.tags.map((tagId) => (
                  <span key={tagId} className="chip">
                    {tagName(tagId)}
                  </span>
                ))}
              </td>
              <td className="amount" data-testid="amount">
                {formatMinorUnits(transaction.amount, transaction.currency)}
              </td>
              <td className="actions">
                <button type="button" className="link" onClick={() => setEditing(transaction)}>
                  Edit
                </button>
                <button type="button" className="link" onClick={() => setAttaching(transaction)}>
                  Receipts
                </button>
                <button
                  type="button"
                  className="link"
                  onClick={() => void deleteWithReceipts(transaction.id)}
                >
                  Delete
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      {attaching === null ? null : (
        <ReceiptsPanel app={app} transaction={attaching} onClose={() => setAttaching(null)} />
      )}
    </section>
  );
}

/**
 * The receipts of one transaction: what the household has attached, whoever
 * attached it. The object carries the household as an attribute the bucket's
 * rules read, so every member opens the same file and nobody else can.
 */
function ReceiptsPanel({
  app,
  transaction,
  onClose,
}: {
  app: RationalApp;
  transaction: Transaction;
  onClose: () => void;
}) {
  const [receipts, setReceipts] = useState<readonly Receipt[] | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const reload = useCallback(async () => {
    const store = app.receipts;
    if (store === null) return;
    try {
      setReceipts(await store.list(transaction.id));
      setProblem(null);
    } catch (error) {
      setProblem(error instanceof Error ? error.message : "the receipts could not be listed");
    }
  }, [app, transaction.id]);
  useEffect(() => {
    void reload();
  }, [reload]);

  const attach = async (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    event.target.value = "";
    if (file === undefined || app.receipts === null) return;
    setBusy(true);
    try {
      await app.receipts.attach(transaction.id, file);
      await reload();
    } catch (error) {
      setProblem(error instanceof Error ? error.message : "the receipt could not be attached");
    } finally {
      setBusy(false);
    }
  };

  const open = async (path: string) => {
    const blob = await app.receipts?.open(path);
    if (blob === null || blob === undefined) {
      setProblem("that receipt is no longer stored");
      return;
    }
    globalThis.open(URL.createObjectURL(blob), "_blank", "noopener");
  };

  return (
    <div className="panel" data-testid="receipts-panel">
      <div className="section-heading">
        <h3>Receipts for {transaction.description}</h3>
        <button type="button" className="link" onClick={onClose}>
          Close
        </button>
      </div>
      {problem === null ? null : (
        <p className="notice error" role="alert">
          {problem}
        </p>
      )}
      <label>
        Attach an image or a PDF
        <input type="file" accept="image/*,application/pdf" disabled={busy} onChange={attach} />
      </label>
      {receipts === null ? (
        <p role="status">Loading receipts…</p>
      ) : receipts.length === 0 ? (
        <p className="muted">No receipts yet.</p>
      ) : (
        <ul aria-label="Receipts">
          {receipts.map((receipt) => (
            <li key={receipt.path} data-testid={`receipt-${receipt.name}`}>
              <button type="button" className="link" onClick={() => void open(receipt.path)}>
                {receipt.name}
              </button>
              <small> {Math.ceil(receipt.sizeBytes / 1024)} KB</small>
              <button
                type="button"
                className="link"
                onClick={() => void app.receipts?.remove(receipt.path).then(reload)}
              >
                Remove
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

interface SplitRow {
  readonly id: string;
  readonly categoryId: string;
  readonly amount: string;
  readonly note: string;
}

interface Option {
  readonly id: string;
  readonly name: string;
}

function TransactionForm({
  app,
  transaction,
  accounts,
  categories,
  tags,
  defaultAccountId,
  onDone,
}: {
  app: RationalApp;
  transaction: Transaction | null;
  accounts: ReadonlyArray<Option & { currency: string }>;
  categories: readonly Option[];
  tags: readonly Option[];
  defaultAccountId: string;
  onDone: () => void;
}) {
  const [accountId, setAccountId] = useState(transaction?.account_id ?? defaultAccountId);
  const account = accounts.find((candidate) => candidate.id === accountId);
  const currency = transaction?.currency ?? account?.currency ?? "USD";
  const [date, setDate] = useState(transaction?.date ?? new Date().toISOString().slice(0, 10));
  const [amount, setAmount] = useState(
    transaction === null ? "" : amountToText(transaction.amount, transaction.currency),
  );
  const [description, setDescription] = useState(transaction?.description ?? "");
  const [categoryId, setCategoryId] = useState(transaction?.category_id ?? "");
  const [selectedTags, setSelectedTags] = useState<readonly string[]>(transaction?.tags ?? []);
  const [notes, setNotes] = useState(transaction?.notes ?? "");
  const [splits, setSplits] = useState<readonly SplitRow[]>(
    (transaction?.splits ?? []).map((split) => ({
      id: split.id,
      categoryId: split.category_id ?? "",
      amount: amountToText(split.amount, transaction?.currency ?? currency),
      note: split.note ?? "",
    })),
  );
  const [error, setError] = useState<string | null>(null);

  const parsedSplits = (): Split[] =>
    splits.map((row) => ({
      id: row.id,
      amount: safeParse(row.amount, currency),
      ...(row.categoryId === "" ? {} : { category_id: row.categoryId }),
      ...(row.note.trim() === "" ? {} : { note: row.note.trim() }),
    }));
  const parsedAmount = safeParse(amount, currency);
  const splitCheck = validateSplits(parsedAmount, parsedSplits());

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    const writes = app.writes;
    if (writes === null) return;
    try {
      const input = {
        account_id: accountId,
        date,
        amount: parseAmount(amount, currency),
        currency,
        description,
        category_id: categoryId,
        tags: selectedTags,
        notes,
        splits: splits.map((row) => ({
          id: row.id,
          amount: parseAmount(row.amount, currency),
          ...(row.categoryId === "" ? {} : { category_id: row.categoryId }),
          ...(row.note.trim() === "" ? {} : { note: row.note.trim() }),
        })),
      };
      if (transaction === null) {
        await writes.createTransaction(input);
      } else {
        await writes.updateTransaction(transaction.id, {
          ...input,
          category_id: input.category_id === "" ? null : input.category_id,
          notes: input.notes.trim() === "" ? null : input.notes,
        });
      }
      onDone();
    } catch (caught) {
      if (caught instanceof ValidationError && caught.difference !== undefined) {
        setError(
          `${caught.message}: ${describeDifference(caught.difference, parsedAmount, currency)}. Nothing was saved.`,
        );
      } else {
        setError(
          caught instanceof ValidationError || caught instanceof RangeError
            ? caught.message
            : "The transaction could not be saved.",
        );
      }
    }
  };

  return (
    <form
      className="editor"
      onSubmit={(event) => void submit(event)}
      aria-label="Transaction editor"
    >
      <h2>{transaction === null ? "New transaction" : "Edit transaction"}</h2>
      <div className="grid">
        <label>
          Account
          <select
            name="account"
            value={accountId}
            onChange={(event) => setAccountId(event.target.value)}
          >
            {accounts.map((candidate) => (
              <option key={candidate.id} value={candidate.id}>
                {candidate.name}
              </option>
            ))}
          </select>
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
          Amount ({currency})
          <input
            name="amount"
            inputMode="decimal"
            required
            placeholder="-12.34"
            value={amount}
            onChange={(event) => setAmount(event.target.value)}
          />
        </label>
        <label>
          Description
          <input
            name="description"
            required
            value={description}
            onChange={(event) => setDescription(event.target.value)}
          />
        </label>
        <label>
          Category
          <select
            name="category"
            value={categoryId}
            onChange={(event) => setCategoryId(event.target.value)}
            disabled={splits.length > 0}
          >
            <option value="">{splits.length > 0 ? "split" : "Uncategorized"}</option>
            {categories.map((category) => (
              <option key={category.id} value={category.id}>
                {category.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          Notes
          <input name="notes" value={notes} onChange={(event) => setNotes(event.target.value)} />
        </label>
      </div>
      <fieldset className="tags">
        <legend>Tags</legend>
        {tags.length === 0 ? <small>No tags yet.</small> : null}
        {tags.map((tag) => (
          <label key={tag.id} className="chip-option">
            <input
              type="checkbox"
              name="tags"
              value={tag.id}
              checked={selectedTags.includes(tag.id)}
              onChange={(event) =>
                setSelectedTags(
                  event.target.checked
                    ? [...selectedTags, tag.id]
                    : selectedTags.filter((candidate) => candidate !== tag.id),
                )
              }
            />
            {tag.name}
          </label>
        ))}
      </fieldset>
      <fieldset className="split-editor" data-testid="splits">
        <legend>Splits</legend>
        {splits.map((row, index) => (
          <div key={row.id} className="split-row" data-testid="split-row">
            <select
              aria-label={`Split ${index + 1} category`}
              value={row.categoryId}
              onChange={(event) =>
                setSplits(replaceRow(splits, index, { categoryId: event.target.value }))
              }
            >
              <option value="">Uncategorized</option>
              {categories.map((category) => (
                <option key={category.id} value={category.id}>
                  {category.name}
                </option>
              ))}
            </select>
            <input
              aria-label={`Split ${index + 1} amount`}
              inputMode="decimal"
              value={row.amount}
              onChange={(event) =>
                setSplits(replaceRow(splits, index, { amount: event.target.value }))
              }
            />
            <input
              aria-label={`Split ${index + 1} note`}
              placeholder="note"
              value={row.note}
              onChange={(event) =>
                setSplits(replaceRow(splits, index, { note: event.target.value }))
              }
            />
            <button
              type="button"
              className="link"
              onClick={() => setSplits(splits.filter((_, candidate) => candidate !== index))}
            >
              Remove
            </button>
          </div>
        ))}
        <div className="split-footer">
          <button
            type="button"
            className="secondary"
            onClick={() =>
              setSplits([
                ...splits,
                {
                  id: `split_${Date.now().toString(36)}_${splits.length}`,
                  categoryId: "",
                  amount: "",
                  note: "",
                },
              ])
            }
          >
            Add split
          </button>
          {splits.length === 0 ? null : (
            <span
              className={splitCheck.ok ? "hint" : "error"}
              data-testid="split-difference"
              role="status"
            >
              {splitCheck.ok
                ? "Splits add up."
                : `Splits ${describeDifference(splitCheck.difference, parsedAmount, currency)}.`}
            </span>
          )}
        </div>
      </fieldset>
      {error === null ? null : (
        <p className="error" role="alert" data-testid="form-error">
          {error}
        </p>
      )}
      <div className="actions">
        <button type="submit">Save transaction</button>
        <button type="button" className="secondary" onClick={onDone}>
          Cancel
        </button>
      </div>
    </form>
  );
}

function replaceRow(
  rows: readonly SplitRow[],
  index: number,
  patch: Partial<SplitRow>,
): readonly SplitRow[] {
  return rows.map((row, candidate) => (candidate === index ? { ...row, ...patch } : row));
}

function safeParse(text: string, currency: string): number {
  try {
    return parseAmount(text, currency);
  } catch {
    return Number.NaN;
  }
}

/**
 * "Short" and "over" follow the direction of the transaction: splits of an
 * outflow are short while they cover less of it than the whole amount.
 */
function describeDifference(difference: number, amount: number, currency: string): string {
  if (!Number.isFinite(difference)) return "have an invalid amount";
  const direction = Number.isFinite(amount) && amount !== 0 ? Math.sign(amount) : 1;
  const remaining = difference * direction;
  return remaining > 0
    ? `are short by ${formatMinorUnits(Math.abs(difference), currency)}`
    : `are over by ${formatMinorUnits(Math.abs(difference), currency)}`;
}
