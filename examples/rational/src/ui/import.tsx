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
import "./styles/settings-pages.css";

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
    <section aria-labelledby="import-title" data-testid="import-screen">
      <div className="heading">
        <h1 id="import-title">Import</h1>
      </div>
      <p className="hint">
        Nothing is written until you have seen what would be. Rows already in the account are
        recognized by date, amount, and description and left alone; what is imported waits in Needs
        review until somebody looks at it.
      </p>
      {problem === null ? null : (
        <p className="notice error" role="alert">
          {problem}
        </p>
      )}
      {outcome === null ? null : (
        <p className="notice success" role="status" data-testid="import-outcome">
          Imported {outcome.created} of {outcome.rowCount} rows; {outcome.duplicates} were already
          here.
          {outcome.waiting === 0 ? null : (
            <>
              {" "}
              {outcome.waiting} {outcome.waiting === 1 ? "is" : "are"} waiting in{" "}
              <a href={transactionsHash({ review: "needs" })}>Needs review</a>.
            </>
          )}
        </p>
      )}

      <div className="editor">
        <div className="grid">
          <label>
            Account
            <select value={accountId} onChange={(event) => setAccountId(event.target.value)}>
              <option value="">Choose an account</option>
              {accounts.map((account) => (
                <option key={account.id} value={account.id}>
                  {account.name}
                </option>
              ))}
            </select>
          </label>
          <label>
            CSV file
            <input type="file" accept=".csv,text/csv" onChange={(event) => void choose(event)} />
          </label>
        </div>
      </div>

      {table === null || mapping === null ? null : (
        <>
          <h3>Columns</h3>
          <div className="mapping" data-testid="mapping">
            {(["date", "description", "amount", "debit", "credit"] as const).map((field) => (
              <label key={field}>
                {field}
                <select
                  value={mapping[field] ?? ""}
                  onChange={(event) => setColumn(field, event.target.value)}
                >
                  <option value="">—</option>
                  {table.headers.map((header) => (
                    <option key={header} value={header}>
                      {header}
                    </option>
                  ))}
                </select>
              </label>
            ))}
            <label>
              date order
              <select
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
              </select>
            </label>
          </div>
        </>
      )}

      {plan === null ? null : (
        <>
          <h3>Preview</h3>
          <p data-testid="import-summary">
            {plan.importable.length} to import, {plan.duplicates.size} already here,{" "}
            {plan.unreadable.length} unreadable.
          </p>
          <table className="data-table" aria-label="Preview">
            <thead>
              <tr>
                <th scope="col">Line</th>
                <th scope="col">Date</th>
                <th scope="col">Description</th>
                <th scope="col" className="amount">
                  Amount
                </th>
                <th scope="col">Outcome</th>
              </tr>
            </thead>
            <tbody>
              {plan.rows.slice(0, 20).map((row) => (
                <tr key={row.line} data-testid={`preview-${row.line}`}>
                  <td>{row.line}</td>
                  <td>{row.date}</td>
                  <td>{row.description}</td>
                  <td className="amount">
                    {row.problem === undefined ? formatMinorUnits(row.amount, currency) : "—"}
                  </td>
                  <td data-testid="outcome">
                    {row.problem ?? (plan.duplicates.has(row.line) ? "already here" : "import")}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          <button
            type="button"
            disabled={busy || plan.importable.length === 0}
            onClick={() => void runImport()}
          >
            {busy ? "Importing…" : `Import ${plan.importable.length} transactions`}
          </button>
        </>
      )}
    </section>
  );
}
