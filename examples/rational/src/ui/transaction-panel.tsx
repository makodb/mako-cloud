import {
  Alert,
  AlertDescription,
  Badge,
  Button,
  Card,
  CardContent,
  CardHeader,
  Checkbox,
  Field,
  Input,
  Label,
  NativeSelect,
  Sheet,
  SheetBody,
  SheetContent,
  SheetDescription,
  SheetFooter,
  SheetHeader,
  SheetTitle,
  cn,
} from "@mako-cloud/ui";
import { Paperclip, Trash2 } from "lucide-react";
import {
  type ChangeEvent,
  type ComponentProps,
  type KeyboardEvent,
  type ReactNode,
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

/** The small-caps label a screen puts over a group of things. */
const EYEBROW = "m-0 text-xs font-semibold uppercase tracking-wider text-muted-foreground";

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
    <Sheet
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
    >
      <SheetContent
        side="right"
        aria-label="Transaction details"
        data-testid="transaction-panel"
        data-transaction-id={transaction.id}
        className="gap-0 sm:max-w-lg"
      >
        <SheetHeader className="gap-3 border-b pr-12">
          <div className="flex items-start justify-between gap-4">
            <div className="grid min-w-0 gap-1">
              <SheetTitle data-testid="panel-title" className="truncate text-lg">
                {merchant.name}
              </SheetTitle>
              <SheetDescription>
                {account?.name ?? transaction.account_id} · {transaction.date}
              </SheetDescription>
            </div>
            <p
              className={cn(
                "money m-0 shrink-0 text-2xl font-semibold tracking-tight",
                transaction.amount > 0 && "text-positive",
              )}
              data-testid="panel-amount"
            >
              {formatMinorUnits(transaction.amount, transaction.currency)}
            </p>
          </div>
          {transaction.pending !== true &&
          transaction.transfer_id === undefined &&
          transaction.hidden !== true &&
          transaction.adjustment !== true &&
          transaction.rule_id === undefined ? null : (
            <div className="flex flex-wrap gap-1.5">
              {transaction.pending === true ? (
                <MarkerBadge kind="pending">pending</MarkerBadge>
              ) : null}
              {transaction.transfer_id === undefined ? null : (
                <MarkerBadge kind="transfer">transfer</MarkerBadge>
              )}
              {transaction.hidden === true ? <MarkerBadge kind="hidden">hidden</MarkerBadge> : null}
              {transaction.adjustment === true ? (
                <MarkerBadge kind="plain">balance update</MarkerBadge>
              ) : null}
              {transaction.rule_id === undefined ? null : (
                <MarkerBadge kind="plain">filed by a rule</MarkerBadge>
              )}
            </div>
          )}
          {problem === null ? null : (
            <Alert variant="destructive" role="alert" data-testid="panel-error">
              <AlertDescription className="block">{problem}</AlertDescription>
            </Alert>
          )}
        </SheetHeader>

        <SheetBody className="gap-5 py-5">
          <PanelSection id="panel-fields-title" title="Details" className="border-t-0 pt-0">
            <div className="grid gap-3 sm:grid-cols-2">
              <CommitField
                id="panel-description"
                label="Description"
                value={transaction.description}
                onCommit={(description) => void patch({ description })}
              />
              <Field label="Date" htmlFor="panel-date">
                <Input
                  id="panel-date"
                  type="date"
                  value={transaction.date}
                  onChange={(event) => {
                    if (event.target.value !== "") void patch({ date: event.target.value });
                  }}
                />
              </Field>
              <CommitField
                id="panel-amount-field"
                label={`Amount (${transaction.currency})`}
                value={amountToText(transaction.amount, transaction.currency)}
                inputMode="decimal"
                onCommit={commitAmount}
              />
              <Field label="Account" htmlFor="panel-account">
                <NativeSelect
                  id="panel-account"
                  value={transaction.account_id}
                  onChange={(event) => void patch({ account_id: event.target.value })}
                >
                  {accounts.map((candidate) => (
                    <option key={candidate.id} value={candidate.id}>
                      {candidate.name}
                    </option>
                  ))}
                </NativeSelect>
              </Field>
              <Field label="Category" htmlFor="panel-category">
                <NativeSelect
                  id="panel-category"
                  value={split ? "" : (transaction.category_id ?? "")}
                  disabled={split}
                  onChange={(event) =>
                    void patch({
                      category_id: event.target.value === "" ? null : event.target.value,
                    })
                  }
                >
                  <CategoryOptions
                    taxonomy={taxonomy}
                    blankLabel={split ? "split" : "Uncategorized"}
                  />
                </NativeSelect>
              </Field>
              <CommitField
                id="panel-notes"
                label="Notes"
                value={transaction.notes ?? ""}
                onCommit={(notes) => void patch({ notes: notes.trim() === "" ? null : notes })}
              />
            </div>
            <fieldset className="m-0 grid min-w-0 gap-2 border-0 p-0">
              <legend className="mb-2 p-0 text-sm font-medium leading-none">Tags</legend>
              {tags.length === 0 ? (
                <small className="text-xs text-muted-foreground">No tags yet.</small>
              ) : null}
              <div className="flex flex-wrap gap-x-4 gap-y-2">
                {tags.map((tag) => (
                  <div key={tag.id} className="flex items-center gap-2">
                    <Checkbox
                      id={`panel-tag-${tag.id}`}
                      checked={transaction.tags.includes(tag.id)}
                      onCheckedChange={(checked) =>
                        void patch({
                          tags:
                            checked === true
                              ? [...transaction.tags, tag.id]
                              : transaction.tags.filter((candidate) => candidate !== tag.id),
                        })
                      }
                    />
                    <Label htmlFor={`panel-tag-${tag.id}`} className="font-normal">
                      {tag.name}
                    </Label>
                  </div>
                ))}
              </div>
            </fieldset>
            {split ? (
              <p className="m-0 text-sm text-muted-foreground">
                Split {transaction.splits.length} ways; the splits are edited in the editor.
              </p>
            ) : null}
            <div className="flex flex-wrap gap-2">
              <Button variant="secondary" onClick={() => onEdit(transaction)}>
                {split ? "Edit splits" : "Open in editor"}
              </Button>
            </div>
          </PanelSection>

          <PanelSection id="panel-merchant-title" title="Merchant">
            <Field label="Merchant" htmlFor="panel-merchant">
              <NativeSelect
                id="panel-merchant"
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
              </NativeSelect>
            </Field>
            {merchant.id === null ? (
              <div className="flex flex-wrap gap-2">
                <Button variant="secondary" onClick={() => void createMerchant()}>
                  Create merchant from description
                </Button>
              </div>
            ) : null}
          </PanelSection>

          <PanelSection id="panel-flags-title" title="Review">
            <div className="flex items-center gap-2">
              <Checkbox
                id="panel-reviewed"
                checked={transaction.reviewed === true}
                onCheckedChange={(checked) =>
                  void run((writes) => writes.markReviewed([transaction.id], checked === true))
                }
              />
              <Label htmlFor="panel-reviewed" className="font-normal">
                Reviewed
              </Label>
            </div>
            <div className="flex items-center gap-2">
              <Checkbox
                id="panel-hidden"
                checked={transaction.hidden === true}
                onCheckedChange={(checked) =>
                  void run((writes) => writes.setHidden([transaction.id], checked === true))
                }
              />
              <Label htmlFor="panel-hidden" className="font-normal">
                Hidden from budgets and reports
              </Label>
            </div>
          </PanelSection>

          <PanelSection id="panel-recurring-title" title="Recurring">
            {transaction.recurrence_id === undefined ? (
              <div className="flex flex-wrap items-end gap-2">
                <Field label="Interval" htmlFor="panel-interval" className="min-w-40 flex-1">
                  <NativeSelect
                    id="panel-interval"
                    aria-label="Recurrence interval"
                    value={interval}
                    onChange={(event) => setInterval(event.target.value as Recurrence["interval"])}
                  >
                    {INTERVALS.map((entry) => (
                      <option key={entry.value} value={entry.value}>
                        {entry.label}
                      </option>
                    ))}
                  </NativeSelect>
                </Field>
                <Button
                  variant="secondary"
                  onClick={() => void run((writes) => writes.markRecurring(transaction, interval))}
                >
                  Mark recurring
                </Button>
              </div>
            ) : (
              <p className="m-0 text-sm text-muted-foreground" data-testid="panel-recurrence">
                Part of a recurring bill.{" "}
                <a href={routeHash({ name: "recurring" })}>See recurring charges</a>
              </p>
            )}
          </PanelSection>

          <PanelSection id="panel-transfer-title" title="Transfer" data-testid="panel-transfer">
            {transaction.transfer_id !== undefined ? (
              <div className="grid gap-3">
                {paired === null ? (
                  <p className="m-0 text-sm text-muted-foreground">
                    The other leg of this transfer is not on this device.
                  </p>
                ) : (
                  <p className="m-0 text-sm" data-testid="transfer-other-leg">
                    Paired with <strong>{paired.description}</strong> on{" "}
                    {accountName(paired.account_id)} · {paired.date} ·{" "}
                    {formatMinorUnits(paired.amount, paired.currency)}
                  </p>
                )}
                <div className="flex flex-wrap gap-2">
                  <Button
                    variant="secondary"
                    onClick={() =>
                      void run((writes) => writes.unpairTransfer(transaction.transfer_id ?? ""))
                    }
                  >
                    Unpair
                  </Button>
                </div>
              </div>
            ) : (
              <div className="grid gap-3">
                {suggestions.length === 0 ? (
                  <p className="m-0 text-sm text-muted-foreground">
                    No matching transaction within three days.
                  </p>
                ) : (
                  <ul className="m-0 grid list-none gap-2 p-0" aria-label="Transfer suggestions">
                    {suggestions.map((suggestion) => {
                      const otherId =
                        suggestion.outflowId === transaction.id
                          ? suggestion.inflowId
                          : suggestion.outflowId;
                      const other = transactions.find((candidate) => candidate.id === otherId);
                      return (
                        <li
                          key={otherId}
                          data-testid={`suggestion-${otherId}`}
                          className="flex items-center justify-between gap-3 rounded-lg border bg-muted/40 px-3 py-2 text-sm"
                        >
                          <span className="min-w-0">
                            {other === undefined ? (
                              otherId
                            ) : (
                              <>
                                <strong>{other.description}</strong> on{" "}
                                {accountName(other.account_id)} · {other.date} ·{" "}
                                {formatMinorUnits(other.amount, other.currency)}
                              </>
                            )}
                            {suggestion.days === 0
                              ? " · same day"
                              : ` · ${suggestion.days} ${suggestion.days === 1 ? "day" : "days"} apart`}
                          </span>
                          <Button
                            size="sm"
                            onClick={() => void pair(suggestion.outflowId, suggestion.inflowId)}
                          >
                            Pair
                          </Button>
                        </li>
                      );
                    })}
                  </ul>
                )}
                {candidates.length === 0 ? null : (
                  <div className="flex flex-wrap items-end gap-2">
                    <Field label="Pair with" htmlFor="panel-pair-with" className="min-w-40 flex-1">
                      <NativeSelect
                        id="panel-pair-with"
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
                      </NativeSelect>
                    </Field>
                    <Button variant="secondary" disabled={pairWith === ""} onClick={pairManually}>
                      Pair selected
                    </Button>
                  </div>
                )}
              </div>
            )}
          </PanelSection>

          <PanelSection id="panel-receipts-title" title="Receipts">
            <ReceiptsPanel app={app} transaction={transaction} />
          </PanelSection>
        </SheetBody>

        <SheetFooter className="flex-row items-center justify-between">
          <Button asChild variant="outline">
            <a
              className="no-underline hover:no-underline"
              href={ruleHash}
              data-testid="create-rule"
            >
              Create rule from this
            </a>
          </Button>
          <Button
            variant="outline"
            className="border-destructive/40 text-destructive hover:bg-destructive/10 hover:text-destructive"
            onClick={() => void remove()}
          >
            <Trash2 aria-hidden="true" />
            Delete
          </Button>
        </SheetFooter>
      </SheetContent>
    </Sheet>
  );
}

