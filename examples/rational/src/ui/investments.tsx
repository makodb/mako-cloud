import {
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  DonutChart,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  seriesColor,
} from "@mako-cloud/ui";
import { Plus } from "lucide-react";
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
import { PageHeader, Stat } from "./account-form.js";
import { ASSET_CLASS_LABELS, HoldingForm, HoldingsTable } from "./holdings.js";
import { useBehavior, useQuery } from "./hooks.js";
import { routeHash } from "./router.js";

/** The table's cells sit flush with the card that holds it. */
const TABLE_IN_CARD =
  "[&_td:first-child]:pl-5 [&_td:last-child]:pr-5 [&_th:first-child]:pl-5 [&_th:last-child]:pr-5";

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
    <section
      aria-labelledby="investments-title"
      data-testid="investments-screen"
      className="grid gap-6"
    >
      <PageHeader
        id="investments-title"
        title="Investments"
        subtitle="Every position the household holds, valued into the balance of the account that holds it."
        actions={
          canEdit && firstAccount !== undefined ? (
            <Button onClick={() => setEditing({ accountId: firstAccount.id, holding: null })}>
              <Plus aria-hidden="true" />
              Add holding
            </Button>
          ) : undefined
        }
      />

      <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3" data-testid="investments-total">
        {totals.length === 0 ? (
          <Stat
            data-testid={`investments-total-${currency}`}
            label={`Investments (${currency})`}
            value={formatMinorUnits(0, currency)}
            valueTestId="total"
            note="No investment or crypto accounts yet."
          />
        ) : (
          totals.map((total) => (
            <Stat
              key={total.currency}
              data-testid={`investments-total-${total.currency}`}
              label={`Investments (${total.currency})`}
              value={formatMinorUnits(total.total, total.currency)}
              valueTestId="total"
              note={`${countText(
                investmentAccounts.filter((account) => account.currency === total.currency).length,
                "account",
              )} · ${countText(
                rows.filter((row) => row.account.currency === total.currency).length,
                "position",
              )}`}
            />
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
        <p
          className="m-0 rounded-xl border border-dashed px-6 py-10 text-center text-sm text-muted-foreground"
          data-testid="investments-empty"
        >
          No investment or crypto account yet. Add one on the{" "}
          <a className="text-primary" href={routeHash({ name: "accounts" })}>
            Accounts
          </a>{" "}
          page and its positions show up here.
        </p>
      ) : (
        <>
          <Card>
            <CardHeader>
              <CardTitle>Allocation</CardTitle>
            </CardHeader>
            <CardContent className="grid gap-8">
              {totals.map((total) => (
                <Allocation key={total.currency} rows={rows} currency={total.currency} />
              ))}
            </CardContent>
          </Card>

          <Card className="gap-4 pb-0">
            <CardHeader>
              <CardTitle>Investment accounts</CardTitle>
            </CardHeader>
            <CardContent className="px-0">
              <Table aria-label="Investment accounts" className={TABLE_IN_CARD}>
                <TableHeader>
                  <TableRow>
                    <TableHead scope="col">Account</TableHead>
                    <TableHead scope="col">Type</TableHead>
                    <TableHead scope="col">Institution</TableHead>
                    <TableHead scope="col" className="money text-right">
                      Positions
                    </TableHead>
                    <TableHead scope="col" className="money text-right">
                      Holdings
                    </TableHead>
                    <TableHead scope="col" className="money text-right">
                      Cash
                    </TableHead>
                    <TableHead scope="col" className="money text-right">
                      Balance
                    </TableHead>
                    {canEdit ? (
                      <TableHead scope="col" className="text-right">
                        <span className="sr-only">Actions</span>
                      </TableHead>
                    ) : null}
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {investmentAccounts.map((account) => (
                    <InvestmentAccountRow
                      key={account.id}
                      account={account}
                      balance={balances.get(account.id) ?? account.opening_balance}
                      canEdit={canEdit}
                      onAdd={() => setEditing({ accountId: account.id, holding: null })}
                    />
                  ))}
                </TableBody>
              </Table>
            </CardContent>
          </Card>

          <Card className="gap-4 pb-0">
            <CardHeader>
              <CardTitle>Holdings</CardTitle>
            </CardHeader>
            <CardContent className="px-0">
              <HoldingsTable
                app={app}
                rows={rows}
                showAccount
                canEdit={canEdit}
                onEdit={(row) => setEditing({ accountId: row.account.id, holding: row })}
                emptyMessage="No holdings yet. Add one to an investment or crypto account."
              />
            </CardContent>
          </Card>
        </>
      )}
    </section>
  );
}

