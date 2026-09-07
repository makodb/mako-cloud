import {
  Alert,
  AlertDescription,
  Badge,
  Button,
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  Field,
  Input,
  NativeSelect,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@mako-cloud/ui";
import { Check, CircleAlert, FileUp } from "lucide-react";
import { type ChangeEvent, useMemo, useState } from "react";

import type { RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import type { HouseholdCollectionId } from "../model/types.js";
import {
  type ColumnMapping,
  type CsvTable,
  parseCsv,
  parseRows,
  planImport,
  proposeMapping,
} from "../selectors/csv.js";
import { formatMinorUnits } from "../selectors/money.js";
import { applyRules } from "../selectors/rules.js";
import { useQuery } from "./hooks.js";
import { transactionsHash } from "./router.js";

/**
 * Importing a bank's CSV export.
 *
 * Nothing is written until the person has seen what would be: the mapping the
 * file suggests, the first rows under it, which rows are already here, and
 * which could not be read. Duplicate detection is against what this account
 * already holds, by date, amount, and normalized description.
 *
 * What is imported arrives needing review -- nobody in the household has
 * looked at these rows, only at the file -- unless a rule marked it reviewed
 * as it came in. The outcome says how many are waiting and where.
 */

/** What an import did, kept as numbers so the outcome can carry a link. */
interface Outcome {
  readonly created: number;
  readonly rowCount: number;
  readonly duplicates: number;
  readonly waiting: number;
}

/** The columns a mapping names, in the order the file usually has them. */
const MAPPED_FIELDS = ["date", "description", "amount", "debit", "credit"] as const;

export function ImportScreen({
  app,
  session,
  currency,
}: {
  app: RationalApp;
  session: ScopeSession<HouseholdCollectionId>;
  currency: string;
}) {
  const accounts = useQuery(
    session.collection("accounts")?.find({ sort: [{ name: "asc" }] }) ?? null,
  );
  const transactions = useQuery(session.collection("transactions")?.find() ?? null);
  const rules = useQuery(session.collection("rules")?.find() ?? null);
  const [filename, setFilename] = useState<string | null>(null);
  const [table, setTable] = useState<CsvTable | null>(null);
  const [mapping, setMapping] = useState<ColumnMapping | null>(null);
  const [accountId, setAccountId] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const [outcome, setOutcome] = useState<Outcome | null>(null);
  const [busy, setBusy] = useState(false);

  const plan = useMemo(() => {
    if (table === null || mapping === null || accountId === "") return null;
    return planImport(parseRows(table, mapping, currency), transactions, accountId);
  }, [table, mapping, accountId, currency, transactions]);

  const choose = async (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    event.target.value = "";
    if (file === undefined) return;
    setOutcome(null);
    const parsed = parseCsv(await file.text());
    const proposed = proposeMapping(parsed);
    setFilename(file.name);
    setTable(parsed);
    setMapping(proposed);
    setProblem(
      proposed === null
        ? "No date, description, and amount columns were recognized. Choose them below."
        : null,
    );
  };

  const runImport = async () => {
    if (plan === null || app.writes === null || filename === null) return;
    setBusy(true);
    try {
      // The rules run as the rows arrive, with every action a rule can take:
      // a category, a merchant, tags, hiding, and marking reviewed -- the
      // same engine the nightly job applies to what arrives overnight.
      const rows = plan.importable.map((row) => {
        const matched = applyRules(rules, {
          description: row.description,
          amount: row.amount,
          account_id: accountId,
        });
        return {
          date: row.date,
          description: row.description,
          amount: row.amount,
          ...(matched?.categoryId === undefined ? {} : { categoryId: matched.categoryId }),
          ...(matched?.merchantId === undefined ? {} : { merchantId: matched.merchantId }),
          ...(matched === null ? {} : { ruleId: matched.rule.id }),
          ...(matched === null || matched.tags.length === 0 ? {} : { tags: matched.tags }),
          ...(matched?.hide === true ? { hidden: true } : {}),
          ...(matched?.markReviewed === true ? { reviewed: true } : {}),
        };
      });
      const result = await app.writes.importTransactions({
        accountId,
        currency,
        filename,
        rowCount: plan.rows.length,
        duplicateCount: plan.duplicates.size,
        rows,
      });
      setOutcome({
        created: result.created,
        rowCount: result.rowCount,
        duplicates: result.duplicates,
        waiting: rows.filter((row) => row.reviewed !== true).length,
      });
      setTable(null);
      setMapping(null);
      setFilename(null);
    } catch (error) {
      setProblem(error instanceof Error ? error.message : "the import failed");
    } finally {
      setBusy(false);
    }
  };

  const setColumn = (field: keyof ColumnMapping, value: string) => {
    if (mapping === null) return;
    setMapping({ ...mapping, [field]: value === "" ? undefined : value } as ColumnMapping);
  };

  return (
    <section aria-labelledby="import-title" data-testid="import-screen" className="grid gap-6">
      <div className="grid gap-1">
        <h1 id="import-title" className="text-2xl">
          Import
        </h1>
        <p className="text-sm text-muted-foreground">
          A bank's CSV export, previewed before a single row is written.
        </p>
      </div>
      {problem === null ? null : (
        <Alert variant="destructive">
          <CircleAlert />
          <AlertDescription className="block">{problem}</AlertDescription>
        </Alert>
      )}
      {outcome === null ? null : (
        <Alert variant="positive" role="status" data-testid="import-outcome">
          <Check />
          <AlertDescription className="block">
            Imported {outcome.created} of {outcome.rowCount} rows; {outcome.duplicates} were already
            here.
            {outcome.waiting === 0 ? null : (
              <>
                {" "}
                {outcome.waiting} {outcome.waiting === 1 ? "is" : "are"} waiting in{" "}
                <a href={transactionsHash({ review: "needs" })}>Needs review</a>.
              </>
            )}
          </AlertDescription>
        </Alert>
      )}

      <Card>
        <CardHeader>
          <CardDescription className="max-w-prose">
            Nothing is written until you have seen what would be. Rows already in the account are
            recognized by date, amount, and description and left alone; what is imported waits in
            Needs review until somebody looks at it.
          </CardDescription>
        </CardHeader>
        <CardContent>
          <div className="grid gap-4 sm:grid-cols-2">
            <Field label="Account" htmlFor="import-account">
              <NativeSelect
                id="import-account"
                value={accountId}
                onChange={(event) => setAccountId(event.target.value)}
              >
                <option value="">Choose an account</option>
                {accounts.map((account) => (
                  <option key={account.id} value={account.id}>
                    {account.name}
                  </option>
                ))}
              </NativeSelect>
            </Field>
            <Field label="CSV file" htmlFor="import-file">
              <Input
                id="import-file"
                type="file"
                accept=".csv,text/csv"
                onChange={(event) => void choose(event)}
              />
            </Field>
          </div>
        </CardContent>
      </Card>

      {table === null || mapping === null ? null : (
        <Card>
          <CardHeader>
            <h3 className="text-base font-semibold leading-none">Columns</h3>
          </CardHeader>
          <CardContent>
            <div className="grid gap-4 sm:grid-cols-3" data-testid="mapping">
              {MAPPED_FIELDS.map((field) => (
                <Field key={field} label={field} htmlFor={`mapping-${field}`}>
                  <NativeSelect
                    id={`mapping-${field}`}
                    value={mapping[field] ?? ""}
                    onChange={(event) => setColumn(field, event.target.value)}
                  >
                    <option value="">—</option>
                    {table.headers.map((header) => (
                      <option key={header} value={header}>
                        {header}
                      </option>
                    ))}
                  </NativeSelect>
                </Field>
              ))}
              <Field label="date order" htmlFor="mapping-date-order">
                <NativeSelect
                  id="mapping-date-order"
                  value={mapping.dateOrder}
                  onChange={(event) =>
                    setMapping({
                      ...mapping,
                      dateOrder: event.target.value as ColumnMapping["dateOrder"],
                    })
                  }
                >
                  <option value="ISO">YYYY-MM-DD</option>
                  <option value="MDY">MM/DD/YYYY</option>
                  <option value="DMY">DD/MM/YYYY</option>
                </NativeSelect>
              </Field>
            </div>
          </CardContent>
        </Card>
      )}

      {plan === null ? null : (
        <Card>
          <CardHeader>
            <h3 className="text-base font-semibold leading-none">Preview</h3>
            <CardDescription data-testid="import-summary">
              {plan.importable.length} to import, {plan.duplicates.size} already here,{" "}
              {plan.unreadable.length} unreadable.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <Table aria-label="Preview">
              <TableHeader>
                <TableRow>
                  <TableHead scope="col">Line</TableHead>
                  <TableHead scope="col">Date</TableHead>
                  <TableHead scope="col">Description</TableHead>
                  <TableHead scope="col" className="text-right">
                    Amount
                  </TableHead>
                  <TableHead scope="col">Outcome</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {plan.rows.slice(0, 20).map((row) => {
                  const duplicate = plan.duplicates.has(row.line);
                  return (
                    <TableRow key={row.line} data-testid={`preview-${row.line}`}>
                      <TableCell className="tabular-nums text-muted-foreground">
                        {row.line}
                      </TableCell>
                      <TableCell className="tabular-nums">{row.date}</TableCell>
                      <TableCell className="whitespace-normal">{row.description}</TableCell>
                      <TableCell className="money">
                        {row.problem === undefined ? formatMinorUnits(row.amount, currency) : "—"}
                      </TableCell>
                      <TableCell data-testid="outcome">
                        <Badge
                          variant={
                            row.problem !== undefined
                              ? "destructive"
                              : duplicate
                                ? "secondary"
                                : "positive"
                          }
                        >
                          {row.problem ?? (duplicate ? "already here" : "import")}
                        </Badge>
                      </TableCell>
                    </TableRow>
                  );
                })}
              </TableBody>
            </Table>
          </CardContent>
          <CardFooter>
            <Button
              disabled={busy || plan.importable.length === 0}
              onClick={() => void runImport()}
            >
              <FileUp />
              {busy ? "Importing…" : `Import ${plan.importable.length} transactions`}
            </Button>
          </CardFooter>
        </Card>
      )}
    </section>
  );
}
