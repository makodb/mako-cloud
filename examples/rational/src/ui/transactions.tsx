import { type FormEvent, useEffect, useMemo, useRef, useState } from "react";

import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import { ValidationError } from "../data/writes.js";
import {
  type Account,
  type HouseholdCollectionId,
  isCategory,
  isGroup,
  isMerchant,
  isTag,
  type Split,
  type TaxonomyEntry,
  type Transaction,
} from "../model/types.js";
import { transactionsCsv } from "../selectors/export.js";
import {
  activeFilterCount,
  type HiddenFilter,
  parseTransactionQuery,
  type ReviewFilter,
  selectGroupedByDate,
  selectQueriedTransactions,
  serializeTransactionQuery,
  TRANSACTION_SORTS,
  type TransactionQuery,
  type TransactionSort,
  type TransferFilter,
} from "../selectors/filters.js";
import { merchantResolver } from "../selectors/merchants.js";
import { amountToText, formatMinorUnits, parseAmount } from "../selectors/money.js";
import { needsReview, selectReviewCount } from "../selectors/review.js";
import { validateSplits } from "../selectors/splits.js";
import { selectAvailableMonths, sumAmounts } from "../selectors/transactions.js";
import { useQuery } from "./hooks.js";
import { type Route, transactionsHash } from "./router.js";
import { CategoryOptions, ReceiptsPanel, TransactionPanel } from "./transaction-panel.js";
import "./styles/transactions.css";

/**
 * The transactions screen: every filter in the address, a list grouped by day,
 * a bulk bar over a selection, and a detail panel for one transaction.
 *
 * The query codec puts "uncategorized" in the address as an empty category,
 * but the hash router drops empty values -- a link ending in `category=`
 * would come back as no filter at all. The screen therefore writes the word
 * `none` for that one case and translates it back on the way in; every other
 * key is the codec's own.
 */
const UNCATEGORIZED = "none";

/** How many rows are drawn before the list asks to show more; a day is never cut in half. */
const PAGE = 250;

/** The bar's debounce: long enough to skip keystrokes, short enough to feel live. */
const DEBOUNCE_MS = 150;

const SORT_LABELS: Readonly<Record<TransactionSort, string>> = {
  date_desc: "Newest first",
  date_asc: "Oldest first",
  amount_desc: "Largest first",
  amount_asc: "Smallest first",
};

function readQuery(params: Readonly<Record<string, string>>): TransactionQuery {
  const query = parseTransactionQuery(new URLSearchParams(params));
  return query.categoryId === UNCATEGORIZED ? { ...query, categoryId: "" } : query;
}

function queryHash(query: TransactionQuery): string {
  const written = query.categoryId === "" ? { ...query, categoryId: UNCATEGORIZED } : query;
  return transactionsHash(Object.fromEntries(serializeTransactionQuery(written)));
}

