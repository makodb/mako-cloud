import {
  Alert,
  AlertDescription,
  Avatar,
  AvatarFallback,
  Badge,
  Button,
  Card,
  CardContent,
  Input,
  Label,
  NativeSelect,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  cn,
  initials,
} from "@mako-cloud/ui";
import { CircleAlert, CircleCheck, Search } from "lucide-react";
import { type FormEvent, type KeyboardEvent, useMemo, useState } from "react";

import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import type { HouseholdWrites } from "../data/writes.js";
import {
  type HouseholdCollectionId,
  isMerchant,
  type Merchant,
  type Transaction,
} from "../model/types.js";
import { type MerchantRow, merchantResolver, selectMerchantRows } from "../selectors/merchants.js";
import { formatMinorUnits } from "../selectors/money.js";
import { normalizeDescription } from "../selectors/transactions.js";
import { useQuery } from "./hooks.js";

/**
 * Merchants: the names a household gives to the statement lines that mean
 * the same payee.
 *
 * The list is every merchant the household has named and every name a
 * transaction resolves to without one -- a cleaned reading of the statement
 * text, which is what the transaction list shows meanwhile. Naming one of
 * those writes a merchant document with the descriptions that meant it as
 * its patterns and refiles the transactions, so the next charge with the same
 * text resolves to the name on arrival. Merging folds one merchant into
 * another the same way; the shared resolver decides what a transaction is
 * called, so the nightly job and this page never disagree.
 */
type RowKey = string;

function keyOf(row: MerchantRow): RowKey {
  return row.id === null ? `name:${row.name}` : `id:${row.id}`;
}

/** `Corner Grocer` → `corner-grocer`, for the row of a merchant that has no id yet. */
function slug(name: string): string {
  return name
    .toLowerCase()
    .replaceAll(/[^a-z0-9]+/gu, "-")
    .replaceAll(/^-+|-+$/gu, "");
}

function plural(count: number, noun: string): string {
  return `${count} ${count === 1 ? noun : `${noun}s`}`;
}

/** A row's small actions share one compact look. */
const ROW_ACTION = "h-7 px-2 text-xs";

