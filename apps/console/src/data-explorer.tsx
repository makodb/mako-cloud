import { type FormEvent, useCallback, useEffect, useRef, useState } from "react";

import type {
  ApplicationUserSummary,
  Collection,
  DataJob,
  ExplorerDocument,
  ExplorerDocumentPage,
  ExplorerGrant,
  ExplorerMutationRequest,
  ExplorerQueryPlan,
  ExplorerQueryRequest,
  ExplorerRevision,
  ExplorerSimulation,
} from "@mako-cloud/management-sdk";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useDeveloperAuth } from "./auth.js";
import { useManagementClient } from "./management.js";

const POLICY_OPERATIONS = ["get", "browse", "query", "plan", "simulate"] as const;
const ADMIN_OPERATIONS = [
  "get",
  "browse",
  "query",
  "plan",
  "history",
  "simulate",
  "mutate",
] as const;

type PageSource =
  | { readonly kind: "browse"; readonly includeRetainedTombstones: boolean }
  | { readonly kind: "query"; readonly request: ExplorerQueryRequest };

interface ConflictView {
  readonly original: ExplorerDocument | null;
  readonly proposed: ExplorerMutationRequest;
  readonly current: ExplorerDocument | null;
}

export function DataExplorer({
  projectId,
  environmentId,
  onCreateIndex,
  adminEnabled,
  jobsEnabled,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly onCreateIndex: (collectionId: string, requiredIndex: string) => void;
  readonly adminEnabled: boolean;
  readonly jobsEnabled: boolean;
}) {
  const client = useManagementClient();
  const [collections, setCollections] = useState<Collection[] | null>(null);
  const [users, setUsers] = useState<ApplicationUserSummary[]>([]);
  const [collectionId, setCollectionId] = useState("");
  const [grant, setGrant] = useState<ExplorerGrant | null>(null);
  const grantRef = useRef<ExplorerGrant | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [page, setPage] = useState<ExplorerDocumentPage | null>(null);
  const [pageSource, setPageSource] = useState<PageSource | null>(null);
  const [selected, setSelected] = useState<ExplorerDocument | null>(null);
  const [history, setHistory] = useState<ExplorerRevision[] | null>(null);
  const [plan, setPlan] = useState<ExplorerQueryPlan | null>(null);
  const [simulation, setSimulation] = useState<ExplorerSimulation | null>(null);
  const [conflict, setConflict] = useState<ConflictView | null>(null);
  const [auditReference, setAuditReference] = useState<string | null>(null);

  const clearExplorerState = useCallback(() => {
    setPage(null);
    setPageSource(null);
    setSelected(null);
    setHistory(null);
    setPlan(null);
    setSimulation(null);
    setConflict(null);
    setAuditReference(null);
    setFailure(null);
  }, []);

  const revoke = useCallback(async () => {
    const current = grantRef.current;
    grantRef.current = null;
    setGrant(null);
    clearExplorerState();
    if (current !== null) {
      try {
        await client.revokeExplorerGrant(projectId, environmentId, current.grantId);
      } catch {
        // The local credential is already gone. Expiry and server-side revocation remain authoritative.
      }
    }
  }, [clearExplorerState, client, environmentId, projectId]);

  useEffect(() => {
    let active = true;
    void Promise.all([
      client.listCollections(projectId, environmentId),
      client.searchApplicationUsers(projectId, environmentId, { limit: 100 }),
    ]).then(
      ([nextCollections, userResult]) => {
        if (!active) return;
        setCollections(nextCollections);
        setUsers(userResult.users.filter((user) => user.status === "active"));
        setCollectionId((current) =>
          nextCollections.some((collection) => collection.id === current)
            ? current
            : (nextCollections[0]?.id ?? ""),
        );
      },
      (error: unknown) => active && setFailure(toConsoleApiFailure(error)),
    );
    return () => {
      active = false;
      const current = grantRef.current;
      grantRef.current = null;
      if (current !== null) {
        void client.revokeExplorerGrant(projectId, environmentId, current.grantId).catch(() => {});
      }
    };
  }, [client, environmentId, projectId]);

  const switchCollection = (nextCollectionId: string) => {
    if (nextCollectionId === collectionId) return;
    void revoke();
    setCollectionId(nextCollectionId);
  };

  const issueGrant = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    const mode = requiredText(data, "mode");
    try {
      if (collectionId === "") throw new Error("Select a collection first.");
      if (mode === "administrative" && !adminEnabled) {
        throw new Error("Administrative data access is disabled during staged rollout.");
      }
      if (
        mode === "administrative" &&
        !form.querySelector<HTMLInputElement>("[name=confirm]")?.checked
      ) {
        throw new Error("Confirm that administrative access bypasses document policies.");
      }
      await revoke();
      const next = await client.issueExplorerGrant(projectId, environmentId, {
        tenant: { projectId, environmentId },
        collectionId,
        mode: mode === "administrative" ? "administrative" : "policy_preview",
        operations: mode === "administrative" ? [...ADMIN_OPERATIONS] : [...POLICY_OPERATIONS],
        applicationUserId:
          mode === "administrative" ? null : requiredText(data, "applicationUserId"),
        reason: mode === "administrative" ? requiredText(data, "reason") : null,
        durationSeconds: 300,
      });
      grantRef.current = next;
      setGrant(next);
      setFailure(null);
      form.reset();
    } catch (error) {
      setFailure(consoleFailure(error));
    }
  };

  const requireCapability = () => {
    const current = grantRef.current;
    if (current === null || current.expiresAtUnixSeconds <= Math.floor(Date.now() / 1000)) {
      grantRef.current = null;
      setGrant(null);
      throw new Error("Explorer access expired. Create a new scoped grant.");
    }
    return current.capability;
  };

  const browse = async (cursor: string | null = null, includeRetainedTombstones = false) => {
    try {
      const result = await client.explorerBrowseDocuments(
        projectId,
        environmentId,
        collectionId,
        { limit: 25, cursor, includeRetainedTombstones },
        requireCapability(),
      );
      setPage(result);
      setPageSource({ kind: "browse", includeRetainedTombstones });
      setFailure(null);
    } catch (error) {
      setFailure(consoleFailure(error));
    }
  };

  const lookup = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    try {
      const document = await client.explorerGetDocument(
        projectId,
        environmentId,
        collectionId,
        requiredText(new FormData(event.currentTarget), "documentId"),
        requireCapability(),
      );
      setSelected(document);
      setHistory(null);
      setFailure(null);
    } catch (error) {
      setFailure(consoleFailure(error));
    }
  };

  const executeQuery = async (form: HTMLFormElement, cursor: string | null = null) => {
    try {
      const request = queryFromForm(new FormData(form), cursor);
      const capability = requireCapability();
      const nextPlan = await client.explorerPlanQuery(
        projectId,
        environmentId,
        collectionId,
        request,
        capability,
      );
      setPlan(nextPlan);
      if (!nextPlan.supported) {
        setPage(null);
        setPageSource(null);
        return;
      }
      const result = await client.explorerQueryDocuments(
        projectId,
        environmentId,
        collectionId,
        request,
        capability,
      );
      setPage(result);
      setPageSource({ kind: "query", request: { ...request, cursor: null } });
      setFailure(null);
    } catch (error) {
      setFailure(consoleFailure(error));
    }
  };

  const nextPage = async () => {
    if (page?.nextCursor === null || page?.nextCursor === undefined || pageSource === null) return;
    if (pageSource.kind === "browse") {
      await browse(page.nextCursor, pageSource.includeRetainedTombstones);
      return;
    }
    try {
      const result = await client.explorerQueryDocuments(
        projectId,
        environmentId,
        collectionId,
        { ...pageSource.request, cursor: page.nextCursor },
        requireCapability(),
      );
      setPage(result);
      setFailure(null);
    } catch (error) {
      setFailure(consoleFailure(error));
    }
  };

  const loadHistory = async (document: ExplorerDocument) => {
    try {
      setHistory(
        await client.explorerDocumentHistory(
          projectId,
          environmentId,
          collectionId,
          document.documentId,
          requireCapability(),
        ),
      );
      setSelected(document);
      setFailure(null);
    } catch (error) {
      setFailure(consoleFailure(error));
    }
  };

  const mutate = async (event: FormEvent<HTMLFormElement>, simulateOnly: boolean) => {
    event.preventDefault();
    try {
      const data = new FormData(event.currentTarget);
      const kind = requiredText(data, "kind") as ExplorerMutationRequest["kind"];
      const proposed: ExplorerMutationRequest = {
        kind,
        documentId: requiredText(data, "documentId"),
        expectedRevision: optionalText(data, "expectedRevision"),
        schemaVersion: positiveInteger(data, "schemaVersion"),
        idempotencyKey: crypto.randomUUID(),
        content: kind === "delete" ? null : parseJsonObject(requiredText(data, "content")),
      };
      if (simulateOnly) {
        setSimulation(
          await client.explorerSimulateMutation(
            projectId,
            environmentId,
            collectionId,
            proposed,
            requireCapability(),
          ),
        );
        setConflict(null);
        return;
      }
      if (grantRef.current?.mode !== "administrative") {
        throw new Error("Policy preview can simulate writes but cannot commit them.");
      }
      const result = await client.explorerMutateDocument(
        projectId,
        environmentId,
        collectionId,
        proposed,
        requireCapability(),
      );
      setAuditReference(result.auditReference);
      setSimulation(null);
      if (result.conflict !== null) {
        setConflict({ original: selected, proposed, current: result.conflict });
      } else {
        setConflict(null);
        setSelected(result.document);
      }
      setFailure(null);
    } catch (error) {
      setFailure(consoleFailure(error));
    }
  };

  const currentCollection = collections?.find((collection) => collection.id === collectionId);
  return (
    <div className="explorer-stack">
      <header className="section-heading">
        <div>
          <p className="eyebrow">Data explorer</p>
          <h2>Documents</h2>
          <p>Browse through a short-lived, collection-scoped access grant.</p>
        </div>
        <label>
          Collection
          <select
            value={collectionId}
            onChange={(event) => switchCollection(event.currentTarget.value)}
          >
            {collections?.map((collection) => (
              <option key={collection.id} value={collection.id}>
                {collection.id}
              </option>
            ))}
          </select>
        </label>
      </header>
      <ApiFailureNotice failure={failure} />
      {collections === null ? (
        <p>Loading collections…</p>
      ) : collections.length === 0 ? (
        <p className="notice">
          Create and activate a collection schema before opening the explorer.
        </p>
      ) : null}
      {grant === null && collectionId !== "" ? (
        <GrantForm users={users} adminEnabled={adminEnabled} onSubmit={issueGrant} />
      ) : grant !== null ? (
        <GrantBanner grant={grant} users={users} onRevoke={() => void revoke()} />
      ) : null}
      {grant !== null ? (
        <>
          <div className="explorer-tools">
            <section className="panel">
              <h3>Canonical browse</h3>
              <p>Snapshot-consistent primary-key order; hidden policy rows are not counted.</p>
              <div className="button-row">
                <button type="button" onClick={() => void browse(null, false)}>
                  Browse documents
                </button>
                {grant.mode === "administrative" ? (
                  <button
                    type="button"
                    className="secondary"
                    onClick={() => void browse(null, true)}
                  >
                    Include retained tombstones
                  </button>
                ) : null}
              </div>
            </section>
            <section className="panel">
              <h3>Exact primary key</h3>
              <form className="inline-form" onSubmit={(event) => void lookup(event)}>
                <label>
                  Document ID
                  <input name="documentId" required maxLength={512} />
                </label>
                <button type="submit">Get</button>
              </form>
            </section>
          </div>
          <QueryBuilder
            onSubmit={(form) => void executeQuery(form)}
            plan={plan}
            onCreateIndex={() =>
              plan?.requiredIndex !== null &&
              onCreateIndex(collectionId, JSON.stringify(plan?.requiredIndex ?? {}))
            }
          />
          {page !== null ? (
            <DocumentResults
              page={page}
              canViewHistory={grant.operations.includes("history")}
              onSelect={setSelected}
              onHistory={(document) => void loadHistory(document)}
              onNext={() => void nextPage()}
            />
          ) : null}
          {selected !== null ? <DocumentDetail document={selected} history={history} /> : null}
          <MutationEditor
            collection={currentCollection ?? null}
            selected={selected}
            simulation={simulation}
            canCommit={grant.mode === "administrative"}
            onSubmit={mutate}
          />
          {conflict !== null ? (
            <ConflictComparison
              conflict={conflict}
              onReload={() => {
                setSelected(conflict.current);
                setConflict(null);
              }}
              onCancel={() => setConflict(null)}
              onPrepare={() => {
                setSelected(conflict.current);
                setConflict(null);
              }}
            />
          ) : null}
          {auditReference !== null ? (
            <p className="notice">
              Audit reference <code>{auditReference}</code>
            </p>
          ) : null}
          {jobsEnabled ? (
            <DataJobs
              projectId={projectId}
              environmentId={environmentId}
              collectionId={collectionId}
            />
          ) : null}
        </>
      ) : null}
    </div>
  );
}

