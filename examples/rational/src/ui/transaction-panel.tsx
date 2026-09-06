import {
  type ChangeEvent,
  type KeyboardEvent,
  useCallback,
  useEffect,
  useMemo,
  useState,
} from "react";

import type { RationalApp } from "../data/rational.js";
import type { Receipt } from "../data/receipts.js";
import type { HouseholdWrites } from "../data/writes.js";
import {
  type Account,
  isCategory,
  isGroup,
  isMerchant,
  isTag,
  type Recurrence,
  type TaxonomyEntry,
  type Transaction,
} from "../model/types.js";
import { cleanDescription, merchantResolver } from "../selectors/merchants.js";
import { amountToText, formatMinorUnits, parseAmount } from "../selectors/money.js";
import { normalizeDescription } from "../selectors/transactions.js";
import {
  otherLeg,
  pairingCandidates,
  selectTransferSuggestions,
  suggestionsInvolving,
} from "../selectors/transfers.js";
import { routeHash } from "./router.js";

/**
 * One transaction, close up.
 *
 * The list shows what a row can carry; this is where a person changes one
 * thing about one transaction without opening the whole editor -- the
 * description the bank mangled, the category a rule got wrong, the merchant
 * nobody has named yet, whether it has been looked at, whether it counts. Each
 * field is its own patch through `HouseholdWrites`, so a change lands the
 * moment it is made and the other member sees it as one small edit rather than
 * a rewrite of the document. Splits stay in the editor: they are the one edit
 * that has to be checked as a whole before anything is written.
 */
const INTERVALS: ReadonlyArray<{ readonly value: Recurrence["interval"]; readonly label: string }> =
  [
    { value: "weekly", label: "Every week" },
    { value: "biweekly", label: "Every two weeks" },
    { value: "monthly", label: "Every month" },
    { value: "quarterly", label: "Every quarter" },
    { value: "yearly", label: "Every year" },
  ];

