import {
  Alert,
  AlertDescription,
  Button,
  Field,
  Input,
  NativeSelect,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  cn,
} from "@mako-cloud/ui";
import { type FormEvent, useId, useState } from "react";

import type { RationalApp } from "../data/rational.js";
import { ValidationError } from "../data/writes.js";
import { type Account, ASSET_CLASSES, type AssetClass } from "../model/types.js";
import type { HoldingRow } from "../selectors/holdings.js";
import { amountToText, formatMinorUnits, parseAmount } from "../selectors/money.js";
import { EditorDialog, todayIso } from "./account-form.js";
import { routeHash } from "./router.js";

/**
 * Positions, as the investments page and an investment account's own page
 * show them. Both draw the same table and open the same editor, so a
 * holding reads the same wherever it is met; the numbers come from the
 * holdings selector, valued by the shared engine that also fills the account
 * balance and the nightly snapshot.
 */

export const ASSET_CLASS_LABELS: Readonly<Record<AssetClass, string>> = {
  stock: "Stocks",
  etf: "ETFs",
  fund: "Funds",
  bond: "Bonds",
  cash: "Cash",
  crypto: "Crypto",
  other: "Other",
};

/** Units held, as a person would write them: whole when whole, fractional when not. */
export function quantityText(quantity: number): string {
  return quantity.toLocaleString("en-US", { maximumFractionDigits: 6 });
}

/**
 * A gain as "+$50.00 (5.3%)", signed, with the share of what was paid when
 * that is known; a dash when no cost basis was recorded, because "no gain"
 * and "unknown gain" are different answers.
 */
export function gainText(gain: number | null, costBasis: number | undefined, currency: string) {
  if (gain === null) return "—";
  const amount = `${gain > 0 ? "+" : ""}${formatMinorUnits(gain, currency)}`;
  if (costBasis === undefined || costBasis <= 0) return amount;
  return `${amount} (${((gain / costBasis) * 100).toFixed(1)}%)`;
}

function gainClass(gain: number | null): string | undefined {
  if (gain === null || gain === 0) return undefined;
  return gain > 0 ? "text-positive" : "text-destructive";
}

/** The table's cells sit flush with the card that holds it. */
const TABLE_IN_CARD =
  "[&_td:first-child]:pl-5 [&_td:last-child]:pr-5 [&_th:first-child]:pl-5 [&_th:last-child]:pr-5";

export function HoldingsTable({
  app,
  rows,
  showAccount,
  canEdit,
  onEdit,
  ariaLabel = "Holdings",
  emptyMessage = "No holdings yet.",
}: {
  app: RationalApp;
  rows: readonly HoldingRow[];
  showAccount: boolean;
  canEdit: boolean;
  onEdit: (row: HoldingRow) => void;
  ariaLabel?: string;
  emptyMessage?: string;
}) {
  const [pricing, setPricing] = useState<string | null>(null);
  const [problem, setProblem] = useState<string | null>(null);
  const columns = 6 + (showAccount ? 1 : 0) + (canEdit ? 1 : 0);

  const remove = async (row: HoldingRow) => {
    setProblem(null);
    try {
      await app.writes?.removeHolding(row.account.id, row.holding.id);
    } catch (caught) {
      setProblem(caught instanceof Error ? caught.message : "The holding could not be removed.");
    }
  };

  return (
    <>
      {problem === null ? null : (
        <Alert variant="destructive" role="alert" className="mx-5 mb-4 w-auto">
          <AlertDescription>{problem}</AlertDescription>
        </Alert>
      )}
      <Table aria-label={ariaLabel} className={TABLE_IN_CARD}>
        <TableHeader>
          <TableRow>
            {showAccount ? <TableHead scope="col">Account</TableHead> : null}
            <TableHead scope="col">Holding</TableHead>
            <TableHead scope="col" className="money text-right">
              Quantity
            </TableHead>
            <TableHead scope="col" className="money text-right">
              Price
            </TableHead>
            <TableHead scope="col" className="money text-right">
              Value
            </TableHead>
            <TableHead scope="col" className="money text-right">
              Cost basis
            </TableHead>
            <TableHead scope="col" className="money text-right">
              Gain
            </TableHead>
            {canEdit ? (
              <TableHead scope="col" className="text-right">
                <span className="sr-only">Actions</span>
              </TableHead>
            ) : null}
          </TableRow>
        </TableHeader>
        <TableBody>
          {rows.length === 0 ? (
            <TableRow>
              <TableCell colSpan={columns} className="py-8 text-center text-muted-foreground">
                {emptyMessage}
              </TableCell>
            </TableRow>
          ) : null}
          {rows.map((row) => {
            const currency = row.account.currency;
            const { holding } = row;
            return (
              <TableRow
                key={`${row.account.id}:${holding.id}`}
                data-testid={`holding-${holding.id}`}
                data-symbol={holding.symbol}
              >
                {showAccount ? (
                  <TableCell>
                    <a
                      className="text-primary"
                      href={routeHash({ name: "account", accountId: row.account.id })}
                    >
                      {row.account.name}
                    </a>
                  </TableCell>
                ) : null}
                <TableHead
                  scope="row"
                  className="h-auto min-w-48 py-2 leading-tight whitespace-normal text-foreground"
                >
                  {holding.symbol}
                  <small
                    data-testid="name"
                    className="block max-w-64 text-xs font-normal text-muted-foreground"
                  >
                    {holding.name}
                  </small>
                </TableHead>
                <TableCell className="money" data-testid="quantity">
                  {quantityText(holding.quantity)}
                </TableCell>
                <TableCell
                  className="money"
                  data-testid="price"
                  title={
                    holding.price_as_of === undefined ? undefined : `as of ${holding.price_as_of}`
                  }
                >
                  {formatMinorUnits(holding.price, currency)}
                </TableCell>
                <TableCell className="money font-medium" data-testid="value">
                  {formatMinorUnits(row.value, currency)}
                </TableCell>
                <TableCell className="money text-muted-foreground">
                  {holding.cost_basis === undefined
                    ? "—"
                    : formatMinorUnits(holding.cost_basis, currency)}
                </TableCell>
                <TableCell className={cn("money", gainClass(row.gain))} data-testid="gain">
                  {gainText(row.gain, holding.cost_basis, currency)}
                </TableCell>
                {canEdit ? (
                  <TableCell className="text-right">
                    {pricing === holding.id ? (
                      <PriceForm app={app} row={row} onDone={() => setPricing(null)} />
                    ) : (
                      <span className="inline-flex items-center justify-end gap-1">
                        <Button
                          variant="link"
                          size="sm"
                          className="h-auto px-1"
                          onClick={() => setPricing(holding.id)}
                        >
                          Update price
                        </Button>
                        <Button
                          variant="link"
                          size="sm"
                          className="h-auto px-1"
                          onClick={() => onEdit(row)}
                        >
                          Edit
                        </Button>
                        <Button
                          variant="link"
                          size="sm"
                          className="h-auto px-1 text-destructive"
                          onClick={() => void remove(row)}
                        >
                          Remove
                        </Button>
                      </span>
                    )}
                  </TableCell>
                ) : null}
              </TableRow>
            );
          })}
        </TableBody>
      </Table>
    </>
  );
}