function GrantForm({
  users,
  onSubmit,
  adminEnabled,
}: {
  readonly users: ApplicationUserSummary[];
  readonly onSubmit: (event: FormEvent<HTMLFormElement>) => void;
  readonly adminEnabled: boolean;
}) {
  const [mode, setMode] = useState("policy_preview");
  return (
    <section className="panel" aria-labelledby="grant-title">
      <h3 id="grant-title">Create scoped access</h3>
      <form onSubmit={onSubmit}>
        <label>
          Mode
          <select name="mode" value={mode} onChange={(event) => setMode(event.currentTarget.value)}>
            <option value="policy_preview">Policy preview (read and simulate)</option>
            {adminEnabled ? (
              <option value="administrative">Administrative data access</option>
            ) : null}
          </select>
        </label>
        {mode === "policy_preview" ? (
          users.length === 0 ? (
            <p className="notice warning">
              No active application user is available for policy preview.
            </p>
          ) : (
            <label>
              Application user
              <select name="applicationUserId" required>
                {users.map((user) => (
                  <option key={user.id} value={user.id}>
                    {user.email ?? user.id}
                  </option>
                ))}
              </select>
            </label>
          )
        ) : (
          <>
            <p className="notice warning">
              Administrative mode bypasses document policies. Every use is audited.
            </p>
            <label>
              Access reason
              <textarea name="reason" required maxLength={500} />
            </label>
            <label className="checkbox-label">
              <input name="confirm" type="checkbox" required />I understand this access bypasses
              application document policies.
            </label>
          </>
        )}
        <button type="submit" disabled={mode === "policy_preview" && users.length === 0}>
          Create access grant
        </button>
      </form>
    </section>
  );
}