/** One of the panel's stacked sections: a small-caps title over its content. */
function PanelSection({
  id,
  title,
  className,
  children,
  ...rest
}: ComponentProps<"section"> & { id: string; title: string; children: ReactNode }) {
  return (
    <section aria-labelledby={id} className={cn("grid gap-3 border-t pt-5", className)} {...rest}>
      <h3 id={id} className={EYEBROW}>
        {title}
      </h3>
      {children}
    </section>
  );
}

/**
 * The words a row or the panel hangs on a transaction: what it waits for,
 * what it is part of, what it is left out of. Each kind has one look so the
 * list and the panel agree.
 */
export function MarkerBadge({
  kind,
  children,
  ...rest
}: ComponentProps<typeof Badge> & {
  kind: "review" | "transfer" | "hidden" | "pending" | "plain";
}) {
  if (kind === "review") {
    return (
      <Badge variant="warning" {...rest}>
        {children}
      </Badge>
    );
  }
  return (
    <Badge
      variant="outline"
      className={cn(
        "border-dashed",
        kind === "transfer" && "border-primary/50 text-primary",
        (kind === "hidden" || kind === "pending") && "text-muted-foreground",
      )}
      {...rest}
    >
      {children}
    </Badge>
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
  id,
  label,
  value,
  onCommit,
  inputMode,
}: {
  id: string;
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
    <Field label={label} htmlFor={id}>
      <Input
        id={id}
        value={draft}
        {...(inputMode === undefined ? {} : { inputMode })}
        className={inputMode === "decimal" ? "tabular-nums" : undefined}
        onChange={(event) => setDraft(event.target.value)}
        onBlur={commit}
        onKeyDown={onKeyDown}
      />
    </Field>
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

  const fileId = `receipt-file-${transaction.id}`;
  const body = (
    <>
      {problem === null ? null : (
        <Alert variant="destructive" role="alert">
          <AlertDescription className="block">{problem}</AlertDescription>
        </Alert>
      )}
      <Field label="Attach an image or a PDF" htmlFor={fileId}>
        <Input
          id={fileId}
          type="file"
          accept="image/*,application/pdf"
          disabled={busy}
          onChange={attach}
          className="cursor-pointer pt-1.5 text-muted-foreground"
        />
      </Field>
      {receipts === null ? (
        <p className="m-0 text-sm text-muted-foreground" role="status">
          Loading receipts…
        </p>
      ) : receipts.length === 0 ? (
        <p className="m-0 text-sm text-muted-foreground">No receipts yet.</p>
      ) : (
        <ul aria-label="Receipts" className="m-0 grid list-none gap-1 p-0">
          {receipts.map((receipt) => (
            <li
              key={receipt.path}
              data-testid={`receipt-${receipt.name}`}
              className="flex items-center gap-2 rounded-md border bg-muted/40 px-3 py-1.5 text-sm"
            >
              <Paperclip aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
              <Button
                variant="link"
                size="sm"
                className="h-auto min-w-0 justify-start truncate p-0 text-sm"
                onClick={() => void open(receipt.path)}
              >
                {receipt.name}
              </Button>
              <small className="text-xs text-muted-foreground">
                {" "}
                {Math.ceil(receipt.sizeBytes / 1024)} KB
              </small>
              <Button
                variant="link"
                size="sm"
                className="ml-auto h-auto p-0 text-xs text-muted-foreground hover:text-destructive"
                onClick={() => void app.receipts?.remove(receipt.path).then(reload)}
              >
                Remove
              </Button>
            </li>
          ))}
        </ul>
      )}
    </>
  );

  if (onClose === undefined) {
    return (
      <div className="grid gap-3" data-testid="receipts-panel">
        {body}
      </div>
    );
  }
  return (
    <Card data-testid="receipts-panel">
      <CardHeader className="flex flex-row items-start justify-between gap-4">
        <h3 className="m-0 text-base font-semibold leading-none">
          Receipts for {transaction.description}
        </h3>
        <Button variant="ghost" size="sm" className="-my-1.5 shrink-0" onClick={onClose}>
          Close
        </Button>
      </CardHeader>
      <CardContent className="grid gap-3">{body}</CardContent>
    </Card>
  );
}
