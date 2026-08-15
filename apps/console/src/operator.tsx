import { type FormEvent, type ReactNode, useState } from "react";

import type {
  AbuseResponse,
  CreateAbuseResponseRequest,
  CreateQuotaOverrideRequest,
  OperatorProjectView,
  ProvisioningWorkflow,
  QuotaOverride,
  SupportPermission,
  SupportSession,
} from "@mako-cloud/management-sdk";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useOperatorAuth } from "./operator-auth.js";
import { useOperatorClient } from "./operator-management.js";
import { OperatorWaitListPanel } from "./operator-waitlist.js";
import { LifecycleBadge } from "./projects.js";
import { confirmDestructiveAction } from "./safety.js";

const QUOTA_RESOURCES: readonly CreateQuotaOverrideRequest["resource"][] = [
  "environments",
  "collections_per_environment",
  "storage_bytes",
  "replication_requests_per_minute",
  "replication_bytes_per_month",
  "application_users",
  "edge_functions",
  "edge_invocations_per_month",
  "edge_compute_milliseconds_per_month",
  "log_bytes_per_month",
];

const SUPPORT_PERMISSIONS: readonly SupportPermission[] = [
  "project_metadata_read",
  "application_user_read",
  "logs_read",
  "document_read",
];

export function RequireOperatorSession({ children }: { readonly children: ReactNode }) {
  const { state, signIn } = useOperatorAuth();
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  if (state.status === "authenticated") {
    return children;
  }
  if (state.status === "loading") {
    return <OperatorCentered title="Checking the separate operator session…" />;
  }
  if (state.status === "unconfigured") {
    return (
      <OperatorCentered title="Operator authentication is not configured">
        The console host must supply a separate operator authentication adapter. Developer sessions
        are intentionally not accepted on operator routes.
      </OperatorCentered>
    );
  }
  if (state.status === "error") {
    return (
      <OperatorCentered title="Operator authentication unavailable">
        {state.message}
      </OperatorCentered>
    );
  }
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const submittedPassword = password;
    setPassword("");
    setPending(true);
    setFailure(null);
    try {
      await signIn(email.trim(), submittedPassword);
    } catch (error) {
      setFailure(
        error instanceof Error && error.message.length <= 512
          ? error.message
          : "Operator authentication is unavailable.",
      );
      setPending(false);
    }
  };
  return (
    <OperatorCentered title="Sign in as a platform operator">
      <p>
        Use the email and password for an active developer identity with a separate operator
        entitlement. Ordinary developer and wait-list sessions are never accepted here.
      </p>
      {failure === null ? null : <p role="alert">{failure}</p>}
      <form onSubmit={(event) => void submit(event)}>
        <label>
          Email
          <input
            type="email"
            autoComplete="username"
            value={email}
            required
            maxLength={320}
            onChange={(event) => setEmail(event.currentTarget.value)}
          />
        </label>
        <label>
          Password
          <input
            type="password"
            autoComplete="current-password"
            value={password}
            required
            maxLength={1024}
            onChange={(event) => setPassword(event.currentTarget.value)}
          />
        </label>
        <p>The protected operator session expires within one hour.</p>
        <button type="submit" disabled={pending || email.trim() === "" || password === ""}>
          {pending ? "Signing in…" : "Open operator console"}
        </button>
      </form>
    </OperatorCentered>
  );
}