function GrantBanner({
  grant,
  users,
  onRevoke,
}: {
  readonly grant: ExplorerGrant;
  readonly users: ApplicationUserSummary[];
  readonly onRevoke: () => void;
}) {
  const preview = users.find((user) => user.id === grant.applicationUserId);
  return (
    <aside
      className={`notice ${grant.mode === "administrative" ? "warning" : "success"}`}
      aria-live="polite"
    >
      <strong>
        {grant.mode === "administrative"
          ? "Administrative policy bypass active"
          : "Policy preview active"}
      </strong>
      <p>
        {grant.mode === "administrative"
          ? "Document operations are privileged and audited."
          : `Evaluating policies as ${preview?.email ?? grant.applicationUserId ?? "selected user"}. This is not a user session.`}{" "}
        Expires {new Date(grant.expiresAtUnixSeconds * 1000).toLocaleTimeString()}.
      </p>
      <button type="button" className="secondary" onClick={onRevoke}>
        End access
      </button>
    </aside>
  );
}

function QueryBuilder({
  onSubmit,
  plan,
  onCreateIndex,
}: {
  readonly onSubmit: (form: HTMLFormElement) => void;
  readonly plan: ExplorerQueryPlan | null;
  readonly onCreateIndex: () => void;
}) {
  const nextPredicateId = useRef(2);
  const [predicateRows, setPredicateRows] = useState(["predicate-1"]);
  return (
    <section className="panel">
      <h3>Indexed query</h3>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          onSubmit(event.currentTarget);
        }}
      >
        {predicateRows.map((rowId, index) => (
          <fieldset className="query-row" key={rowId}>
            <legend>Predicate {index + 1}</legend>
            <label>
              Field
              <input name="predicateField" required />
            </label>
            <label>
              Operator
              <select name="predicateOperator" defaultValue="equal">
                <option value="equal">equals</option>
                <option value="greater_than">greater than</option>
                <option value="greater_than_or_equal">at least</option>
                <option value="less_than">less than</option>
                <option value="less_than_or_equal">at most</option>
              </select>
            </label>
            <label>
              JSON value
              <input name="predicateValue" defaultValue='"value"' required />
            </label>
          </fieldset>
        ))}
        <div className="button-row">
          <button
            type="button"
            className="secondary"
            disabled={predicateRows.length >= 8}
            onClick={() => {
              const id = nextPredicateId.current;
              nextPredicateId.current += 1;
              setPredicateRows((rows) => [...rows, `predicate-${id}`]);
            }}
          >
            Add predicate
          </button>
          <button
            type="button"
            className="secondary"
            disabled={predicateRows.length <= 1}
            onClick={() => setPredicateRows((rows) => rows.slice(0, -1))}
          >
            Remove predicate
          </button>
        </div>
        <div className="query-row">
          <label>
            Sort field
            <input name="sortField" />
          </label>
          <label>
            Direction
            <select name="sortDirection">
              <option value="ascending">Ascending</option>
              <option value="descending">Descending</option>
            </select>
          </label>
          <label>
            Limit
            <input name="limit" type="number" min="1" max="100" defaultValue="25" required />
          </label>
        </div>
        <button type="submit">Plan and run query</button>
      </form>
      {plan !== null ? (
        plan.supported ? (
          <p className="notice success">
            Using index <strong>{plan.indexName}</strong>; effective limit {plan.effectiveLimit}.
          </p>
        ) : (
          <div className="notice warning">
            <p>This query needs an active index. No collection scan was attempted.</p>
            <code>{JSON.stringify(plan.requiredIndex)}</code>
            <br />
            <button type="button" onClick={onCreateIndex}>
              Review index creation
            </button>
          </div>
        )
      ) : null}
    </section>
  );
}

