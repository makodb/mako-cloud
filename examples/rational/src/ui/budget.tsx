import { type KeyboardEvent, useMemo, useState } from "react";

import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import { type HouseholdWrites, ValidationError } from "../data/writes.js";
import {
  type BudgetKind,
  type BudgetMode,
  type Household,
  type HouseholdCollectionId,
  isCategory,
  type TaxonomyEntry,
} from "../model/types.js";
import {
  budgetMonth,
  copyMonthPlan,
  INCOME_SUBJECT,
  isMonthKey,
  type MonthBudgetCategory,
  type MonthBudgetGroup,
  type MonthBudgetModel,
  monthTitle,
  nextMonth,
  previousMonth,
  selectBudgetMonth,
  UNGROUPED_ID,
} from "../selectors/budget-month.js";
import { plannedMonthlyTotal } from "../selectors/goals.js";
import { amountToText, formatMinorUnits, parseAmount } from "../selectors/money.js";
import { ProgressBar, type ProgressTone } from "./charts/index.js";
import { useQuery } from "./hooks.js";
import { type Route, routeHash, transactionsHash } from "./router.js";
import "./styles/budget.css";

/**
 * The budget, one month at a time.
 *
 * Every number here is the shared engine's (`functions/shared/budgets.ts`,
 * through `selectBudgetMonth`): the same model decides the over-budget alerts
 * at night, so what the page shows as "left" is what the alert would be
 * about. The screen only decides how to lay it out, and that is what the
 * household's budget mode chooses between -- Monarch's two presentations of
 * one set of budgets. Category mode is a group per section with a row per
 * category; flex mode is three buckets, the last of which is whatever income
 * leaves after fixed costs, non-monthly targets, and what the goals take.
 *
 * Amounts are edited in place. A budget is one document per subject per
 * month, so typing into a row is a `setBudget` and clearing it a
 * `deleteBudget`; there is no form to submit, because a budget is a table of
 * numbers a person adjusts, not a record they file.
 */
