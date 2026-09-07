import {
  Button,
  Card,
  CardContent,
  Checkbox,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Field,
  Input,
  Label,
  NativeSelect,
  cn,
} from "@mako-cloud/ui";
import { type ComponentProps, type FormEvent, type ReactNode, useId, useState } from "react";

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
import {
  amountToText,
  formatMinorUnits,
  minorUnitDigits,
  parseAmount,
} from "../selectors/money.js";
import { useQuery } from "./hooks.js";

/**
 * What the account screens share: the editor an account is created and
 * changed with, the range picker every balance chart wears, the dialog every
 * editor opens in, the tiles a total is shown on, and how a member is named.
 * The accounts list and an account's own page both open the same editor, so
 * a field added here is added everywhere an account is edited.
 */

export function todayIso(): string {
  return new Date().toISOString().slice(0, 10);
}

/** The name a member is shown by: their address, or their id before one is known. */
export function memberLabel(membership: Membership): string {
  return membership.email ?? membership.user_id;
}

const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/** "2026-08-28" as "Aug 28": what fits under a chart's axis. */
export function shortDate(value: string | number): string {
  const text = String(value);
  const month = Number(text.slice(5, 7));
  const day = Number(text.slice(8, 10));
  if (!Number.isInteger(month) || month < 1 || month > 12 || !Number.isInteger(day) || day < 1) {
    return text;
  }
  return `${MONTHS[month - 1]} ${day}`;
}

/**
 * Minor units as a chart reads them: to the cent below a thousand, and
 * "$262.5K" above it, since an axis has room for neither eleven characters
 * nor a wrong number.
 */
