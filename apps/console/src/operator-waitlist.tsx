import { type FormEvent, useCallback, useEffect, useMemo, useRef, useState } from "react";

import type { DeveloperApplicant, DeveloperApplicantPage } from "@mako-cloud/management-sdk";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useOperatorClient } from "./operator-management.js";
import { confirmDestructiveAction } from "./safety.js";

const PAGE_SIZE = 25;

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
  const selectAllRef = useRef<HTMLInputElement>(null);

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

  useEffect(() => {
    if (selectAllRef.current !== null) {
      selectAllRef.current.indeterminate = someVisibleSelected;
    }
  }, [someVisibleSelected]);

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
    <section className="panel full-span" aria-labelledby="developer-waitlist-title">
      <div className="section-heading">
        <div>
          <p className="eyebrow">Explicit permission required</p>
          <h2 id="developer-waitlist-title">Applicants awaiting review</h2>
        </div>
        <button
          type="button"
          className="secondary"
          disabled={batchPending}
          onClick={() => void load(cursor)}
        >
          Refresh
        </button>
      </div>
      <p>
        Review verified developer identities. Approval activates only the developer identity; it
        does not change operator access or create a tenant, project, or membership.
      </p>
      <ApiFailureNotice failure={failure} />
      {batchResult === null ? null : (
        <p role="status">
          Batch approval finished: {batchResult.committed} committed, {batchResult.failed} failed.
        </p>
      )}
      <label className="search-field">
        Filter this page by applicant, email, or developer ID
        <input
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
      </label>
      {loading ? (
        <p aria-live="polite">Loading a bounded page of applicants…</p>
      ) : applicants.length === 0 ? (
        <p>No matching wait-listed applicants are on this page.</p>
      ) : (
        <>
          <section className="stacked-section notice" aria-labelledby="batch-approval-title">
            <div>
              <h3 id="batch-approval-title">Batch approve</h3>
              <p>
                Select up to {PAGE_SIZE} applicants on this page. Each approval commits
                independently.
              </p>
            </div>
            <label>
              Private review reason / case reference (optional)
              <textarea
                value={batchReason}
                maxLength={1_024}
                disabled={batchPending}
                onChange={(event) => {
                  setBatchReason(event.currentTarget.value);
                  setBatchValidation(null);
                }}
              />
            </label>
            {batchValidation === null ? null : <p role="alert">{batchValidation}</p>}
            <button
              type="button"
              disabled={batchPending || selectedApplicants.length === 0}
              onClick={() => void approveSelected()}
            >
              {batchPending
                ? "Approving selected…"
                : `Approve selected (${selectedApplicants.length})`}
            </button>
          </section>
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th scope="col">
                    <input
                      ref={selectAllRef}
                      type="checkbox"
                      checked={allVisibleSelected}
                      disabled={batchPending}
                      aria-label="Select all visible applicants for batch approval"
                      onChange={(event) => setAllVisibleSelected(event.currentTarget.checked)}
                    />
                  </th>
                  <th scope="col">Applicant</th>
                  <th scope="col">Email</th>
                  <th scope="col">Developer status</th>
                  <th scope="col">Operator access</th>
                  <th scope="col">Created</th>
                  <th scope="col">Action</th>
                </tr>
              </thead>
              <tbody>
                {applicants.map((applicant) => (
                  <tr key={applicant.developerIdentityId}>
                    <td>
                      <input
                        type="checkbox"
                        checked={batchSelection.has(applicant.developerIdentityId)}
                        disabled={batchPending}
                        aria-label={`Select ${applicant.email} for batch approval`}
                        onChange={(event) =>
                          setApplicantSelected(
                            applicant.developerIdentityId,
                            event.currentTarget.checked,
                          )
                        }
                      />
                    </td>
                    <td>
                      <strong>{applicant.displayName}</strong>
                      <br />
                      <code>{applicant.developerIdentityId}</code>
                    </td>
                    <td>{applicant.email}</td>
                    <td>{applicant.status}</td>
                    <td>{applicant.operatorEntitlementStatus}</td>
                    <td>{new Date(applicant.createdAt).toLocaleString()}</td>
                    <td>
                      <button
                        type="button"
                        className="secondary"
                        disabled={batchPending}
                        onClick={() => void openApplicant(applicant.developerIdentityId)}
                      >
                        Review
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        </>
      )}
      <nav className="button-row spread" aria-label="Wait-list pagination">
        <button
          type="button"
          className="secondary"
          disabled={previous.length === 0 || loading || batchPending}
          onClick={() => {
            const history = [...previous];
            const prior = history.pop();
            setPrevious(history);
            void load(prior);
          }}
        >
          Previous page
        </button>
        <span>Up to {PAGE_SIZE} applicants per page</span>
        <button
          type="button"
          className="secondary"
          disabled={page?.nextCursor == null || loading || batchPending}
          onClick={() => {
            setPrevious((history) => [...history, cursor]);
            void load(page?.nextCursor ?? undefined);
          }}
        >
          Next page
        </button>
      </nav>
      {selected === null ? null : (
        <ApplicantDecisionPanel
          applicant={selected}
          onClose={() => setSelected(null)}
          onDecision={decide}
        />
      )}
    </section>
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
    <aside className="stacked-section notice warning" aria-labelledby="applicant-detail-title">
      <div className="button-row spread">
        <div>
          <h3 id="applicant-detail-title">{applicant.displayName}</h3>
          <p>
            {applicant.email} · <code>{applicant.developerIdentityId}</code>
          </p>
        </div>
        <button type="button" className="secondary" onClick={onClose}>
          Close detail
        </button>
      </div>
      <dl className="definition-grid">
        <div>
          <dt>Developer status</dt>
          <dd>{applicant.status}</dd>
        </div>
        <div>
          <dt>Operator access</dt>
          <dd>{applicant.operatorEntitlementStatus}</dd>
        </div>
        <div>
          <dt>Email verified</dt>
          <dd>{applicant.emailVerified ? "yes" : "no"}</dd>
        </div>
        <div>
          <dt>Authorization epoch</dt>
          <dd>{applicant.authorizationEpoch}</dd>
        </div>
        <div>
          <dt>Last updated</dt>
          <dd>{new Date(applicant.updatedAt).toLocaleString()}</dd>
        </div>
      </dl>
      {applicant.status !== "waitlisted" ? (
        <p role="status">This applicant already has the authoritative status shown above.</p>
      ) : (
        <form onSubmit={submit}>
          <label>
            Private review reason / case reference (optional)
            <textarea name="reason" maxLength={1_024} />
          </label>
          <fieldset>
            <legend>Decision</legend>
            <label className="checkbox-label">
              <input type="radio" name="action" value="approve" required /> Approve developer
            </label>
            <label className="checkbox-label">
              <input type="radio" name="action" value="reject" required /> Reject developer
            </label>
          </fieldset>
          {validation === null ? null : <p role="alert">{validation}</p>}
          <button type="submit" disabled={pending}>
            {pending ? "Committing decision…" : "Review and confirm decision"}
          </button>
        </form>
      )}
    </aside>
  );
}