export function BudgetScreen({
  app,
  session,
  route,
  currency,
  household,
}: {
  app: RationalApp;
  session: ScopeSession<HouseholdCollectionId>;
  route: Extract<Route, { name: "budget" }>;
  currency: string;
  household: Household | null;
}) {
  const budgets = useQuery(session.collection("budgets")?.find() ?? null);
  const transactions = useQuery(session.collection("transactions")?.find() ?? null);
  const taxonomy = useQuery(session.collection("taxonomy")?.find() ?? null);
  const goals = useQuery(session.collection("goals")?.find() ?? null);
  const [problem, setProblem] = useState<string | null>(null);
  // Remembered with its month, so moving to another month does not carry a
  // "copied 6 budgets" that was about a different one.
  const [copied, setCopied] = useState<{ readonly month: string; readonly count: number } | null>(
    null,
  );

  const month = route.month !== undefined && isMonthKey(route.month) ? route.month : currentMonth();
  const mode: BudgetMode = household?.budget_mode ?? "category";
  const model = selectBudgetMonth(budgets, transactions, taxonomy, month, currency, goals);
  const planned = plannedMonthlyTotal(goals, currency);
  const previous = previousMonth(month);
  const copyable = copyMonthPlan(budgets, previous, month).length;
  const readOnly = app.roleIn(household?.id ?? null) === "viewer";
  const icons = useMemo(() => iconsOf(taxonomy), [taxonomy]);
  // A household with an account in a second currency budgets it apart, and
  // nothing is converted; the footer runs the same engine once per currency
  // the month's budgets name so their totals are the same kind of number.
  const otherTotals = useMemo(() => {
    const currencies = [
      ...new Set(
        budgets
          .filter((budget) => budget.month === month && budget.currency !== currency)
          .map((budget) => budget.currency),
      ),
    ].sort();
    return currencies.map((other) => ({
      currency: other,
      ...budgetMonth(budgets, transactions, taxonomy, month, other, goals).totals,
    }));
  }, [budgets, transactions, taxonomy, month, currency, goals]);

  const go = (target: string) => {
    window.location.hash = routeHash({ name: "budget", month: target });
  };
  const requireWrites = (): HouseholdWrites => {
    const writes = app.writes;
    if (writes === null) throw new ValidationError("the household is not open");
    return writes;
  };
  const attempt = async (write: () => Promise<unknown>) => {
    setProblem(null);
    try {
      await write();
    } catch (caught) {
      setProblem(describe(caught));
    }
  };
  const setBudget = (kind: BudgetKind, subject: string, amount: number, rollover: boolean) =>
    attempt(() =>
      requireWrites().setBudget({ kind, category_id: subject, month, amount, currency, rollover }),
    );
  const clearBudget = (subject: string) =>
    attempt(() => requireWrites().deleteBudget(subject, month));
  const copyLastMonth = () =>
    attempt(async () => {
      const count = await requireWrites().copyBudgets(previous, month);
      setCopied({ month, count });
    });

  const editing: Editing = {
    currency,
    readOnly,
    setBudget,
    clearBudget,
    onProblem: setProblem,
  };

  return (
    <section aria-labelledby="budget-title" data-testid="budget-screen" className="budget">
      <div className="heading">
        <h1 id="budget-title">Budget</h1>
        <div className="budget-tools">
          <div className="month-pager">
            <button
              type="button"
              className="secondary"
              aria-label="Previous month"
              onClick={() => go(previous)}
            >
              ‹
            </button>
            <span className="month-title" data-testid="budget-month">
              {monthTitle(month)}
            </span>
            <button
              type="button"
              className="secondary"
              aria-label="Next month"
              onClick={() => go(nextMonth(month))}
            >
              ›
            </button>
          </div>
          <button
            type="button"
            className="secondary"
            disabled={readOnly || copyable === 0}
            title={
              copyable === 0
                ? `Nothing in ${monthTitle(previous)} that ${monthTitle(month)} lacks`
                : undefined
            }
            onClick={() => void copyLastMonth()}
          >
            Copy last month
          </button>
        </div>
      </div>
      <p className="hint mode-hint">
        {mode === "flex" ? "Flex budget" : "Budget by category"} ·{" "}
        <a href={routeHash({ name: "settings", page: "household" })}>
          Change in Settings › Household
        </a>
      </p>
      {problem === null ? null : (
        <p className="notice error" role="alert" data-testid="budget-error">
          {problem}
        </p>
      )}
      {copied === null || copied.month !== month ? null : (
        <p className="hint" role="status" data-testid="copy-status">
          {copied.count === 0
            ? `Nothing to copy: ${monthTitle(month)} already has everything ${monthTitle(previous)} had.`
            : `Copied ${copied.count} ${copied.count === 1 ? "budget" : "budgets"} from ${monthTitle(previous)}.`}
        </p>
      )}

      <Summary model={model} planned={planned} mode={mode} editing={editing} />

      {mode === "flex" ? (
        <FlexBuckets model={model} planned={planned} icons={icons} editing={editing} />
      ) : (
        <>
          {model.groups.map((group) => (
            <GroupSection
              key={group.group.id}
              group={group}
              month={month}
              icons={icons}
              editing={editing}
            />
          ))}
          <Unbudgeted model={model} month={month} taxonomy={taxonomy} editing={editing} />
        </>
      )}

      <table className="list budget-totals" aria-label="Budget totals">
        <thead>
          <tr>
            <th scope="col">Total</th>
            <th scope="col" className="amount">
              Budgeted
            </th>
            <th scope="col" className="amount">
              Spent
            </th>
            <th scope="col" className="amount">
              Remaining
            </th>
          </tr>
        </thead>
        <tbody>
          {[{ currency, ...model.totals }, ...otherTotals].map((total) => (
            <tr key={total.currency} data-testid={`budget-total-${total.currency}`}>
              <th scope="row">{total.currency}</th>
              <td className="amount" data-testid="budgeted">
                {formatMinorUnits(total.budgeted, total.currency)}
              </td>
              <td className="amount" data-testid="spent">
                {formatMinorUnits(total.spent, total.currency)}
              </td>
              <td className={`amount${total.remaining < 0 ? " over" : ""}`} data-testid="remaining">
                {formatMinorUnits(total.remaining, total.currency)}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}

/** What the row and bucket components need to change a budget, in one bundle. */
interface Editing {
  readonly currency: string;
  readonly readOnly: boolean;
  readonly setBudget: (
    kind: BudgetKind,
    subject: string,
    amount: number,
    rollover: boolean,
  ) => Promise<void>;
  readonly clearBudget: (subject: string) => Promise<void>;
  readonly onProblem: (message: string | null) => void;
}

/**
 * The month in five numbers. Expected income is the one a person sets here
 * rather than in a row, because it is the top of the flex arithmetic and the
 * thing "left to budget" is measured from; it opens into a field on click and
 * closes when the field is left.
 */
function Summary({
  model,
  planned,
  mode,
  editing,
}: {
  model: MonthBudgetModel;
  planned: number;
  mode: BudgetMode;
  editing: Editing;
}) {
  const [settingIncome, setSettingIncome] = useState(false);
  const { currency } = editing;
  const fmt = (amount: number) => formatMinorUnits(amount, currency);
  const remaining = mode === "flex" ? model.flex.flexible.remaining : model.totals.remaining;
  return (
    <div className="totals budget-summary">
      <div className="total" data-testid="expected-income">
        <span>Expected income</span>
        {settingIncome ? (
          <AmountField
            label="Expected income"
            amount={model.income.budget === null ? null : model.income.budget.amount}
            placeholder={amountToText(0, currency)}
            autoFocus
            editing={editing}
            onSave={(amount) => editing.setBudget("income", INCOME_SUBJECT, amount, false)}
            onClear={() => editing.clearBudget(INCOME_SUBJECT)}
            onDone={() => setSettingIncome(false)}
          />
        ) : (
          <strong>
            <button
              type="button"
              className="link amount-button"
              aria-label="Set expected income"
              disabled={editing.readOnly}
              onClick={() => setSettingIncome(true)}
            >
              {model.income.expected === 0 ? "Set" : fmt(model.income.expected)}
            </button>
          </strong>
        )}
        <small>received {fmt(model.income.actual)}</small>
      </div>
      <div className="total" data-testid="budgeted">
        <span>Budgeted</span>
        <strong>{fmt(model.totals.budgeted)}</strong>
      </div>
      <div className="total" data-testid="spent-total">
        <span>Spent</span>
        <strong>{fmt(model.totals.spent)}</strong>
      </div>
      <div className="total" data-testid="remaining-total">
        <span>{mode === "flex" ? "Flexible remaining" : "Remaining"}</span>
        <strong className={remaining < 0 ? "over" : undefined}>{fmt(remaining)}</strong>
      </div>
      <div className="total" data-testid="left-to-budget">
        <span>Left to budget</span>
        <strong className={model.totals.leftToBudget < 0 ? "over" : undefined}>
          {fmt(model.totals.leftToBudget)}
        </strong>
        <small>after {fmt(planned)} to goals</small>
      </div>
    </div>
  );
}

/**
 * One group: its figures and bar, the choice between budgeting it whole or by
 * category, and a row per category. A group budgeted as a whole hides its
 * categories' own fields -- the engine already reports them as unbudgeted,
 * and a field that would write a document the page then ignores is a trap.
 */
function GroupSection({
  group,
  month,
  icons,
  editing,
}: {
  group: MonthBudgetGroup;
  month: string;
  icons: ReadonlyMap<string, string>;
  editing: Editing;
}) {
  const { currency } = editing;
  const fmt = (amount: number) => formatMinorUnits(amount, currency);
  const id = group.group.id === UNGROUPED_ID ? "ungrouped" : group.group.id;
  const whole = group.budget !== null;
  const ownAmounts = group.categories.reduce(
    (total, entry) => total + (entry.budget?.amount ?? 0),
    0,
  );
  return (
    <section
      className="budget-group"
      data-testid={`budget-group-${id}`}
      aria-labelledby={`budget-group-${id}-title`}
    >
      <header className="group-heading">
        <div>
          <h2 id={`budget-group-${id}-title`}>{group.group.name}</h2>
          <p className="group-figures">
            <span data-testid="group-spent">{fmt(group.spent)}</span> of{" "}
            <span data-testid="group-budgeted">{fmt(group.budgeted)}</span> ·{" "}
            <span
              data-testid="group-remaining"
              className={group.remaining < 0 ? "over" : undefined}
            >
              {fmt(group.remaining)}
            </span>{" "}
            left
          </p>
        </div>
        {group.group.id === UNGROUPED_ID ? null : whole && group.budget !== null ? (
          <div className="group-budget">
            <span className="inline-field">
              <span className="field-name">Group budget</span>
              <AmountField
                label={`Budget for ${group.group.name}`}
                amount={group.budget.amount}
                placeholder={amountToText(0, currency)}
                editing={editing}
                onSave={(amount) =>
                  editing.setBudget(
                    "group",
                    group.group.id,
                    amount,
                    group.budget?.budget.rollover ?? false,
                  )
                }
                onClear={() => editing.clearBudget(group.group.id)}
              />
            </span>
            <label className="chip-option">
              <input
                type="checkbox"
                aria-label={`Roll over ${group.group.name}`}
                checked={group.budget.budget.rollover}
                disabled={editing.readOnly}
                onChange={(event) =>
                  void editing.setBudget(
                    "group",
                    group.group.id,
                    group.budget?.amount ?? 0,
                    event.target.checked,
                  )
                }
              />
              rolls over
            </label>
            {group.budget.carriedIn === 0 ? null : (
              <small data-testid="group-carried-in">{fmt(group.budget.carriedIn)} carried in</small>
            )}
            <button
              type="button"
              className="link"
              disabled={editing.readOnly}
              onClick={() => void editing.clearBudget(group.group.id)}
            >
              Budget categories separately
            </button>
          </div>
        ) : (
          <button
            type="button"
            className="link"
            disabled={editing.readOnly}
            onClick={() => void editing.setBudget("group", group.group.id, ownAmounts, false)}
          >
            Budget this group as a whole
          </button>
        )}
      </header>
      <ProgressBar
        value={group.spent}
        max={group.budgeted}
        label={`${group.group.name} budget used`}
        tone={toneFor(group.spent, group.budgeted)}
      />
      <table className="list budget-table" aria-label={`${group.group.name} categories`}>
        <thead>
          <tr>
            <th scope="col">Category</th>
            <th scope="col" className="amount">
              Budget
            </th>
            <th scope="col">Roll over</th>
            <th scope="col" className="amount">
              Carried in
            </th>
            <th scope="col" className="amount">
              Allowance
            </th>
            <th scope="col" className="amount">
              Spent
            </th>
            <th scope="col" className="amount">
              Remaining
            </th>
            <th scope="col" className="progress-column">
              Used
            </th>
          </tr>
        </thead>
        <tbody>
          {group.categories.length === 0 ? (
            <tr>
              <td colSpan={8} className="empty">
                No categories in this group.
              </td>
            </tr>
          ) : null}
          {group.categories.map((entry) => (
            <CategoryRow
              key={entry.category.id}
              entry={entry}
              underGroupBudget={whole}
              month={month}
              icon={icons.get(entry.category.id)}
              editing={editing}
            />
          ))}
        </tbody>
      </table>
    </section>
  );
}

function CategoryRow({
  entry,
  underGroupBudget,
  month,
  icon,
  editing,
}: {
  entry: MonthBudgetCategory;
  underGroupBudget: boolean;
  month: string;
  icon: string | undefined;
  editing: Editing;
}) {
  const { currency } = editing;
  const fmt = (amount: number) => formatMinorUnits(amount, currency);
  const { category, budget, spent, share } = entry;
  const dash = <span className="muted">—</span>;
  return (
    <tr data-testid={`budget-${category.id}`} data-percent={budget?.percent ?? 0}>
      <th scope="row">
        <span className="category-icon" aria-hidden="true">
          {icon ?? ""}
        </span>
        <a href={transactionsHash({ category: category.id, month })}>{category.name}</a>
        {share === 0 ? null : (
          <small className="share-hint" data-testid="suggested">
            {" "}
            {fmt(category.target_amount ?? 0)} every {category.target_months ?? 0} months ·{" "}
            {fmt(share)} a month
          </small>
        )}
      </th>
      <td className="amount">
        {underGroupBudget ? (
          <span className="muted" title="Budgeted with its group">
            in group
          </span>
        ) : (
          <AmountField
            label={`Budget for ${category.name}`}
            amount={budget === null ? null : budget.amount}
            placeholder={amountToText(share, currency)}
            editing={editing}
            onSave={(amount) =>
              editing.setBudget("category", category.id, amount, budget?.budget.rollover ?? false)
            }
            onClear={() => editing.clearBudget(category.id)}
          />
        )}
      </td>
      <td>
        {underGroupBudget ? (
          dash
        ) : (
          <input
            type="checkbox"
            className="rollover"
            aria-label={`Roll over ${category.name}`}
            checked={budget?.budget.rollover ?? false}
            disabled={editing.readOnly || budget === null}
            title={budget === null ? "Set a budget first" : undefined}
            onChange={(event) =>
              void editing.setBudget(
                "category",
                category.id,
                budget?.amount ?? 0,
                event.target.checked,
              )
            }
          />
        )}
      </td>
      <td className="amount" data-testid="carried-in">
        {budget === null || budget.carriedIn === 0 ? dash : fmt(budget.carriedIn)}
      </td>
      <td className="amount" data-testid="allowance">
        {budget === null ? dash : fmt(budget.allowance)}
      </td>
      <td className="amount" data-testid="spent">
        {fmt(spent)}
      </td>
      <td
        className={`amount${budget !== null && budget.remaining < 0 ? " over" : ""}`}
        data-testid="remaining"
      >
        {budget === null ? dash : fmt(budget.remaining)}
      </td>
      <td className="progress-column">
        {budget === null ? null : (
          <ProgressBar
            value={spent}
            max={budget.allowance}
            label={`${category.name} budget used`}
            tone={toneFor(spent, budget.allowance)}
          />
        )}
      </td>
    </tr>
  );
}

/**
 * Spending the month has no number against: categories with outflows and no
 * budget of their own or their group's, and outflows under no category at
 * all. "Budget it" starts the category at what it has already spent -- the
 * one amount the household can be sure is not too small -- and the row moves
 * up into its group.
 */
function Unbudgeted({
  model,
  month,
  taxonomy,
  editing,
}: {
  model: MonthBudgetModel;
  month: string;
  taxonomy: readonly TaxonomyEntry[];
  editing: Editing;
}) {
  if (model.unbudgeted.length === 0) return null;
  const { currency } = editing;
  const fmt = (amount: number) => formatMinorUnits(amount, currency);
  const nameOf = (id: string) => taxonomy.find((entry) => entry.id === id)?.name ?? id;
  const total = model.unbudgeted.reduce((sum, entry) => sum + entry.spent, 0);
  return (
    <section
      className="budget-group unbudgeted"
      data-testid="unbudgeted"
      aria-labelledby="unbudgeted-title"
    >
      <header className="group-heading">
        <div>
          <h2 id="unbudgeted-title">Unbudgeted spending</h2>
          <p className="group-figures">
            <span data-testid="unbudgeted-total">{fmt(total)}</span> with no budget against it
          </p>
        </div>
      </header>
      <table className="list budget-table" aria-label="Unbudgeted spending">
        <thead>
          <tr>
            <th scope="col">Category</th>
            <th scope="col" className="amount">
              Spent
            </th>
            <th scope="col">
              <span className="visually-hidden">Actions</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {model.unbudgeted.map((entry) => (
            <tr
              key={entry.categoryId === "" ? "uncategorized" : entry.categoryId}
              data-testid={`unbudgeted-${entry.categoryId === "" ? "uncategorized" : entry.categoryId}`}
            >
              <th scope="row">
                {entry.categoryId === "" ? (
                  <span className="muted">Uncategorized</span>
                ) : (
                  nameOf(entry.categoryId)
                )}
              </th>
              <td className="amount" data-testid="spent">
                {fmt(entry.spent)}
              </td>
              <td className="actions">
                {entry.categoryId === "" ? (
                  <a href={transactionsHash({ month })}>Categorize</a>
                ) : (
                  <button
                    type="button"
                    className="link"
                    disabled={editing.readOnly}
                    onClick={() =>
                      void editing.setBudget("category", entry.categoryId, entry.spent, false)
                    }
                  >
                    Budget it
                  </button>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}

/**
 * Flex mode: the same budgets as three buckets. Fixed and non-monthly are
 * lists of the categories in those buckets -- the fixed ones still editable,
 * since a rent that changed is changed here -- and Flexible is the number
 * that is left, with the four terms that made it spelled out so the household
 * can see which one to move when it is too small.
 */
function FlexBuckets({
  model,
  planned,
  icons,
  editing,
}: {
  model: MonthBudgetModel;
  planned: number;
  icons: ReadonlyMap<string, string>;
  editing: Editing;
}) {
  const { currency } = editing;
  const fmt = (amount: number) => formatMinorUnits(amount, currency);
  const entries = model.groups.flatMap((group) =>
    group.categories.map((entry) => ({ entry, group })),
  );
  const fixed = entries.filter(({ entry }) => entry.category.budget_bucket === "fixed");
  const nonMonthly = entries.filter(({ entry }) => entry.category.budget_bucket === "non_monthly");
  const flexible = entries
    .filter(
      ({ entry }) =>
        entry.category.budget_bucket !== "fixed" &&
        entry.category.budget_bucket !== "non_monthly" &&
        entry.spent !== 0,
    )
    .sort((left, right) => right.entry.spent - left.entry.spent);
  const uncategorized = model.unbudgeted.find((entry) => entry.categoryId === "")?.spent ?? 0;
  const { flex } = model;
  return (
    <div className="buckets">
      <section className="bucket" data-testid="bucket-fixed" aria-labelledby="bucket-fixed-title">
        <header className="group-heading">
          <div>
            <h2 id="bucket-fixed-title">Fixed</h2>
            <p className="group-figures">
              <span data-testid="bucket-spent">{fmt(flex.fixed.spent)}</span> of{" "}
              <span data-testid="bucket-budgeted">{fmt(flex.fixed.budgeted)}</span>
            </p>
          </div>
        </header>
        <ProgressBar
          value={flex.fixed.spent}
          max={flex.fixed.budgeted}
          label="Fixed costs used"
          tone={toneFor(flex.fixed.spent, flex.fixed.budgeted)}
        />
        <table className="list budget-table" aria-label="Fixed categories">
          <thead>
            <tr>
              <th scope="col">Category</th>
              <th scope="col" className="amount">
                Budget
              </th>
              <th scope="col" className="amount">
                Spent
              </th>
            </tr>
          </thead>
          <tbody>
            {fixed.length === 0 ? (
              <tr>
                <td colSpan={3} className="empty">
                  No fixed categories. Mark rent, loans, and bills as fixed in Settings ›
                  Categories.
                </td>
              </tr>
            ) : null}
            {fixed.map(({ entry, group }) => (
              <tr key={entry.category.id} data-testid={`budget-${entry.category.id}`}>
                <th scope="row">
                  <span className="category-icon" aria-hidden="true">
                    {icons.get(entry.category.id) ?? ""}
                  </span>
                  {entry.category.name}
                  <small> {group.group.name}</small>
                </th>
                <td className="amount">
                  {group.budget !== null ? (
                    <span className="muted">in group</span>
                  ) : (
                    <AmountField
                      label={`Budget for ${entry.category.name}`}
                      amount={entry.budget === null ? null : entry.budget.amount}
                      placeholder={amountToText(0, currency)}
                      editing={editing}
                      onSave={(amount) =>
                        editing.setBudget(
                          "category",
                          entry.category.id,
                          amount,
                          entry.budget?.budget.rollover ?? false,
                        )
                      }
                      onClear={() => editing.clearBudget(entry.category.id)}
                    />
                  )}
                </td>
                <td className="amount" data-testid="spent">
                  {fmt(entry.spent)}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>

      <section
        className="bucket"
        data-testid="bucket-non-monthly"
        aria-labelledby="bucket-non-monthly-title"
      >
        <header className="group-heading">
          <div>
            <h2 id="bucket-non-monthly-title">Non-monthly</h2>
            <p className="group-figures">
              <span data-testid="bucket-spent">{fmt(flex.nonMonthly.spent)}</span> spent ·{" "}
              <span data-testid="bucket-share">{fmt(flex.nonMonthly.share)}</span> set aside a month
            </p>
          </div>
        </header>
        <ProgressBar
          value={flex.nonMonthly.spent}
          max={flex.nonMonthly.share}
          label="Non-monthly share used"
          tone={toneFor(flex.nonMonthly.spent, flex.nonMonthly.share)}
        />
        <table className="list budget-table" aria-label="Non-monthly categories">
          <thead>
            <tr>
              <th scope="col">Category</th>
              <th scope="col">Target</th>
              <th scope="col" className="amount">
                A month
              </th>
              <th scope="col" className="amount">
                Spent
              </th>
            </tr>
          </thead>
          <tbody>
            {nonMonthly.length === 0 ? (
              <tr>
                <td colSpan={4} className="empty">
                  No non-monthly categories. Give a category a target in Settings › Categories.
                </td>
              </tr>
            ) : null}
            {nonMonthly.map(({ entry }) => (
              <tr key={entry.category.id} data-testid={`budget-${entry.category.id}`}>
                <th scope="row">
                  <span className="category-icon" aria-hidden="true">
                    {icons.get(entry.category.id) ?? ""}
                  </span>
                  {entry.category.name}
                </th>
                <td>
                  {entry.share === 0 ? (
                    <span className="muted">no target yet</span>
                  ) : (
                    `${fmt(entry.category.target_amount ?? 0)} every ${entry.category.target_months ?? 0} months`
                  )}
                </td>
                <td className="amount" data-testid="share">
                  {fmt(entry.share)}
                </td>
                <td className="amount" data-testid="spent">
                  {fmt(entry.spent)}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>

      <section
        className="bucket flexible"
        data-testid="bucket-flexible"
        aria-labelledby="bucket-flexible-title"
      >
        <header className="group-heading">
          <div>
            <h2 id="bucket-flexible-title">Flexible</h2>
            <p className="group-figures">
              <span data-testid="flexible-spent">{fmt(flex.flexible.spent)}</span> of{" "}
              <span data-testid="flexible-allowance">{fmt(flex.flexible.allowance)}</span> ·{" "}
              <span
                data-testid="flexible-remaining"
                className={flex.flexible.remaining < 0 ? "over" : undefined}
              >
                {fmt(flex.flexible.remaining)}
              </span>{" "}
              left
            </p>
          </div>
        </header>
        <ProgressBar
          value={flex.flexible.spent}
          max={flex.flexible.allowance}
          label="Flexible spending used"
          tone={toneFor(flex.flexible.spent, flex.flexible.allowance)}
        />
        <dl className="breakdown" aria-label="How the flexible amount is made">
          <dt>Expected income</dt>
          <dd data-testid="term-income">{fmt(model.income.expected)}</dd>
          <dt>− Fixed budgets</dt>
          <dd data-testid="term-fixed">{fmt(flex.fixed.budgeted)}</dd>
          <dt>− Non-monthly, a month</dt>
          <dd data-testid="term-non-monthly">{fmt(flex.nonMonthly.share)}</dd>
          <dt>− Planned goal contributions</dt>
          <dd data-testid="term-goals">{fmt(planned)}</dd>
          <dt className="result">= Flexible</dt>
          <dd className="result" data-testid="term-flexible">
            {fmt(flex.flexible.allowance)}
          </dd>
        </dl>
        <table className="list budget-table" aria-label="Flexible spending">
          <thead>
            <tr>
              <th scope="col">Category</th>
              <th scope="col" className="amount">
                Spent
              </th>
            </tr>
          </thead>
          <tbody>
            {flexible.length === 0 && uncategorized === 0 ? (
              <tr>
                <td colSpan={2} className="empty">
                  Nothing flexible spent yet this month.
                </td>
              </tr>
            ) : null}
            {flexible.map(({ entry, group }) => (
              <tr key={entry.category.id} data-testid={`budget-${entry.category.id}`}>
                <th scope="row">
                  <span className="category-icon" aria-hidden="true">
                    {icons.get(entry.category.id) ?? ""}
                  </span>
                  {entry.category.name}
                  <small> {group.group.name}</small>
                </th>
                <td className="amount" data-testid="spent">
                  {fmt(entry.spent)}
                </td>
              </tr>
            ))}
            {uncategorized === 0 ? null : (
              <tr data-testid="budget-uncategorized">
                <th scope="row">
                  <span className="muted">Uncategorized</span>
                </th>
                <td className="amount" data-testid="spent">
                  {fmt(uncategorized)}
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </section>
    </div>
  );
}

/**
 * An amount edited in place. Uncontrolled and re-keyed by the stored amount:
 * what the person is typing is theirs until they leave the field, and a
 * change from another device replaces the text only when the stored number
 * actually moved. Enter leaves the field, so there is one path to saving;
 * Escape puts the stored text back and leaves without saving; an emptied
 * field means "no budget" and clears the document rather than storing zero.
 */
function AmountField({
  label,
  amount,
  placeholder,
  autoFocus = false,
  editing,
  onSave,
  onClear,
  onDone,
}: {
  label: string;
  amount: number | null;
  placeholder: string;
  autoFocus?: boolean;
  editing: Editing;
  onSave: (amount: number) => Promise<void>;
  onClear: () => Promise<void>;
  onDone?: () => void;
}) {
  const { currency } = editing;
  const stored = amount === null ? "" : amountToText(amount, currency);
  const onKeyDown = (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter") {
      event.preventDefault();
      event.currentTarget.blur();
    } else if (event.key === "Escape") {
      event.currentTarget.value = stored;
      event.currentTarget.blur();
    }
  };
  return (
    <input
      key={stored}
      className="amount-field"
      aria-label={label}
      inputMode="decimal"
      defaultValue={stored}
      placeholder={placeholder}
      disabled={editing.readOnly}
      // biome-ignore lint/a11y/noAutofocus: the field appears because the person clicked to set it
      autoFocus={autoFocus}
      onKeyDown={onKeyDown}
      onBlur={(event) => {
        const text = event.currentTarget.value.trim();
        if (text === "") {
          if (amount !== null) void onClear();
          onDone?.();
          return;
        }
        let parsed: number;
        try {
          parsed = parseAmount(text, currency);
        } catch (caught) {
          editing.onProblem(describe(caught));
          onDone?.();
          return;
        }
        if (parsed !== amount) void onSave(parsed);
        onDone?.();
      }}
    />
  );
}

/** Calm under the limit, a warning near it, danger past it; a limit of zero is only trouble once spent. */
function toneFor(spent: number, allowance: number): ProgressTone {
  if (allowance <= 0) return spent > 0 ? "danger" : "accent";
  const ratio = spent / allowance;
  return ratio > 1 ? "danger" : ratio >= 0.85 ? "warn" : "accent";
}

function currentMonth(): string {
  return new Date().toISOString().slice(0, 7);
}

function iconsOf(taxonomy: readonly TaxonomyEntry[]): ReadonlyMap<string, string> {
  const icons = new Map<string, string>();
  for (const entry of taxonomy) {
    if (isCategory(entry) && entry.icon !== undefined) icons.set(entry.id, entry.icon);
  }
  return icons;
}

function describe(caught: unknown): string {
  return caught instanceof ValidationError || caught instanceof RangeError
    ? caught.message
    : "The budget could not be saved.";
}