export function TransactionPanel({
  app,
  transaction,
  transactions,
  accounts,
  taxonomy,
  onClose,
  onEdit,
  onDeleted,
}: {
  app: RationalApp;
  transaction: Transaction;
  /** Every transaction of the household, for the transfer the panel may pair. */
  transactions: readonly Transaction[];
  accounts: readonly Account[];
  taxonomy: readonly TaxonomyEntry[];
  onClose: () => void;
  /** Open the full editor -- the only place splits are changed. */
  onEdit: (transaction: Transaction) => void;
  onDeleted: () => void;
}) {
  const [problem, setProblem] = useState<string | null>(null);
  const [interval, setInterval] = useState<Recurrence["interval"]>("monthly");
  const [pairWith, setPairWith] = useState("");
  const resolve = useMemo(() => merchantResolver(taxonomy), [taxonomy]);
  const merchant = resolve(transaction);
  const merchants = useMemo(
    () => taxonomy.filter(isMerchant).sort((left, right) => left.name.localeCompare(right.name)),
    [taxonomy],
  );
  const tags = useMemo(() => taxonomy.filter(isTag), [taxonomy]);
  const accountName = (id: string) => accounts.find((account) => account.id === id)?.name ?? id;
  const account = accounts.find((candidate) => candidate.id === transaction.account_id);
  const suggestions = suggestionsInvolving(selectTransferSuggestions(transactions), transaction.id);
  const paired = otherLeg(transactions, transaction);
  const candidates = useMemo(
    () => pairingCandidates(transactions, transaction),
    [transactions, transaction],
  );

  /** Every write from the panel goes through here so a refusal is shown, not swallowed. */
  const run = async (action: (writes: HouseholdWrites) => Promise<unknown>) => {
    const writes = app.writes;
    if (writes === null) return;
    setProblem(null);
    try {
      await action(writes);
    } catch (error) {
      setProblem(error instanceof Error ? error.message : "The change could not be saved.");
    }
  };

  const patch = (fields: Parameters<HouseholdWrites["updateTransaction"]>[1]) =>
    run((writes) => writes.updateTransaction(transaction.id, fields));

  const commitAmount = (text: string) => {
    let amount: number;
    try {
      amount = parseAmount(text, transaction.currency);
    } catch (error) {
      setProblem(error instanceof Error ? error.message : "That is not an amount.");
      return;
    }
    void patch({ amount });
  };

  const createMerchant = () =>
    run(async (writes) => {
      const normalized = normalizeDescription(transaction.description);
      const name = cleanDescription(normalized) || transaction.description.trim();
      const created = await writes.createMerchant(name, [transaction.description]);
      await writes.setMerchant(transaction.id, created.id);
    });

  const pair = (outflowId: string, inflowId: string) =>
    run((writes) => writes.pairTransfer(outflowId, inflowId));

  const pairManually = () => {
    const other = transactions.find((candidate) => candidate.id === pairWith);
    if (other === undefined) return;
    const [outflow, inflow] = transaction.amount < 0 ? [transaction, other] : [other, transaction];
    setPairWith("");
    void pair(outflow.id, inflow.id);
  };

  const remove = () =>
    run(async (writes) => {
      // A receipt outlives its transaction unless something removes it: the
      // bucket knows nothing about the document that referred to it.
      await app.receipts?.removeAll(transaction.id).catch(() => undefined);
      await writes.deleteTransaction(transaction.id);
      onDeleted();
    });

  const ruleHash = `${routeHash({ name: "settings", page: "rules" })}?from=${encodeURIComponent(
    transaction.id,
  )}`;
  const split = transaction.splits.length > 0;

  return (
    <aside
      className="detail-panel"
      aria-label="Transaction details"
      data-testid="transaction-panel"
      data-transaction-id={transaction.id}
    >
      <div className="section-heading">
        <div>
          <h2 data-testid="panel-title">{merchant.name}</h2>
          <p className="muted">
            {account?.name ?? transaction.account_id} · {transaction.date}
          </p>
        </div>
        <button type="button" className="link" onClick={onClose}>
          Close
        </button>
      </div>
      <p
        className={`panel-amount${transaction.amount > 0 ? " positive" : ""}`}
        data-testid="panel-amount"
      >
        {formatMinorUnits(transaction.amount, transaction.currency)}
      </p>
      <div className="panel-markers">
        {transaction.pending === true ? <span className="chip marker pending">pending</span> : null}
        {transaction.transfer_id === undefined ? null : (
          <span className="chip marker transfer">transfer</span>
        )}
        {transaction.hidden === true ? <span className="chip marker hidden">hidden</span> : null}
        {transaction.adjustment === true ? (
          <span className="chip marker adjustment">balance update</span>
        ) : null}
        {transaction.rule_id === undefined ? null : (
          <span className="chip marker">filed by a rule</span>
        )}
      </div>
      {problem === null ? null : (
        <p className="notice error" role="alert" data-testid="panel-error">
          {problem}
        </p>
      )}

      <section aria-labelledby="panel-fields-title">
        <h3 id="panel-fields-title">Details</h3>
        <div className="field-grid">
          <CommitField
            label="Description"
            value={transaction.description}
            onCommit={(description) => void patch({ description })}
          />
          <label>
            Date
            <input
              type="date"
              value={transaction.date}
              onChange={(event) => {
                if (event.target.value !== "") void patch({ date: event.target.value });
              }}
            />
          </label>
          <CommitField
            label={`Amount (${transaction.currency})`}
            value={amountToText(transaction.amount, transaction.currency)}
            inputMode="decimal"
            onCommit={commitAmount}
          />
          <label>
            Account
            <select
              value={transaction.account_id}
              onChange={(event) => void patch({ account_id: event.target.value })}
            >
              {accounts.map((candidate) => (
                <option key={candidate.id} value={candidate.id}>
                  {candidate.name}
                </option>
              ))}
            </select>
          </label>
          <label>
            Category
            <select
              value={split ? "" : (transaction.category_id ?? "")}
              disabled={split}
              onChange={(event) =>
                void patch({ category_id: event.target.value === "" ? null : event.target.value })
              }
            >
              <CategoryOptions taxonomy={taxonomy} blankLabel={split ? "split" : "Uncategorized"} />
            </select>
          </label>
          <CommitField
            label="Notes"
            value={transaction.notes ?? ""}
            onCommit={(notes) => void patch({ notes: notes.trim() === "" ? null : notes })}
          />
        </div>
        <fieldset className="tags">
          <legend>Tags</legend>
          {tags.length === 0 ? <small>No tags yet.</small> : null}
          {tags.map((tag) => (
            <label key={tag.id} className="chip-option">
              <input
                type="checkbox"
                checked={transaction.tags.includes(tag.id)}
                onChange={(event) =>
                  void patch({
                    tags: event.target.checked
                      ? [...transaction.tags, tag.id]
                      : transaction.tags.filter((candidate) => candidate !== tag.id),
                  })
                }
              />
              {tag.name}
            </label>
          ))}
        </fieldset>
        {split ? (
          <p className="hint">
            Split {transaction.splits.length} ways; the splits are edited in the editor.
          </p>
        ) : null}
        <div className="actions">
          <button type="button" className="secondary" onClick={() => onEdit(transaction)}>
            {split ? "Edit splits" : "Open in editor"}
          </button>
        </div>
      </section>

      <section aria-labelledby="panel-merchant-title">
        <h3 id="panel-merchant-title">Merchant</h3>
        <label>
          Merchant
          <select
            aria-label="Merchant"
            value={transaction.merchant_id ?? ""}
            onChange={(event) =>
              void run((writes) =>
                writes.setMerchant(
                  transaction.id,
                  event.target.value === "" ? null : event.target.value,
                ),
              )
            }
          >
            <option value="">
              {merchant.id === null || transaction.merchant_id === undefined
                ? `Automatic (${merchant.name})`
                : "Automatic"}
            </option>
            {merchants.map((candidate) => (
              <option key={candidate.id} value={candidate.id}>
                {candidate.name}
              </option>
            ))}
          </select>
        </label>
        {merchant.id === null ? (
          <div className="actions">
            <button type="button" className="secondary" onClick={() => void createMerchant()}>
              Create merchant from description
            </button>
          </div>
        ) : null}
      </section>

      <section aria-labelledby="panel-flags-title">
        <h3 id="panel-flags-title">Review</h3>
        <label className="chip-option">
          <input
            type="checkbox"
            checked={transaction.reviewed === true}
            onChange={(event) =>
              void run((writes) => writes.markReviewed([transaction.id], event.target.checked))
            }
          />
          Reviewed
        </label>
        <label className="chip-option">
          <input
            type="checkbox"
            checked={transaction.hidden === true}
            onChange={(event) =>
              void run((writes) => writes.setHidden([transaction.id], event.target.checked))
            }
          />
          Hidden from budgets and reports
        </label>
      </section>

      <section aria-labelledby="panel-recurring-title">
        <h3 id="panel-recurring-title">Recurring</h3>
        {transaction.recurrence_id === undefined ? (
          <div className="inline-controls">
            <label>
              Interval
              <select
                aria-label="Recurrence interval"
                value={interval}
                onChange={(event) => setInterval(event.target.value as Recurrence["interval"])}
              >
                {INTERVALS.map((entry) => (
                  <option key={entry.value} value={entry.value}>
                    {entry.label}
                  </option>
                ))}
              </select>
            </label>
            <button
              type="button"
              className="secondary"
              onClick={() => void run((writes) => writes.markRecurring(transaction, interval))}
            >
              Mark recurring
            </button>
          </div>
        ) : (
          <p className="hint" data-testid="panel-recurrence">
            Part of a recurring bill.{" "}
            <a href={routeHash({ name: "recurring" })}>See recurring charges</a>
          </p>
        )}
      </section>

      <section aria-labelledby="panel-transfer-title" data-testid="panel-transfer">
        <h3 id="panel-transfer-title">Transfer</h3>
        {transaction.transfer_id !== undefined ? (
          <div>
            {paired === null ? (
              <p className="hint">The other leg of this transfer is not on this device.</p>
            ) : (
              <p data-testid="transfer-other-leg">
                Paired with <strong>{paired.description}</strong> on{" "}
                {accountName(paired.account_id)} · {paired.date} ·{" "}
                {formatMinorUnits(paired.amount, paired.currency)}
              </p>
            )}
            <div className="actions">
              <button
                type="button"
                className="secondary"
                onClick={() =>
                  void run((writes) => writes.unpairTransfer(transaction.transfer_id ?? ""))
                }
              >
                Unpair
              </button>
            </div>
          </div>
        ) : (
          <div>
            {suggestions.length === 0 ? (
              <p className="hint">No matching transaction within three days.</p>
            ) : (
              <ul className="suggestions" aria-label="Transfer suggestions">
                {suggestions.map((suggestion) => {
                  const otherId =
                    suggestion.outflowId === transaction.id
                      ? suggestion.inflowId
                      : suggestion.outflowId;
                  const other = transactions.find((candidate) => candidate.id === otherId);
                  return (
                    <li key={otherId} data-testid={`suggestion-${otherId}`}>
                      <span>
                        {other === undefined ? (
                          otherId
                        ) : (
                          <>
                            <strong>{other.description}</strong> on {accountName(other.account_id)}{" "}
                            · {other.date} · {formatMinorUnits(other.amount, other.currency)}
                          </>
                        )}
                        {suggestion.days === 0
                          ? " · same day"
                          : ` · ${suggestion.days} ${suggestion.days === 1 ? "day" : "days"} apart`}
                      </span>
                      <button
                        type="button"
                        onClick={() => void pair(suggestion.outflowId, suggestion.inflowId)}
                      >
                        Pair
                      </button>
                    </li>
                  );
                })}
              </ul>
            )}
            {candidates.length === 0 ? null : (
              <div className="inline-controls">
                <label>
                  Pair with
                  <select
                    aria-label="Pair with"
                    value={pairWith}
                    onChange={(event) => setPairWith(event.target.value)}
                  >
                    <option value="">Choose a transaction…</option>
                    {candidates.map((candidate) => (
                      <option key={candidate.id} value={candidate.id}>
                        {candidate.date} · {accountName(candidate.account_id)} ·{" "}
                        {formatMinorUnits(candidate.amount, candidate.currency)} ·{" "}
                        {candidate.description}
                      </option>
                    ))}
                  </select>
                </label>
                <button
                  type="button"
                  className="secondary"
                  disabled={pairWith === ""}
                  onClick={pairManually}
                >
                  Pair selected
                </button>
              </div>
            )}
          </div>
        )}
      </section>

      <section aria-labelledby="panel-receipts-title">
        <h3 id="panel-receipts-title">Receipts</h3>
        <ReceiptsPanel app={app} transaction={transaction} />
      </section>

      <div className="actions panel-footer">
        <a className="button-link" href={ruleHash} data-testid="create-rule">
          Create rule from this
        </a>
        <button type="button" className="secondary danger" onClick={() => void remove()}>
          Delete
        </button>
      </div>
    </aside>
  );
}

