import { type FormEvent, type KeyboardEvent, useState } from "react";

import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import type { CategoryDeletion, HouseholdWrites, Patch } from "../data/writes.js";
import {
  type BudgetBucket,
  type Category,
  type CategoryGroup,
  type CategoryKind,
  type HouseholdCollectionId,
  isCategory,
  isGroup,
  type TaxonomyEntry,
} from "../model/types.js";
import { memoizeLast } from "../selectors/memo.js";
import { amountToText, parseAmount } from "../selectors/money.js";
import { useBehavior, useQuery } from "./hooks.js";
import "./styles/taxonomy.css";

/**
 * Categories, in their groups.
 *
 * Monarch's shape: a group is a heading with a kind -- income, expense,
 * transfer -- and the categories under it are what transactions are filed
 * into. Both are documents in the `taxonomy` collection behind a `kind`, so
 * the screen reads the collection once and takes it apart here. Order is a
 * `sort_order` on each document; moving a row rewrites the positions of its
 * neighbours rather than inventing a fractional number, so every device that
 * sorts by position agrees on the list. A brand-new household is seeded with
 * the default set on first open (`rational.ts`), which is why an empty list
 * here is only ever a moment old.
 */
const KINDS: ReadonlyArray<{ readonly kind: CategoryKind; readonly label: string }> = [
  { kind: "expense", label: "Expense" },
  { kind: "income", label: "Income" },
  { kind: "transfer", label: "Transfer" },
];

const BUCKETS: ReadonlyArray<{ readonly bucket: BudgetBucket; readonly label: string }> = [
  { bucket: "fixed", label: "Fixed" },
  { bucket: "flexible", label: "Flexible" },
  { bucket: "non_monthly", label: "Non-monthly" },
];

function isBucket(value: string): value is BudgetBucket {
  return BUCKETS.some((entry) => entry.bucket === value);
}

/** An entry that was never positioned sorts after every one that was. */
const UNPOSITIONED = Number.MAX_SAFE_INTEGER;

function byPosition(left: TaxonomyEntry, right: TaxonomyEntry): number {
  return (
    (left.sort_order ?? UNPOSITIONED) - (right.sort_order ?? UNPOSITIONED) ||
    left.name.localeCompare(right.name) ||
    left.id.localeCompare(right.id)
  );
}

export interface TaxonomySection {
  /** Null for the categories that belong to no group, or to a group that is gone. */
  readonly group: CategoryGroup | null;
  readonly categories: readonly Category[];
}

/**
 * The groups in their order, each with its categories in theirs, and one
 * trailing section for whatever belongs to no group -- a category made before
 * groups existed, or whose group was deleted on another device.
 */
export function taxonomyTree(entries: readonly TaxonomyEntry[]): readonly TaxonomySection[] {
  const groups = entries.filter(isGroup).sort(byPosition);
  const members = new Map<string, Category[]>(groups.map((group) => [group.id, []]));
  const ungrouped: Category[] = [];
  for (const entry of entries) {
    if (!isCategory(entry)) continue;
    const home = entry.parent_id === undefined ? undefined : members.get(entry.parent_id);
    if (home === undefined) ungrouped.push(entry);
    else home.push(entry);
  }
  const sections: TaxonomySection[] = groups.map((group) => ({
    group,
    categories: (members.get(group.id) ?? []).sort(byPosition),
  }));
  if (ungrouped.length > 0) sections.push({ group: null, categories: ungrouped.sort(byPosition) });
  return sections;
}

const selectTaxonomyTree = memoizeLast(taxonomyTree);

/** The list with one id moved a step; null when the move is impossible. */
function swapped(ids: readonly string[], id: string, direction: "up" | "down"): string[] | null {
  const ordered = [...ids];
  const index = ordered.indexOf(id);
  const target = direction === "up" ? index - 1 : index + 1;
  if (index === -1 || target < 0 || target >= ordered.length) return null;
  const moved = ordered[index];
  const neighbour = ordered[target];
  if (moved === undefined || neighbour === undefined) return null;
  ordered[index] = neighbour;
  ordered[target] = moved;
  return ordered;
}

function plural(count: number, noun: string): string {
  return `${count} ${count === 1 ? noun : `${noun}s`}`;
}

