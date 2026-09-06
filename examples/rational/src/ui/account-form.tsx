import { type FormEvent, useState } from "react";

import type { RationalApp } from "../data/rational.js";
import { ValidationError } from "../data/writes.js";
import {
  ACCOUNT_TYPE_LABELS,
  ACCOUNT_TYPES,
  type Account,
  type AccountType,
  HOLDING_TYPES,
  type Membership,
  TRACKED_TYPES,
} from "../model/types.js";
import { HISTORY_RANGE_KEYS, type HistoryRangeKey } from "../selectors/history.js";
import { amountToText, parseAmount } from "../selectors/money.js";
import { useQuery } from "./hooks.js";

/**
 * What the account screens share: the editor an account is created and
 * changed with, the range picker every balance chart wears, and how a member
 * is named. The accounts list and an account's own page both open the same
 * editor, so a field added here is added everywhere an account is edited.
 */

export function todayIso(): string {
  return new Date().toISOString().slice(0, 10);
}

/** The name a member is shown by: their address, or their id before one is known. */
export function memberLabel(membership: Membership): string {
  return membership.email ?? membership.user_id;
}

/**
 * The household's active members, read from the directory this device holds.
 * `app.state.memberships` is the signed-in person's own memberships -- one per
 * household -- so it cannot list the other members; the directory database
 * can, because the policy replicates every membership of a household one
 * belongs to. Until that has arrived, the person themself is the one member.
 */
export function useHouseholdMembers(app: RationalApp): readonly Membership[] {
  const householdId = app.state.currentHouseholdId;
  const members = useQuery(
    householdId === null
      ? null
      : (app.directory?.session
          ?.collection("memberships")
          ?.find({ selector: { household_id: householdId, status: "active" } }) ?? null),
  );
  if (members.length > 0) return members;
  return app.state.memberships.filter((membership) => membership.household_id === householdId);
}

export function HistoryRangePicker({
  value,
  onChange,
  label = "Range",
}: {
  value: HistoryRangeKey;
  onChange: (next: HistoryRangeKey) => void;
  label?: string;
}) {
  return (
    <fieldset className="range-picker">
      <legend className="visually-hidden">{label}</legend>
      {HISTORY_RANGE_KEYS.map((key) => (
        <button
          key={key}
          type="button"
          className="secondary"
          aria-pressed={key === value}
          onClick={() => onChange(key)}
        >
          {key}
        </button>
      ))}
    </fieldset>
  );
}

export function AccountForm({
  app,
  account,
  defaultCurrency,
  onDone,
}: {
  app: RationalApp;
  account: Account | null;
  defaultCurrency: string;
  onDone: () => void;
}) {
  const members = useHouseholdMembers(app);
  const [name, setName] = useState(account?.name ?? "");
  const [type, setType] = useState<AccountType>(account?.type ?? "checking");
  const [currency, setCurrency] = useState(account?.currency ?? defaultCurrency);
  const [openingBalance, setOpeningBalance] = useState(
    account === null ? "0.00" : amountToText(account.opening_balance, account.currency),
  );
  const [openingDate, setOpeningDate] = useState(account?.opening_date ?? todayIso());
  const [institution, setInstitution] = useState(account?.institution ?? "");
  const [ownerId, setOwnerId] = useState(account?.owner_id ?? "");
  const [hidden, setHidden] = useState(account?.hide_from_net_worth === true);
  const [error, setError] = useState<string | null>(null);
  const tracked = TRACKED_TYPES.includes(type);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setError(null);
    const writes = app.writes;
    if (writes === null) return;
    try {
      const upperCurrency = currency.trim().toUpperCase();
      const balance = parseAmount(openingBalance, upperCurrency);
      if (account === null) {
        await writes.createAccount({
          name,
          type,
          currency: upperCurrency,
          opening_balance: balance,
          opening_date: openingDate,
          institution,
          hide_from_net_worth: hidden,
          owner_id: ownerId,
        });
      } else {
        // Positions are valued into the balance only of an account that can
        // hold them; retyping a brokerage as a checking account would leave
        // its holdings counted by nothing, so that is refused here.
        if (
          (account.holdings?.length ?? 0) > 0 &&
          !HOLDING_TYPES.includes(type) &&
          type !== account.type
        ) {
          throw new ValidationError("remove the account's holdings before changing its type");
        }
        await writes.updateAccount(account.id, {
          name,
          type,
          currency: upperCurrency,
          opening_balance: balance,
          opening_date: openingDate,
          institution: institution.trim() === "" ? null : institution.trim(),
          hide_from_net_worth: hidden ? true : null,
          owner_id: ownerId === "" ? null : ownerId,
        });
      }
      onDone();
    } catch (caught) {
      setError(
        caught instanceof ValidationError || caught instanceof RangeError
          ? caught.message
          : "The account could not be saved.",
      );
    }
  };

  return (
    <form className="editor" onSubmit={(event) => void submit(event)} aria-label="Account editor">
      <h2>{account === null ? "New account" : `Edit ${account.name}`}</h2>
      <div className="grid">
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
          Type
          <select
            name="type"
            value={type}
            onChange={(event) => setType(event.target.value as AccountType)}
          >
            {ACCOUNT_TYPES.map((candidate) => (
              <option key={candidate} value={candidate}>
                {ACCOUNT_TYPE_LABELS[candidate]}
              </option>
            ))}
          </select>
        </label>
        <label>
          Currency
          <input
            name="currency"
            required
            maxLength={3}
            value={currency}
            onChange={(event) => setCurrency(event.target.value)}
          />
        </label>
        <label>
          {tracked ? "Opening balance (current value)" : "Opening balance"}
          <input
            name="opening_balance"
            inputMode="decimal"
            value={openingBalance}
            onChange={(event) => setOpeningBalance(event.target.value)}
          />
        </label>
        <label>
          Opening date
          <input
            name="opening_date"
            type="date"
            required
            value={openingDate}
            onChange={(event) => setOpeningDate(event.target.value)}
          />
        </label>
        <label>
          Institution
          <input
            name="institution"
            value={institution}
            onChange={(event) => setInstitution(event.target.value)}
          />
        </label>
        <label>
          Owner
          <select
            name="owner_id"
            value={ownerId}
            onChange={(event) => setOwnerId(event.target.value)}
          >
            <option value="">Shared</option>
            {members.map((membership) => (
              <option key={membership.user_id} value={membership.user_id}>
                {memberLabel(membership)}
              </option>
            ))}
          </select>
        </label>
      </div>
      <label className="chip-option">
        <input
          type="checkbox"
          name="hide_from_net_worth"
          checked={hidden}
          onChange={(event) => setHidden(event.target.checked)}
        />
        Hide from net worth
      </label>
      {tracked ? (
        <p className="hint">
          A tracked account&apos;s value is set here and updated in place from its page; the changes
          never count as income or spending.
        </p>
      ) : null}
      {error === null ? null : (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      <div className="actions">
        <button type="submit">Save account</button>
        <button type="button" className="secondary" onClick={onDone}>
          Cancel
        </button>
      </div>
    </form>
  );
}
