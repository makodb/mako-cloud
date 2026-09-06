import { type FormEvent, useState } from "react";

import type { RationalApp } from "../data/rational.js";
import { ValidationError } from "../data/writes.js";
import { type Account, ASSET_CLASSES, type AssetClass } from "../model/types.js";
import type { HoldingRow } from "../selectors/holdings.js";
import { amountToText, formatMinorUnits, parseAmount } from "../selectors/money.js";
import { todayIso } from "./account-form.js";
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

function gainClass(gain: number | null): string {
  if (gain === null || gain === 0) return "";
  return gain > 0 ? "gain-positive" : "gain-negative";
}

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
        <p className="notice error" role="alert">
          {problem}
        </p>
      )}
      <div className="table-scroll">
        <table className="list" aria-label={ariaLabel}>
          <thead>
            <tr>
              {showAccount ? <th scope="col">Account</th> : null}
              <th scope="col">Holding</th>
              <th scope="col" className="amount">
                Quantity
              </th>
              <th scope="col" className="amount">
                Price
              </th>
              <th scope="col" className="amount">
                Value
              </th>
              <th scope="col" className="amount">
                Cost basis
              </th>
              <th scope="col" className="amount">
                Gain
              </th>
              {canEdit ? (
                <th scope="col">
                  <span className="visually-hidden">Actions</span>
                </th>
              ) : null}
            </tr>
          </thead>
          <tbody>
            {rows.length === 0 ? (
              <tr>
                <td colSpan={columns} className="empty">
                  {emptyMessage}
                </td>
              </tr>
            ) : null}
            {rows.map((row) => {
              const currency = row.account.currency;
              const { holding } = row;
              return (
                <tr
                  key={`${row.account.id}:${holding.id}`}
                  data-testid={`holding-${holding.id}`}
                  data-symbol={holding.symbol}
                >
                  {showAccount ? (
                    <td>
                      <a href={routeHash({ name: "account", accountId: row.account.id })}>
                        {row.account.name}
                      </a>
                    </td>
                  ) : null}
                  <th scope="row" className="holding-symbol">
                    {holding.symbol}
                    <small data-testid="name">{holding.name}</small>
                  </th>
                  <td className="amount" data-testid="quantity">
                    {quantityText(holding.quantity)}
                  </td>
                  <td
                    className="amount"
                    data-testid="price"
                    title={
                      holding.price_as_of === undefined ? undefined : `as of ${holding.price_as_of}`
                    }
                  >
                    {formatMinorUnits(holding.price, currency)}
                  </td>
                  <td className="amount" data-testid="value">
                    {formatMinorUnits(row.value, currency)}
                  </td>
                  <td className="amount">
                    {holding.cost_basis === undefined
                      ? "—"
                      : formatMinorUnits(holding.cost_basis, currency)}
                  </td>
                  <td className={`amount ${gainClass(row.gain)}`} data-testid="gain">
                    {gainText(row.gain, holding.cost_basis, currency)}
                  </td>
                  {canEdit ? (
                    <td className="actions">
                      {pricing === holding.id ? (
                        <PriceForm app={app} row={row} onDone={() => setPricing(null)} />
                      ) : (
                        <>
                          <button
                            type="button"
                            className="link"
                            onClick={() => setPricing(holding.id)}
                          >
                            Update price
                          </button>
                          <button type="button" className="link" onClick={() => onEdit(row)}>
                            Edit
                          </button>
                          <button type="button" className="link" onClick={() => void remove(row)}>
                            Remove
                          </button>
                        </>
                      )}
                    </td>
                  ) : null}
                </tr>
              );
            })}
          </tbody>
        </table>
      </div>
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
      className="inline-form"
      aria-label={`Update price of ${row.holding.symbol}`}
      onSubmit={(event) => void submit(event)}
    >
      <input
        aria-label="New price"
        inputMode="decimal"
        required
        value={price}
        onChange={(event) => setPrice(event.target.value)}
      />
      <input
        aria-label="Price date"
        type="date"
        required
        value={date}
        onChange={(event) => setDate(event.target.value)}
      />
      <button type="submit">Save price</button>
      <button type="button" className="link" onClick={onDone}>
        Cancel
      </button>
      {error === null ? null : (
        <span className="error" role="alert">
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
    <form className="editor" onSubmit={(event) => void submit(event)} aria-label="Holding editor">
      <h2>
        {holding === null
          ? `Add holding${account === null ? "" : ` to ${account.name}`}`
          : `Edit ${holding.holding.symbol}`}
      </h2>
      <div className="grid">
        {accounts.length > 1 || holding !== null ? (
          <label>
            Account
            <select
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
            </select>
          </label>
        ) : null}
        <label>
          Symbol
          <input
            name="symbol"
            required
            maxLength={32}
            value={symbol}
            onChange={(event) => setSymbol(event.target.value)}
          />
        </label>
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
          Quantity
          <input
            name="quantity"
            inputMode="decimal"
            required
            value={quantity}
            onChange={(event) => setQuantity(event.target.value)}
          />
        </label>
        <label>
          Price per unit ({currency})
          <input
            name="price"
            inputMode="decimal"
            required
            value={price}
            onChange={(event) => setPrice(event.target.value)}
          />
        </label>
        <label>
          Cost basis ({currency})
          <input
            name="cost_basis"
            inputMode="decimal"
            placeholder="what the whole position cost"
            value={costBasis}
            onChange={(event) => setCostBasis(event.target.value)}
          />
        </label>
        <label>
          Asset class
          <select
            name="asset_class"
            value={assetClass}
            onChange={(event) => setAssetClass(event.target.value as AssetClass)}
          >
            {ASSET_CLASSES.map((candidate) => (
              <option key={candidate} value={candidate}>
                {ASSET_CLASS_LABELS[candidate]}
              </option>
            ))}
          </select>
        </label>
        <label>
          Price as of
          <input
            name="price_as_of"
            type="date"
            required
            value={priceAsOf}
            onChange={(event) => setPriceAsOf(event.target.value)}
          />
        </label>
      </div>
      {error === null ? null : (
        <p className="error" role="alert" data-testid="form-error">
          {error}
        </p>
      )}
      <div className="actions">
        <button type="submit">Save holding</button>
        <button type="button" className="secondary" onClick={onDone}>
          Cancel
        </button>
      </div>
    </form>
  );
}