export function OperatorConsoleScreen({ onExit }: { readonly onExit: () => void }) {
  const client = useOperatorClient();
  const { state, signOut } = useOperatorAuth();
  const [projectId, setProjectId] = useState("");
  const [view, setView] = useState<OperatorProjectView | null>(null);
  const [workflow, setWorkflow] = useState<ProvisioningWorkflow | null>(null);
  const [quotaOverride, setQuotaOverride] = useState<QuotaOverride | null>(null);
  const [abuseResponse, setAbuseResponse] = useState<AbuseResponse | null>(null);
  const [supportSession, setSupportSession] = useState<SupportSession | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const permissions = new Set(state.status === "authenticated" ? state.session.permissions : []);

  const lookup = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    const nextProjectId = requiredText(data, "projectId");
    try {
      setView(await client.getOperatorProject(nextProjectId));
      setProjectId(nextProjectId);
      setFailure(null);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  };

  return (
    <div className="operator-shell">
      <a className="skip-link" href="#operator-main-content">
        Skip to operator actions
      </a>
      <header className="operator-topbar">
        <div>
          <p className="eyebrow">Restricted surface</p>
          <strong>Mako Cloud Operator Console</strong>
        </div>
        <div className="account">
          <span>{state.status === "authenticated" ? state.session.profile.email : ""}</span>
          <button type="button" className="secondary" onClick={onExit}>
            Exit operator console
          </button>
          <button type="button" onClick={() => void signOut()}>
            Sign out
          </button>
        </div>
      </header>
      <main className="workspace" id="operator-main-content" tabIndex={-1}>
        <section aria-labelledby="operator-title">
          <div className="section-heading">
            <div>
              <p className="eyebrow">Audited administration</p>
              <h1 id="operator-title">Tenant operations</h1>
            </div>
          </div>
          <p className="notice warning">
            Every action on this surface requires a case-quality reason and is recorded against the
            separate operator identity.
          </p>
          <ApiFailureNotice failure={failure} />
          {permissions.has("waitlist_review") ? <OperatorWaitListPanel /> : null}
          {permissions.has("tenant_read") ? (
            <section className="panel full-span" aria-labelledby="tenant-lookup-title">
              <h2 id="tenant-lookup-title">Tenant lookup</h2>
              <form className="inline-form" onSubmit={(event) => void lookup(event)}>
                <label>
                  Exact project ID
                  <input name="projectId" required pattern="prj_[A-Za-z0-9_-]{8,64}" />
                </label>
                <button type="submit">Open operator-safe view</button>
              </form>
              {view === null ? null : <OperatorProjectSummary view={view} />}
            </section>
          ) : (
            <p className="notice">This operator entitlement does not include tenant lookup.</p>
          )}
          {!permissions.has("tenant_read") ? null : view === null ? (
            <p>Select a project before using scoped operator actions.</p>
          ) : (
            <div className="split-grid stacked-section">
              {permissions.has("provisioning_repair") ? (
                <RepairPanel
                  onValidationError={(message) => setFailure({ message, requestId: null })}
                  onSubmit={async (workflowId, action, reason) => {
                    try {
                      setWorkflow(
                        await client.repairOperatorProvisioning(projectId, workflowId, {
                          action,
                          reason,
                        }),
                      );
                      setFailure(null);
                    } catch (error) {
                      setFailure(toConsoleApiFailure(error));
                    }
                  }}
                  workflow={workflow}
                />
              ) : null}
              {permissions.has("quota_override") ? (
                <QuotaOverridePanel
                  result={quotaOverride}
                  onValidationError={(message) => setFailure({ message, requestId: null })}
                  onSubmit={async (input) => {
                    try {
                      setQuotaOverride(await client.createOperatorQuotaOverride(projectId, input));
                      setFailure(null);
                    } catch (error) {
                      setFailure(toConsoleApiFailure(error));
                    }
                  }}
                />
              ) : null}
              {permissions.has("abuse_response") ? (
                <AbuseResponsePanel
                  environments={view.environments.map((environment) => environment.id)}
                  result={abuseResponse}
                  onValidationError={(message) => setFailure({ message, requestId: null })}
                  onSubmit={async (input) => {
                    try {
                      setAbuseResponse(await client.createOperatorAbuseResponse(projectId, input));
                      setFailure(null);
                    } catch (error) {
                      setFailure(toConsoleApiFailure(error));
                    }
                  }}
                />
              ) : null}
              {permissions.has("support_access") ? (
                <SupportSessionPanel
                  environments={view.environments.map((environment) => environment.id)}
                  session={supportSession}
                  onValidationError={(message) => setFailure({ message, requestId: null })}
                  onCreate={async (input) => {
                    try {
                      setSupportSession(await client.createSupportSession(projectId, input));
                      setFailure(null);
                    } catch (error) {
                      setFailure(toConsoleApiFailure(error));
                    }
                  }}
                  onRevoke={async (sessionId, reason) => {
                    try {
                      setSupportSession(
                        await client.revokeSupportSession(projectId, sessionId, reason),
                      );
                      setFailure(null);
                    } catch (error) {
                      setFailure(toConsoleApiFailure(error));
                    }
                  }}
                />
              ) : null}
            </div>
          )}
        </section>
      </main>
    </div>
  );
}

