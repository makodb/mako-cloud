import {
  Alert,
  AlertDescription,
  AlertTitle,
  Badge,
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  Checkbox,
  Eyebrow,
  Field,
  Input,
  Label,
  NativeSelect,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  Textarea,
  cn,
} from "@mako-cloud/ui";
import { AlertTriangle, CheckCircle2, Info, RefreshCw, ShieldAlert } from "lucide-react";
import { type FormEvent, type ReactNode, useCallback, useEffect, useRef, useState } from "react";

import type {
  ApplicationUserSummary,
  ArtifactGrant,
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

/** JSON that is read: a bordered, scrolling block in the code face. */
const JSON_BLOCK =
  "m-0 overflow-x-auto rounded-md border bg-muted/40 p-3 font-mono text-xs leading-5";
/** JSON that is written: the same face inside a textarea. */
const JSON_FIELD = "min-h-40 max-h-[32rem] font-mono text-xs leading-5 md:text-xs";
const CODE = "break-all font-mono text-xs";

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
    <div className="grid gap-6">
      <header className="flex flex-wrap items-end justify-between gap-4">
        <div className="grid gap-1">
          <Eyebrow>Data explorer</Eyebrow>
          <h2 className="m-0 text-2xl font-semibold tracking-tight">Documents</h2>
          <p className="m-0 text-sm text-muted-foreground">
            Browse through a short-lived, collection-scoped access grant.
          </p>
        </div>
        <Field label="Collection" htmlFor="explorer-collection" className="min-w-48">
          <NativeSelect
            id="explorer-collection"
            className="font-mono"
            value={collectionId}
            onChange={(event) => switchCollection(event.currentTarget.value)}
          >
            {collections?.map((collection) => (
              <option key={collection.id} value={collection.id}>
                {collection.id}
              </option>
            ))}
          </NativeSelect>
        </Field>
      </header>
      <ApiFailureNotice failure={failure} />
      {collections === null ? (
        <p className="m-0 text-sm text-muted-foreground">Loading collections…</p>
      ) : collections.length === 0 ? (
        <Alert>
          <Info aria-hidden="true" />
          <AlertDescription>
            Create and activate a collection schema before opening the explorer.
          </AlertDescription>
        </Alert>
      ) : null}
      {grant === null && collectionId !== "" ? (
        <GrantForm users={users} adminEnabled={adminEnabled} onSubmit={issueGrant} />
      ) : grant !== null ? (
        <GrantBanner grant={grant} users={users} onRevoke={() => void revoke()} />
      ) : null}
      {grant !== null ? (
        <>
          <div className="grid items-start gap-4 md:grid-cols-2">
            <Card>
              <CardHeader>
                <CardTitle as="h3">Canonical browse</CardTitle>
                <CardDescription>
                  Snapshot-consistent primary-key order; hidden policy rows are not counted.
                </CardDescription>
              </CardHeader>
              <CardContent className="flex flex-wrap items-center gap-2">
                <Button onClick={() => void browse(null, false)}>Browse documents</Button>
                {grant.mode === "administrative" ? (
                  <Button variant="outline" onClick={() => void browse(null, true)}>
                    Include retained tombstones
                  </Button>
                ) : null}
              </CardContent>
            </Card>
            <Card>
              <CardHeader>
                <CardTitle as="h3">Exact primary key</CardTitle>
              </CardHeader>
              <CardContent>
                <form
                  className="flex flex-wrap items-end gap-2"
                  onSubmit={(event) => void lookup(event)}
                >
                  <Field
                    label="Document ID"
                    htmlFor="lookup-document-id"
                    className="min-w-0 flex-1"
                  >
                    <Input
                      id="lookup-document-id"
                      name="documentId"
                      required
                      maxLength={512}
                      className="font-mono"
                    />
                  </Field>
                  <Button type="submit">Get</Button>
                </form>
              </CardContent>
            </Card>
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
            <Alert>
              <Info aria-hidden="true" />
              <AlertDescription className="block">
                Audit reference <code className={CODE}>{auditReference}</code>
              </AlertDescription>
            </Alert>
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
    <Card aria-labelledby="grant-title">
      <CardHeader>
        <CardTitle as="h3" id="grant-title">
          Create scoped access
        </CardTitle>
      </CardHeader>
      <CardContent>
        <form className="grid max-w-xl gap-4" onSubmit={onSubmit}>
          <Field label="Mode" htmlFor="grant-mode">
            <NativeSelect
              id="grant-mode"
              name="mode"
              value={mode}
              onChange={(event) => setMode(event.currentTarget.value)}
            >
              <option value="policy_preview">Policy preview (read and simulate)</option>
              {adminEnabled ? (
                <option value="administrative">Administrative data access</option>
              ) : null}
            </NativeSelect>
          </Field>
          {mode === "policy_preview" ? (
            users.length === 0 ? (
              <Alert variant="warning">
                <AlertTriangle aria-hidden="true" />
                <AlertDescription>
                  No active application user is available for policy preview.
                </AlertDescription>
              </Alert>
            ) : (
              <Field label="Application user" htmlFor="grant-application-user">
                <NativeSelect id="grant-application-user" name="applicationUserId" required>
                  {users.map((user) => (
                    <option key={user.id} value={user.id}>
                      {user.email ?? user.id}
                    </option>
                  ))}
                </NativeSelect>
              </Field>
            )
          ) : (
            <>
              <Alert variant="warning">
                <ShieldAlert aria-hidden="true" />
                <AlertDescription>
                  Administrative mode bypasses document policies. Every use is audited.
                </AlertDescription>
              </Alert>
              <Field label="Access reason" htmlFor="grant-reason">
                <Textarea id="grant-reason" name="reason" required maxLength={500} />
              </Field>
              {/* The submit handler checks the box itself so a missing confirmation
                  is reported as a visible failure rather than blocked silently. */}
              <div className="flex items-start gap-2">
                <Checkbox id="grant-confirm" name="confirm" className="mt-0.5" />
                <Label htmlFor="grant-confirm" className="leading-snug font-normal">
                  I understand this access bypasses application document policies.
                </Label>
              </div>
            </>
          )}
          <div>
            <Button type="submit" disabled={mode === "policy_preview" && users.length === 0}>
              Create access grant
            </Button>
          </div>
        </form>
      </CardContent>
    </Card>
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
  const administrative = grant.mode === "administrative";
  return (
    <aside aria-live="polite">
      <Alert variant={administrative ? "warning" : "positive"}>
        {administrative ? <ShieldAlert aria-hidden="true" /> : <CheckCircle2 aria-hidden="true" />}
        <AlertTitle>
          {administrative ? "Administrative policy bypass active" : "Policy preview active"}
        </AlertTitle>
        <AlertDescription>
          <p className="m-0">
            {administrative
              ? "Document operations are privileged and audited."
              : `Evaluating policies as ${preview?.email ?? grant.applicationUserId ?? "selected user"}. This is not a user session.`}{" "}
            Expires {new Date(grant.expiresAtUnixSeconds * 1000).toLocaleTimeString()}.
          </p>
          <Button variant="outline" size="sm" className="mt-1" onClick={onRevoke}>
            End access
          </Button>
        </AlertDescription>
      </Alert>
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
    <Card>
      <CardHeader>
        <CardTitle as="h3">Indexed query</CardTitle>
      </CardHeader>
      <CardContent className="grid gap-4">
        <form
          className="grid gap-4"
          onSubmit={(event) => {
            event.preventDefault();
            onSubmit(event.currentTarget);
          }}
        >
          {predicateRows.map((rowId, index) => (
            <fieldset
              className="m-0 grid min-w-0 gap-3 rounded-md border px-3 pt-1 pb-3 sm:grid-cols-3"
              key={rowId}
            >
              <legend className="px-1 text-xs font-medium text-muted-foreground">
                Predicate {index + 1}
              </legend>
              <Field label="Field" htmlFor={`${rowId}-field`}>
                <Input id={`${rowId}-field`} name="predicateField" required className="font-mono" />
              </Field>
              <Field label="Operator" htmlFor={`${rowId}-operator`}>
                <NativeSelect
                  id={`${rowId}-operator`}
                  name="predicateOperator"
                  defaultValue="equal"
                >
                  <option value="equal">equals</option>
                  <option value="greater_than">greater than</option>
                  <option value="greater_than_or_equal">at least</option>
                  <option value="less_than">less than</option>
                  <option value="less_than_or_equal">at most</option>
                </NativeSelect>
              </Field>
              <Field label="JSON value" htmlFor={`${rowId}-value`}>
                <Input
                  id={`${rowId}-value`}
                  name="predicateValue"
                  defaultValue='"value"'
                  required
                  className="font-mono"
                />
              </Field>
            </fieldset>
          ))}
          <div className="flex flex-wrap items-center gap-2">
            <Button
              variant="outline"
              size="sm"
              disabled={predicateRows.length >= 8}
              onClick={() => {
                const id = nextPredicateId.current;
                nextPredicateId.current += 1;
                setPredicateRows((rows) => [...rows, `predicate-${id}`]);
              }}
            >
              Add predicate
            </Button>
            <Button
              variant="outline"
              size="sm"
              disabled={predicateRows.length <= 1}
              onClick={() => setPredicateRows((rows) => rows.slice(0, -1))}
            >
              Remove predicate
            </Button>
          </div>
          <div className="grid gap-3 sm:grid-cols-3">
            <Field label="Sort field" htmlFor="query-sort-field">
              <Input id="query-sort-field" name="sortField" className="font-mono" />
            </Field>
            <Field label="Direction" htmlFor="query-sort-direction">
              <NativeSelect id="query-sort-direction" name="sortDirection">
                <option value="ascending">Ascending</option>
                <option value="descending">Descending</option>
              </NativeSelect>
            </Field>
            <Field label="Limit" htmlFor="query-limit">
              <Input
                id="query-limit"
                name="limit"
                type="number"
                min="1"
                max="100"
                defaultValue="25"
                required
              />
            </Field>
          </div>
          <div>
            <Button type="submit">Plan and run query</Button>
          </div>
        </form>
        {plan !== null ? (
          plan.supported ? (
            <Alert variant="positive">
              <CheckCircle2 aria-hidden="true" />
              <AlertDescription className="block">
                Using index <strong className="font-mono">{plan.indexName}</strong>; effective limit{" "}
                {plan.effectiveLimit}.
              </AlertDescription>
            </Alert>
          ) : (
            <Alert variant="warning">
              <AlertTriangle aria-hidden="true" />
              <AlertDescription>
                <p className="m-0">
                  This query needs an active index. No collection scan was attempted.
                </p>
                <code className={CODE}>{JSON.stringify(plan.requiredIndex)}</code>
                <Button size="sm" className="mt-1" onClick={onCreateIndex}>
                  Review index creation
                </Button>
              </AlertDescription>
            </Alert>
          )
        ) : null}
      </CardContent>
    </Card>
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
    <Card>
      <CardHeader>
        <CardTitle as="h3">Results</CardTitle>
        <CardDescription>
          <small className="text-xs">
            Snapshot <span className="font-mono">{page.snapshot}</span>
          </small>
        </CardDescription>
      </CardHeader>
      <CardContent className="grid gap-4">
        {page.items.length === 0 ? (
          <p className="m-0 text-sm text-muted-foreground">No authorized documents matched.</p>
        ) : (
          <div className="overflow-hidden rounded-md border">
            <Table>
              <TableHeader>
                <TableRow className="hover:bg-transparent">
                  <TableHead scope="col">Primary key</TableHead>
                  <TableHead scope="col">Revision</TableHead>
                  <TableHead scope="col">State</TableHead>
                  <TableHead scope="col">Actions</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {page.items.map((document) => (
                  <TableRow key={`${document.documentId}:${document.revision}`}>
                    <TableCell>
                      <code className={CODE}>{document.documentId}</code>
                    </TableCell>
                    <TableCell>
                      <code className={CODE}>{document.revision}</code>
                    </TableCell>
                    <TableCell>
                      <Badge variant={document.deleted ? "secondary" : "outline"}>
                        {document.deleted ? "retained tombstone" : "current"}
                      </Badge>
                    </TableCell>
                    <TableCell>
                      <div className="flex flex-wrap gap-1">
                        <RowAction onClick={() => onSelect(document)}>View JSON</RowAction>
                        {canViewHistory ? (
                          <RowAction onClick={() => onHistory(document)}>History</RowAction>
                        ) : null}
                      </div>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        )}
        <div>
          <Button variant="outline" disabled={page.nextCursor === null} onClick={onNext}>
            Next page
          </Button>
        </div>
      </CardContent>
    </Card>
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
    <Card>
      <CardHeader>
        <CardTitle as="h3" className="leading-snug">
          {document.deleted ? "Deleted document" : "Current document"}:{" "}
          <span className="font-mono">{document.documentId}</span>
        </CardTitle>
      </CardHeader>
      <CardContent className="grid gap-4">
        <pre className={JSON_BLOCK}>{JSON.stringify(document.content, null, 2)}</pre>
        {history !== null ? (
          <div className="grid gap-2">
            <h4 className="m-0 text-sm font-medium">Retained revision metadata</h4>
            <div className="overflow-hidden rounded-md border">
              <Table>
                <TableHeader>
                  <TableRow className="hover:bg-transparent">
                    <TableHead scope="col">Revision</TableHead>
                    <TableHead scope="col">Schema</TableHead>
                    <TableHead scope="col">Committed</TableHead>
                    <TableHead scope="col">State / retention</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {history.map((entry) => (
                    <TableRow key={entry.revision}>
                      <TableCell>
                        <code className={CODE}>{entry.revision}</code>
                      </TableCell>
                      <TableCell>v{entry.schemaVersion}</TableCell>
                      <TableCell>
                        {entry.committedAtUnixSeconds === null
                          ? "unknown"
                          : new Date(entry.committedAtUnixSeconds * 1000).toLocaleString()}
                      </TableCell>
                      <TableCell className="whitespace-normal">
                        {entry.deleted
                          ? `deleted; retained until ${formatTime(entry.retainedUntilUnixSeconds)}`
                          : "historical/current"}
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </div>
          </div>
        ) : null}
      </CardContent>
    </Card>
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
  const accepted = simulation?.allowed === true && simulation.schemaValid;
  return (
    <Card>
      <CardHeader>
        <CardTitle as="h3">Document mutation</CardTitle>
        <CardDescription>
          {canCommit
            ? "Administrative commits use conditional revision and idempotency checks."
            : "Policy preview only evaluates this draft; it cannot write."}
        </CardDescription>
      </CardHeader>
      <CardContent className="grid gap-4">
        <form ref={formRef} className="grid gap-4" onSubmit={(event) => onSubmit(event, false)}>
          <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-4">
            <Field label="Action" htmlFor="mutation-kind">
              <NativeSelect
                id="mutation-kind"
                name="kind"
                value={kind}
                onChange={(event) =>
                  setKind(event.currentTarget.value as ExplorerMutationRequest["kind"])
                }
              >
                <option value="create">Create</option>
                <option value="update">Update</option>
                <option value="delete">Delete</option>
              </NativeSelect>
            </Field>
            <Field label="Document ID" htmlFor="mutation-document-id">
              <Input
                id="mutation-document-id"
                name="documentId"
                required
                defaultValue={selected?.documentId ?? ""}
                key={`id:${selected?.documentId ?? ""}`}
                className="font-mono"
              />
            </Field>
            <Field label="Expected revision" htmlFor="mutation-expected-revision">
              <Input
                id="mutation-expected-revision"
                name="expectedRevision"
                defaultValue={selected?.revision ?? ""}
                key={`rev:${selected?.revision ?? ""}`}
                placeholder="Required for update/delete"
                className="font-mono"
              />
            </Field>
            <Field label="Schema version" htmlFor="mutation-schema-version">
              <Input
                id="mutation-schema-version"
                name="schemaVersion"
                type="number"
                min="1"
                required
                defaultValue={selected?.schemaVersion ?? collection?.schemaVersion ?? 1}
                key={`schema:${selected?.schemaVersion ?? collection?.schemaVersion ?? 1}`}
              />
            </Field>
          </div>
          {kind !== "delete" ? (
            <Field label="JSON document" htmlFor="mutation-content">
              <Textarea
                id="mutation-content"
                name="content"
                rows={12}
                required
                defaultValue={JSON.stringify(selected?.content ?? {}, null, 2)}
                key={`content:${selected?.revision ?? "new"}`}
                spellCheck={false}
                className={JSON_FIELD}
              />
            </Field>
          ) : null}
          <div className="flex flex-wrap items-center gap-2">
            <Button
              variant={canCommit ? "outline" : "default"}
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
            </Button>
            {canCommit ? <Button type="submit">Commit conditionally</Button> : null}
          </div>
        </form>
        {simulation !== null ? (
          <Alert variant={accepted ? "positive" : "warning"}>
            {accepted ? <CheckCircle2 aria-hidden="true" /> : <AlertTriangle aria-hidden="true" />}
            <AlertTitle>{accepted ? "Simulation allowed" : "Simulation rejected"}</AlertTitle>
            <AlertDescription>
              <p className="m-0">
                {simulation.wouldConflict
                  ? "The current revision would conflict."
                  : "No current revision conflict was detected."}
              </p>
              {simulation.diagnostics.length > 0 ? (
                <ul className="m-0 list-disc pl-4">
                  {simulation.diagnostics.map((item) => (
                    <li key={item}>{item}</li>
                  ))}
                </ul>
              ) : null}
            </AlertDescription>
          </Alert>
        ) : null}
      </CardContent>
    </Card>
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
    <Card role="alert" className="border-destructive/40">
      <CardHeader>
        <CardTitle as="h3" className="flex items-center gap-2 leading-snug text-destructive">
          <AlertTriangle aria-hidden="true" className="size-4 shrink-0" />
          Revision conflict—no automatic merge was attempted
        </CardTitle>
      </CardHeader>
      <CardContent className="grid gap-4">
        <div className="grid gap-4 md:grid-cols-3">
          <JsonColumn label="Original" value={conflict.original?.content ?? null} />
          <JsonColumn label="Proposed" value={conflict.proposed.content} />
          <JsonColumn label="Current" value={conflict.current?.content ?? null} />
        </div>
        <div className="flex flex-wrap items-center gap-2">
          <Button onClick={onReload}>Reload current</Button>
          <Button variant="outline" onClick={onPrepare}>
            Prepare a new update
          </Button>
          <Button variant="outline" onClick={onCancel}>
            Cancel
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}

function JsonColumn({ label, value }: { readonly label: string; readonly value: unknown }) {
  return (
    <div className="grid min-w-0 gap-2">
      <h4 className="m-0 text-sm font-medium">{label}</h4>
      <pre className={JSON_BLOCK}>{JSON.stringify(value, null, 2)}</pre>
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
  const [detail, setDetail] = useState<DataJob | null>(null);
  const [uploadGrant, setUploadGrant] = useState<ArtifactGrant | null>(null);
  const reload = useCallback(async () => {
    try {
      const items = (await client.listDataJobs(projectId, environmentId)).filter(
        (job) => job.collectionId === collectionId,
      );
      setJobs(items);
      // The open detail follows the list so polling keeps it current between
      // explicit refreshes.
      setDetail((current) =>
        current === null ? null : (items.find((job) => job.jobId === current.jobId) ?? current),
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
  const cancel = async (job: DataJob) => {
    try {
      await client.cancelDataJob(projectId, environmentId, job.jobId);
      await reload();
    } catch (error) {
      setFailure(consoleFailure(error));
    }
  };
  const openDetail = async (jobId: string) => {
    try {
      setDetail(await client.getDataJob(projectId, environmentId, jobId));
      setUploadGrant(null);
      setFailure(null);
    } catch (error) {
      setFailure(consoleFailure(error));
    }
  };
  const issueUploadGrant = async (job: DataJob) => {
    try {
      setUploadGrant(await client.createDataJobUploadGrant(projectId, environmentId, job.jobId));
      setFailure(null);
    } catch (error) {
      setFailure(consoleFailure(error));
    }
  };
  return (
    <Card>
      <CardHeader className="flex flex-wrap items-start justify-between gap-4">
        <div className="grid gap-1">
          <CardTitle as="h3">Import and export jobs</CardTitle>
          <CardDescription>
            Imports require a dry run. Cancellation never rolls back rows already committed.
          </CardDescription>
        </div>
        <div className="grid gap-2">
          <div className="flex items-start gap-2">
            <Checkbox
              id="export-scope-confirmed"
              className="mt-0.5"
              checked={exportScopeConfirmed}
              onCheckedChange={(checked) => setExportScopeConfirmed(checked === true)}
            />
            <Label htmlFor="export-scope-confirmed" className="leading-snug font-normal">
              Export the full authorized collection snapshot
            </Label>
          </div>
          <div>
            <Button size="sm" disabled={!exportScopeConfirmed} onClick={() => void startExport()}>
              Create export
            </Button>
          </div>
        </div>
      </CardHeader>
      <CardContent className="grid gap-4">
        <ApiFailureNotice failure={failure} />
        <form
          className="grid items-end gap-3 rounded-md border p-3 sm:grid-cols-[minmax(0,2fr)_minmax(0,1fr)_minmax(0,1fr)_auto]"
          onSubmit={(event) => void uploadImport(event)}
        >
          <Field label="JSON Lines file" htmlFor="import-file">
            <Input
              id="import-file"
              name="file"
              type="file"
              accept=".jsonl,.ndjson,application/x-ndjson"
              required
            />
          </Field>
          <Field label="Schema version" htmlFor="import-schema-version">
            <Input
              id="import-schema-version"
              name="schemaVersion"
              type="number"
              min="1"
              defaultValue="1"
              required
            />
          </Field>
          <Field label="Conflict strategy" htmlFor="import-strategy">
            <NativeSelect id="import-strategy" name="strategy">
              <option value="create_only">Create only</option>
              <option value="update_existing">Update existing</option>
              <option value="upsert">Upsert</option>
            </NativeSelect>
          </Field>
          <Button type="submit" variant="outline">
            Upload and dry run
          </Button>
        </form>
        {pendingConfirmation === null ? null : (
          <aside>
            <Alert variant="warning">
              <AlertTriangle aria-hidden="true" />
              <h4 className="col-start-2 m-0 text-sm font-medium tracking-tight">
                Confirm import execution
              </h4>
              <AlertDescription>
                <p className="m-0">
                  Job <code className={CODE}>{pendingConfirmation.jobId}</code> will apply{" "}
                  {pendingConfirmation.manifest?.rowCount ?? 0} rows using{" "}
                  <strong>{pendingConfirmation.conflictStrategy}</strong>. Manifest{" "}
                  <code className={CODE}>{manifestDigest(pendingConfirmation)}</code>.
                </p>
                <form
                  className="grid w-full gap-3"
                  onSubmit={(event) => void confirm(event, pendingConfirmation)}
                >
                  {/* The submit handler checks the acknowledgement itself and reports
                      a missing one as a visible failure. */}
                  <div className="flex items-start gap-2">
                    <Checkbox
                      id="acknowledge-partial-cancellation"
                      name="acknowledgePartialImportCancellation"
                      value="yes"
                      className="mt-0.5"
                    />
                    <Label
                      htmlFor="acknowledge-partial-cancellation"
                      className="leading-snug font-normal"
                    >
                      I understand cancellation stops future rows and does not roll back committed
                      rows.
                    </Label>
                  </div>
                  <div className="flex flex-wrap items-center gap-2">
                    <Button type="submit" size="sm">
                      Confirm execution
                    </Button>
                    <Button
                      variant="outline"
                      size="sm"
                      onClick={() => setPendingConfirmation(null)}
                    >
                      Cancel
                    </Button>
                  </div>
                </form>
              </AlertDescription>
            </Alert>
          </aside>
        )}
        {jobs.length === 0 ? (
          <p className="m-0 text-sm text-muted-foreground">No data jobs for this collection.</p>
        ) : (
          <div className="overflow-hidden rounded-md border">
            <Table>
              <TableHeader>
                <TableRow className="hover:bg-transparent">
                  <TableHead scope="col">Job</TableHead>
                  <TableHead scope="col">Type / state</TableHead>
                  <TableHead scope="col">Progress</TableHead>
                  <TableHead scope="col">Errors / manifest</TableHead>
                  <TableHead scope="col">Actions</TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {jobs.map((job) => (
                  <TableRow key={job.jobId}>
                    <TableCell>
                      <code className={CODE}>{job.jobId}</code>
                    </TableCell>
                    <TableCell>
                      {job.kind} / {job.state}
                    </TableCell>
                    <TableCell className="whitespace-normal text-xs text-muted-foreground">
                      {Object.entries(job.progress)
                        .map(([key, value]) => `${key}: ${value}`)
                        .join(", ") || "not started"}
                    </TableCell>
                    <TableCell className="max-w-xs whitespace-normal text-xs">
                      <span className={cn(job.errors.length === 0 && "font-mono")}>
                        {job.errors.slice(0, 5).join("; ") || (manifestDigest(job) ?? "—")}
                      </span>
                    </TableCell>
                    <TableCell>
                      <div className="flex flex-wrap gap-1">
                        <RowAction
                          aria-label={`Details for ${job.jobId}`}
                          onClick={() => void openDetail(job.jobId)}
                        >
                          Details
                        </RowAction>
                        {job.state === "awaiting_confirmation" ? (
                          <RowAction emphasis onClick={() => setPendingConfirmation(job)}>
                            Review execution
                          </RowAction>
                        ) : null}
                        {cancellable(job) ? (
                          <RowAction onClick={() => void cancel(job)}>Cancel</RowAction>
                        ) : null}
                        {job.kind === "export" && job.state === "succeeded" ? (
                          <RowAction emphasis onClick={() => void download(job)}>
                            Download and verify
                          </RowAction>
                        ) : null}
                      </div>
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
          </div>
        )}
        {detail === null ? null : (
          <DataJobDetail
            job={detail}
            uploadGrant={uploadGrant}
            onRefresh={() => void openDetail(detail.jobId)}
            onClose={() => {
              setDetail(null);
              setUploadGrant(null);
            }}
            onReview={() => setPendingConfirmation(detail)}
            onCancel={() => void cancel(detail)}
            onDownload={() => void download(detail)}
            onIssueUploadGrant={() => void issueUploadGrant(detail)}
          />
        )}
      </CardContent>
    </Card>
  );
}

// One job in full: status, kind, counts, timestamps, the retained failure
// diagnostic, and the grant actions its state allows.
function DataJobDetail({
  job,
  uploadGrant,
  onRefresh,
  onClose,
  onReview,
  onCancel,
  onDownload,
  onIssueUploadGrant,
}: {
  readonly job: DataJob;
  readonly uploadGrant: ArtifactGrant | null;
  readonly onRefresh: () => void;
  readonly onClose: () => void;
  readonly onReview: () => void;
  readonly onCancel: () => void;
  readonly onDownload: () => void;
  readonly onIssueUploadGrant: () => void;
}) {
  const diagnostics = job.errors.map((message, position) => ({ id: `${position}`, message }));
  // `job-detail` is not decoration: it is the handle the jobs scenario opens
  // this panel by.
  return (
    <Card
      as="article"
      aria-labelledby="job-detail-title"
      className="job-detail gap-4 bg-muted/20 py-4"
    >
      <CardContent className="grid gap-4 px-4">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <h4 id="job-detail-title" className="m-0 text-sm font-semibold">
            Job <span className="font-mono">{job.jobId}</span>
          </h4>
          <div className="flex flex-wrap items-center gap-2">
            <Button variant="outline" size="sm" onClick={onRefresh}>
              <RefreshCw aria-hidden="true" />
              Refresh
            </Button>
            <Button variant="outline" size="sm" onClick={onClose}>
              Close
            </Button>
          </div>
        </div>
        <dl className="m-0 grid gap-3 sm:grid-cols-2 lg:grid-cols-4">
          <MetaItem term="Status">{job.state.replaceAll("_", " ")}</MetaItem>
          <MetaItem term="Kind">{job.kind}</MetaItem>
          <MetaItem term="Collection">
            <code className={CODE}>{job.collectionId}</code>
          </MetaItem>
          <MetaItem term="Conflict strategy">
            {job.conflictStrategy === null ? "—" : job.conflictStrategy.replaceAll("_", " ")}
          </MetaItem>
          <MetaItem term="Created by">
            <code className={CODE}>{job.creatorId}</code>
          </MetaItem>
          <MetaItem term="Created">{formatTime(job.createdAtUnixSeconds)}</MetaItem>
          <MetaItem term="Updated">{formatTime(job.updatedAtUnixSeconds)}</MetaItem>
          <MetaItem term="Expires">{formatTime(job.expiresAtUnixSeconds)}</MetaItem>
        </dl>
        <div className="grid gap-2">
          <h5 className="m-0 text-xs font-semibold uppercase tracking-wider text-muted-foreground">
            Counts
          </h5>
          <dl className="m-0 grid gap-3 sm:grid-cols-3 lg:grid-cols-6">
            {Object.entries(job.progress).map(([key, value]) => (
              <MetaItem key={key} term={key}>
                <span className="tabular-nums">{value.toLocaleString()}</span>
              </MetaItem>
            ))}
          </dl>
        </div>
        {job.manifest === null ? null : (
          <p className="m-0 text-sm text-muted-foreground">
            Manifest: {job.manifest.rowCount.toLocaleString()} rows,{" "}
            {job.manifest.byteCount.toLocaleString()} bytes, schema version{" "}
            {job.manifest.schemaVersion}, digest <code className={CODE}>{job.manifest.digest}</code>
            , finalized {formatTime(job.manifest.finalizedAtUnixSeconds)}.
          </p>
        )}
        {diagnostics.length > 0 ? (
          <Alert variant="destructive" role="status">
            <AlertTriangle aria-hidden="true" />
            <AlertTitle>Failure diagnostic</AlertTitle>
            <AlertDescription>
              <ul className="m-0 list-disc pl-4">
                {diagnostics.map((diagnostic) => (
                  <li key={diagnostic.id}>{diagnostic.message}</li>
                ))}
              </ul>
            </AlertDescription>
          </Alert>
        ) : job.state === "failed" ? (
          <Alert variant="destructive" role="status">
            <AlertTriangle aria-hidden="true" />
            <AlertDescription>The job failed without a retained diagnostic.</AlertDescription>
          </Alert>
        ) : null}
        <div className="flex flex-wrap items-center gap-2">
          {job.kind === "import" && job.state === "awaiting_upload" ? (
            <Button size="sm" onClick={onIssueUploadGrant}>
              Issue upload grant
            </Button>
          ) : null}
          {job.kind === "export" && job.state === "succeeded" ? (
            <Button size="sm" onClick={onDownload}>
              Download and verify
            </Button>
          ) : null}
          {job.state === "awaiting_confirmation" ? (
            <Button size="sm" onClick={onReview}>
              Review execution
            </Button>
          ) : null}
          {cancellable(job) ? (
            <Button size="sm" variant="outline" onClick={onCancel}>
              Cancel job
            </Button>
          ) : null}
        </div>
        {uploadGrant === null ? null : (
          <Alert role="status">
            <Info aria-hidden="true" />
            <AlertDescription className="block">
              Upload grant: <code className={CODE}>{uploadGrant.method}</code>{" "}
              <code className={CODE}>{uploadGrant.url}</code>, expires{" "}
              {formatTime(uploadGrant.expiresAtUnixSeconds)}. Send the JSON Lines file with a{" "}
              <code className={CODE}>Digest: sha-256=…</code> header, then run the dry run.
            </AlertDescription>
          </Alert>
        )}
      </CardContent>
    </Card>
  );
}

/** One of the quiet text actions at the end of a table row; `emphasis` marks the primary one. */
function RowAction({
  emphasis = false,
  className,
  ...props
}: {
  readonly emphasis?: boolean;
  readonly className?: string;
  readonly "aria-label"?: string;
  readonly onClick: () => void;
  readonly children: ReactNode;
}) {
  return (
    <Button
      variant="ghost"
      size="sm"
      className={cn(
        "h-7 px-2 text-xs",
        emphasis
          ? "text-primary hover:text-primary"
          : "text-muted-foreground hover:text-foreground",
        className,
      )}
      {...props}
    />
  );
}

/** One term and its value in a metadata list. */
function MetaItem({ term, children }: { readonly term: string; readonly children: ReactNode }) {
  return (
    <div className="grid gap-0.5">
      <dt className="text-xs font-medium uppercase tracking-wider text-muted-foreground">{term}</dt>
      <dd className="m-0 text-sm">{children}</dd>
    </div>
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
function cancellable(job: DataJob): boolean {
  return ["queued", "running", "cancelling"].includes(job.state);
}
async function sha256Hex(bytes: ArrayBuffer): Promise<string> {
  return Array.from(new Uint8Array(await crypto.subtle.digest("SHA-256", bytes)))
    .map((byte) => byte.toString(16).padStart(2, "0"))
    .join("");
}
