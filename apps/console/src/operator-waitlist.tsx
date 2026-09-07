import {
  Alert,
  AlertDescription,
  Badge,
  Button,
  Card,
  CardAction,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  Checkbox,
  EmptyState,
  Eyebrow,
  Field,
  Input,
  Label,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  Textarea,
} from "@mako-cloud/ui";
import { ChevronLeft, ChevronRight, RefreshCw, Users, X } from "lucide-react";
import {
  type FormEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useId,
  useMemo,
  useState,
} from "react";

import type { DeveloperApplicant, DeveloperApplicantPage } from "@mako-cloud/management-sdk";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useOperatorClient } from "./operator-management.js";
import { confirmDestructiveAction } from "./safety.js";

const PAGE_SIZE = 25;

/**
 * A native radio dressed by the kit's `Input`: the kit has no radio group of
 * its own, and the decision must stay a real radio so the form submits it and
 * the browser enforces `required`.
 */
const RADIO_CLASS = "size-4 shrink-0 rounded-full border-0 p-0 shadow-none accent-primary";

export function OperatorWaitListPanel() {
  const client = useOperatorClient();
  const [page, setPage] = useState<DeveloperApplicantPage | null>(null);
  const [cursor, setCursor] = useState<string | undefined>();
  const [previous, setPrevious] = useState<(string | undefined)[]>([]);
  const [query, setQuery] = useState("");
  const [selected, setSelected] = useState<DeveloperApplicant | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [loading, setLoading] = useState(true);
  const [batchSelection, setBatchSelection] = useState<ReadonlySet<string>>(() => new Set());
  const [batchReason, setBatchReason] = useState("");
  const [batchPending, setBatchPending] = useState(false);
  const [batchValidation, setBatchValidation] = useState<string | null>(null);
  const [batchResult, setBatchResult] = useState<{
    readonly committed: number;
    readonly failed: number;
  } | null>(null);

  const load = useCallback(
    async (nextCursor: string | undefined) => {
      setLoading(true);
      setBatchSelection(new Set());
      setBatchValidation(null);
      setBatchResult(null);
      try {
        setPage(
          await client.listDeveloperWaitList(
            nextCursor === undefined
              ? { limit: PAGE_SIZE }
              : { cursor: nextCursor, limit: PAGE_SIZE },
          ),
        );
        setCursor(nextCursor);
        setFailure(null);
      } catch (error) {
        setFailure(toConsoleApiFailure(error));
      } finally {
        setLoading(false);
      }
    },
    [client],
  );

  useEffect(() => {
    void load(undefined);
  }, [load]);

  const applicants = useMemo(() => {
    const normalized = query.trim().toLocaleLowerCase();
    const values = page?.applicants ?? [];
    if (normalized === "") return values;
    return values.filter(
      (applicant) =>
        applicant.email.toLocaleLowerCase().includes(normalized) ||
        applicant.displayName.toLocaleLowerCase().includes(normalized) ||
        applicant.developerIdentityId.toLocaleLowerCase().includes(normalized),
    );
  }, [page, query]);

  const selectedApplicants = useMemo(
    () => applicants.filter((applicant) => batchSelection.has(applicant.developerIdentityId)),
    [applicants, batchSelection],
  );
  const allVisibleSelected =
    applicants.length > 0 && selectedApplicants.length === applicants.length;
  const someVisibleSelected = selectedApplicants.length > 0 && !allVisibleSelected;

  const openApplicant = async (identityId: string) => {
    try {
      setSelected(await client.getDeveloperWaitListApplicant(identityId));
      setFailure(null);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  };

  const decide = async (action: "approve" | "reject", reason: string) => {
    if (selected === null) return;
    const target = `${selected.displayName} (${selected.email})`;
    if (
      !confirmDestructiveAction({
        action: action === "approve" ? "Approve" : "Reject",
        target,
        consequence:
          action === "approve"
            ? "The account becomes active, pending sessions are revoked, and a fresh sign-in is required. No team membership is created."
            : "The account becomes rejected and all developer sessions are revoked.",
      })
    ) {
      return;
    }
    const idempotencyKey = `waitlist-${action}-${globalThis.crypto.randomUUID()}`;
    try {
      const committed =
        action === "approve"
          ? await client.approveDeveloperWaitListApplicant(
              selected.developerIdentityId,
              reason,
              idempotencyKey,
            )
          : await client.rejectDeveloperWaitListApplicant(
              selected.developerIdentityId,
              reason,
              idempotencyKey,
            );
      setSelected(committed);
      setFailure(null);
      await load(cursor);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
      // A concurrent reviewer may have committed first. The detail read is
      // authoritative and never infers lifecycle from notification delivery.
      try {
        setSelected(await client.getDeveloperWaitListApplicant(selected.developerIdentityId));
      } catch {
        // Retain the original correlated API failure.
      }
    }
  };

  const setApplicantSelected = (identityId: string, checked: boolean) => {
    setBatchResult(null);
    setBatchValidation(null);
    setBatchSelection((current) => {
      const next = new Set(current);
      if (checked) next.add(identityId);
      else next.delete(identityId);
      return next;
    });
  };

  const setAllVisibleSelected = (checked: boolean) => {
    setBatchResult(null);
    setBatchValidation(null);
    setBatchSelection(
      checked ? new Set(applicants.map((applicant) => applicant.developerIdentityId)) : new Set(),
    );
  };

  const approveSelected = async () => {
    const reason = batchReason.trim();
    if (selectedApplicants.length === 0 || selectedApplicants.length > PAGE_SIZE) {
      setBatchValidation(`Select between 1 and ${PAGE_SIZE} visible applicants.`);
      return;
    }
    if (reason !== "" && !(8 <= reason.length && reason.length <= 1_024)) {
      setBatchValidation("The optional private reason must contain 8 to 1,024 characters.");
      return;
    }
    if (
      !confirmDestructiveAction({
        action: "Approve selected",
        target: `${selectedApplicants.length} visible wait-listed developer account${selectedApplicants.length === 1 ? "" : "s"}`,
        consequence:
          "Each account is approved independently. Valid approvals remain committed if another selected account fails, and no team membership is created.",
      })
    ) {
      return;
    }

    const batchId = globalThis.crypto.randomUUID();
    let committed = 0;
    let failed = 0;
    let firstFailure: ConsoleApiFailure | null = null;
    setBatchPending(true);
    setBatchValidation(null);
    setBatchResult(null);
    setSelected(null);
    try {
      for (const [index, applicant] of selectedApplicants.entries()) {
        try {
          await client.approveDeveloperWaitListApplicant(
            applicant.developerIdentityId,
            reason === "" ? undefined : reason,
            `waitlist-batch-approve-${batchId}-${index + 1}`,
          );
          committed += 1;
        } catch (error) {
          failed += 1;
          firstFailure ??= toConsoleApiFailure(error);
        }
      }
      await load(cursor);
      setFailure(firstFailure);
      setBatchResult({ committed, failed });
    } finally {
      setBatchPending(false);
    }
  };

  return (
    <Card aria-labelledby="developer-waitlist-title">
      <CardHeader>
        <Eyebrow>Explicit permission required</Eyebrow>
        <CardTitle id="developer-waitlist-title">Applicants awaiting review</CardTitle>
        <CardDescription>
          Review verified developer identities. Approval activates only the developer identity; it
          does not change operator access or create a tenant, project, or membership.
        </CardDescription>
        <CardAction>
          <Button
            variant="outline"
            size="sm"
            disabled={batchPending}
            onClick={() => void load(cursor)}
          >
            <RefreshCw aria-hidden="true" />
            Refresh
          </Button>
        </CardAction>
      </CardHeader>
      <CardContent className="grid gap-4">
        <ApiFailureNotice failure={failure} />
        {batchResult === null ? null : (
          <Alert role="status">
            <AlertDescription className="block text-foreground">
              Batch approval finished: {batchResult.committed} committed, {batchResult.failed}{" "}
              failed.
            </AlertDescription>
          </Alert>
        )}
        <Field
          label="Filter this page by applicant, email, or developer ID"
          htmlFor="operator-waitlist-filter"
          className="max-w-xl"
        >
          <Input
            id="operator-waitlist-filter"
            type="search"
            value={query}
            maxLength={200}
            disabled={batchPending}
            onChange={(event) => {
              setQuery(event.currentTarget.value);
              setBatchSelection(new Set());
              setBatchValidation(null);
              setBatchResult(null);
            }}
          />
        </Field>
        {loading ? (
          <p aria-live="polite" className="m-0 text-sm text-muted-foreground">
            Loading a bounded page of applicants…
          </p>
        ) : applicants.length === 0 ? (
          <EmptyState
            icon={<Users aria-hidden="true" />}
            title="No matching wait-listed applicants are on this page."
          />
        ) : (
          <>
            <Card
              aria-labelledby="batch-approval-title"
              className="gap-4 bg-muted/30 py-4 shadow-none"
            >
              <CardHeader>
                <CardTitle as="h3" id="batch-approval-title">
                  Batch approve
                </CardTitle>
                <CardDescription>
                  Select up to {PAGE_SIZE} applicants on this page. Each approval commits
                  independently.
                </CardDescription>
              </CardHeader>
              <CardContent className="grid gap-4">
                <Field
                  label="Private review reason / case reference (optional)"
                  htmlFor="operator-waitlist-batch-reason"
                >
                  <Textarea
                    id="operator-waitlist-batch-reason"
                    value={batchReason}
                    maxLength={1_024}
                    disabled={batchPending}
                    onChange={(event) => {
                      setBatchReason(event.currentTarget.value);
                      setBatchValidation(null);
                    }}
                  />
                </Field>
                {batchValidation === null ? null : (
                  <Alert variant="destructive">
                    <AlertDescription className="block">{batchValidation}</AlertDescription>
                  </Alert>
                )}
                <div>
                  <Button
                    disabled={batchPending || selectedApplicants.length === 0}
                    onClick={() => void approveSelected()}
                  >
                    {batchPending
                      ? "Approving selected…"
                      : `Approve selected (${selectedApplicants.length})`}
                  </Button>
                </div>
              </CardContent>
            </Card>
            <div className="overflow-hidden rounded-lg border">
              <Table>
                <TableHeader>
                  <TableRow className="hover:bg-transparent">
                    <TableHead scope="col" className="w-10 pl-3">
                      <Checkbox
                        checked={someVisibleSelected ? "indeterminate" : allVisibleSelected}
                        disabled={batchPending}
                        aria-label="Select all visible applicants for batch approval"
                        onCheckedChange={(checked) => setAllVisibleSelected(checked === true)}
                      />
                    </TableHead>
                    <TableHead scope="col">Applicant</TableHead>
                    <TableHead scope="col">Email</TableHead>
                    <TableHead scope="col">Developer status</TableHead>
                    <TableHead scope="col">Operator access</TableHead>
                    <TableHead scope="col">Created</TableHead>
                    <TableHead scope="col" className="text-right">
                      Action
                    </TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {applicants.map((applicant) => {
                    const isSelected = batchSelection.has(applicant.developerIdentityId);
                    return (
                      <TableRow
                        key={applicant.developerIdentityId}
                        data-state={isSelected ? "selected" : undefined}
                      >
                        <TableCell className="w-10 pl-3">
                          <Checkbox
                            checked={isSelected}
                            disabled={batchPending}
                            aria-label={`Select ${applicant.email} for batch approval`}
                            onCheckedChange={(checked) =>
                              setApplicantSelected(applicant.developerIdentityId, checked === true)
                            }
                          />
                        </TableCell>
                        <TableCell>
                          <div className="grid gap-0.5">
                            <strong className="font-medium">{applicant.displayName}</strong>
                            <code className="font-mono text-xs text-muted-foreground">
                              {applicant.developerIdentityId}
                            </code>
                          </div>
                        </TableCell>
                        <TableCell>{applicant.email}</TableCell>
                        <TableCell>
                          <StatusBadge value={applicant.status} />
                        </TableCell>
                        <TableCell>
                          <StatusBadge value={applicant.operatorEntitlementStatus} />
                        </TableCell>
                        <TableCell className="text-muted-foreground tabular-nums">
                          {new Date(applicant.createdAt).toLocaleString()}
                        </TableCell>
                        <TableCell className="text-right">
                          <Button
                            variant="outline"
                            size="sm"
                            disabled={batchPending}
                            onClick={() => void openApplicant(applicant.developerIdentityId)}
                          >
                            Review
                          </Button>
                        </TableCell>
                      </TableRow>
                    );
                  })}
                </TableBody>
              </Table>
            </div>
          </>
        )}
        <nav
          className="flex flex-wrap items-center justify-between gap-2"
          aria-label="Wait-list pagination"
        >
          <Button
            variant="outline"
            size="sm"
            disabled={previous.length === 0 || loading || batchPending}
            onClick={() => {
              const history = [...previous];
              const prior = history.pop();
              setPrevious(history);
              void load(prior);
            }}
          >
            <ChevronLeft aria-hidden="true" />
            Previous page
          </Button>
          <span className="text-xs text-muted-foreground">
            Up to {PAGE_SIZE} applicants per page
          </span>
          <Button
            variant="outline"
            size="sm"
            disabled={page?.nextCursor == null || loading || batchPending}
            onClick={() => {
              setPrevious((history) => [...history, cursor]);
              void load(page?.nextCursor ?? undefined);
            }}
          >
            Next page
            <ChevronRight aria-hidden="true" />
          </Button>
        </nav>
        {selected === null ? null : (
          <ApplicantDecisionPanel
            applicant={selected}
            onClose={() => setSelected(null)}
            onDecision={decide}
          />
        )}
      </CardContent>
    </Card>
  );
}

function ApplicantDecisionPanel({
  applicant,
  onClose,
  onDecision,
}: {
  readonly applicant: DeveloperApplicant;
  readonly onClose: () => void;
  readonly onDecision: (action: "approve" | "reject", reason: string) => Promise<void>;
}) {
  const id = useId();
  const [pending, setPending] = useState(false);
  const [validation, setValidation] = useState<string | null>(null);
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    const action = data.get("action");
    const reason = String(data.get("reason") ?? "").trim();
    if (action !== "approve" && action !== "reject") {
      setValidation("Choose a decision.");
      return;
    }
    if (reason !== "" && !(8 <= reason.length && reason.length <= 1_024)) {
      setValidation("The optional private reason must contain 8 to 1,024 characters.");
      return;
    }
    setValidation(null);
    setPending(true);
    void onDecision(action, reason).finally(() => setPending(false));
  };
  return (
    <aside
      className="grid gap-5 rounded-xl border border-warning/40 bg-warning/5 p-5"
      aria-labelledby="applicant-detail-title"
    >
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="grid gap-1">
          <h3 id="applicant-detail-title" className="text-base leading-none">
            {applicant.displayName}
          </h3>
          <p className="m-0 text-sm text-muted-foreground">
            {applicant.email} ·{" "}
            <code className="font-mono text-xs">{applicant.developerIdentityId}</code>
          </p>
        </div>
        <Button variant="outline" size="sm" onClick={onClose}>
          <X aria-hidden="true" />
          Close detail
        </Button>
      </div>
      <dl className="m-0 grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
        <Definition term="Developer status">
          <StatusBadge value={applicant.status} />
        </Definition>
        <Definition term="Operator access">
          <StatusBadge value={applicant.operatorEntitlementStatus} />
        </Definition>
        <Definition term="Email verified">{applicant.emailVerified ? "yes" : "no"}</Definition>
        <Definition term="Authorization epoch">
          <span className="tabular-nums">{applicant.authorizationEpoch}</span>
        </Definition>
        <Definition term="Last updated">
          {new Date(applicant.updatedAt).toLocaleString()}
        </Definition>
      </dl>
      {applicant.status !== "waitlisted" ? (
        <p role="status" className="m-0 text-sm text-muted-foreground">
          This applicant already has the authoritative status shown above.
        </p>
      ) : (
        <form className="grid gap-4" onSubmit={submit}>
          <Field label="Private review reason / case reference (optional)" htmlFor={`${id}-reason`}>
            <Textarea id={`${id}-reason`} name="reason" maxLength={1_024} />
          </Field>
          <fieldset className="m-0 grid gap-2 border-0 p-0">
            <legend className="mb-2 p-0 text-sm font-medium leading-none">Decision</legend>
            <div className="flex items-center gap-2">
              <Input
                id={`${id}-approve`}
                type="radio"
                name="action"
                value="approve"
                required
                className={RADIO_CLASS}
              />
              <Label htmlFor={`${id}-approve`} className="font-normal">
                Approve developer
              </Label>
            </div>
            <div className="flex items-center gap-2">
              <Input
                id={`${id}-reject`}
                type="radio"
                name="action"
                value="reject"
                required
                className={RADIO_CLASS}
              />
              <Label htmlFor={`${id}-reject`} className="font-normal">
                Reject developer
              </Label>
            </div>
          </fieldset>
          {validation === null ? null : (
            <Alert variant="destructive">
              <AlertDescription className="block">{validation}</AlertDescription>
            </Alert>
          )}
          <div>
            <Button type="submit" disabled={pending}>
              {pending ? "Committing decision…" : "Review and confirm decision"}
            </Button>
          </div>
        </form>
      )}
    </aside>
  );
}

/** A lifecycle word as a badge: the value is the server's own, shown as it is. */
function StatusBadge({ value }: { readonly value: string }) {
  const variant =
    value === "active"
      ? "positive"
      : value === "waitlisted"
        ? "warning"
        : value === "rejected" || value === "disabled" || value === "deleted" || value === "revoked"
          ? "destructive"
          : "secondary";
  return <Badge variant={variant}>{value}</Badge>;
}

function Definition({ term, children }: { readonly term: string; readonly children: ReactNode }) {
  return (
    <div className="grid gap-1">
      <dt className="text-xs font-semibold tracking-wider text-muted-foreground uppercase">
        {term}
      </dt>
      <dd className="m-0 text-sm wrap-anywhere">{children}</dd>
    </div>
  );
}