/**
 * A field that saves when the person is done with it -- on blur or Enter --
 * rather than on every keystroke, so a description being retyped is not
 * pushed to the other member letter by letter. The draft follows the stored
 * value when that changes underneath it, which is what a live edit from
 * another device looks like.
 */
function CommitField({
  label,
  value,
  onCommit,
  inputMode,
}: {
  label: string;
  value: string;
  onCommit: (value: string) => void;
  inputMode?: "decimal" | "text";
}) {
  const [draft, setDraft] = useState(value);
  useEffect(() => {
    setDraft(value);
  }, [value]);
  const commit = () => {
    if (draft !== value) onCommit(draft);
  };
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter") {
      event.preventDefault();
      commit();
    }
  };
  return (
    <label>
      {label}
      <input
        value={draft}
        {...(inputMode === undefined ? {} : { inputMode })}
        onChange={(event) => setDraft(event.target.value)}
        onBlur={commit}
        onKeyDown={onKeyDown}
      />
    </label>
  );
}

/**
 * The options of a category picker: a blank first, then every group as an
 * optgroup in the household's order with its categories in theirs, then the
 * categories that belong to no group. Archived categories are left out -- a
 * picker is for filing, and an archived category is one nobody files into.
 * Shared by the editor, the filters, the bulk bar, and the panel so every
 * picker in the screen reads the same.
 */