/**
 * A new price for one position, in its own row. A price is an edit of the
 * account, not a transaction: the value of a position moving is not money the
 * household received or spent, and the write helper keeps it that way.
 */
function PriceForm({
  app,
  row,
  onDone,
}: {
  app: RationalApp;
  row: HoldingRow;
  onDone: () => void;
}) {
  const currency = row.account.currency;
  const [price, setPrice] = useState(amountToText(row.holding.price, currency));
  const [date, setDate] = useState(todayIso());
  const [error, setError] = useState<string | null>(null);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    const writes = app.writes;
    if (writes === null) return;
    try {
      await writes.updateHoldingPrice(
        row.account.id,
        row.holding.id,
        parseAmount(price, currency),
        date,
      );
      onDone();
    } catch (caught) {
      setError(
        caught instanceof ValidationError || caught instanceof RangeError
          ? caught.message
          : "The price could not be saved.",
      );
    }
  };

  return (
    <form
      className="inline-flex flex-wrap items-center justify-end gap-2"
      aria-label={`Update price of ${row.holding.symbol}`}
      onSubmit={(event) => void submit(event)}
    >
      <Input
        aria-label="New price"
        inputMode="decimal"
        required
        className="h-8 w-28 text-sm tabular-nums"
        value={price}
        onChange={(event) => setPrice(event.target.value)}
      />
      <Input
        aria-label="Price date"
        type="date"
        required
        className="h-8 w-40 text-sm"
        value={date}
        onChange={(event) => setDate(event.target.value)}
      />
      <Button type="submit" size="sm">
        Save price
      </Button>
      <Button variant="link" size="sm" className="h-auto px-1" onClick={onDone}>
        Cancel
      </Button>
      {error === null ? null : (
        <span className="text-xs text-destructive" role="alert">
          {error}
        </span>
      )}
    </form>
  );
}

/**
 * The editor for one position: new, or the one in `holding`. Given several
 * accounts and no position yet, the person chooses which account holds it; a
 * position being edited stays where it is, since moving one between accounts
 * is a sale and a purchase rather than an edit.
 */