function percent(share: number): string {
  return `${Math.round(share * 100)}%`;
}

/**
 * How one currency's positions divide by asset class. Nothing is converted,
 * so a household with a brokerage in two currencies gets two rings rather
 * than one that adds unlike things. The ring shows proportion; the legend
 * beside it carries every number, since an angle is not a figure anyone can
 * read off.
 */
function Allocation({ rows, currency }: { rows: readonly HoldingRow[]; currency: string }) {
  const slices = useMemo(() => allocationByClass(rows, currency), [rows, currency]);
  const format = (value: number) => formatMinorUnits(value, currency);
  const drawn = slices
    .map((slice, index) => ({ slice, index }))
    .filter(({ slice }) => Number.isFinite(slice.value) && slice.value > 0);
  const sum = drawn.reduce((acc, { slice }) => acc + slice.value, 0);
  const title = `Allocation of ${currency} holdings by asset class: ${slices
    .map((slice) => `${ASSET_CLASS_LABELS[slice.asset_class]} ${percent(slice.share)}`)
    .join(", ")}`;

  if (slices.length === 0 || sum <= 0) {
    return (
      <p className="m-0 text-sm text-muted-foreground" data-testid={`allocation-${currency}`}>
        No positions in {currency} yet.
      </p>
    );
  }

  return (
    <div
      className="grid items-center gap-8 sm:grid-cols-[minmax(0,15rem)_minmax(0,26rem)]"
      data-testid={`allocation-${currency}`}
    >
      <DonutChart
        data={drawn.map(({ slice, index }) => ({
          name: ASSET_CLASS_LABELS[slice.asset_class],
          value: slice.value,
          color: seriesColor(index),
        }))}
        title={title}
        height={220}
        formatValue={format}
        center={{ label: "by asset class", value: format(sum) }}
      />
      <ol className="m-0 grid list-none gap-2 p-0 text-sm">
        {slices.map((slice, index) => (
          <li
            key={slice.asset_class}
            data-key={slice.asset_class}
            className="grid grid-cols-[auto_minmax(0,1fr)_auto_auto] items-center gap-3"
          >
            <span
              aria-hidden="true"
              className="size-2.5 rounded-[3px]"
              style={{ background: seriesColor(index) }}
            />
            <span>{ASSET_CLASS_LABELS[slice.asset_class]}</span>
            <span className="text-muted-foreground tabular-nums">
              {percent(Math.max(0, slice.value) / sum)}
            </span>
            <span className="money font-medium">{format(slice.value)}</span>
          </li>
        ))}
      </ol>
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
    <TableRow data-testid={`investment-account-${account.id}`} data-name={account.name}>
      <TableCell>
        <a
          className="font-medium text-primary"
          href={routeHash({ name: "account", accountId: account.id })}
        >
          {account.name}
        </a>
      </TableCell>
      <TableCell>{ACCOUNT_TYPE_LABELS[account.type]}</TableCell>
      <TableCell>
        {account.institution ?? <span className="text-muted-foreground">—</span>}
      </TableCell>
      <TableCell className="money">{account.holdings?.length ?? 0}</TableCell>
      <TableCell className="money" data-testid="holdings-value">
        {formatMinorUnits(positions, account.currency)}
      </TableCell>
      <TableCell className="money" data-testid="cash">
        {formatMinorUnits(balance - positions, account.currency)}
      </TableCell>
      <TableCell className="money font-medium" data-testid="balance">
        {formatMinorUnits(balance, account.currency)}
      </TableCell>
      {canEdit ? (
        <TableCell className="text-right">
          <Button variant="link" size="sm" className="h-auto px-1" onClick={onAdd}>
            Add holding
          </Button>
        </TableCell>
      ) : null}
    </TableRow>
  );
}

function countText(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}