function DocumentResults({
  page,
  canViewHistory,
  onSelect,
  onHistory,
  onNext,
}: {
  readonly page: ExplorerDocumentPage;
  readonly canViewHistory: boolean;
  readonly onSelect: (document: ExplorerDocument) => void;
  readonly onHistory: (document: ExplorerDocument) => void;
  readonly onNext: () => void;
}) {
  return (
    <section className="panel">
      <div className="section-heading">
        <div>
          <h3>Results</h3>
          <small>Snapshot {page.snapshot}</small>
        </div>
      </div>
      {page.items.length === 0 ? (
        <p>No authorized documents matched.</p>
      ) : (
        <div className="table-scroll">
          <table>
            <thead>
              <tr>
                <th>Primary key</th>
                <th>Revision</th>
                <th>State</th>
                <th>Actions</th>
              </tr>
            </thead>
            <tbody>
              {page.items.map((document) => (
                <tr key={`${document.documentId}:${document.revision}`}>
                  <td>
                    <code>{document.documentId}</code>
                  </td>
                  <td>
                    <code>{document.revision}</code>
                  </td>
                  <td>{document.deleted ? "retained tombstone" : "current"}</td>
                  <td>
                    <div className="button-row">
                      <button
                        type="button"
                        className="secondary"
                        onClick={() => onSelect(document)}
                      >
                        View JSON
                      </button>
                      {canViewHistory ? (
                        <button
                          type="button"
                          className="secondary"
                          onClick={() => onHistory(document)}
                        >
                          History
                        </button>
                      ) : null}
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <button type="button" disabled={page.nextCursor === null} onClick={onNext}>
        Next page
      </button>
    </section>
  );
}

function DocumentDetail({
  document,
  history,
}: {
  readonly document: ExplorerDocument;
  readonly history: ExplorerRevision[] | null;
}) {
  return (
    <section className="panel">
      <h3>
        {document.deleted ? "Deleted document" : "Current document"}: {document.documentId}
      </h3>
      <pre className="json-view">{JSON.stringify(document.content, null, 2)}</pre>
      {history !== null ? (
        <div>
          <h4>Retained revision metadata</h4>
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th>Revision</th>
                  <th>Schema</th>
                  <th>Committed</th>
                  <th>State / retention</th>
                </tr>
              </thead>
              <tbody>
                {history.map((entry) => (
                  <tr key={entry.revision}>
                    <td>
                      <code>{entry.revision}</code>
                    </td>
                    <td>v{entry.schemaVersion}</td>
                    <td>
                      {entry.committedAtUnixSeconds === null
                        ? "unknown"
                        : new Date(entry.committedAtUnixSeconds * 1000).toLocaleString()}
                    </td>
                    <td>
                      {entry.deleted
                        ? `deleted; retained until ${formatTime(entry.retainedUntilUnixSeconds)}`
                        : "historical/current"}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </div>
      ) : null}
    </section>
  );
}

function MutationEditor({
  collection,
  selected,
  simulation,
  canCommit,
  onSubmit,
}: {
  readonly collection: Collection | null;
  readonly selected: ExplorerDocument | null;
  readonly simulation: ExplorerSimulation | null;
  readonly canCommit: boolean;
  readonly onSubmit: (event: FormEvent<HTMLFormElement>, simulateOnly: boolean) => void;
}) {
  const formRef = useRef<HTMLFormElement>(null);
  const [kind, setKind] = useState<ExplorerMutationRequest["kind"]>("update");
  return (
    <section className="panel">
      <h3>Document mutation</h3>
      <p>
        {canCommit
          ? "Administrative commits use conditional revision and idempotency checks."
          : "Policy preview only evaluates this draft; it cannot write."}
      </p>
      <form ref={formRef} onSubmit={(event) => onSubmit(event, false)}>
        <label>
          Action
          <select
            name="kind"
            value={kind}
            onChange={(event) =>
              setKind(event.currentTarget.value as ExplorerMutationRequest["kind"])
            }
          >
            <option value="create">Create</option>
            <option value="update">Update</option>
            <option value="delete">Delete</option>
          </select>
        </label>
        <label>
          Document ID
          <input
            name="documentId"
            required
            defaultValue={selected?.documentId ?? ""}
            key={`id:${selected?.documentId ?? ""}`}
          />
        </label>
        <label>
          Expected revision
          <input
            name="expectedRevision"
            defaultValue={selected?.revision ?? ""}
            key={`rev:${selected?.revision ?? ""}`}
            placeholder="Required for update/delete"
          />
        </label>
        <label>
          Schema version
          <input
            name="schemaVersion"
            type="number"
            min="1"
            required
            defaultValue={selected?.schemaVersion ?? collection?.schemaVersion ?? 1}
            key={`schema:${selected?.schemaVersion ?? collection?.schemaVersion ?? 1}`}
          />
        </label>
        {kind !== "delete" ? (
          <label>
            JSON document
            <textarea
              name="content"
              rows={12}
              required
              defaultValue={JSON.stringify(selected?.content ?? {}, null, 2)}
              key={`content:${selected?.revision ?? "new"}`}
            />
          </label>
        ) : null}
        <div className="button-row">
          <button
            type="button"
            onClick={() => {
              const form = formRef.current;
              if (form === null || !form.reportValidity()) return;
              const synthetic = {
                preventDefault() {},
                currentTarget: form,
              } as FormEvent<HTMLFormElement>;
              onSubmit(synthetic, true);
            }}
          >
            Parse, validate, and simulate
          </button>
          {canCommit ? <button type="submit">Commit conditionally</button> : null}
        </div>
      </form>
      {simulation !== null ? (
        <div
          className={`notice ${simulation.allowed && simulation.schemaValid ? "success" : "warning"}`}
        >
          <strong>
            {simulation.allowed && simulation.schemaValid
              ? "Simulation allowed"
              : "Simulation rejected"}
          </strong>
          <p>
            {simulation.wouldConflict
              ? "The current revision would conflict."
              : "No current revision conflict was detected."}
          </p>
          {simulation.diagnostics.length > 0 ? (
            <ul>
              {simulation.diagnostics.map((item) => (
                <li key={item}>{item}</li>
              ))}
            </ul>
          ) : null}
        </div>
      ) : null}
    </section>
  );
}

function ConflictComparison({
  conflict,
  onReload,
  onCancel,
  onPrepare,
}: {
  readonly conflict: ConflictView;
  readonly onReload: () => void;
  readonly onCancel: () => void;
  readonly onPrepare: () => void;
}) {
  return (
    <section className="panel conflict-panel" role="alert">
      <h3>Revision conflict—no automatic merge was attempted</h3>
      <div className="comparison-grid">
        <JsonColumn label="Original" value={conflict.original?.content ?? null} />
        <JsonColumn label="Proposed" value={conflict.proposed.content} />
        <JsonColumn label="Current" value={conflict.current?.content ?? null} />
      </div>
      <div className="button-row">
        <button type="button" onClick={onReload}>
          Reload current
        </button>
        <button type="button" className="secondary" onClick={onPrepare}>
          Prepare a new update
        </button>
        <button type="button" className="secondary" onClick={onCancel}>
          Cancel
        </button>
      </div>
    </section>
  );
}

function JsonColumn({ label, value }: { readonly label: string; readonly value: unknown }) {
  return (
    <div>
      <h4>{label}</h4>
      <pre className="json-view">{JSON.stringify(value, null, 2)}</pre>
    </div>
  );
}

function DataJobs({
  projectId,
  environmentId,
  collectionId,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly collectionId: string;
}) {
  const client = useManagementClient();
  const { state } = useDeveloperAuth();
  const [jobs, setJobs] = useState<DataJob[]>([]);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [exportScopeConfirmed, setExportScopeConfirmed] = useState(false);
  const [pendingConfirmation, setPendingConfirmation] = useState<DataJob | null>(null);
  const reload = useCallback(async () => {
    try {
      setJobs(
        (await client.listDataJobs(projectId, environmentId)).filter(
          (job) => job.collectionId === collectionId,
        ),
      );
      setFailure(null);
    } catch (error) {
      setFailure(consoleFailure(error));
    }
  }, [client, collectionId, environmentId, projectId]);
  useEffect(() => {
    void reload();
    const timer = window.setInterval(() => void reload(), 5000);
    return () => window.clearInterval(timer);
  }, [reload]);

  const startExport = async () => {
    try {
      if (!exportScopeConfirmed) throw new Error("Confirm the export scope first.");
      await client.createDataJob(
        projectId,
        environmentId,
        { kind: "export", collectionId, conflictStrategy: null },
        crypto.randomUUID(),
      );
      setExportScopeConfirmed(false);
      await reload();
    } catch (error) {
      setFailure(consoleFailure(error));
    }
  };
  const uploadImport = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    try {
      const data = new FormData(event.currentTarget);
      const file = data.get("file");
      if (!(file instanceof File) || file.size === 0)
        throw new Error("Choose a non-empty JSON Lines file.");
      if (file.size > 512 * 1024 * 1024) throw new Error("Import uploads are limited to 512 MiB.");
      const digest = await sha256Hex(await file.arrayBuffer());
      const job = await client.createDataJob(
        projectId,
        environmentId,
        {
          kind: "import",
          collectionId,
          conflictStrategy: requiredText(data, "strategy") as
            | "create_only"
            | "update_existing"
            | "upsert",
        },
        crypto.randomUUID(),
      );
      const grant = await client.createDataJobUploadGrant(projectId, environmentId, job.jobId);
      const response = await fetch(new URL(grant.url, window.location.origin), {
        method: "PUT",
        headers: {
          Authorization: `Bearer ${state.status === "authenticated" ? state.session.accessToken : ""}`,
          Digest: `sha-256=${digest}`,
          "Content-Type": "application/x-ndjson",
        },
        body: file,
      });
      if (!response.ok) throw new Error("The import upload was not accepted.");
      await client.dryRunDataJobImport(projectId, environmentId, job.jobId, {
        uploadDigest: `sha256:${digest}`,
        schemaVersion: positiveInteger(data, "schemaVersion"),
      });
      await reload();
    } catch (error) {
      setFailure(consoleFailure(error));
    }
  };
  const confirm = async (event: FormEvent<HTMLFormElement>, job: DataJob) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    if (data.get("acknowledgePartialImportCancellation") !== "yes") {
      setFailure(
        consoleFailure(
          new Error("Acknowledge the partial-cancellation behavior before execution."),
        ),
      );
      return;
    }
    const digest = manifestDigest(job);
    if (digest === null) {
      setFailure(consoleFailure(new Error("Dry run did not return a manifest digest.")));
      return;
    }
    try {
      await client.confirmDataJob(projectId, environmentId, job.jobId, {
        expectedManifestDigest: digest,
        acknowledgePartialImportCancellation: true,
      });
      setPendingConfirmation(null);
      await reload();
    } catch (error) {
      setFailure(consoleFailure(error));
    }
  };
  const download = async (job: DataJob) => {
    try {
      const grant = await client.createDataJobDownloadGrant(projectId, environmentId, job.jobId);
      const response = await fetch(new URL(grant.url, window.location.origin), {
        headers: {
          Authorization: `Bearer ${state.status === "authenticated" ? state.session.accessToken : ""}`,
        },
      });
      if (!response.ok) throw new Error("The expiring export download was not accepted.");
      const bytes = await response.arrayBuffer();
      const actual = `sha256:${await sha256Hex(bytes)}`;
      if (grant.digest !== null && grant.digest !== undefined && actual !== grant.digest)
        throw new Error("Export integrity verification failed.");
      const responseDigest = response.headers.get("Digest");
      if (responseDigest !== null && responseDigest !== actual.replace("sha256:", "sha-256="))
        throw new Error("Export response digest did not match the downloaded bytes.");
      const anchor = document.createElement("a");
      anchor.href = URL.createObjectURL(new Blob([bytes], { type: "application/x-ndjson" }));
      anchor.download = `${collectionId}-${job.jobId}.jsonl`;
      anchor.click();
      URL.revokeObjectURL(anchor.href);
    } catch (error) {
      setFailure(consoleFailure(error));
    }
  };
  return (
    <section className="panel">
      <div className="section-heading">
        <div>
          <h3>Import and export jobs</h3>
          <p>Imports require a dry run. Cancellation never rolls back rows already committed.</p>
        </div>
        <div>
          <label className="checkbox-label">
            <input
              type="checkbox"
              checked={exportScopeConfirmed}
              onChange={(event) => setExportScopeConfirmed(event.currentTarget.checked)}
            />{" "}
            Export the full authorized collection snapshot
          </label>
          <button type="button" disabled={!exportScopeConfirmed} onClick={() => void startExport()}>
            Create export
          </button>
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      <form className="job-create-form" onSubmit={(event) => void uploadImport(event)}>
        <label>
          JSON Lines file
          <input name="file" type="file" accept=".jsonl,.ndjson,application/x-ndjson" required />
        </label>
        <label>
          Schema version
          <input name="schemaVersion" type="number" min="1" defaultValue="1" required />
        </label>
        <label>
          Conflict strategy
          <select name="strategy">
            <option value="create_only">Create only</option>
            <option value="update_existing">Update existing</option>
            <option value="upsert">Upsert</option>
          </select>
        </label>
        <button type="submit">Upload and dry run</button>
      </form>
      {pendingConfirmation === null ? null : (
        <aside className="notice warning">
          <h4>Confirm import execution</h4>
          <p>
            Job <code>{pendingConfirmation.jobId}</code> will apply{" "}
            {pendingConfirmation.manifest?.rowCount ?? 0} rows using{" "}
            <strong>{pendingConfirmation.conflictStrategy}</strong>. Manifest{" "}
            <code>{manifestDigest(pendingConfirmation)}</code>.
          </p>
          <form onSubmit={(event) => void confirm(event, pendingConfirmation)}>
            <label className="checkbox-label">
              <input
                name="acknowledgePartialImportCancellation"
                type="checkbox"
                value="yes"
                required
              />{" "}
              I understand cancellation stops future rows and does not roll back committed rows.
            </label>
            <div className="button-row">
              <button type="submit">Confirm execution</button>
              <button
                type="button"
                className="secondary"
                onClick={() => setPendingConfirmation(null)}
              >
                Cancel
              </button>
            </div>
          </form>
        </aside>
      )}
      {jobs.length === 0 ? (
        <p>No data jobs for this collection.</p>
      ) : (
        <div className="table-scroll">
          <table>
            <thead>
              <tr>
                <th>Job</th>
                <th>Type / state</th>
                <th>Progress</th>
                <th>Errors / manifest</th>
                <th>Actions</th>
              </tr>
            </thead>
            <tbody>
              {jobs.map((job) => (
                <tr key={job.jobId}>
                  <td>
                    <code>{job.jobId}</code>
                  </td>
                  <td>
                    {job.kind} / {job.state}
                  </td>
                  <td>
                    {Object.entries(job.progress)
                      .map(([key, value]) => `${key}: ${value}`)
                      .join(", ") || "not started"}
                  </td>
                  <td>{job.errors.slice(0, 5).join("; ") || (manifestDigest(job) ?? "—")}</td>
                  <td>
                    <div className="button-row">
                      {job.state === "awaiting_confirmation" ? (
                        <button type="button" onClick={() => setPendingConfirmation(job)}>
                          Review execution
                        </button>
                      ) : null}
                      {["queued", "running", "cancelling"].includes(job.state) ? (
                        <button
                          type="button"
                          className="secondary"
                          onClick={() =>
                            void client
                              .cancelDataJob(projectId, environmentId, job.jobId)
                              .then(reload)
                              .catch((error: unknown) => setFailure(consoleFailure(error)))
                          }
                        >
                          Cancel
                        </button>
                      ) : null}
                      {job.kind === "export" && job.state === "succeeded" ? (
                        <button type="button" onClick={() => void download(job)}>
                          Download and verify
                        </button>
                      ) : null}
                    </div>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}

function queryFromForm(data: FormData, cursor: string | null): ExplorerQueryRequest {
  const fields = data.getAll("predicateField");
  const operators = data.getAll("predicateOperator");
  const values = data.getAll("predicateValue");
  const sortField = optionalText(data, "sortField");
  return {
    predicates: fields.map((field, index) => ({
      field: String(field).trim(),
      operator: String(operators[index]) as ExplorerQueryRequest["predicates"][number]["operator"],
      value: JSON.parse(String(values[index])) as unknown,
    })),
    sort:
      sortField === null
        ? []
        : [
            {
              field: sortField,
              direction: requiredText(data, "sortDirection") as "ascending" | "descending",
            },
          ],
    limit: positiveInteger(data, "limit"),
    cursor,
  };
}

function requiredText(data: FormData, name: string): string {
  const value = data.get(name);
  if (typeof value !== "string" || value.trim() === "") throw new Error(`${name} is required.`);
  return value.trim();
}
function optionalText(data: FormData, name: string): string | null {
  const value = data.get(name);
  return typeof value === "string" && value.trim() !== "" ? value.trim() : null;
}
function positiveInteger(data: FormData, name: string): number {
  const value = Number(requiredText(data, name));
  if (!Number.isSafeInteger(value) || value < 1)
    throw new Error(`${name} must be a positive integer.`);
  return value;
}
function parseJsonObject(value: string): Record<string, unknown> {
  const parsed = JSON.parse(value) as unknown;
  if (typeof parsed !== "object" || parsed === null || Array.isArray(parsed))
    throw new Error("Document content must be a JSON object.");
  return parsed as Record<string, unknown>;
}
function consoleFailure(error: unknown): ConsoleApiFailure {
  return error instanceof Error && !("requestId" in error)
    ? { message: error.message, requestId: null }
    : toConsoleApiFailure(error);
}
function formatTime(value: number | null): string {
  return value === null ? "not retained" : new Date(value * 1000).toLocaleString();
}
function manifestDigest(job: DataJob): string | null {
  return job.manifest?.digest ?? null;
}
async function sha256Hex(bytes: ArrayBuffer): Promise<string> {
  return Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)))
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}