function OperatorProjectSummary({ view }: { readonly view: OperatorProjectView }) {
  return (
    <div className="stacked-section">
      <div className="button-row spread">
        <div>
          <strong>{view.project.name}</strong>
          <p>
            <code>{view.project.id}</code> · {view.project.region}
          </p>
        </div>
        <LifecycleBadge state={view.project.state} />
      </div>
      <ul className="signal-list">
        {view.environments.map((environment) => (
          <li key={environment.id}>
            <strong>{environment.name}</strong> <code>{environment.id}</code>{" "}
            <LifecycleBadge state={environment.state} />
          </li>
        ))}
      </ul>
    </div>
  );
}

export function RepairPanel({
  workflow,
  onSubmit,
  onValidationError,
}: {
  readonly workflow: ProvisioningWorkflow | null;
  readonly onSubmit: (
    workflowId: string,
    action: "requeue" | "retry_compensation",
    reason: string,
  ) => Promise<void>;
  readonly onValidationError: (message: string) => void;
}) {
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    try {
      const data = new FormData(event.currentTarget);
      void onSubmit(
        requiredText(data, "workflowId"),
        requiredText(data, "action") as "requeue" | "retry_compensation",
        requiredReason(data),
      );
    } catch (error) {
      onValidationError(validationMessage(error));
    }
  };
  return (
    <section className="panel" aria-labelledby="repair-title">
      <h2 id="repair-title">Provisioning repair</h2>
      <form onSubmit={submit}>
        <label>
          Workflow ID
          <input name="workflowId" required minLength={8} />
        </label>
        <label>
          Repair action
          <select name="action">
            <option value="requeue">Requeue workflow</option>
            <option value="retry_compensation">Retry compensation</option>
          </select>
        </label>
        <ReasonField />
        <button type="submit">Apply reasoned repair</button>
      </form>
      {workflow === null ? null : <Result value={workflow} label="Provisioning workflow updated" />}
    </section>
  );
}

export function QuotaOverridePanel({
  result,
  onSubmit,
  onValidationError,
}: {
  readonly result: QuotaOverride | null;
  readonly onSubmit: (input: CreateQuotaOverrideRequest) => Promise<void>;
  readonly onValidationError: (message: string) => void;
}) {
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    try {
      const data = new FormData(event.currentTarget);
      const expiry = optionalText(data, "expiresAt");
      void onSubmit({
        id: requiredText(data, "id"),
        resource: requiredText(data, "resource") as CreateQuotaOverrideRequest["resource"],
        limit: positiveInteger(data, "limit"),
        reason: requiredReason(data),
        expiresAt: expiry === null ? null : new Date(expiry).toISOString(),
      });
    } catch (error) {
      onValidationError(validationMessage(error));
    }
  };
  return (
    <section className="panel" aria-labelledby="quota-override-title">
      <h2 id="quota-override-title">Quota override</h2>
      <form onSubmit={submit}>
        <label>
          Override ID
          <input name="id" required pattern="qov_[A-Za-z0-9_-]{8,96}" defaultValue={newId("qov")} />
        </label>
        <label>
          Resource
          <select name="resource">
            {QUOTA_RESOURCES.map((resource) => (
              <option key={resource} value={resource}>
                {resource.replaceAll("_", " ")}
              </option>
            ))}
          </select>
        </label>
        <label>
          Limit
          <input name="limit" type="number" min="1" required />
        </label>
        <label>
          Expires at (blank means no expiry)
          <input name="expiresAt" type="datetime-local" />
        </label>
        <ReasonField />
        <button type="submit">Create audited override</button>
      </form>
      {result === null ? null : <Result value={result} label="Quota override created" />}
    </section>
  );
}