/** A query with one key changed, or removed when the value is `undefined`. */
function withKey<Key extends keyof TransactionQuery>(
  query: TransactionQuery,
  key: Key,
  value: TransactionQuery[Key] | undefined,
): TransactionQuery {
  const next: Record<string, unknown> = { ...query };
  if (value === undefined) delete next[key];
  else next[key] = value;
  return next as TransactionQuery;
}

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
  const taxonomy = useQuery(
    session.collection("taxonomy")?.find({ sort: [{ name: "asc" }] }) ?? null,
  );
  const rules = useQuery(session.collection("rules")?.find() ?? null);
  const all = useQuery(session.collection("transactions")?.find() ?? null);

  // The route object is replaced only when the hash changes, so the parsed
  // query and the selector's context keep their identity between renders and
  // the memoized selectors get to remember their work.
  const query = useMemo(() => readQuery(route.params), [route.params]);
  const context = useMemo(() => ({ categories: taxonomy, merchants: taxonomy }), [taxonomy]);
  const transactions = selectQueriedTransactions(all, query, context);
  const groups = selectGroupedByDate(transactions);
  const months = selectAvailableMonths(all);
  const reviewCount = selectReviewCount(all);
  const resolve = useMemo(() => merchantResolver(taxonomy), [taxonomy]);
  const categories = useMemo(() => taxonomy.filter(isCategory), [taxonomy]);
  const groupsById = useMemo(
    () => new Map(taxonomy.filter(isGroup).map((group) => [group.id, group] as const)),
    [taxonomy],
  );
  const tags = useMemo(() => taxonomy.filter(isTag), [taxonomy]);
  const merchants = useMemo(
    () => taxonomy.filter(isMerchant).sort((left, right) => left.name.localeCompare(right.name)),
    [taxonomy],
  );

  const [editing, setEditing] = useState<Transaction | "new" | null>(null);
  const [attaching, setAttaching] = useState<string | null>(null);
  const [detailId, setDetailId] = useState<string | null>(null);
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set());
  const [limit, setLimit] = useState(PAGE);
  const [problem, setProblem] = useState<string | null>(null);
  // The bulk bar unmounts with the selection it applied to, so what it did is
  // reported here, where the message outlives the bar.
  const [bulkOutcome, setBulkOutcome] = useState<string | null>(null);

  const navigate = (next: TransactionQuery) => {
    window.location.hash = queryHash(next);
  };
  const set = <Key extends keyof TransactionQuery>(
    key: Key,
    value: TransactionQuery[Key] | undefined,
  ) => navigate(withKey(query, key, value));

  // The detail panel and the stand-alone receipts panel are two views of one
  // transaction; opening either closes the other so the screen never shows
  // the same receipts twice.
  const openDetails = (id: string) => {
    setAttaching(null);
    setDetailId(id);
  };
  const openReceipts = (id: string) => {
    setDetailId(null);
    setAttaching(id);
  };
  const detail = detailId === null ? null : (all.find((entry) => entry.id === detailId) ?? null);
  const attachingTo =
    attaching === null ? null : (all.find((entry) => entry.id === attaching) ?? null);

  const existing = useMemo(() => new Set(all.map((entry) => entry.id)), [all]);
  const selectedIds = useMemo(
    () => [...selected].filter((id) => existing.has(id)),
    [selected, existing],
  );

  const deleteWithReceipts = async (ids: readonly string[]) => {
    const writes = app.writes;
    if (writes === null) return;
    setProblem(null);
    try {
      for (const id of ids) {
        // A receipt outlives its transaction unless something removes it:
        // the bucket knows nothing about the document that referred to it.
        await app.receipts?.removeAll(id).catch(() => undefined);
        await writes.deleteTransaction(id);
      }
    } catch (error) {
      setProblem(error instanceof Error ? error.message : "The transaction could not be deleted.");
    }
  };

  const exportCsv = () => {
    const csv = transactionsCsv(transactions, { accounts, taxonomy });
    const today = new Date().toISOString().slice(0, 10);
    downloadText(`rational-transactions-${today}.csv`, csv, "text/csv;charset=utf-8");
  };

  const categoryName = (id: string | undefined) =>
    id === undefined ? "" : (categories.find((category) => category.id === id)?.name ?? id);
  const categoryIcon = (id: string | undefined): string | null => {
    if (id === undefined) return null;
    const category = categories.find((entry) => entry.id === id);
    if (category === undefined) return null;
    if (category.icon !== undefined) return category.icon;
    const group = category.parent_id === undefined ? undefined : groupsById.get(category.parent_id);
    return group?.icon ?? null;
  };
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
    query.accountId === undefined
      ? currency
      : (accounts.find((account) => account.id === query.accountId)?.currency ?? currency);
  const filterCount = activeFilterCount(query);

  // Whole days up to the row budget, so a day's total is never the total of
  // the part of it that happened to fit.
  let drawn = 0;
  const shownGroups = groups.filter((group, index) => {
    if (index > 0 && drawn >= limit) return false;
    drawn += group.transactions.length;
    return true;
  });
  const remaining = transactions.length - drawn;

  const toggle = (id: string, checked: boolean) => {
    const next = new Set(selected);
    if (checked) next.add(id);
    else next.delete(id);
    setSelected(next);
  };
  const allShownSelected =
    transactions.length > 0 && transactions.every((entry) => selected.has(entry.id));

  return (
    <section aria-labelledby="transactions-title" className="transactions-screen">
      <div className="heading">
        <h1 id="transactions-title">Transactions</h1>
        <div className="heading-actions">
          <button
            type="button"
            className={`secondary quick-filter${query.review === "needs" ? " active" : ""}`}
            aria-pressed={query.review === "needs"}
            data-testid="needs-review-filter"
            onClick={() => set("review", query.review === "needs" ? undefined : "needs")}
          >
            Needs review ({reviewCount})
          </button>
          <button
            type="button"
            className="secondary"
            onClick={exportCsv}
            disabled={transactions.length === 0}
          >
            Export CSV
          </button>
          <button type="button" onClick={() => setEditing("new")} disabled={accounts.length === 0}>
            New transaction
          </button>
        </div>
      </div>

      <FilterBar
        query={query}
        set={set}
        navigate={navigate}
        accounts={accounts}
        taxonomy={taxonomy}
        tags={tags}
        merchants={merchants}
        months={months}
        currency={listCurrency}
        filterCount={filterCount}
      />

      <div className="list-summary">
        <span className="summary" data-testid="transaction-summary">
          {transactions.length} {transactions.length === 1 ? "transaction" : "transactions"} · net{" "}
          {formatMinorUnits(total, listCurrency)}
        </span>
      </div>

      {problem === null ? null : (
        <p className="notice error" role="alert">
          {problem}
        </p>
      )}

      {editing === null ? null : (
        <TransactionForm
          app={app}
          transaction={editing === "new" ? null : editing}
          accounts={accounts}
          taxonomy={taxonomy}
          tags={tags}
          merchants={merchants}
          defaultAccountId={query.accountId ?? accounts[0]?.id ?? ""}
          onDone={() => setEditing(null)}
        />
      )}

      {selectedIds.length === 0 ? null : (
        <BulkBar
          app={app}
          ids={selectedIds}
          taxonomy={taxonomy}
          tags={tags}
          onDelete={() => void deleteWithReceipts(selectedIds).then(() => setSelected(new Set()))}
          onClear={() => setSelected(new Set())}
          onApplied={setBulkOutcome}
        />
      )}
      {bulkOutcome === null ? null : (
        <p className="hint" role="status" data-testid="bulk-outcome">
          {bulkOutcome}
        </p>
      )}

      <table className="list transactions" aria-label="Transactions">
        <thead>
          <tr>
            <th scope="col" className="select">
              <input
                type="checkbox"
                aria-label="Select all"
                checked={allShownSelected}
                disabled={transactions.length === 0}
                onChange={(event) =>
                  setSelected(
                    event.target.checked
                      ? new Set([...selected, ...transactions.map((entry) => entry.id)])
                      : new Set(
                          [...selected].filter(
                            (id) => !transactions.some((entry) => entry.id === id),
                          ),
                        ),
                  )
                }
              />
            </th>
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
        {transactions.length === 0 ? (
          <tbody>
            <tr>
              <td colSpan={8} className="empty">
                {all.length === 0 ? "No transactions yet." : "No transactions match."}
              </td>
            </tr>
          </tbody>
        ) : null}
        {shownGroups.map((group) => (
          <tbody key={group.date} data-testid={`day-${group.date}`}>
            <tr className="day">
              <th scope="rowgroup" colSpan={6}>
                {group.date}
                <small className="muted">
                  {" "}
                  · {group.transactions.length}{" "}
                  {group.transactions.length === 1 ? "transaction" : "transactions"}
                </small>
              </th>
              <th scope="rowgroup" className="amount" data-testid="day-total">
                {formatMinorUnits(group.total, listCurrency)}
              </th>
              <th scope="rowgroup" />
            </tr>
            {group.transactions.map((transaction) => {
              const merchant = resolve(transaction);
              const showMerchant =
                merchant.id === null
                  ? merchant.name.toLowerCase() !== transaction.description.trim().toLowerCase()
                  : merchant.name !== transaction.description;
              const isSelected = selected.has(transaction.id);
              const icon = categoryIcon(transaction.category_id);
              return (
                <tr
                  key={transaction.id}
                  data-testid={`transaction-${transaction.id}`}
                  data-description={transaction.description}
                  data-selected={isSelected ? "true" : "false"}
                  className={transaction.hidden === true ? "muted" : undefined}
                >
                  <td className="select">
                    <input
                      type="checkbox"
                      aria-label={`Select ${transaction.description}`}
                      checked={isSelected}
                      onChange={(event) => toggle(transaction.id, event.target.checked)}
                    />
                  </td>
                  <td className="date">{transaction.date}</td>
                  <td>
                    {showMerchant ? (
                      <span className="merchant-name" data-testid="merchant">
                        {merchant.name}
                      </span>
                    ) : null}
                    <span
                      className={showMerchant ? "statement" : "merchant-name"}
                      data-testid="description"
                    >
                      {transaction.description}
                    </span>
                    {transaction.notes === undefined ? null : (
                      <small className="notes"> {transaction.notes}</small>
                    )}
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
                    {transaction.splits.length > 0 ? (
                      "split"
                    ) : (
                      <>
                        {icon === null ? null : (
                          <span className="category-icon" aria-hidden="true">
                            {icon}{" "}
                          </span>
                        )}
                        {categoryName(transaction.category_id)}
                      </>
                    )}
                    {transaction.splits.length > 0 ||
                    ruleName(transaction.rule_id) === null ? null : (
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
                    {needsReview(transaction) ? (
                      <span className="chip marker review" data-testid="marker-needs-review">
                        needs review
                      </span>
                    ) : null}
                    {transaction.hidden === true ? (
                      <span className="chip marker hidden" data-testid="marker-hidden">
                        hidden
                      </span>
                    ) : null}
                    {transaction.transfer_id === undefined ? null : (
                      <span className="chip marker transfer" data-testid="marker-transfer">
                        transfer
                      </span>
                    )}
                    {transaction.pending === true ? (
                      <span className="chip marker pending" data-testid="marker-pending">
                        pending
                      </span>
                    ) : null}
                  </td>
                  <td
                    className={`amount${transaction.amount > 0 ? " positive" : ""}`}
                    data-testid="amount"
                  >
                    {formatMinorUnits(transaction.amount, transaction.currency)}
                  </td>
                  <td className="actions">
                    <button
                      type="button"
                      className="link"
                      onClick={() => openDetails(transaction.id)}
                    >
                      Details
                    </button>
                    <button type="button" className="link" onClick={() => setEditing(transaction)}>
                      Edit
                    </button>
                    <button
                      type="button"
                      className="link"
                      onClick={() => openReceipts(transaction.id)}
                    >
                      Receipts
                    </button>
                    <button
                      type="button"
                      className="link"
                      onClick={() => void deleteWithReceipts([transaction.id])}
                    >
                      Delete
                    </button>
                  </td>
                </tr>
              );
            })}
          </tbody>
        ))}
      </table>
      {remaining > 0 ? (
        <div className="actions">
          <button type="button" className="secondary" onClick={() => setLimit(limit + PAGE)}>
            Show {Math.min(remaining, PAGE)} more
          </button>
        </div>
      ) : null}

      {attachingTo === null ? null : (
        <ReceiptsPanel app={app} transaction={attachingTo} onClose={() => setAttaching(null)} />
      )}
      {detail === null ? null : (
        <TransactionPanel
          key={detail.id}
          app={app}
          transaction={detail}
          transactions={all}
          accounts={accounts}
          taxonomy={taxonomy}
          onClose={() => setDetailId(null)}
          onEdit={(transaction) => {
            setDetailId(null);
            setEditing(transaction);
          }}
          onDeleted={() => setDetailId(null)}
        />
      )}
    </section>
  );
}

/**
 * Every filter the list offers, each writing straight into the address. The
 * text-ish inputs (search, the amount bounds) are debounced so a person can
 * type a whole word before the list -- and the browser history -- move.
 */
function FilterBar({
  query,
  set,
  navigate,
  accounts,
  taxonomy,
  tags,
  merchants,
  months,
  currency,
  filterCount,
}: {
  query: TransactionQuery;
  set: <Key extends keyof TransactionQuery>(
    key: Key,
    value: TransactionQuery[Key] | undefined,
  ) => void;
  navigate: (query: TransactionQuery) => void;
  accounts: readonly Account[];
  taxonomy: readonly TaxonomyEntry[];
  tags: readonly TaxonomyEntry[];
  merchants: readonly TaxonomyEntry[];
  months: readonly string[];
  currency: string;
  filterCount: number;
}) {
  const [search, setSearch] = useDebouncedText(query.text ?? "", (text) =>
    set("text", text.trim() === "" ? undefined : text.trim()),
  );
  const [min, setMin] = useDebouncedAmount(query.amountMin, currency, (value) =>
    set("amountMin", value),
  );
  const [max, setMax] = useDebouncedAmount(query.amountMax, currency, (value) =>
    set("amountMax", value),
  );
  // The month select is the earlier screen's; it lives on because a month is
  // the range people reach for most, and links that carry it still open.
  const monthOptions =
    query.month !== undefined && !months.includes(query.month) ? [query.month, ...months] : months;

  return (
    <div className="filter-bar" data-testid="filter-bar">
      <label className="search">
        Search
        <input
          type="search"
          aria-label="Search transactions"
          placeholder="Description, merchant, notes, or amount"
          value={search}
          onChange={(event) => setSearch(event.target.value)}
        />
      </label>
      <label>
        Account
        <select
          aria-label="Filter by account"
          value={query.accountId ?? ""}
          onChange={(event) =>
            set("accountId", event.target.value === "" ? undefined : event.target.value)
          }
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
        Category
        <select
          aria-label="Filter by category"
          value={
            query.categoryId === undefined
              ? ""
              : query.categoryId === ""
                ? UNCATEGORIZED
                : query.categoryId
          }
          onChange={(event) =>
            set(
              "categoryId",
              event.target.value === ""
                ? undefined
                : event.target.value === UNCATEGORIZED
                  ? ""
                  : event.target.value,
            )
          }
        >
          <CategoryOptions
            taxonomy={taxonomy}
            blankLabel="All categories"
            extra={[{ value: UNCATEGORIZED, label: "Uncategorized" }]}
          />
        </select>
      </label>
      <label>
        Tag
        <select
          aria-label="Filter by tag"
          value={query.tagId ?? ""}
          onChange={(event) =>
            set("tagId", event.target.value === "" ? undefined : event.target.value)
          }
        >
          <option value="">All tags</option>
          {tags.map((tag) => (
            <option key={tag.id} value={tag.id}>
              {tag.name}
            </option>
          ))}
        </select>
      </label>
      <label>
        Merchant
        <select
          aria-label="Filter by merchant"
          value={query.merchantId ?? ""}
          onChange={(event) =>
            set("merchantId", event.target.value === "" ? undefined : event.target.value)
          }
        >
          <option value="">All merchants</option>
          {merchants.map((merchant) => (
            <option key={merchant.id} value={merchant.id}>
              {merchant.name}
            </option>
          ))}
        </select>
      </label>
      <label>
        Month
        <select
          aria-label="Filter by month"
          value={query.month ?? ""}
          onChange={(event) =>
            set("month", event.target.value === "" ? undefined : event.target.value)
          }
        >
          <option value="">All months</option>
          {monthOptions.map((month) => (
            <option key={month} value={month}>
              {month}
            </option>
          ))}
        </select>
      </label>
      <label>
        From
        <input
          type="date"
          aria-label="From"
          value={query.from ?? ""}
          onChange={(event) =>
            set("from", event.target.value === "" ? undefined : event.target.value)
          }
        />
      </label>
      <label>
        To
        <input
          type="date"
          aria-label="To"
          value={query.to ?? ""}
          onChange={(event) =>
            set("to", event.target.value === "" ? undefined : event.target.value)
          }
        />
      </label>
      <label className="amount-bound">
        Min amount
        <input
          inputMode="decimal"
          aria-label="Min amount"
          placeholder="0.00"
          value={min}
          onChange={(event) => setMin(event.target.value)}
        />
      </label>
      <label className="amount-bound">
        Max amount
        <input
          inputMode="decimal"
          aria-label="Max amount"
          placeholder="any"
          value={max}
          onChange={(event) => setMax(event.target.value)}
        />
      </label>
      <label>
        Review
        <select
          aria-label="Review"
          value={query.review ?? ""}
          onChange={(event) =>
            set(
              "review",
              event.target.value === "" ? undefined : (event.target.value as ReviewFilter),
            )
          }
        >
          <option value="">All</option>
          <option value="needs">Needs review</option>
          <option value="reviewed">Reviewed</option>
        </select>
      </label>
      <label>
        Hidden
        <select
          aria-label="Hidden"
          value={query.hidden ?? ""}
          onChange={(event) =>
            set(
              "hidden",
              event.target.value === "" ? undefined : (event.target.value as HiddenFilter),
            )
          }
        >
          <option value="">Exclude hidden</option>
          <option value="include">Include hidden</option>
          <option value="only">Only hidden</option>
        </select>
      </label>
      <label>
        Transfers
        <select
          aria-label="Transfers"
          value={query.transfers ?? ""}
          onChange={(event) =>
            set(
              "transfers",
              event.target.value === "" ? undefined : (event.target.value as TransferFilter),
            )
          }
        >
          <option value="">Include transfers</option>
          <option value="exclude">Exclude transfers</option>
          <option value="only">Only transfers</option>
        </select>
      </label>
      <label>
        Sort
        <select
          aria-label="Sort"
          value={query.sort ?? "date_desc"}
          onChange={(event) =>
            set(
              "sort",
              event.target.value === "date_desc"
                ? undefined
                : (event.target.value as TransactionSort),
            )
          }
        >
          {TRANSACTION_SORTS.map((sort) => (
            <option key={sort} value={sort}>
              {SORT_LABELS[sort]}
            </option>
          ))}
        </select>
      </label>
      <div className="filter-state" data-testid="filter-state">
        {filterCount === 0 ? (
          <span className="muted">No filters</span>
        ) : (
          <>
            <span className="chip active-count" data-testid="active-filter-count">
              {filterCount} {filterCount === 1 ? "filter" : "filters"}
            </span>
            <button
              type="button"
              className="link"
              onClick={() => navigate(query.sort === undefined ? {} : { sort: query.sort })}
            >
              Clear filters
            </button>
          </>
        )}
      </div>
    </div>
  );
}

/**
 * A text input whose committed value lives elsewhere -- in the address -- and
 * arrives back a beat later. The draft is what the person sees; it is
 * committed after a pause in typing, and it follows the committed value only
 * when that changed for some other reason (a cleared filter, a link), never
 * because the value it just wrote came back.
 */
function useDebouncedText(
  committed: string,
  commit: (value: string) => void,
): [string, (next: string) => void] {
  const [draft, setDraft] = useState(committed);
  const written = useRef(committed);
  const commitRef = useRef(commit);
  commitRef.current = commit;
  useEffect(() => {
    if (committed !== written.current) {
      written.current = committed;
      setDraft(committed);
    }
  }, [committed]);
  useEffect(() => {
    if (draft.trim() === written.current.trim()) return undefined;
    const timer = window.setTimeout(() => {
      written.current = draft;
      commitRef.current(draft);
    }, DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [draft]);
  return [draft, setDraft];
}

/**
 * The same, for an amount bound: the address holds minor units, the input
 * holds decimal text, and "20" must not be rewritten as "20.00" under the
 * person's cursor when its own value returns.
 */
function useDebouncedAmount(
  committed: number | undefined,
  currency: string,
  commit: (value: number | undefined) => void,
): [string, (next: string) => void] {
  const [draft, setDraft] = useState(
    committed === undefined ? "" : amountToText(committed, currency),
  );
  const written = useRef(committed);
  const commitRef = useRef(commit);
  commitRef.current = commit;
  useEffect(() => {
    if (committed !== written.current) {
      written.current = committed;
      setDraft(committed === undefined ? "" : amountToText(committed, currency));
    }
  }, [committed, currency]);
  useEffect(() => {
    const parsed = draft.trim() === "" ? undefined : safeParse(draft, currency);
    // Half-typed text is not a bound yet; wait for it to become one.
    if (parsed !== undefined && Number.isNaN(parsed)) return undefined;
    const value = parsed === undefined ? undefined : Math.abs(parsed);
    if (value === written.current) return undefined;
    const timer = window.setTimeout(() => {
      written.current = value;
      commitRef.current(value);
    }, DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [draft, currency]);
  return [draft, setDraft];
}

type BulkReview = "" | "reviewed" | "unreviewed";
type BulkHidden = "" | "hide" | "unhide";

/**
 * One change over many transactions. The choices are gathered and applied
 * together so setting a category, adding a tag, and marking the lot reviewed
 * is one action and one moment in the other member's stream, not three.
 */
function BulkBar({
  app,
  ids,
  taxonomy,
  tags,
  onDelete,
  onClear,
  onApplied,
}: {
  app: RationalApp;
  ids: readonly string[];
  taxonomy: readonly TaxonomyEntry[];
  tags: readonly TaxonomyEntry[];
  onDelete: () => void;
  onClear: () => void;
  /** What the bar did, for the screen to show once the bar itself is gone. */
  onApplied: (message: string) => void;
}) {
  const [category, setCategory] = useState("");
  const [tag, setTag] = useState("");
  const [review, setReview] = useState<BulkReview>("");
  const [hidden, setHidden] = useState<BulkHidden>("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const nothingChosen = category === "" && tag === "" && review === "" && hidden === "";

  const apply = async (event: FormEvent) => {
    event.preventDefault();
    const writes = app.writes;
    if (writes === null || nothingChosen) return;
    setBusy(true);
    setError(null);
    try {
      let touched = 0;
      if (category !== "") {
        touched = Math.max(
          touched,
          await writes.bulkPatchTransactions(ids, {
            category_id: category === UNCATEGORIZED ? null : category,
          }),
        );
      }
      if (tag !== "") touched = Math.max(touched, await writes.addTagsToTransactions(ids, [tag]));
      if (review !== "") {
        touched = Math.max(touched, await writes.markReviewed(ids, review === "reviewed"));
      }
      if (hidden !== "") {
        touched = Math.max(touched, await writes.setHidden(ids, hidden === "hide"));
      }
      onApplied(`Updated ${touched} ${touched === 1 ? "transaction" : "transactions"}.`);
      setCategory("");
      setTag("");
      setReview("");
      setHidden("");
      onClear();
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "The changes could not be saved.");
    } finally {
      setBusy(false);
    }
  };

  return (
    <form className="bulk-bar" data-testid="bulk-bar" aria-label="Bulk edit" onSubmit={apply}>
      <strong data-testid="bulk-count">{ids.length} selected</strong>
      <label>
        Category
        <select
          aria-label="Bulk category"
          value={category}
          onChange={(event) => setCategory(event.target.value)}
        >
          <CategoryOptions
            taxonomy={taxonomy}
            blankLabel="Leave as is"
            extra={[{ value: UNCATEGORIZED, label: "Uncategorized" }]}
          />
        </select>
      </label>
      <label>
        Add tag
        <select aria-label="Bulk tag" value={tag} onChange={(event) => setTag(event.target.value)}>
          <option value="">None</option>
          {tags.map((entry) => (
            <option key={entry.id} value={entry.id}>
              {entry.name}
            </option>
          ))}
        </select>
      </label>
      <label>
        Review
        <select
          aria-label="Bulk review"
          value={review}
          onChange={(event) => setReview(event.target.value as BulkReview)}
        >
          <option value="">Leave as is</option>
          <option value="reviewed">Mark reviewed</option>
          <option value="unreviewed">Mark unreviewed</option>
        </select>
      </label>
      <label>
        Hidden
        <select
          aria-label="Bulk hidden"
          value={hidden}
          onChange={(event) => setHidden(event.target.value as BulkHidden)}
        >
          <option value="">Leave as is</option>
          <option value="hide">Hide</option>
          <option value="unhide">Unhide</option>
        </select>
      </label>
      <div className="bulk-actions">
        <button type="submit" disabled={busy || nothingChosen}>
          Apply changes
        </button>
        <button type="button" className="secondary danger" disabled={busy} onClick={onDelete}>
          Delete selected
        </button>
        <button type="button" className="link" onClick={onClear}>
          Clear selection
        </button>
      </div>
      {error === null ? null : (
        <p className="error" role="alert">
          {error}
        </p>
      )}
    </form>
  );
}

interface SplitRow {
  readonly id: string;
  readonly categoryId: string;
  readonly amount: string;
  readonly note: string;
}

function TransactionForm({
  app,
  transaction,
  accounts,
  taxonomy,
  tags,
  merchants,
  defaultAccountId,
  onDone,
}: {
  app: RationalApp;
  transaction: Transaction | null;
  accounts: readonly Account[];
  taxonomy: readonly TaxonomyEntry[];
  tags: readonly TaxonomyEntry[];
  merchants: readonly TaxonomyEntry[];
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
  const [merchantId, setMerchantId] = useState(transaction?.merchant_id ?? "");
  const [selectedTags, setSelectedTags] = useState<readonly string[]>(transaction?.tags ?? []);
  const [notes, setNotes] = useState(transaction?.notes ?? "");
  const [hidden, setHidden] = useState(transaction?.hidden === true);
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
        merchant_id: merchantId,
        tags: selectedTags,
        notes,
        splits: splits.map((row) => ({
          id: row.id,
          amount: parseAmount(row.amount, currency),
          ...(row.categoryId === "" ? {} : { category_id: row.categoryId }),
          ...(row.note.trim() === "" ? {} : { note: row.note.trim() }),
        })),
        ...(hidden ? { hidden: true } : {}),
      };
      if (transaction === null) {
        await writes.createTransaction(input);
      } else {
        await writes.updateTransaction(transaction.id, {
          ...input,
          category_id: input.category_id === "" ? null : input.category_id,
          merchant_id: input.merchant_id === "" ? null : input.merchant_id,
          notes: input.notes.trim() === "" ? null : input.notes,
          hidden: hidden ? true : null,
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
            <CategoryOptions
              taxonomy={taxonomy}
              blankLabel={splits.length > 0 ? "split" : "Uncategorized"}
            />
          </select>
        </label>
        <label>
          Merchant
          <select
            name="merchant"
            value={merchantId}
            onChange={(event) => setMerchantId(event.target.value)}
          >
            <option value="">Automatic</option>
            {merchants.map((merchant) => (
              <option key={merchant.id} value={merchant.id}>
                {merchant.name}
              </option>
            ))}
          </select>
        </label>
        <label>
          Notes
          <input name="notes" value={notes} onChange={(event) => setNotes(event.target.value)} />
        </label>
        <label className="chip-option hidden-option">
          <input
            type="checkbox"
            name="hidden"
            checked={hidden}
            onChange={(event) => setHidden(event.target.checked)}
          />
          Hidden
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
              <CategoryOptions taxonomy={taxonomy} blankLabel="Uncategorized" />
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

/**
 * Hand the browser a file: a blob URL on a link that is clicked and removed.
 * The URL is revoked once the click has been dispatched; a download that has
 * started keeps its own reference to the bytes.
 */
function downloadText(filename: string, text: string, type: string): void {
  const url = URL.createObjectURL(new Blob([text], { type }));
  const anchor = document.createElement("a");
  anchor.href = url;
  anchor.download = filename;
  anchor.hidden = true;
  document.body.append(anchor);
  anchor.click();
  anchor.remove();
  window.setTimeout(() => URL.revokeObjectURL(url), 0);
}
