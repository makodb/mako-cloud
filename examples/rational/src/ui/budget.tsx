import {
  Alert,
  AlertDescription,
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  Checkbox,
  Input,
  Label,
  Progress,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  cn,
} from "@mako-cloud/ui";
import { ChevronLeft, ChevronRight, Copy } from "lucide-react";
import { type ComponentProps, type KeyboardEvent, type ReactNode, useMemo, useState } from "react";

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
import { useQuery } from "./hooks.js";
import { type Route, routeHash, transactionsHash } from "./router.js";

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
    <section aria-labelledby="budget-title" data-testid="budget-screen" className="grid gap-6">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div className="grid gap-1">
          <h1 id="budget-title" className="text-2xl">
            Budget
          </h1>
          <p className="text-sm text-muted-foreground">
            {mode === "flex" ? "Flex budget" : "Budget by category"} ·{" "}
            <a href={routeHash({ name: "settings", page: "household" })}>
              Change in Settings › Household
            </a>
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-3">
          <div className="flex items-center gap-1">
            <Button
              variant="outline"
              size="icon-sm"
              aria-label="Previous month"
              onClick={() => go(previous)}
            >
              <ChevronLeft />
            </Button>
            {/* Wide enough for "September 2026" so the buttons do not shuffle
                as the month changes under them. */}
            <span
              className="min-w-[9.5rem] text-center font-semibold tabular-nums"
              data-testid="budget-month"
            >
              {monthTitle(month)}
            </span>
            <Button
              variant="outline"
              size="icon-sm"
              aria-label="Next month"
              onClick={() => go(nextMonth(month))}
            >
              <ChevronRight />
            </Button>
          </div>
          <Button
            variant="outline"
            disabled={readOnly || copyable === 0}
            title={
              copyable === 0
                ? `Nothing in ${monthTitle(previous)} that ${monthTitle(month)} lacks`
                : undefined
            }
            onClick={() => void copyLastMonth()}
          >
            <Copy />
            Copy last month
          </Button>
        </div>
      </div>
      {problem === null ? null : (
        <Alert variant="destructive" role="alert" data-testid="budget-error">
          <AlertDescription>{problem}</AlertDescription>
        </Alert>
      )}
      {copied === null || copied.month !== month ? null : (
        <Alert role="status" data-testid="copy-status">
          <AlertDescription className="text-foreground">
            {copied.count === 0
              ? `Nothing to copy: ${monthTitle(month)} already has everything ${monthTitle(previous)} had.`
              : `Copied ${copied.count} ${copied.count === 1 ? "budget" : "budgets"} from ${monthTitle(previous)}.`}
          </AlertDescription>
        </Alert>
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

      <Card className="py-3">
        <CardContent>
          <Table aria-label="Budget totals">
            <TableHeader>
              <TableRow>
                <TableHead scope="col">Total</TableHead>
                <MoneyHead scope="col">Budgeted</MoneyHead>
                <MoneyHead scope="col">Spent</MoneyHead>
                <MoneyHead scope="col">Remaining</MoneyHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {[{ currency, ...model.totals }, ...otherTotals].map((total) => (
                <TableRow key={total.currency} data-testid={`budget-total-${total.currency}`}>
                  <RowHead>{total.currency}</RowHead>
                  <TableCell className="money" data-testid="budgeted">
                    {formatMinorUnits(total.budgeted, total.currency)}
                  </TableCell>
                  <TableCell className="money" data-testid="spent">
                    {formatMinorUnits(total.spent, total.currency)}
                  </TableCell>
                  <TableCell
                    className={cn("money", total.remaining < 0 && OVER)}
                    data-testid="remaining"
                  >
                    {formatMinorUnits(total.remaining, total.currency)}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </CardContent>
      </Card>
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

/** An amount past its limit: the one colour the page uses for trouble. */
const OVER = "font-medium text-destructive";

/** A dash where a row has no number to show. */
const DASH = <span className="text-muted-foreground">—</span>;

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
    <div className="grid grid-cols-2 gap-4 md:grid-cols-3 xl:grid-cols-5">
      <Tile
        label="Expected income"
        detail={`received ${fmt(model.income.actual)}`}
        data-testid="expected-income"
      >
        {settingIncome ? (
          <AmountField
            label="Expected income"
            amount={model.income.budget === null ? null : model.income.budget.amount}
            placeholder={amountToText(0, currency)}
            autoFocus
            editing={editing}
            className="h-9 text-lg font-semibold md:text-lg"
            onSave={(amount) => editing.setBudget("income", INCOME_SUBJECT, amount, false)}
            onClear={() => editing.clearBudget(INCOME_SUBJECT)}
            onDone={() => setSettingIncome(false)}
          />
        ) : (
          <strong className="text-xl font-semibold tabular-nums">
            <Button
              variant="link"
              className="h-auto p-0 text-xl font-semibold text-foreground tabular-nums underline decoration-input decoration-dashed underline-offset-4 hover:text-primary hover:decoration-primary"
              aria-label="Set expected income"
              disabled={editing.readOnly}
              onClick={() => setSettingIncome(true)}
            >
              {model.income.expected === 0 ? "Set" : fmt(model.income.expected)}
            </Button>
          </strong>
        )}
      </Tile>
      <Tile label="Budgeted" data-testid="budgeted">
        <TileValue>{fmt(model.totals.budgeted)}</TileValue>
      </Tile>
      <Tile label="Spent" data-testid="spent-total">
        <TileValue>{fmt(model.totals.spent)}</TileValue>
      </Tile>
      <Tile
        label={mode === "flex" ? "Flexible remaining" : "Remaining"}
        data-testid="remaining-total"
      >
        <TileValue className={remaining < 0 ? OVER : undefined}>{fmt(remaining)}</TileValue>
      </Tile>
      <Tile
        label="Left to budget"
        detail={`after ${fmt(planned)} to goals`}
        data-testid="left-to-budget"
      >
        <TileValue className={model.totals.leftToBudget < 0 ? OVER : undefined}>
          {fmt(model.totals.leftToBudget)}
        </TileValue>
      </Tile>
    </div>
  );
}

/** One figure of the summary: what it is, the number, and a word about it. */
function Tile({
  label,
  detail,
  className,
  children,
  ...props
}: ComponentProps<"section"> & { label: string; detail?: string }) {
  return (
    <Card className={cn("gap-0 py-4", className)} {...props}>
      <CardContent className="grid gap-1 px-4">
        <span className="text-xs font-semibold uppercase tracking-wider text-muted-foreground">
          {label}
        </span>
        {children}
        {detail === undefined ? null : (
          <small className="text-xs text-muted-foreground">{detail}</small>
        )}
      </CardContent>
    </Card>
  );
}

function TileValue({
  className,
  children,
}: {
  className?: string | undefined;
  children: ReactNode;
}) {
  return (
    <strong className={cn("text-xl font-semibold tabular-nums", className)}>{children}</strong>
  );
}

/** A column header over money: right-aligned like the figures under it. */
function MoneyHead({ className, ...props }: ComponentProps<"th">) {
  return <TableHead className={cn("money text-right", className)} {...props} />;
}

/** A row's own heading cell: the kit's `th`, in the body's colour and weight. */
function RowHead({ className, ...props }: ComponentProps<"th">) {
  return (
    <TableHead
      scope="row"
      className={cn("h-auto whitespace-normal py-2 font-medium text-foreground", className)}
      {...props}
    />
  );
}

/** A cell that says the month has nothing for this table. */
function EmptyRow({ colSpan, children }: { colSpan: number; children: ReactNode }) {
  return (
    <TableRow>
      <TableCell
        colSpan={colSpan}
        className="py-6 text-center whitespace-normal text-muted-foreground"
      >
        {children}
      </TableCell>
    </TableRow>
  );
}

/** The heading of a group or a bucket: its name and its figures, side by side with its controls. */
function GroupHeading({
  id,
  title,
  figures,
  children,
}: {
  id: string;
  title: string;
  figures: ReactNode;
  children?: ReactNode;
}) {
  return (
    <CardHeader className="flex flex-wrap items-start justify-between gap-4">
      <div className="grid gap-1">
        <CardTitle id={id} className="text-lg">
          {title}
        </CardTitle>
        <p className="text-sm text-muted-foreground tabular-nums">{figures}</p>
      </div>
      {children}
    </CardHeader>
  );
}

/** The emoji a category chose, in a fixed slot so names line up whether or not they have one. */
function CategoryIcon({ icon }: { icon: string | undefined }) {
  return (
    <span className="inline-block w-6 text-center" aria-hidden="true">
      {icon ?? ""}
    </span>
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
    <Card
      className="gap-4"
      data-testid={`budget-group-${id}`}
      aria-labelledby={`budget-group-${id}-title`}
    >
      <GroupHeading
        id={`budget-group-${id}-title`}
        title={group.group.name}
        figures={
          <>
            <span data-testid="group-spent">{fmt(group.spent)}</span> of{" "}
            <span data-testid="group-budgeted">{fmt(group.budgeted)}</span> ·{" "}
            <span data-testid="group-remaining" className={group.remaining < 0 ? OVER : undefined}>
              {fmt(group.remaining)}
            </span>{" "}
            left
          </>
        }
      >
        {group.group.id === UNGROUPED_ID ? null : whole && group.budget !== null ? (
          <div className="flex flex-wrap items-center gap-4">
            <span className="flex items-center gap-2">
              <span className="text-xs font-medium text-muted-foreground">Group budget</span>
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
            <span className="flex items-center gap-2">
              <Checkbox
                id={`budget-group-${id}-rollover`}
                aria-label={`Roll over ${group.group.name}`}
                checked={group.budget.budget.rollover}
                disabled={editing.readOnly}
                onCheckedChange={(checked) =>
                  void editing.setBudget(
                    "group",
                    group.group.id,
                    group.budget?.amount ?? 0,
                    checked === true,
                  )
                }
              />
              <Label htmlFor={`budget-group-${id}-rollover`} className="font-normal">
                rolls over
              </Label>
            </span>
            {group.budget.carriedIn === 0 ? null : (
              <small className="text-xs text-muted-foreground" data-testid="group-carried-in">
                {fmt(group.budget.carriedIn)} carried in
              </small>
            )}
            <Button
              variant="link"
              size="sm"
              className="h-auto p-0"
              disabled={editing.readOnly}
              onClick={() => void editing.clearBudget(group.group.id)}
            >
              Budget categories separately
            </Button>
          </div>
        ) : (
          <Button
            variant="link"
            size="sm"
            className="h-auto p-0"
            disabled={editing.readOnly}
            onClick={() => void editing.setBudget("group", group.group.id, ownAmounts, false)}
          >
            Budget this group as a whole
          </Button>
        )}
      </GroupHeading>
      <CardContent className="grid gap-3">
        <Progress
          value={percentOf(group.spent, group.budgeted)}
          tone={toneFor(group.spent, group.budgeted)}
          aria-label={`${group.group.name} budget used`}
        />
        <Table aria-label={`${group.group.name} categories`}>
          <TableHeader>
            <TableRow>
              <TableHead scope="col">Category</TableHead>
              <MoneyHead scope="col">Budget</MoneyHead>
              <TableHead scope="col">Roll over</TableHead>
              <MoneyHead scope="col">Carried in</MoneyHead>
              <MoneyHead scope="col">Allowance</MoneyHead>
              <MoneyHead scope="col">Spent</MoneyHead>
              <MoneyHead scope="col">Remaining</MoneyHead>
              <TableHead scope="col" className="w-28 min-w-20">
                Used
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {group.categories.length === 0 ? (
              <EmptyRow colSpan={8}>No categories in this group.</EmptyRow>
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
          </TableBody>
        </Table>
      </CardContent>
    </Card>
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
  return (
    <TableRow data-testid={`budget-${category.id}`} data-percent={budget?.percent ?? 0}>
      <RowHead>
        <CategoryIcon icon={icon} />
        <a href={transactionsHash({ category: category.id, month })}>{category.name}</a>
        {share === 0 ? null : (
          <small
            className="block text-xs font-normal text-muted-foreground"
            data-testid="suggested"
          >
            {" "}
            {fmt(category.target_amount ?? 0)} every {category.target_months ?? 0} months ·{" "}
            {fmt(share)} a month
          </small>
        )}
      </RowHead>
      <TableCell className="money">
        {underGroupBudget ? (
          <span className="text-muted-foreground" title="Budgeted with its group">
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
      </TableCell>
      <TableCell>
        {underGroupBudget ? (
          DASH
        ) : (
          <Checkbox
            aria-label={`Roll over ${category.name}`}
            checked={budget?.budget.rollover ?? false}
            disabled={editing.readOnly || budget === null}
            title={budget === null ? "Set a budget first" : undefined}
            onCheckedChange={(checked) =>
              void editing.setBudget("category", category.id, budget?.amount ?? 0, checked === true)
            }
          />
        )}
      </TableCell>
      <TableCell className="money" data-testid="carried-in">
        {budget === null || budget.carriedIn === 0 ? DASH : fmt(budget.carriedIn)}
      </TableCell>
      <TableCell className="money" data-testid="allowance">
        {budget === null ? DASH : fmt(budget.allowance)}
      </TableCell>
      <TableCell className="money" data-testid="spent">
        {fmt(spent)}
      </TableCell>
      <TableCell
        className={cn("money", budget !== null && budget.remaining < 0 && OVER)}
        data-testid="remaining"
      >
        {budget === null ? DASH : fmt(budget.remaining)}
      </TableCell>
      <TableCell className="w-28 min-w-20">
        {budget === null ? null : (
          <Progress
            className="h-1.5"
            value={percentOf(spent, budget.allowance)}
            tone={toneFor(spent, budget.allowance)}
            aria-label={`${category.name} budget used`}
          />
        )}
      </TableCell>
    </TableRow>
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
    <Card className="gap-4" data-testid="unbudgeted" aria-labelledby="unbudgeted-title">
      <GroupHeading
        id="unbudgeted-title"
        title="Unbudgeted spending"
        figures={
          <>
            <span data-testid="unbudgeted-total">{fmt(total)}</span> with no budget against it
          </>
        }
      />
      <CardContent>
        <Table aria-label="Unbudgeted spending">
          <TableHeader>
            <TableRow>
              <TableHead scope="col">Category</TableHead>
              <MoneyHead scope="col">Spent</MoneyHead>
              <TableHead scope="col">
                <span className="sr-only">Actions</span>
              </TableHead>
            </TableRow>
          </TableHeader>
          <TableBody>
            {model.unbudgeted.map((entry) => (
              <TableRow
                key={entry.categoryId === "" ? "uncategorized" : entry.categoryId}
                data-testid={`unbudgeted-${entry.categoryId === "" ? "uncategorized" : entry.categoryId}`}
              >
                <RowHead>
                  {entry.categoryId === "" ? (
                    <span className="text-muted-foreground">Uncategorized</span>
                  ) : (
                    nameOf(entry.categoryId)
                  )}
                </RowHead>
                <TableCell className="money" data-testid="spent">
                  {fmt(entry.spent)}
                </TableCell>
                <TableCell className="text-right">
                  {entry.categoryId === "" ? (
                    <a href={transactionsHash({ month })}>Categorize</a>
                  ) : (
                    <Button
                      variant="link"
                      size="sm"
                      className="h-auto p-0"
                      disabled={editing.readOnly}
                      onClick={() =>
                        void editing.setBudget("category", entry.categoryId, entry.spent, false)
                      }
                    >
                      Budget it
                    </Button>
                  )}
                </TableCell>
              </TableRow>
            ))}
          </TableBody>
        </Table>
      </CardContent>
    </Card>
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
    <div className="grid items-start gap-5 lg:grid-cols-2">
      <Card className="gap-4" data-testid="bucket-fixed" aria-labelledby="bucket-fixed-title">
        <GroupHeading
          id="bucket-fixed-title"
          title="Fixed"
          figures={
            <>
              <span data-testid="bucket-spent">{fmt(flex.fixed.spent)}</span> of{" "}
              <span data-testid="bucket-budgeted">{fmt(flex.fixed.budgeted)}</span>
            </>
          }
        />
        <CardContent className="grid gap-3">
          <Progress
            value={percentOf(flex.fixed.spent, flex.fixed.budgeted)}
            tone={toneFor(flex.fixed.spent, flex.fixed.budgeted)}
            aria-label="Fixed costs used"
          />
          <Table aria-label="Fixed categories">
            <TableHeader>
              <TableRow>
                <TableHead scope="col">Category</TableHead>
                <MoneyHead scope="col">Budget</MoneyHead>
                <MoneyHead scope="col">Spent</MoneyHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {fixed.length === 0 ? (
                <EmptyRow colSpan={3}>
                  No fixed categories. Mark rent, loans, and bills as fixed in Settings ›
                  Categories.
                </EmptyRow>
              ) : null}
              {fixed.map(({ entry, group }) => (
                <TableRow key={entry.category.id} data-testid={`budget-${entry.category.id}`}>
                  <RowHead>
                    <CategoryIcon icon={icons.get(entry.category.id)} />
                    {entry.category.name}
                    <small className="text-xs text-muted-foreground"> {group.group.name}</small>
                  </RowHead>
                  <TableCell className="money">
                    {group.budget !== null ? (
                      <span className="text-muted-foreground">in group</span>
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
                  </TableCell>
                  <TableCell className="money" data-testid="spent">
                    {fmt(entry.spent)}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </CardContent>
      </Card>

      <Card
        className="gap-4"
        data-testid="bucket-non-monthly"
        aria-labelledby="bucket-non-monthly-title"
      >
        <GroupHeading
          id="bucket-non-monthly-title"
          title="Non-monthly"
          figures={
            <>
              <span data-testid="bucket-spent">{fmt(flex.nonMonthly.spent)}</span> spent ·{" "}
              <span data-testid="bucket-share">{fmt(flex.nonMonthly.share)}</span> set aside a month
            </>
          }
        />
        <CardContent className="grid gap-3">
          <Progress
            value={percentOf(flex.nonMonthly.spent, flex.nonMonthly.share)}
            tone={toneFor(flex.nonMonthly.spent, flex.nonMonthly.share)}
            aria-label="Non-monthly share used"
          />
          <Table aria-label="Non-monthly categories">
            <TableHeader>
              <TableRow>
                <TableHead scope="col">Category</TableHead>
                <TableHead scope="col">Target</TableHead>
                <MoneyHead scope="col">A month</MoneyHead>
                <MoneyHead scope="col">Spent</MoneyHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {nonMonthly.length === 0 ? (
                <EmptyRow colSpan={4}>
                  No non-monthly categories. Give a category a target in Settings › Categories.
                </EmptyRow>
              ) : null}
              {nonMonthly.map(({ entry }) => (
                <TableRow key={entry.category.id} data-testid={`budget-${entry.category.id}`}>
                  <RowHead>
                    <CategoryIcon icon={icons.get(entry.category.id)} />
                    {entry.category.name}
                  </RowHead>
                  <TableCell className="whitespace-normal">
                    {entry.share === 0 ? (
                      <span className="text-muted-foreground">no target yet</span>
                    ) : (
                      `${fmt(entry.category.target_amount ?? 0)} every ${entry.category.target_months ?? 0} months`
                    )}
                  </TableCell>
                  <TableCell className="money" data-testid="share">
                    {fmt(entry.share)}
                  </TableCell>
                  <TableCell className="money" data-testid="spent">
                    {fmt(entry.spent)}
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </CardContent>
      </Card>

      <Card
        className="gap-4 lg:col-span-2"
        data-testid="bucket-flexible"
        aria-labelledby="bucket-flexible-title"
      >
        <GroupHeading
          id="bucket-flexible-title"
          title="Flexible"
          figures={
            <>
              <span data-testid="flexible-spent">{fmt(flex.flexible.spent)}</span> of{" "}
              <span data-testid="flexible-allowance">{fmt(flex.flexible.allowance)}</span> ·{" "}
              <span
                data-testid="flexible-remaining"
                className={flex.flexible.remaining < 0 ? OVER : undefined}
              >
                {fmt(flex.flexible.remaining)}
              </span>{" "}
              left
            </>
          }
        />
        <CardContent className="grid gap-4">
          <Progress
            value={percentOf(flex.flexible.spent, flex.flexible.allowance)}
            tone={toneFor(flex.flexible.spent, flex.flexible.allowance)}
            aria-label="Flexible spending used"
          />
          <dl
            className="grid grid-cols-[minmax(0,1fr)_auto] gap-x-6 gap-y-1 rounded-lg border bg-muted/40 px-4 py-3 text-sm"
            aria-label="How the flexible amount is made"
          >
            <dt className="text-muted-foreground">Expected income</dt>
            <dd className="money" data-testid="term-income">
              {fmt(model.income.expected)}
            </dd>
            <dt className="text-muted-foreground">− Fixed budgets</dt>
            <dd className="money" data-testid="term-fixed">
              {fmt(flex.fixed.budgeted)}
            </dd>
            <dt className="text-muted-foreground">− Non-monthly, a month</dt>
            <dd className="money" data-testid="term-non-monthly">
              {fmt(flex.nonMonthly.share)}
            </dd>
            <dt className="text-muted-foreground">− Planned goal contributions</dt>
            <dd className="money" data-testid="term-goals">
              {fmt(planned)}
            </dd>
            <dt className="mt-1 border-t border-input pt-1.5 font-semibold">= Flexible</dt>
            <dd
              className="money mt-1 border-t border-input pt-1.5 font-semibold"
              data-testid="term-flexible"
            >
              {fmt(flex.flexible.allowance)}
            </dd>
          </dl>
          <Table aria-label="Flexible spending">
            <TableHeader>
              <TableRow>
                <TableHead scope="col">Category</TableHead>
                <MoneyHead scope="col">Spent</MoneyHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {flexible.length === 0 && uncategorized === 0 ? (
                <EmptyRow colSpan={2}>Nothing flexible spent yet this month.</EmptyRow>
              ) : null}
              {flexible.map(({ entry, group }) => (
                <TableRow key={entry.category.id} data-testid={`budget-${entry.category.id}`}>
                  <RowHead>
                    <CategoryIcon icon={icons.get(entry.category.id)} />
                    {entry.category.name}
                    <small className="text-xs text-muted-foreground"> {group.group.name}</small>
                  </RowHead>
                  <TableCell className="money" data-testid="spent">
                    {fmt(entry.spent)}
                  </TableCell>
                </TableRow>
              ))}
              {uncategorized === 0 ? null : (
                <TableRow data-testid="budget-uncategorized">
                  <RowHead>
                    <span className="text-muted-foreground">Uncategorized</span>
                  </RowHead>
                  <TableCell className="money" data-testid="spent">
                    {fmt(uncategorized)}
                  </TableCell>
                </TableRow>
              )}
            </TableBody>
          </Table>
        </CardContent>
      </Card>
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
  className,
  onSave,
  onClear,
  onDone,
}: {
  label: string;
  amount: number | null;
  placeholder: string;
  autoFocus?: boolean;
  editing: Editing;
  /** Sizing for where the field sits; a table cell's narrow figure by default. */
  className?: string;
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
    <Input
      key={stored}
      className={cn("money ml-auto h-8 w-28 px-2 text-sm md:text-sm", className)}
      aria-label={label}
      inputMode="decimal"
      defaultValue={stored}
      placeholder={placeholder}
      disabled={editing.readOnly}
      // The field appears because the person clicked to set it, so it takes focus.
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

/** How full a bar is, as the percentage the kit's Progress takes; nothing over a limit of zero. */
function percentOf(value: number, max: number): number {
  if (!(max > 0) || !Number.isFinite(value)) return 0;
  return Math.min(100, Math.max(0, (value / max) * 100));
}

/** Calm under the limit, a warning near it, danger past it; a limit of zero is only trouble once spent. */
function toneFor(spent: number, allowance: number): "primary" | "warning" | "destructive" {
  if (allowance <= 0) return spent > 0 ? "destructive" : "primary";
  const ratio = spent / allowance;
  return ratio > 1 ? "destructive" : ratio >= 0.85 ? "warning" : "primary";
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