export function AbuseResponsePanel({
  environments,
  result,
  onSubmit,
  onValidationError,
}: {
  readonly environments: readonly string[];
  readonly result: AbuseResponse | null;
  readonly onSubmit: (input: CreateAbuseResponseRequest) => Promise<void>;
  readonly onValidationError: (message: string) => void;
}) {
  const [target, setTarget] = useState("project");
  const submit = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    try {
      const data = new FormData(event.currentTarget);
      const input: CreateAbuseResponseRequest = {
        id: requiredText(data, "id"),
        target:
          target === "project"
            ? { kind: "project" }
            : { kind: "environment", environmentId: requiredText(data, "environmentId") },
        reason: requiredReason(data),
      };
      if (
        confirmDestructiveAction({
          action: "Suspend",
          target: target === "project" ? "the entire project" : "the selected environment",
          consequence:
            "Tenant application traffic will be interrupted immediately as an abuse response.",
        })
      ) {
        void onSubmit(input);
      }
    } catch (error) {
      onValidationError(validationMessage(error));
    }
  };
  return (
    <section className="panel" aria-labelledby="abuse-response-title">
      <h2 id="abuse-response-title">Abuse response</h2>
      <p>This immediately suspends the selected project or environment.</p>
      <form onSubmit={submit}>
        <label>
          Response ID
          <input name="id" required pattern="abr_[A-Za-z0-9_-]{8,96}" defaultValue={newId("abr")} />
        </label>
        <label>
          Target scope
          <select value={target} onChange={(event) => setTarget(event.currentTarget.value)}>
            <option value="project">Entire project</option>
            <option value="environment">One environment</option>
          </select>
        </label>
        {target === "project" ? null : (
          <label>
            Environment
            <select name="environmentId">
              {environments.map((environmentId) => (
                <option key={environmentId} value={environmentId}>
                  {environmentId}
                </option>
              ))}
            </select>
          </label>
        )}
        <ReasonField />
        <button type="submit" className="danger">
          Suspend scope
        </button>
      </form>
      {result === null ? null : <Result value={result} label="Abuse response applied" />}
    </section>
  );
}