export function compactMoney(amount: number, currency: string): string {
  const major = amount / 10 ** minorUnitDigits(currency);
  if (Math.abs(major) < 1000) return formatMinorUnits(amount, currency);
  try {
    return new Intl.NumberFormat("en-US", {
      style: "currency",
      currency,
      notation: "compact",
      maximumFractionDigits: 1,
    }).format(major);
  } catch {
    return formatMinorUnits(amount, currency);
  }
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

/** A screen's title, what it is about under it, and what can be done from it beside. */
export function PageHeader({
  id,
  title,
  subtitle,
  actions,
}: {
  id: string;
  title: string;
  subtitle?: string;
  actions?: ReactNode;
}) {
  return (
    <div className="flex flex-wrap items-start justify-between gap-4">
      <div className="grid gap-1">
        <h1 id={id} className="m-0 text-2xl font-semibold tracking-tight">
          {title}
        </h1>
        {subtitle === undefined ? null : (
          <p className="m-0 text-sm text-muted-foreground">{subtitle}</p>
        )}
      </div>
      {actions === undefined ? null : (
        <div className="flex flex-wrap items-center justify-end gap-2">{actions}</div>
      )}
    </div>
  );
}

/** One figure on a tile: what it is, the number, and a word about it. */
export function Stat({
  label,
  value,
  valueTestId,
  note,
  className,
  ...rest
}: {
  label: string;
  value: string;
  valueTestId?: string;
  note?: ReactNode;
} & Omit<ComponentProps<typeof Card>, "children">) {
  return (
    <Card className={cn("gap-0 py-4", className)} {...rest}>
      <CardContent className="grid gap-1 px-4">
        <span className="text-xs font-semibold tracking-wider text-muted-foreground uppercase">
          {label}
        </span>
        <strong
          data-testid={valueTestId}
          className="text-2xl font-semibold tracking-tight tabular-nums"
        >
          {value}
        </strong>
        {note === undefined ? null : (
          <small className="text-xs text-muted-foreground">{note}</small>
        )}
      </CardContent>
    </Card>
  );
}

/**
 * The modal every editor opens in: a titled form with Cancel and the save
 * beside each other at the foot. Escape, the close button, and Cancel all
 * call `onDone`; the form's own submit is the only way to save.
 */
export function EditorDialog({
  title,
  description,
  formLabel,
  formTestId,
  submitLabel,
  onDone,
  onSubmit,
  children,
}: {
  title: string;
  description?: string;
  formLabel: string;
  formTestId?: string;
  submitLabel: string;
  onDone: () => void;
  onSubmit: (event: FormEvent) => void;
  children: ReactNode;
}) {
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) onDone();
      }}
    >
      <DialogContent
        className="sm:max-w-xl"
        {...(description === undefined ? { "aria-describedby": undefined } : {})}
      >
        <form
          aria-label={formLabel}
          data-testid={formTestId}
          className="grid gap-5"
          onSubmit={onSubmit}
        >
          <DialogHeader>
            <DialogTitle>{title}</DialogTitle>
            {description === undefined ? null : (
              <DialogDescription>{description}</DialogDescription>
            )}
          </DialogHeader>
          {children}
          <DialogFooter>
            <Button type="button" variant="outline" onClick={onDone}>
              Cancel
            </Button>
            <Button type="submit">{submitLabel}</Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
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
    <fieldset className="m-0 inline-flex min-w-0 items-center gap-0.5 rounded-lg border-0 bg-muted p-[3px]">
      <legend className="sr-only">{label}</legend>
      {HISTORY_RANGE_KEYS.map((key) => (
        <Button
          key={key}
          type="button"
          variant="ghost"
          size="sm"
          aria-pressed={key === value}
          className={cn(
            "h-7 px-2.5 text-xs text-muted-foreground tabular-nums hover:bg-transparent hover:text-foreground",
            key === value && "bg-card text-foreground shadow-sm hover:bg-card",
          )}
          onClick={() => onChange(key)}
        >
          {key}
        </Button>
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
  const id = useId();
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
    <EditorDialog
      title={account === null ? "New account" : `Edit ${account.name}`}
      formLabel="Account editor"
      submitLabel="Save account"
      onDone={onDone}
      onSubmit={(event) => void submit(event)}
    >
      <div className="grid gap-4 sm:grid-cols-2">
        <Field label="Name" htmlFor={`${id}-name`}>
          <Input
            id={`${id}-name`}
            name="name"
            required
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </Field>
        <Field label="Type" htmlFor={`${id}-type`}>
          <NativeSelect
            id={`${id}-type`}
            name="type"
            value={type}
            onChange={(event) => setType(event.target.value as AccountType)}
          >
            {ACCOUNT_TYPES.map((candidate) => (
              <option key={candidate} value={candidate}>
                {ACCOUNT_TYPE_LABELS[candidate]}
              </option>
            ))}
          </NativeSelect>
        </Field>
        <Field label="Currency" htmlFor={`${id}-currency`}>
          <Input
            id={`${id}-currency`}
            name="currency"
            required
            maxLength={3}
            value={currency}
            onChange={(event) => setCurrency(event.target.value)}
          />
        </Field>
        <Field
          label={tracked ? "Opening balance (current value)" : "Opening balance"}
          htmlFor={`${id}-opening-balance`}
        >
          <Input
            id={`${id}-opening-balance`}
            name="opening_balance"
            inputMode="decimal"
            className="tabular-nums"
            value={openingBalance}
            onChange={(event) => setOpeningBalance(event.target.value)}
          />
        </Field>
        <Field label="Opening date" htmlFor={`${id}-opening-date`}>
          <Input
            id={`${id}-opening-date`}
            name="opening_date"
            type="date"
            required
            value={openingDate}
            onChange={(event) => setOpeningDate(event.target.value)}
          />
        </Field>
        <Field label="Institution" htmlFor={`${id}-institution`}>
          <Input
            id={`${id}-institution`}
            name="institution"
            value={institution}
            onChange={(event) => setInstitution(event.target.value)}
          />
        </Field>
        <Field label="Owner" htmlFor={`${id}-owner`}>
          <NativeSelect
            id={`${id}-owner`}
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
          </NativeSelect>
        </Field>
      </div>
      <div className="flex items-center gap-2">
        <Checkbox
          id={`${id}-hidden`}
          name="hide_from_net_worth"
          checked={hidden}
          onCheckedChange={(checked) => setHidden(checked === true)}
        />
        <Label htmlFor={`${id}-hidden`}>Hide from net worth</Label>
      </div>
      {tracked ? (
        <p className="m-0 text-sm text-muted-foreground">
          A tracked account&apos;s value is set here and updated in place from its page; the changes
          never count as income or spending.
        </p>
      ) : null}
      {error === null ? null : (
        <p className="m-0 text-sm text-destructive" role="alert">
          {error}
        </p>
      )}
    </EditorDialog>
  );
}
