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
  Textarea,
} from "@mako-cloud/ui";
import { ChevronDown, CircleCheck, ShieldCheck } from "lucide-react";
import { type FormEvent, type ReactNode, useId, useState } from "react";

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
  "object_storage_bytes",
  "object_egress_bytes_per_month",
];

const SUPPORT_PERMISSIONS: readonly SupportPermission[] = [
  "project_metadata_read",
  "application_user_read",
  "logs_read",
  "document_read",
];

const SKIP_LINK_CLASS =
  "sr-only focus:not-sr-only focus:absolute focus:top-2 focus:left-2 focus:z-50 focus:rounded-md focus:bg-card focus:px-3 focus:py-2 focus:shadow-md";

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
        <p className="m-0 text-sm text-muted-foreground">
          The console host must supply a separate operator authentication adapter. Developer
          sessions are intentionally not accepted on operator routes.
        </p>
      </OperatorCentered>
    );
  }
  if (state.status === "error") {
    return (
      <OperatorCentered title="Operator authentication unavailable">
        <p className="m-0 text-sm text-muted-foreground">{state.message}</p>
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
      <p className="m-0 text-sm text-muted-foreground">
        Use the email and password for an active developer identity with a separate operator
        entitlement. Ordinary developer and wait-list sessions are never accepted here.
      </p>
      {failure === null ? null : (
        <Alert variant="destructive">
          <AlertDescription className="block">{failure}</AlertDescription>
        </Alert>
      )}
      <form className="grid gap-4" onSubmit={(event) => void submit(event)}>
        <Field label="Email" htmlFor="operator-sign-in-email">
          <Input
            id="operator-sign-in-email"
            type="email"
            autoComplete="username"
            value={email}
            required
            maxLength={320}
            onChange={(event) => setEmail(event.currentTarget.value)}
          />
        </Field>
        <Field label="Password" htmlFor="operator-sign-in-password">
          <Input
            id="operator-sign-in-password"
            type="password"
            autoComplete="current-password"
            value={password}
            required
            maxLength={1024}
            onChange={(event) => setPassword(event.currentTarget.value)}
          />
        </Field>
        <p className="m-0 text-xs text-muted-foreground">
          The protected operator session expires within one hour.
        </p>
        <Button
          type="submit"
          className="w-full"
          disabled={pending || email.trim() === "" || password === ""}
        >
          {pending ? "Signing in…" : "Open operator console"}
        </Button>
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
    <div className="flex min-h-screen flex-col bg-background text-foreground">
      <a className={SKIP_LINK_CLASS} href="#operator-main-content">
        Skip to operator actions
      </a>
      <header className="flex flex-wrap items-center gap-x-4 gap-y-2 border-b bg-card px-4 py-3 sm:px-6">
        <div className="flex min-w-0 items-center gap-3">
          <ShieldCheck aria-hidden="true" className="size-5 shrink-0 text-primary" />
          <div className="grid min-w-0 leading-tight">
            <Eyebrow>Restricted surface</Eyebrow>
            <strong className="truncate text-sm">Mako Cloud Operator Console</strong>
          </div>
          <Badge variant="outline" className="border-warning/50 bg-warning/10">
            Operator
          </Badge>
        </div>
        <div className="ml-auto flex flex-wrap items-center gap-2">
          <span className="max-w-56 truncate text-sm text-muted-foreground">
            {state.status === "authenticated" ? state.session.profile.email : ""}
          </span>
          <Button variant="outline" size="sm" onClick={onExit}>
            Exit operator console
          </Button>
          <Button size="sm" onClick={() => void signOut()}>
            Sign out
          </Button>
        </div>
      </header>
      <main
        className="min-w-0 flex-1 px-4 py-6 outline-none sm:px-6"
        id="operator-main-content"
        tabIndex={-1}
      >
        <section aria-labelledby="operator-title" className="grid gap-6">
          <div className="grid gap-1">
            <Eyebrow>Audited administration</Eyebrow>
            <h1 id="operator-title" className="text-2xl">
              Tenant operations
            </h1>
          </div>
          <Alert variant="warning" role="note">
            <AlertDescription className="block">
              Every action on this surface requires a case-quality reason and is recorded against
              the separate operator identity.
            </AlertDescription>
          </Alert>
          <ApiFailureNotice failure={failure} />
          {permissions.has("waitlist_review") ? <OperatorWaitListPanel /> : null}
          {permissions.has("tenant_read") ? (
            <Card aria-labelledby="tenant-lookup-title">
              <CardHeader>
                <CardTitle id="tenant-lookup-title">Tenant lookup</CardTitle>
              </CardHeader>
              <CardContent className="grid gap-4">
                <form
                  className="flex flex-wrap items-end gap-3"
                  onSubmit={(event) => void lookup(event)}
                >
                  <Field
                    label="Exact project ID"
                    htmlFor="operator-tenant-lookup-project"
                    className="min-w-0 flex-1 basis-64"
                  >
                    <Input
                      id="operator-tenant-lookup-project"
                      name="projectId"
                      required
                      pattern="prj_[A-Za-z0-9_-]{8,64}"
                      className="font-mono"
                    />
                  </Field>
                  <Button type="submit">Open operator-safe view</Button>
                </form>
                {view === null ? null : <OperatorProjectSummary view={view} />}
              </CardContent>
            </Card>
          ) : (
            <Alert role="note">
              <AlertDescription className="block">
                This operator entitlement does not include tenant lookup.
              </AlertDescription>
            </Alert>
          )}
          {!permissions.has("tenant_read") ? null : view === null ? (
            <p className="m-0 text-sm text-muted-foreground">
              Select a project before using scoped operator actions.
            </p>
          ) : (
            <div className="grid gap-4 lg:grid-cols-2">
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
    <div className="grid gap-3 border-t pt-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="grid gap-1">
          <strong>{view.project.name}</strong>
          <p className="m-0 text-sm text-muted-foreground">
            <code className="font-mono text-xs">{view.project.id}</code> · {view.project.region}
          </p>
        </div>
        <StateBadge state={view.project.state} />
      </div>
      <ul className="m-0 grid list-none gap-2 p-0">
        {view.environments.map((environment) => (
          <li key={environment.id} className="flex flex-wrap items-center gap-2 text-sm">
            <strong>{environment.name}</strong>{" "}
            <code className="font-mono text-xs text-muted-foreground">{environment.id}</code>{" "}
            <StateBadge state={environment.state} />
          </li>
        ))}
      </ul>
    </div>
  );
}

/** A project or environment lifecycle state, read out with its label. */
function StateBadge({ state }: { readonly state: string }) {
  const variant =
    state === "active"
      ? "positive"
      : state === "suspended" || state === "deleted" || state === "failed"
        ? "destructive"
        : state === "provisioning" || state === "pending" || state === "deleting"
          ? "warning"
          : "secondary";
  return (
    <Badge variant={variant}>
      <span className="sr-only">Status: </span>
      {state.replaceAll("_", " ")}
    </Badge>
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
  const id = useId();
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
    <Card aria-labelledby="repair-title">
      <CardHeader>
        <CardTitle id="repair-title">Provisioning repair</CardTitle>
      </CardHeader>
      <CardContent className="grid gap-4">
        <form className="grid gap-4" onSubmit={submit}>
          <Field label="Workflow ID" htmlFor={`${id}-workflow`}>
            <Input
              id={`${id}-workflow`}
              name="workflowId"
              required
              minLength={8}
              className="font-mono"
            />
          </Field>
          <Field label="Repair action" htmlFor={`${id}-action`}>
            <NativeSelect id={`${id}-action`} name="action">
              <option value="requeue">Requeue workflow</option>
              <option value="retry_compensation">Retry compensation</option>
            </NativeSelect>
          </Field>
          <ReasonField />
          <div>
            <Button type="submit">Apply reasoned repair</Button>
          </div>
        </form>
        {workflow === null ? null : (
          <Result value={workflow} label="Provisioning workflow updated" />
        )}
      </CardContent>
    </Card>
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
  const id = useId();
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
    <Card aria-labelledby="quota-override-title">
      <CardHeader>
        <CardTitle id="quota-override-title">Quota override</CardTitle>
      </CardHeader>
      <CardContent className="grid gap-4">
        <form className="grid gap-4" onSubmit={submit}>
          <Field label="Override ID" htmlFor={`${id}-id`}>
            <Input
              id={`${id}-id`}
              name="id"
              required
              pattern="qov_[A-Za-z0-9_-]{8,96}"
              defaultValue={newId("qov")}
              className="font-mono"
            />
          </Field>
          <Field label="Resource" htmlFor={`${id}-resource`}>
            <NativeSelect id={`${id}-resource`} name="resource">
              {QUOTA_RESOURCES.map((resource) => (
                <option key={resource} value={resource}>
                  {resource.replaceAll("_", " ")}
                </option>
              ))}
            </NativeSelect>
          </Field>
          <Field label="Limit" htmlFor={`${id}-limit`}>
            <Input id={`${id}-limit`} name="limit" type="number" min="1" required />
          </Field>
          <Field label="Expires at (blank means no expiry)" htmlFor={`${id}-expires`}>
            <Input id={`${id}-expires`} name="expiresAt" type="datetime-local" />
          </Field>
          <ReasonField />
          <div>
            <Button type="submit">Create audited override</Button>
          </div>
        </form>
        {result === null ? null : <Result value={result} label="Quota override created" />}
      </CardContent>
    </Card>
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
  const id = useId();
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
    <Card aria-labelledby="abuse-response-title">
      <CardHeader>
        <CardTitle id="abuse-response-title">Abuse response</CardTitle>
        <CardDescription>
          This immediately suspends the selected project or environment.
        </CardDescription>
      </CardHeader>
      <CardContent className="grid gap-4">
        <form className="grid gap-4" onSubmit={submit}>
          <Field label="Response ID" htmlFor={`${id}-id`}>
            <Input
              id={`${id}-id`}
              name="id"
              required
              pattern="abr_[A-Za-z0-9_-]{8,96}"
              defaultValue={newId("abr")}
              className="font-mono"
            />
          </Field>
          <Field label="Target scope" htmlFor={`${id}-target`}>
            <NativeSelect
              id={`${id}-target`}
              value={target}
              onChange={(event) => setTarget(event.currentTarget.value)}
            >
              <option value="project">Entire project</option>
              <option value="environment">One environment</option>
            </NativeSelect>
          </Field>
          {target === "project" ? null : (
            <Field label="Environment" htmlFor={`${id}-environment`}>
              <NativeSelect id={`${id}-environment`} name="environmentId" className="font-mono">
                {environments.map((environmentId) => (
                  <option key={environmentId} value={environmentId}>
                    {environmentId}
                  </option>
                ))}
              </NativeSelect>
            </Field>
          )}
          <ReasonField />
          <div>
            <Button type="submit" variant="destructive">
              Suspend scope
            </Button>
          </div>
        </form>
        {result === null ? null : <Result value={result} label="Abuse response applied" />}
      </CardContent>
    </Card>
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
  const id = useId();
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
    <Card aria-labelledby="support-session-title">
      <CardHeader>
        <CardTitle id="support-session-title">Time-bounded support access</CardTitle>
      </CardHeader>
      <CardContent className="grid gap-4">
        <form className="grid gap-4" onSubmit={create}>
          <Field label="Session ID" htmlFor={`${id}-id`}>
            <Input
              id={`${id}-id`}
              name="id"
              required
              pattern="sup_[A-Za-z0-9_-]{8,96}"
              defaultValue={newId("sup")}
              className="font-mono"
            />
          </Field>
          <Field
            label="Environment scope (blank means project metadata only)"
            htmlFor={`${id}-environment`}
          >
            <NativeSelect id={`${id}-environment`} name="environmentId" defaultValue="">
              <option value="">No environment</option>
              {environments.map((environmentId) => (
                <option key={environmentId} value={environmentId}>
                  {environmentId}
                </option>
              ))}
            </NativeSelect>
          </Field>
          <fieldset className="m-0 grid gap-2 border-0 p-0">
            <legend className="mb-2 p-0 text-sm font-medium leading-none">
              Least-privilege permissions
            </legend>
            {SUPPORT_PERMISSIONS.map((permission) => (
              <div key={permission} className="flex items-center gap-2">
                <Checkbox id={`${id}-${permission}`} name="permissions" value={permission} />
                <Label htmlFor={`${id}-${permission}`} className="font-normal">
                  {permission.replaceAll("_", " ")}
                </Label>
              </div>
            ))}
          </fieldset>
          <Field label="Expires at (maximum eight hours)" htmlFor={`${id}-expires`}>
            <Input id={`${id}-expires`} name="expiresAt" type="datetime-local" required />
          </Field>
          <ReasonField />
          <div>
            <Button type="submit">Create support session</Button>
          </div>
        </form>
        {session === null ? null : (
          <Alert variant="positive" role="status">
            <AlertTitle>
              <strong>Support session {session.state}</strong>
            </AlertTitle>
            <AlertDescription className="block">
              <p className="m-0">
                <code className="font-mono text-xs">{session.id}</code> expires{" "}
                {new Date(session.expiresAt).toLocaleString()}.
              </p>
              {session.state === "active" ? (
                <form className="mt-3 grid w-full gap-3 text-foreground" onSubmit={revoke}>
                  <ReasonField label="Revocation reason" />
                  <div>
                    <Button type="submit" variant="destructive">
                      Revoke support session
                    </Button>
                  </div>
                </form>
              ) : null}
            </AlertDescription>
          </Alert>
        )}
      </CardContent>
    </Card>
  );
}

function ReasonField({
  label = "Required operator reason / case reference",
}: {
  readonly label?: string;
}) {
  const id = useId();
  return (
    <Field label={label} htmlFor={id}>
      <Textarea id={id} name="reason" required minLength={8} maxLength={1024} />
    </Field>
  );
}

/** The server's answer to an action, folded away until the operator wants the record. */
function Result({ value, label }: { readonly value: unknown; readonly label: string }) {
  return (
    <details className="group m-0 rounded-lg border border-positive/30 bg-positive/5 text-sm">
      <summary className="m-0 flex cursor-pointer list-none items-center gap-2 px-4 py-3 font-medium [&::-webkit-details-marker]:hidden">
        <CircleCheck aria-hidden="true" className="size-4 shrink-0 text-positive" />
        {label}
        <ChevronDown
          aria-hidden="true"
          className="ml-auto size-4 shrink-0 text-muted-foreground transition-transform group-open:rotate-180"
        />
      </summary>
      <pre className="m-0 max-h-80 overflow-auto border-t border-positive/30 px-4 py-3 font-mono text-xs leading-relaxed">
        {JSON.stringify(value, null, 2)}
      </pre>
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
    <main className="flex min-h-screen items-center justify-center bg-background px-4 py-10 text-foreground">
      <Card className="w-full max-w-md">
        <CardHeader>
          <Eyebrow className="flex items-center gap-2">
            <ShieldCheck aria-hidden="true" className="size-4 text-primary" />
            Restricted operator route
          </Eyebrow>
          <CardTitle as="h1" className="text-xl">
            {title}
          </CardTitle>
        </CardHeader>
        {children === undefined ? null : (
          <CardContent className="grid gap-4">{children}</CardContent>
        )}
      </Card>
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