export function MerchantsScreen({
  app,
  session,
  currency,
}: {
  app: RationalApp;
  session: ScopeSession<HouseholdCollectionId>;
  currency: string;
}) {
  const transactions = useQuery(session.collection("transactions")?.find() ?? null);
  const taxonomy = useQuery(session.collection("taxonomy")?.find() ?? null);
  const rows = selectMerchantRows(transactions, taxonomy, currency);
  const merchants = useMemo(
    () =>
      taxonomy
        .filter(isMerchant)
        .sort(
          (left, right) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id),
        ),
    [taxonomy],
  );
  const resolve = useMemo(() => merchantResolver(taxonomy), [taxonomy]);

  const [search, setSearch] = useState("");
  const [renaming, setRenaming] = useState<{ key: RowKey; name: string } | null>(null);
  const [merging, setMerging] = useState<{ key: RowKey; winnerId: string } | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);

  const needle = search.trim().toLowerCase();
  const shown =
    needle === "" ? rows : rows.filter((row) => row.name.toLowerCase().includes(needle));

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

  /**
   * The transactions a row stands for. A named merchant's are the ones filed
   * under it or matching its patterns; an unnamed row's are the ones the
   * resolver reads as that cleaned name, which is what the row was built from.
   */
  const transactionsOf = (row: MerchantRow): Transaction[] =>
    transactions.filter((transaction) => {
      const resolved = resolve(transaction);
      return row.id === null
        ? resolved.id === null && resolved.name === row.name
        : resolved.id === row.id;
    });

  /** The normalized statement lines behind an unnamed row, each once. */
  const patternsOf = (row: MerchantRow): string[] => [
    ...new Set(
      transactionsOf(row).map(
        (transaction) =>
          transaction.normalized_description ?? normalizeDescription(transaction.description),
      ),
    ),
  ];

  const saveName = async (row: MerchantRow) => {
    if (renaming === null || renaming.key !== keyOf(row)) return;
    const name = renaming.name.trim();
    const saved = await attempt(async (writes) => {
      if (row.id !== null) {
        await writes.updateMerchant(row.id, { name });
        return;
      }
      // Naming what was only a cleaned description: the document learns the
      // statement lines that meant it, and the transactions are filed under it.
      const ids = transactionsOf(row).map((transaction) => transaction.id);
      const merchant = await writes.createMerchant(name, patternsOf(row));
      const filed = await writes.bulkPatchTransactions(ids, { merchant_id: merchant.id });
      setStatus(`Named ${row.name} "${merchant.name}": ${plural(filed, "transaction")} filed.`);
    }, "The merchant could not be renamed.");
    if (saved) setRenaming(null);
  };

  const merge = async (event: FormEvent<HTMLFormElement>, row: MerchantRow) => {
    event.preventDefault();
    if (merging === null || merging.key !== keyOf(row)) return;
    const winner = merchants.find((merchant) => merchant.id === merging.winnerId);
    if (winner === undefined) {
      setError("Choose the merchant to merge into.");
      return;
    }
    const merged = await attempt(async (writes) => {
      if (row.id !== null) {
        const moved = await writes.mergeMerchants(row.id, winner.id);
        setStatus(
          `Merged ${row.name} into ${winner.name}: ${plural(moved, "transaction")} refiled.`,
        );
        return;
      }
      // An unnamed row has no document to fold in; the winner learns its
      // statement lines and takes its transactions, which is the same outcome.
      const ids = transactionsOf(row).map((transaction) => transaction.id);
      const patterns = [...new Set([...(winner.patterns ?? []), ...patternsOf(row)])];
      if (patterns.length !== (winner.patterns ?? []).length) {
        await writes.updateMerchant(winner.id, { patterns });
      }
      const moved = await writes.bulkPatchTransactions(ids, { merchant_id: winner.id });
      setStatus(`Merged ${row.name} into ${winner.name}: ${plural(moved, "transaction")} refiled.`);
    }, "The merchants could not be merged.");
    if (merged) setMerging(null);
  };

  const renameKeys = (row: MerchantRow) => (event: KeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Enter") {
      event.preventDefault();
      void saveName(row);
    }
    if (event.key === "Escape") setRenaming(null);
  };

  const mergeTargets = (row: MerchantRow): readonly Merchant[] =>
    merchants.filter((merchant) => merchant.id !== row.id);

  return (
    <section
      aria-labelledby="merchants-title"
      data-testid="merchants-screen"
      className="grid gap-6"
    >
      <div className="grid gap-1">
        <h1 id="merchants-title" className="text-2xl">
          Merchants
        </h1>
        <p className="text-sm text-muted-foreground">
          {plural(rows.length, "merchant")} · {plural(transactions.length, "transaction")}
        </p>
      </div>
      <div className="flex flex-wrap items-center gap-3">
        <Label htmlFor="merchant-search">Search</Label>
        <span className="relative w-72">
          <Search
            aria-hidden="true"
            className="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground"
          />
          <Input
            id="merchant-search"
            type="search"
            className="pl-8"
            aria-label="Search merchants"
            placeholder="Find a merchant"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
        </span>
        <span className="text-sm text-muted-foreground" data-testid="merchant-summary">
          {shown.length === rows.length ? "" : `${shown.length} of ${rows.length} shown`}
        </span>
      </div>
      {error === null ? null : (
        <Alert variant="destructive" role="alert">
          <CircleAlert />
          <AlertDescription>{error}</AlertDescription>
        </Alert>
      )}
      {status === null ? null : (
        <Alert variant="positive" role="status" data-testid="merchant-status">
          <CircleCheck />
          <AlertDescription>{status}</AlertDescription>
        </Alert>
      )}
      <Card className="py-2">
        <CardContent className="px-2">
          <Table aria-label="Merchants">
            <TableHeader>
              <TableRow className="hover:bg-transparent">
                <TableHead scope="col">Merchant</TableHead>
                <TableHead scope="col" className="text-right">
                  Transactions
                </TableHead>
                <TableHead scope="col" className="text-right">
                  Spending
                </TableHead>
                <TableHead scope="col">Last</TableHead>
                <TableHead scope="col">
                  <span className="sr-only">Actions</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {shown.length === 0 ? (
                <TableRow>
                  <TableCell colSpan={5} className="py-8 text-center text-muted-foreground">
                    {rows.length === 0 ? "No merchants yet." : "No merchant matches."}
                  </TableCell>
                </TableRow>
              ) : null}
              {shown.map((row) => {
                const key = keyOf(row);
                const targets = mergeTargets(row);
                const unnamed = row.id === null;
                return (
                  <TableRow
                    key={key}
                    data-testid={`merchant-${row.id ?? slug(row.name)}`}
                    data-name={row.name}
                    className={unnamed ? "text-muted-foreground" : undefined}
                  >
                    <TableHead
                      scope="row"
                      className={cn(
                        "h-auto py-2 font-medium whitespace-normal",
                        unnamed ? "text-muted-foreground" : "text-foreground",
                      )}
                    >
                      <div className="flex items-start gap-3">
                        <Avatar className="mt-0.5">
                          <AvatarFallback
                            className={unnamed ? "bg-muted text-muted-foreground" : undefined}
                          >
                            {initials(row.name)}
                          </AvatarFallback>
                        </Avatar>
                        <div className="grid min-w-0 gap-1.5">
                          <div className="flex flex-wrap items-center gap-2">
                            {renaming?.key === key ? (
                              <Input
                                aria-label={`Rename ${row.name}`}
                                className="h-8 w-56"
                                value={renaming.name}
                                onChange={(event) => setRenaming({ key, name: event.target.value })}
                                onKeyDown={renameKeys(row)}
                              />
                            ) : (
                              <span data-testid="name" className="py-1.5">
                                {row.name}
                              </span>
                            )}
                            {unnamed ? <Badge variant="outline">from statement text</Badge> : null}
                          </div>
                          {merging?.key === key ? (
                            <form
                              className="flex flex-wrap items-center gap-2"
                              aria-label={`Merge ${row.name}`}
                              onSubmit={(event) => void merge(event, row)}
                            >
                              <span className="w-56">
                                <NativeSelect
                                  size="sm"
                                  aria-label={`Merge ${row.name} into`}
                                  value={merging.winnerId}
                                  onChange={(event) =>
                                    setMerging({ key, winnerId: event.target.value })
                                  }
                                >
                                  <option value="">Choose a merchant</option>
                                  {targets.map((merchant) => (
                                    <option key={merchant.id} value={merchant.id}>
                                      {merchant.name}
                                    </option>
                                  ))}
                                </NativeSelect>
                              </span>
                              <Button type="submit" size="sm" disabled={merging.winnerId === ""}>
                                Merge
                              </Button>
                              <Button variant="outline" size="sm" onClick={() => setMerging(null)}>
                                Cancel
                              </Button>
                            </form>
                          ) : null}
                        </div>
                      </div>
                    </TableHead>
                    <TableCell className="money" data-testid="count">
                      {row.count}
                    </TableCell>
                    <TableCell className="money" data-testid="total">
                      {formatMinorUnits(row.total, currency)}
                    </TableCell>
                    <TableCell className="tabular-nums">
                      {row.lastDate === "" ? (
                        <span className="text-muted-foreground">—</span>
                      ) : (
                        row.lastDate
                      )}
                    </TableCell>
                    <TableCell className="whitespace-normal">
                      <div className="flex flex-wrap justify-end gap-1">
                        {renaming?.key === key ? (
                          <Button
                            variant="ghost"
                            size="sm"
                            className={ROW_ACTION}
                            onClick={() => void saveName(row)}
                          >
                            Save
                          </Button>
                        ) : (
                          <Button
                            variant="ghost"
                            size="sm"
                            className={ROW_ACTION}
                            onClick={() => setRenaming({ key, name: row.name })}
                          >
                            {unnamed ? "Give it a name" : "Rename"}
                          </Button>
                        )}
                        <Button
                          variant="ghost"
                          size="sm"
                          className={ROW_ACTION}
                          disabled={targets.length === 0}
                          onClick={() =>
                            setMerging(merging?.key === key ? null : { key, winnerId: "" })
                          }
                        >
                          Merge into…
                        </Button>
                      </div>
                    </TableCell>
                  </TableRow>
                );
              })}
            </TableBody>
          </Table>
        </CardContent>
      </Card>
    </section>
  );
}