export function HoldingForm({
  app,
  accounts,
  accountId,
  holding,
  onDone,
}: {
  app: RationalApp;
  accounts: readonly Account[];
  accountId: string;
  holding: HoldingRow | null;
  onDone: () => void;
}) {
  const id = useId();
  const [selectedAccountId, setSelectedAccountId] = useState(holding?.account.id ?? accountId);
  const account =
    accounts.find((candidate) => candidate.id === selectedAccountId) ?? holding?.account ?? null;
  const currency = account?.currency ?? "USD";
  const [symbol, setSymbol] = useState(holding?.holding.symbol ?? "");
  const [name, setName] = useState(holding?.holding.name ?? "");
  const [quantity, setQuantity] = useState(
    holding === null ? "" : String(holding.holding.quantity),
  );
  const [price, setPrice] = useState(
    holding === null ? "" : amountToText(holding.holding.price, currency),
  );
  const [costBasis, setCostBasis] = useState(
    holding?.holding.cost_basis === undefined
      ? ""
      : amountToText(holding.holding.cost_basis, currency),
  );
  const [assetClass, setAssetClass] = useState<AssetClass>(holding?.holding.asset_class ?? "stock");
  const [priceAsOf, setPriceAsOf] = useState(holding?.holding.price_as_of ?? todayIso());
  const [error, setError] = useState<string | null>(null);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    const writes = app.writes;
    if (writes === null || account === null) return;
    try {
      if (quantity.trim() === "") {
        throw new ValidationError("quantity is how many units are held");
      }
      const basis = costBasis.trim();
      await writes.upsertHolding(account.id, {
        ...(holding === null ? {} : { id: holding.holding.id }),
        symbol,
        name,
        quantity: Number(quantity),
        price: parseAmount(price, currency),
        ...(basis === "" ? {} : { cost_basis: parseAmount(basis, currency) }),
        asset_class: assetClass,
        price_as_of: priceAsOf,
      });
      onDone();
    } catch (caught) {
      setError(
        caught instanceof ValidationError || caught instanceof RangeError
          ? caught.message
          : "The holding could not be saved.",
      );
    }
  };

  return (
    <EditorDialog
      title={
        holding === null
          ? `Add holding${account === null ? "" : ` to ${account.name}`}`
          : `Edit ${holding.holding.symbol}`
      }
      formLabel="Holding editor"
      submitLabel="Save holding"
      onDone={onDone}
      onSubmit={(event) => void submit(event)}
    >
      <div className="grid gap-4 sm:grid-cols-2">
        {accounts.length > 1 || holding !== null ? (
          <Field label="Account" htmlFor={`${id}-account`} className="sm:col-span-2">
            <NativeSelect
              id={`${id}-account`}
              name="account_id"
              value={selectedAccountId}
              disabled={holding !== null}
              onChange={(event) => setSelectedAccountId(event.target.value)}
            >
              {(holding === null ? accounts : [holding.account]).map((candidate) => (
                <option key={candidate.id} value={candidate.id}>
                  {candidate.name}
                </option>
              ))}
            </NativeSelect>
          </Field>
        ) : null}
        <Field label="Symbol" htmlFor={`${id}-symbol`}>
          <Input
            id={`${id}-symbol`}
            name="symbol"
            required
            maxLength={32}
            value={symbol}
            onChange={(event) => setSymbol(event.target.value)}
          />
        </Field>
        <Field label="Name" htmlFor={`${id}-name`}>
          <Input
            id={`${id}-name`}
            name="name"
            required
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </Field>
        <Field label="Quantity" htmlFor={`${id}-quantity`}>
          <Input
            id={`${id}-quantity`}
            name="quantity"
            inputMode="decimal"
            required
            className="tabular-nums"
            value={quantity}
            onChange={(event) => setQuantity(event.target.value)}
          />
        </Field>
        <Field label={`Price per unit (${currency})`} htmlFor={`${id}-price`}>
          <Input
            id={`${id}-price`}
            name="price"
            inputMode="decimal"
            required
            className="tabular-nums"
            value={price}
            onChange={(event) => setPrice(event.target.value)}
          />
        </Field>
        <Field label={`Cost basis (${currency})`} htmlFor={`${id}-cost-basis`}>
          <Input
            id={`${id}-cost-basis`}
            name="cost_basis"
            inputMode="decimal"
            placeholder="what the whole position cost"
            className="tabular-nums"
            value={costBasis}
            onChange={(event) => setCostBasis(event.target.value)}
          />
        </Field>
        <Field label="Asset class" htmlFor={`${id}-asset-class`}>
          <NativeSelect
            id={`${id}-asset-class`}
            name="asset_class"
            value={assetClass}
            onChange={(event) => setAssetClass(event.target.value as AssetClass)}
          >
            {ASSET_CLASSES.map((candidate) => (
              <option key={candidate} value={candidate}>
                {ASSET_CLASS_LABELS[candidate]}
              </option>
            ))}
          </NativeSelect>
        </Field>
        <Field label="Price as of" htmlFor={`${id}-price-as-of`}>
          <Input
            id={`${id}-price-as-of`}
            name="price_as_of"
            type="date"
            required
            value={priceAsOf}
            onChange={(event) => setPriceAsOf(event.target.value)}
          />
        </Field>
      </div>
      {error === null ? null : (
        <p className="m-0 text-sm text-destructive" role="alert" data-testid="form-error">
          {error}
        </p>
      )}
    </EditorDialog>
  );
}