export function SupportSessionPanel({
  environments,
  session,
  onCreate,
  onRevoke,
  onValidationError,
}: {
  readonly environments: readonly string[];
  readonly session: SupportSession | null;
  readonly onCreate: (input: {
    id: string;
    environmentId: string | null;
    permissions: SupportPermission[];
    reason: string;
    expiresAt: string;
  }) => Promise<void>;
  readonly onRevoke: (sessionId: string, reason: string) => Promise<void>;
  readonly onValidationError: (message: string) => void;
}) {
  const create = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    try {
      const data = new FormData(event.currentTarget);
      const expiresAt = new Date(requiredText(data, "expiresAt"));
      if (
        Number.isNaN(expiresAt.getTime()) ||
        expiresAt.getTime() <= Date.now() ||
        expiresAt.getTime() > Date.now() + 8 * 60 * 60 * 1_000
      ) {
        throw new Error("Support access must expire within the next eight hours.");
      }
      const permissions = data
        .getAll("permissions")
        .map(String)
        .filter((value): value is SupportPermission =>
          SUPPORT_PERMISSIONS.includes(value as SupportPermission),
        );
      if (permissions.length === 0) {
        throw new Error("Select at least one support permission.");
      }
      void onCreate({
        id: requiredText(data, "id"),
        environmentId: optionalText(data, "environmentId"),
        permissions,
        reason: requiredReason(data),
        expiresAt: expiresAt.toISOString(),
      });
    } catch (error) {
      onValidationError(validationMessage(error));
    }
  };
  const revoke = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (session === null) {
      return;
    }
    try {
      const reason = requiredReason(new FormData(event.currentTarget));
      if (
        confirmDestructiveAction({
          action: "Revoke",
          target: `support session ${session.id}`,
          consequence: "Any further support access using this session will be denied immediately.",
        })
      ) {
        void onRevoke(session.id, reason);
      }
    } catch (error) {
      onValidationError(validationMessage(error));
    }
  };
  return (
    <section className="panel" aria-labelledby="support-session-title">
      <h2 id="support-session-title">Time-bounded support access</h2>
      <form onSubmit={create}>
        <label>
          Session ID
          <input name="id" required pattern="sup_[A-Za-z0-9_-]{8,96}" defaultValue={newId("sup")} />
        </label>
        <label>
          Environment scope (blank means project metadata only)
          <select name="environmentId" defaultValue="">
            <option value="">No environment</option>
            {environments.map((environmentId) => (
              <option key={environmentId} value={environmentId}>
                {environmentId}
              </option>
            ))}
          </select>
        </label>
        <fieldset className="checkbox-grid">
          <legend>Least-privilege permissions</legend>
          {SUPPORT_PERMISSIONS.map((permission) => (
            <label key={permission}>
              <input type="checkbox" name="permissions" value={permission} />
              {permission.replaceAll("_", " ")}
            </label>
          ))}
        </fieldset>
        <label>
          Expires at (maximum eight hours)
          <input name="expiresAt" type="datetime-local" required />
        </label>
        <ReasonField />
        <button type="submit">Create support session</button>
      </form>
      {session === null ? null : (
        <div className="notice success" role="status">
          <strong>Support session {session.state}</strong>
          <p>
            <code>{session.id}</code> expires {new Date(session.expiresAt).toLocaleString()}.
          </p>
          {session.state === "active" ? (
            <form onSubmit={revoke}>
              <ReasonField label="Revocation reason" />
              <button type="submit" className="danger">
                Revoke support session
              </button>
            </form>
          ) : null}
        </div>
      )}
    </section>
  );
}

function ReasonField({
  label = "Required operator reason / case reference",
}: {
  readonly label?: string;
}) {
  return (
    <label>
      {label}
      <textarea name="reason" required minLength={8} maxLength={1024} />
    </label>
  );
}

function Result({ value, label }: { readonly value: unknown; readonly label: string }) {
  return (
    <details className="notice success">
      <summary>{label}</summary>
      <pre className="json-preview">{JSON.stringify(value, null, 2)}</pre>
    </details>
  );
}

function OperatorCentered({
  title,
  children,
}: {
  readonly title: string;
  readonly children?: ReactNode;
}) {
  return (
    <main className="centered operator-shell">
      <section className="panel">
        <p className="eyebrow">Restricted operator route</p>
        <h1>{title}</h1>
        {children}
      </section>
    </main>
  );
}

function requiredReason(data: FormData): string {
  const reason = requiredText(data, "reason");
  if (reason.length < 8 || reason.length > 1_024) {
    throw new Error("The operator reason must contain 8 to 1,024 characters.");
  }
  return reason;
}

function requiredText(data: FormData, name: string): string {
  const value = String(data.get(name) ?? "").trim();
  if (value === "") {
    throw new Error(`${name} is required.`);
  }
  return value;
}

function optionalText(data: FormData, name: string): string | null {
  const value = String(data.get(name) ?? "").trim();
  return value === "" ? null : value;
}

function positiveInteger(data: FormData, name: string): number {
  const value = Number(requiredText(data, name));
  if (!Number.isSafeInteger(value) || value <= 0) {
    throw new Error(`${name} must be a positive integer.`);
  }
  return value;
}

function newId(prefix: "qov" | "abr" | "sup"): string {
  return `${prefix}_${globalThis.crypto.randomUUID().replaceAll("-", "")}`;
}

function validationMessage(error: unknown): string {
  return error instanceof Error && error.message.length <= 512
    ? error.message
    : "The operator action contains invalid input.";
}