function describeDeletion(
  name: string,
  targetName: string | null,
  outcome: CategoryDeletion,
): string {
  const where = targetName === null ? "left uncategorized" : `moved to ${targetName}`;
  return `Deleted ${name}: ${plural(outcome.transactions, "transaction")} ${where}, ${plural(
    outcome.budgets,
    "budget",
  )} removed, ${plural(outcome.rules, "rule")} updated.`;
}

export function CategoriesScreen({
  app,
  session,
}: {
  app: RationalApp;
  session: ScopeSession<HouseholdCollectionId>;
}) {
  const entries = useQuery(session.collection("taxonomy")?.find() ?? null);
  // The household's currency is on its directory document, not in the
  // household database; the non-monthly targets are typed in it.
  const state = useBehavior(app.state$);
  const currency =
    state.households.find((household) => household.id === state.currentHouseholdId)?.currency ??
    "USD";
  const sections = selectTaxonomyTree(entries);
  const groups = sections
    .map((section) => section.group)
    .filter((group): group is CategoryGroup => group !== null);
  const categories = sections.flatMap((section) => section.categories);

  const [adding, setAdding] = useState<"category" | "group" | null>(null);
  const [renaming, setRenaming] = useState<{ id: string; name: string } | null>(null);
  const [deleting, setDeleting] = useState<Category | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [newGroupId, setNewGroupId] = useState("");
  const [newKind, setNewKind] = useState<CategoryKind>("expense");

  /** Run one write, turning a refusal into the message under the heading. */
  const attempt = async (
    action: (writes: HouseholdWrites) => Promise<unknown>,
    fallback: string,
  ): Promise<boolean> => {
    setError(null);
    const writes = app.writes;
    if (writes === null) {
      setError("No household is open.");
      return false;
    }
    try {
      await action(writes);
      return true;
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : fallback);
      return false;
    }
  };

  const createCategory = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    const group = groups.find((candidate) => candidate.id === newGroupId) ?? null;
    // A category takes its kind from its group; only an ungrouped one is asked.
    const kind = group?.category_kind ?? newKind;
    const icon = String(data.get("icon") ?? "").trim();
    const bucket = String(data.get("budget_bucket") ?? "");
    const saved = await attempt(
      (writes) =>
        writes.createCategory(String(data.get("name") ?? ""), kind, {
          ...(group === null ? {} : { groupId: group.id }),
          ...(icon === "" ? {} : { icon }),
          ...(kind === "expense" && isBucket(bucket) ? { budgetBucket: bucket } : {}),
        }),
      "The category could not be saved.",
    );
    if (saved) form.reset();
  };

  const createGroup = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    const kind = String(data.get("kind") ?? "expense");
    const last = groups.reduce((max, group) => Math.max(max, group.sort_order ?? -1), -1);
    const saved = await attempt(
      (writes) =>
        writes.createGroup(
          String(data.get("name") ?? ""),
          KINDS.some((entry) => entry.kind === kind) ? (kind as CategoryKind) : "expense",
          last + 1,
        ),
      "The group could not be saved.",
    );
    if (saved) form.reset();
  };

  const saveName = async (entry: TaxonomyEntry) => {
    if (renaming === null || renaming.id !== entry.id) return;
    const patch = { name: renaming.name };
    const saved = await attempt(
      (writes) =>
        isGroup(entry)
          ? writes.updateGroup(entry.id, patch)
          : writes.updateCategory(entry.id, patch),
      "The name could not be saved.",
    );
    if (saved) setRenaming(null);
  };

  /**
   * Groups are moved by rewriting every position that is not already right,
   * from the list as shown: the same result as swapping two numbers when the
   * positions are consecutive, and a repair when they are not.
   */
  const moveGroup = (group: CategoryGroup, direction: "up" | "down") =>
    void attempt(async (writes) => {
      const ordered = swapped(
        groups.map((candidate) => candidate.id),
        group.id,
        direction,
      );
      if (ordered === null) return;
      for (const [position, id] of ordered.entries()) {
        const current = groups.find((candidate) => candidate.id === id);
        if (current !== undefined && current.sort_order !== position) {
          await writes.updateGroup(id, { sort_order: position });
        }
      }
    }, "The group could not be moved.");

  const moveCategory = (section: TaxonomySection, category: Category, direction: "up" | "down") =>
    void attempt(async (writes) => {
      const ordered = swapped(
        section.categories.map((candidate) => candidate.id),
        category.id,
        direction,
      );
      if (ordered !== null) await writes.reorderCategories(ordered);
    }, "The category could not be moved.");

  const deleteGroup = (group: CategoryGroup) =>
    void attempt((writes) => writes.deleteGroup(group.id), "The group could not be deleted.");

  const confirmDelete = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (deleting === null) return;
    const category = deleting;
    const target = String(new FormData(event.currentTarget).get("reassign_to") ?? "");
    const targetName =
      target === "" ? null : (categories.find((entry) => entry.id === target)?.name ?? target);
    const deleted = await attempt(async (writes) => {
      const outcome = await writes.deleteCategory(category.id, target === "" ? null : target);
      setStatus(describeDeletion(category.name, targetName, outcome));
    }, "The category could not be deleted.");
    if (deleted) setDeleting(null);
  };

  const renameKeys = (entry: TaxonomyEntry) => (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter") {
      event.preventDefault();
      void saveName(entry);
    }
    if (event.key === "Escape") setRenaming(null);
  };

  const nameCell = (entry: TaxonomyEntry) =>
    renaming?.id === entry.id ? (
      <input
        aria-label={`Rename ${entry.name}`}
        value={renaming.name}
        onChange={(event) => setRenaming({ id: entry.id, name: event.target.value })}
        onKeyDown={renameKeys(entry)}
      />
    ) : (
      <span data-testid="name">
        {entry.name}
        {entry.archived === true ? <small> archived</small> : null}
      </span>
    );

  const renameButton = (entry: TaxonomyEntry) =>
    renaming?.id === entry.id ? (
      <button type="button" className="link" onClick={() => void saveName(entry)}>
        Save
      </button>
    ) : (
      <button
        type="button"
        className="link"
        onClick={() => setRenaming({ id: entry.id, name: entry.name })}
      >
        Rename
      </button>
    );

  const selectedGroup = groups.find((group) => group.id === newGroupId) ?? null;
  const newCategoryKind = selectedGroup?.category_kind ?? newKind;

  return (
    <section aria-labelledby="categories-title" data-testid="categories-screen">
      <div className="heading">
        <h1 id="categories-title">Categories</h1>
        <div className="taxonomy-inline">
          <button
            type="button"
            className={adding === "category" ? undefined : "secondary"}
            onClick={() => setAdding(adding === "category" ? null : "category")}
          >
            New category
          </button>
          <button
            type="button"
            className={adding === "group" ? undefined : "secondary"}
            onClick={() => setAdding(adding === "group" ? null : "group")}
          >
            New group
          </button>
        </div>
      </div>
      {error === null ? null : (
        <p className="notice error" role="alert">
          {error}
        </p>
      )}
      {status === null ? null : (
        <p className="taxonomy-status" role="status" data-testid="category-status">
          {status}
        </p>
      )}

      {adding === "category" ? (
        <form
          className="editor"
          aria-label="New category"
          onSubmit={(event) => void createCategory(event)}
        >
          <div className="grid">
            <label>
              Name
              <input name="name" required maxLength={200} />
            </label>
            <label>
              Group
              <select
                name="group_id"
                value={newGroupId}
                onChange={(event) => setNewGroupId(event.target.value)}
              >
                <option value="">No group</option>
                {groups.map((group) => (
                  <option key={group.id} value={group.id}>
                    {group.name}
                  </option>
                ))}
              </select>
            </label>
            {selectedGroup === null ? (
              <label>
                Kind
                <select
                  name="kind"
                  value={newKind}
                  onChange={(event) => setNewKind(event.target.value as CategoryKind)}
                >
                  {KINDS.map((entry) => (
                    <option key={entry.kind} value={entry.kind}>
                      {entry.label}
                    </option>
                  ))}
                </select>
              </label>
            ) : null}
            <label>
              Icon
              <input name="icon" maxLength={16} placeholder="🛒" />
            </label>
            {newCategoryKind === "expense" ? (
              <label>
                Budget bucket
                <select name="budget_bucket" defaultValue="flexible">
                  {BUCKETS.map((entry) => (
                    <option key={entry.bucket} value={entry.bucket}>
                      {entry.label}
                    </option>
                  ))}
                </select>
              </label>
            ) : null}
          </div>
          <p className="hint">
            {selectedGroup === null
              ? "A category without a group is filed under Ungrouped."
              : `${selectedGroup.name} holds ${selectedGroup.category_kind ?? "expense"} categories, so this one will be too.`}
          </p>
          <div className="actions">
            <button type="submit">Add category</button>
            <button type="button" className="secondary" onClick={() => setAdding(null)}>
              Cancel
            </button>
          </div>
        </form>
      ) : null}

      {adding === "group" ? (
        <form
          className="editor"
          aria-label="New group"
          onSubmit={(event) => void createGroup(event)}
        >
          <div className="grid">
            <label>
              Name
              <input name="name" required maxLength={200} />
            </label>
            <label>
              Kind
              <select name="kind" defaultValue="expense">
                {KINDS.map((entry) => (
                  <option key={entry.kind} value={entry.kind}>
                    {entry.label}
                  </option>
                ))}
              </select>
            </label>
          </div>
          <div className="actions">
            <button type="submit">Add group</button>
            <button type="button" className="secondary" onClick={() => setAdding(null)}>
              Cancel
            </button>
          </div>
        </form>
      ) : null}

      {deleting === null ? null : (
        <form
          className="editor taxonomy-dialog"
          role="dialog"
          aria-label={`Delete ${deleting.name}`}
          onSubmit={(event) => void confirmDelete(event)}
        >
          <p>
            Delete <strong>{deleting.name}</strong>? Its transactions, splits, rules, and budgets
            are moved or removed first, so nothing is left pointing at it.
          </p>
          <div className="grid">
            <label>
              Move its transactions to
              <select name="reassign_to" defaultValue="">
                <option value="">Leave them uncategorized</option>
                {sections.map((section) =>
                  section.categories
                    .filter((candidate) => candidate.id !== deleting.id)
                    .map((candidate) => (
                      <option key={candidate.id} value={candidate.id}>
                        {section.group === null
                          ? candidate.name
                          : `${section.group.name} › ${candidate.name}`}
                      </option>
                    )),
                )}
              </select>
            </label>
          </div>
          <div className="actions">
            <button type="submit">Delete category</button>
            <button type="button" className="secondary" onClick={() => setDeleting(null)}>
              Cancel
            </button>
          </div>
        </form>
      )}

      {sections.length === 0 ? <p className="muted">Setting up the default categories…</p> : null}

      {sections.map((section, sectionIndex) => {
        const group = section.group;
        return (
          <div
            key={group?.id ?? "ungrouped"}
            className="taxonomy-group"
            data-testid={group === null ? "group-ungrouped" : `group-${group.id}`}
            data-name={group?.name ?? "Ungrouped"}
          >
            <header>
              <h2>{group === null ? "Ungrouped" : nameCell(group)}</h2>
              <span className="chip">
                {group === null ? "no group" : (group.category_kind ?? "expense")}
              </span>
              {group === null ? null : (
                <div className="actions">
                  <button
                    type="button"
                    className="link"
                    disabled={sectionIndex === 0}
                    onClick={() => moveGroup(group, "up")}
                  >
                    Move up
                  </button>
                  <button
                    type="button"
                    className="link"
                    disabled={sectionIndex >= groups.length - 1}
                    onClick={() => moveGroup(group, "down")}
                  >
                    Move down
                  </button>
                  {renameButton(group)}
                  <button type="button" className="link" onClick={() => deleteGroup(group)}>
                    Delete
                  </button>
                </div>
              )}
            </header>
            <table className="list" aria-label={`${group?.name ?? "Ungrouped"} categories`}>
              <thead>
                <tr>
                  <th scope="col">Category</th>
                  <th scope="col">Budget bucket</th>
                  <th scope="col">
                    <span className="visually-hidden">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {section.categories.length === 0 ? (
                  <tr>
                    <td colSpan={3} className="empty">
                      No categories yet.
                    </td>
                  </tr>
                ) : null}
                {section.categories.map((category, index) => (
                  <tr
                    key={category.id}
                    data-testid={`category-${category.id}`}
                    data-name={category.name}
                    className={category.archived === true ? "muted" : undefined}
                  >
                    <th scope="row">
                      <span className="taxonomy-icon" aria-hidden="true">
                        {category.icon ?? "·"}
                      </span>
                      {nameCell(category)}
                      {group === null ? (
                        <span className="chip">{category.category_kind ?? "expense"}</span>
                      ) : null}
                    </th>
                    <td>
                      {category.category_kind === "expense" ? (
                        <span className="taxonomy-inline">
                          <select
                            className="bucket-select"
                            aria-label={`${category.name} budget bucket`}
                            value={category.budget_bucket ?? ""}
                            onChange={(event) => {
                              const bucket = event.target.value;
                              void attempt(
                                (writes) =>
                                  writes.updateCategory(category.id, {
                                    budget_bucket: isBucket(bucket) ? bucket : null,
                                  }),
                                "The budget bucket could not be saved.",
                              );
                            }}
                          >
                            <option value="">—</option>
                            {BUCKETS.map((entry) => (
                              <option key={entry.bucket} value={entry.bucket}>
                                {entry.label}
                              </option>
                            ))}
                          </select>
                          {category.budget_bucket === "non_monthly" ? (
                            <NonMonthlyTarget
                              key={`${category.id}:${category.updated_at}`}
                              category={category}
                              currency={currency}
                              onSave={(patch) =>
                                void attempt(
                                  (writes) => writes.updateCategory(category.id, patch),
                                  "The target could not be saved.",
                                )
                              }
                              onError={setError}
                            />
                          ) : null}
                        </span>
                      ) : (
                        <span className="muted">—</span>
                      )}
                    </td>
                    <td className="actions">
                      <button
                        type="button"
                        className="link"
                        disabled={index === 0}
                        onClick={() => moveCategory(section, category, "up")}
                      >
                        Move up
                      </button>
                      <button
                        type="button"
                        className="link"
                        disabled={index >= section.categories.length - 1}
                        onClick={() => moveCategory(section, category, "down")}
                      >
                        Move down
                      </button>
                      {renameButton(category)}
                      <button
                        type="button"
                        className="link"
                        onClick={() =>
                          void attempt(
                            (writes) =>
                              writes.updateCategory(category.id, {
                                archived: category.archived === true ? null : true,
                              }),
                            "The category could not be changed.",
                          )
                        }
                      >
                        {category.archived === true ? "Restore" : "Archive"}
                      </button>
                      <button type="button" className="link" onClick={() => setDeleting(category)}>
                        Delete
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        );
      })}
    </section>
  );
}

/**
 * A non-monthly category's target: this much, every so many months. The
 * budget spreads it as `target_amount / target_months` a month. Saved when
 * the person leaves the field or presses Enter, so a half-typed number is
 * never written.
 */
function NonMonthlyTarget({
  category,
  currency,
  onSave,
  onError,
}: {
  category: Category;
  currency: string;
  onSave: (patch: Patch<TaxonomyEntry>) => void;
  onError: (message: string) => void;
}) {
  const [amount, setAmount] = useState(
    category.target_amount === undefined ? "" : amountToText(category.target_amount, currency),
  );
  const [months, setMonths] = useState(
    category.target_months === undefined ? "" : String(category.target_months),
  );

  const commit = () => {
    try {
      const patch: Patch<TaxonomyEntry> = {};
      const targetAmount = amount.trim() === "" ? undefined : parseAmount(amount, currency);
      const targetMonths = months.trim() === "" ? undefined : Number(months);
      if (targetAmount !== category.target_amount) {
        Object.assign(patch, { target_amount: targetAmount ?? null });
      }
      if (targetMonths !== category.target_months) {
        Object.assign(patch, { target_months: targetMonths ?? null });
      }
      if (Object.keys(patch).length > 0) onSave(patch);
    } catch (caught) {
      onError(caught instanceof Error ? caught.message : "That is not an amount.");
    }
  };
  const onEnter = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter") {
      event.preventDefault();
      commit();
    }
  };

  return (
    <span className="target-inputs">
      <input
        aria-label={`${category.name} target amount`}
        inputMode="decimal"
        placeholder={amountToText(120_000, currency)}
        value={amount}
        onChange={(event) => setAmount(event.target.value)}
        onBlur={commit}
        onKeyDown={onEnter}
      />
      every
      <input
        aria-label={`${category.name} target months`}
        type="number"
        min={1}
        max={60}
        placeholder="12"
        value={months}
        onChange={(event) => setMonths(event.target.value)}
        onBlur={commit}
        onKeyDown={onEnter}
      />
      months
    </span>
  );
}