export function CategoryOptions({
  taxonomy,
  blankLabel,
  blankValue = "",
  extra,
}: {
  taxonomy: readonly TaxonomyEntry[];
  blankLabel: string;
  blankValue?: string;
  /** Options after the blank and before the groups; the filter's "Uncategorized". */
  extra?: ReadonlyArray<{ readonly value: string; readonly label: string }>;
}) {
  const byOrder = (left: TaxonomyEntry, right: TaxonomyEntry) =>
    (left.sort_order ?? Number.MAX_SAFE_INTEGER) - (right.sort_order ?? Number.MAX_SAFE_INTEGER) ||
    left.name.localeCompare(right.name);
  const groups = taxonomy.filter(isGroup).sort(byOrder);
  const categories = taxonomy
    .filter((entry) => isCategory(entry) && entry.archived !== true)
    .sort(byOrder);
  const grouped = new Set<string>();
  const sections = groups
    .map((group) => ({
      group,
      members: categories.filter((category) => category.parent_id === group.id),
    }))
    .filter((section) => section.members.length > 0);
  for (const section of sections) {
    for (const member of section.members) grouped.add(member.id);
  }
  const loose = categories.filter((category) => !grouped.has(category.id));
  return (
    <>
      <option value={blankValue}>{blankLabel}</option>
      {(extra ?? []).map((option) => (
        <option key={option.value} value={option.value}>
          {option.label}
        </option>
      ))}
      {sections.map((section) => (
        <optgroup key={section.group.id} label={section.group.name}>
          {section.members.map((category) => (
            <option key={category.id} value={category.id}>
              {category.name}
            </option>
          ))}
        </optgroup>
      ))}
      {loose.length === 0 ? null : (
        <optgroup label={sections.length === 0 ? "Categories" : "Other categories"}>
          {loose.map((category) => (
            <option key={category.id} value={category.id}>
              {category.name}
            </option>
          ))}
        </optgroup>
      )}
    </>
  );
}

/**
 * The receipts of one transaction: what the household has attached, whoever
 * attached it. The object carries the household as an attribute the bucket's
 * rules read, so every member opens the same file and nobody else can. With an
 * `onClose` it is the list's stand-alone panel; without one it sits inside the
 * detail panel.
 */
export function ReceiptsPanel({
  app,
  transaction,
  onClose,
}: {
  app: RationalApp;
  transaction: Transaction;
  onClose?: () => void;
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
    <div
      className={onClose === undefined ? "receipts" : "panel receipts"}
      data-testid="receipts-panel"
    >
      {onClose === undefined ? null : (
        <div className="section-heading">
          <h3>Receipts for {transaction.description}</h3>
          <button type="button" className="link" onClick={onClose}>
            Close
          </button>
        </div>
      )}
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
