import type {
  AutomationPermission,
  AutomationScope,
  AutomationToken,
  FunctionSecret,
  JwtSigningKey,
  ProjectCredential,
  ServiceCredentialScope,
} from "@mako-cloud/management-sdk";
import {
  Alert,
  AlertDescription,
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
  Separator,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@mako-cloud/ui";
import { CircleCheck, Copy, Eye, EyeOff, KeyRound, ShieldAlert } from "lucide-react";
import { type FormEvent, type ReactNode, useCallback, useEffect, useRef, useState } from "react";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useManagementClient } from "./management.js";
import { LifecycleBadge } from "./projects.js";
import { confirmDestructiveAction } from "./safety.js";

const AUTOMATION_PERMISSIONS: readonly AutomationPermission[] = [
  "organization_read",
  "project_read",
  "project_write",
  "environment_read",
  "environment_write",
  "collection_write",
  "policy_write",
  "function_deploy",
  "audit_read",
];
const DOCUMENT_OPERATIONS: readonly ServiceCredentialScope["operations"][number][] = [
  "create",
  "read",
  "update",
  "delete",
];

/** A revealed secret leaves the screen on its own after this long. */
const AUTO_DISMISS_MILLISECONDS = 5 * 60 * 1_000;

/** A record's JSON, shown as the developer would paste it. */
const JSON_PREVIEW =
  "m-0 overflow-x-auto rounded-md border bg-muted/40 p-3 font-mono text-xs leading-relaxed";

interface OneTimeSecret {
  readonly label: string;
  readonly value: string;
}

export function CredentialsScreen({
  projectId,
  environmentId,
  onBack,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly onBack: () => void;
}) {
  const client = useManagementClient();
  const [teamId, setTeamId] = useState<string | null>(null);
  const [tokens, setTokens] = useState<AutomationToken[] | null>(null);
  const [selectedToken, setSelectedToken] = useState<AutomationToken | null>(null);
  const [signingKeys, setSigningKeys] = useState<JwtSigningKey[] | null>(null);
  const [credential, setCredential] = useState<ProjectCredential | null>(null);
  const [functionSecret, setFunctionSecret] = useState<FunctionSecret | null>(null);
  const [oneTime, setOneTime] = useState<OneTimeSecret | null>(null);
  const [initializedKey, setInitializedKey] = useState<JwtSigningKey | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const reload = useCallback(async () => {
    try {
      const project = await client.getProject(projectId);
      const [nextTokens, nextSigningKeys] = await Promise.all([
        client.listAutomationTokens(project.teamId),
        client.listJwtSigningKeys(projectId, environmentId),
      ]);
      setTeamId(project.teamId);
      setTokens(nextTokens);
      setSigningKeys(nextSigningKeys);
      setSelectedToken((current) =>
        current === null ? null : (nextTokens.find((token) => token.id === current.id) ?? null),
      );
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  }, [client, environmentId, projectId]);
  useEffect(() => {
    void reload();
  }, [reload]);

  const createCredential = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    try {
      const kind = requiredText(data, "kind");
      const id = requiredText(data, "id");
      const issue =
        kind === "public"
          ? await client.createPublicProjectKey(projectId, environmentId, id, idempotencyKey())
          : await client.createServiceCredential(
              projectId,
              environmentId,
              id,
              serviceScope(data),
              idempotencyKey(),
            );
      setCredential(issue.credential);
      setOneTime({
        label: `${issue.credential.kind} credential ${issue.credential.id}`,
        value: issue.value,
      });
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };
  const inspectCredential = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    try {
      setCredential(
        await client.getProjectCredential(
          projectId,
          environmentId,
          requiredText(new FormData(event.currentTarget), "credentialId"),
        ),
      );
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };
  const rotateCredential = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (credential === null) {
      return;
    }
    const data = new FormData(event.currentTarget);
    try {
      const issue = await client.rotateProjectCredential(
        projectId,
        environmentId,
        credential.id,
        requiredText(data, "replacementId"),
        nonNegativeInteger(data, "overlapSeconds"),
        idempotencyKey(),
      );
      setCredential(issue.credential);
      setOneTime({ label: `replacement credential ${issue.credential.id}`, value: issue.value });
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };
  const retireCredential = async () => {
    if (credential === null) {
      return;
    }
    if (
      !confirmDestructiveAction({
        action: "Retire",
        target: `credential ${credential.id}`,
        consequence: "New requests using it will be rejected after any configured overlap ends.",
      })
    ) {
      return;
    }
    try {
      await client.retireProjectCredential(projectId, environmentId, credential.id);
      setCredential({ ...credential, state: "retired", retiredAt: new Date().toISOString() });
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };

  const rotateSigningKey = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    try {
      await client.rotateJwtSigningKey(
        projectId,
        environmentId,
        positiveInteger(new FormData(event.currentTarget), "overlapSeconds"),
        idempotencyKey(),
      );
      await reload();
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };
  // Offered only while no key exists; the response carries key metadata and
  // never the private material.
  const initializeSigningKey = async () => {
    if (
      !confirmDestructiveAction({
        action: "Initialize",
        target: `the JWT signing key for environment ${environmentId}`,
        consequence:
          "Application-user tokens for this environment will be signed with the new key. Private key material stays on the server and is never shown.",
      })
    ) {
      return;
    }
    try {
      const key = await client.initializeJwtSigningKey(projectId, environmentId, idempotencyKey());
      setInitializedKey(key);
      setFailure(null);
      await reload();
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };

  const createSecret = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    try {
      const issue = await client.createFunctionSecret(
        projectId,
        environmentId,
        requiredText(new FormData(event.currentTarget), "name"),
        idempotencyKey(),
      );
      setFunctionSecret(issue.secret);
      setOneTime({ label: `function secret ${issue.secret.name}`, value: issue.value });
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };
  const inspectSecret = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    try {
      setFunctionSecret(
        await client.getFunctionSecret(
          projectId,
          environmentId,
          requiredText(new FormData(event.currentTarget), "name"),
        ),
      );
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };
  const secretAction = async (action: "rotate" | "retire") => {
    if (functionSecret === null) {
      return;
    }
    if (
      action === "retire" &&
      !confirmDestructiveAction({
        action: "Retire",
        target: `function secret ${functionSecret.name}`,
        consequence: "Future deployments and invocations cannot attach this secret version.",
      })
    ) {
      return;
    }
    try {
      if (action === "retire") {
        setFunctionSecret(
          await client.retireFunctionSecret(projectId, environmentId, functionSecret.name),
        );
      } else {
        const issue = await client.rotateFunctionSecret(
          projectId,
          environmentId,
          functionSecret.name,
          idempotencyKey(),
        );
        setFunctionSecret(issue.secret);
        setOneTime({ label: `function secret ${issue.secret.name}`, value: issue.value });
      }
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };

  const createToken = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (teamId === null) {
      return;
    }
    const data = new FormData(event.currentTarget);
    try {
      const issue = await client.createAutomationToken(teamId, {
        name: requiredText(data, "name"),
        scope: automationScope(data, projectId, environmentId),
        expiresAt: dateTime(data, "expiresAt"),
      });
      setSelectedToken(issue.token);
      setOneTime({ label: `automation token ${issue.token.name}`, value: issue.secret });
      await reload();
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };
  const rotateToken = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (teamId === null || selectedToken === null) {
      return;
    }
    const data = new FormData(event.currentTarget);
    try {
      const issue = await client.rotateAutomationToken(
        teamId,
        selectedToken.id,
        requiredText(data, "replacementId"),
        dateTime(data, "expiresAt"),
        idempotencyKey(),
      );
      setOneTime({ label: `automation token ${issue.token.name}`, value: issue.secret });
      setSelectedToken(issue.token);
      await reload();
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };
  const revokeToken = async () => {
    if (teamId === null || selectedToken === null) {
      return;
    }
    if (
      !confirmDestructiveAction({
        action: "Revoke",
        target: `automation token ${selectedToken.name}`,
        consequence: "Automation using this token will lose access immediately.",
      })
    ) {
      return;
    }
    try {
      await client.revokeAutomationToken(teamId, selectedToken.id);
      await reload();
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };

  return (
    <section aria-labelledby="credentials-title" className="grid gap-6">
      <div className="grid gap-3">
        <Button
          variant="ghost"
          size="sm"
          className="-ml-2 w-fit text-muted-foreground hover:text-foreground"
          onClick={onBack}
        >
          ← Project
        </Button>
        <div className="grid gap-1">
          <Eyebrow>Environment {environmentId}</Eyebrow>
          <h1 id="credentials-title" className="text-2xl">
            Credentials and secrets
          </h1>
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      <OneTimeValue secret={oneTime} onDismiss={() => setOneTime(null)} />
      <div className="grid items-start gap-6 xl:grid-cols-2">
        <ProjectCredentialsPanel
          credential={credential}
          onCreate={createCredential}
          onInspect={inspectCredential}
          onRotate={rotateCredential}
          onRetire={() => void retireCredential()}
        />
        <SigningKeysPanel
          keys={signingKeys}
          initialized={initializedKey}
          onInitialize={() => void initializeSigningKey()}
          onRotate={rotateSigningKey}
        />
        <FunctionSecretsPanel
          secret={functionSecret}
          onCreate={createSecret}
          onInspect={inspectSecret}
          onRotate={() => void secretAction("rotate")}
          onRetire={() => void secretAction("retire")}
        />
        <AutomationTokensPanel
          tokens={tokens}
          selected={selectedToken}
          onSelect={setSelectedToken}
          onCreate={createToken}
          onRotate={rotateToken}
          onRevoke={() => void revokeToken()}
        />
      </div>
    </section>
  );
}

function ProjectCredentialsPanel({
  credential,
  onCreate,
  onInspect,
  onRotate,
  onRetire,
}: {
  readonly credential: ProjectCredential | null;
  readonly onCreate: (event: FormEvent<HTMLFormElement>) => void;
  readonly onInspect: (event: FormEvent<HTMLFormElement>) => void;
  readonly onRotate: (event: FormEvent<HTMLFormElement>) => void;
  readonly onRetire: () => void;
}) {
  return (
    <Card aria-labelledby="project-credentials-title">
      <CardHeader>
        <CardTitle id="project-credentials-title">Project credentials</CardTitle>
      </CardHeader>
      <CardContent className="grid gap-5">
        <form className="grid gap-4" onSubmit={onCreate}>
          <Field label="Credential kind" htmlFor="credential-kind">
            <NativeSelect id="credential-kind" name="kind" defaultValue="public">
              <option value="public">Public project key</option>
              <option value="service">Service credential</option>
            </NativeSelect>
          </Field>
          <Field label="Credential ID" htmlFor="credential-id">
            <Input id="credential-id" name="id" required maxLength={128} className="font-mono" />
          </Field>
          <Field label="Service collections (comma-separated)" htmlFor="credential-collections">
            <Input
              id="credential-collections"
              name="collections"
              defaultValue="*"
              className="font-mono"
            />
          </Field>
          <CheckboxGroup legend="Service operations">
            {DOCUMENT_OPERATIONS.map((operation) => (
              <CheckboxOption
                key={operation}
                id={`credential-operation-${operation}`}
                name="operation"
                value={operation}
                defaultChecked
              >
                {operation}
              </CheckboxOption>
            ))}
          </CheckboxGroup>
          <Button type="submit" className="justify-self-start">
            Create credential
          </Button>
        </form>
        <Separator />
        <form className="flex flex-wrap items-end gap-3" onSubmit={onInspect}>
          <Field label="Credential ID" htmlFor="inspect-credential-id" className="min-w-48 flex-1">
            <Input
              id="inspect-credential-id"
              name="credentialId"
              required
              maxLength={128}
              className="font-mono"
            />
          </Field>
          <Button type="submit" variant="secondary">
            Inspect
          </Button>
        </form>
        {credential === null ? null : (
          <RecordCard>
            <div className="flex flex-wrap items-center justify-between gap-3">
              <strong className="font-mono text-sm">{credential.id}</strong>
              <LifecycleBadge state={credential.state} />
            </div>
            <p className="m-0 text-sm text-muted-foreground">{credential.kind} credential</p>
            {credential.serviceScope === undefined ? null : (
              <pre className={JSON_PREVIEW}>{JSON.stringify(credential.serviceScope, null, 2)}</pre>
            )}
            <form className="grid gap-4" onSubmit={onRotate}>
              <Field label="Replacement ID" htmlFor="credential-replacement-id">
                <Input
                  id="credential-replacement-id"
                  name="replacementId"
                  required
                  maxLength={128}
                  className="font-mono"
                />
              </Field>
              <Field label="Overlap seconds" htmlFor="credential-overlap-seconds">
                <Input
                  id="credential-overlap-seconds"
                  name="overlapSeconds"
                  type="number"
                  min="0"
                  max="2592000"
                  defaultValue="300"
                  required
                />
              </Field>
              <Button type="submit" variant="secondary" className="justify-self-start">
                Rotate credential
              </Button>
            </form>
            <Button
              variant="ghost"
              size="sm"
              className="w-fit text-destructive hover:text-destructive"
              disabled={credential.state === "retired"}
              onClick={onRetire}
            >
              Retire credential
            </Button>
          </RecordCard>
        )}
      </CardContent>
    </Card>
  );
}

function SigningKeysPanel({
  keys,
  initialized,
  onInitialize,
  onRotate,
}: {
  readonly keys: JwtSigningKey[] | null;
  readonly initialized: JwtSigningKey | null;
  readonly onInitialize: () => void;
  readonly onRotate: (event: FormEvent<HTMLFormElement>) => void;
}) {
  return (
    <Card aria-labelledby="signing-keys-title">
      <CardHeader>
        <CardTitle id="signing-keys-title">JWT signing keys</CardTitle>
        <CardDescription>Private signing material is never returned.</CardDescription>
      </CardHeader>
      <CardContent className="grid gap-5">
        {keys === null ? (
          <p className="m-0 text-sm text-muted-foreground">Loading signing keys…</p>
        ) : keys.length === 0 ? (
          <Alert role="status">
            <KeyRound aria-hidden="true" />
            <AlertDescription>
              <p className="m-0">
                No signing key exists for this environment yet. Application-user sessions cannot be
                issued until one is initialized.
              </p>
              <Button size="sm" className="mt-1" onClick={onInitialize}>
                Initialize signing key
              </Button>
            </AlertDescription>
          </Alert>
        ) : (
          <ul className="m-0 grid list-none gap-2 p-0">
            {keys.map((key) => (
              <li
                className="resource-row flex items-center justify-between gap-4 rounded-lg border p-3 text-left"
                key={key.keyId}
              >
                <span className="grid gap-0.5">
                  <strong className="font-mono text-sm">{key.keyId}</strong>
                  <small className="text-xs text-muted-foreground">
                    {new Date(key.createdAt).toLocaleString()}
                  </small>
                </span>
                <LifecycleBadge state={key.state} />
              </li>
            ))}
          </ul>
        )}
        {initialized === null ? null : (
          <Alert variant="positive" role="status">
            <CircleCheck aria-hidden="true" />
            <AlertDescription>
              <p className="m-0">
                Signing key <code className="font-mono">{initialized.keyId}</code> initialized (
                {initialized.state}) at {new Date(initialized.createdAt).toLocaleString()}.
              </p>
            </AlertDescription>
          </Alert>
        )}
        {keys === null || keys.length === 0 ? null : (
          <form className="grid gap-4" onSubmit={onRotate}>
            <Field label="Verification overlap seconds" htmlFor="signing-key-overlap-seconds">
              <Input
                id="signing-key-overlap-seconds"
                name="overlapSeconds"
                type="number"
                min="1"
                max="2592000"
                defaultValue="3600"
                required
              />
            </Field>
            <Button type="submit" variant="secondary" className="justify-self-start">
              Rotate signing key
            </Button>
          </form>
        )}
      </CardContent>
    </Card>
  );
}

function FunctionSecretsPanel({
  secret,
  onCreate,
  onInspect,
  onRotate,
  onRetire,
}: {
  readonly secret: FunctionSecret | null;
  readonly onCreate: (event: FormEvent<HTMLFormElement>) => void;
  readonly onInspect: (event: FormEvent<HTMLFormElement>) => void;
  readonly onRotate: () => void;
  readonly onRetire: () => void;
}) {
  return (
    <Card aria-labelledby="function-secrets-title">
      <CardHeader>
        <CardTitle id="function-secrets-title">Function secrets</CardTitle>
      </CardHeader>
      <CardContent className="grid gap-5">
        <form className="flex flex-wrap items-end gap-3" onSubmit={onCreate}>
          <Field label="Secret name" htmlFor="function-secret-name" className="min-w-48 flex-1">
            <Input
              id="function-secret-name"
              name="name"
              required
              pattern="[A-Za-z_][A-Za-z0-9_]{0,127}"
              className="font-mono"
            />
          </Field>
          <Button type="submit">Create</Button>
        </form>
        <form className="flex flex-wrap items-end gap-3" onSubmit={onInspect}>
          <Field
            label="Inspect by name"
            htmlFor="function-secret-inspect-name"
            className="min-w-48 flex-1"
          >
            <Input
              id="function-secret-inspect-name"
              name="name"
              required
              pattern="[A-Za-z_][A-Za-z0-9_]{0,127}"
              className="font-mono"
            />
          </Field>
          <Button type="submit" variant="secondary">
            Inspect
          </Button>
        </form>
        {secret === null ? null : (
          <RecordCard>
            <div className="flex flex-wrap items-center justify-between gap-3">
              <strong className="font-mono text-sm">
                {secret.name} v{secret.version}
              </strong>
              <LifecycleBadge state={secret.state} />
            </div>
            <div className="flex flex-wrap gap-2">
              <Button variant="secondary" disabled={secret.state !== "active"} onClick={onRotate}>
                Rotate
              </Button>
              <Button
                variant="ghost"
                className="text-destructive hover:text-destructive"
                disabled={secret.state !== "active"}
                onClick={onRetire}
              >
                Retire
              </Button>
            </div>
          </RecordCard>
        )}
      </CardContent>
    </Card>
  );
}

function AutomationTokensPanel({
  tokens,
  selected,
  onSelect,
  onCreate,
  onRotate,
  onRevoke,
}: {
  readonly tokens: AutomationToken[] | null;
  readonly selected: AutomationToken | null;
  readonly onSelect: (token: AutomationToken) => void;
  readonly onCreate: (event: FormEvent<HTMLFormElement>) => void;
  readonly onRotate: (event: FormEvent<HTMLFormElement>) => void;
  readonly onRevoke: () => void;
}) {
  return (
    <Card aria-labelledby="automation-title">
      <CardHeader>
        <CardTitle id="automation-title">Automation tokens</CardTitle>
      </CardHeader>
      <CardContent className="grid gap-5">
        <form className="grid gap-4" onSubmit={onCreate}>
          <Field label="Token name" htmlFor="automation-token-name">
            <Input id="automation-token-name" name="name" required maxLength={100} />
          </Field>
          <Field label="Expires at" htmlFor="automation-token-expires-at">
            <Input
              id="automation-token-expires-at"
              name="expiresAt"
              type="datetime-local"
              required
            />
          </Field>
          <CheckboxGroup legend="Permissions">
            {AUTOMATION_PERMISSIONS.map((permission) => (
              <CheckboxOption
                key={permission}
                id={`automation-permission-${permission}`}
                name="permission"
                value={permission}
              >
                {permission.replaceAll("_", " ")}
              </CheckboxOption>
            ))}
          </CheckboxGroup>
          <Button type="submit" className="justify-self-start">
            Create scoped token
          </Button>
        </form>
        {tokens === null ? (
          <p className="m-0 text-sm text-muted-foreground">Loading automation tokens…</p>
        ) : tokens.length === 0 ? null : (
          <Table aria-labelledby="automation-title">
            <TableHeader>
              <TableRow className="hover:bg-transparent">
                <TableHead scope="col">Token</TableHead>
                <TableHead scope="col">Status</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {tokens.map((token) => (
                <TableRow
                  key={token.id}
                  data-state={selected?.id === token.id ? "selected" : undefined}
                >
                  <TableCell className="whitespace-normal">
                    <Button
                      variant="link"
                      className="h-auto p-0 font-medium"
                      onClick={() => onSelect(token)}
                    >
                      {token.name}
                    </Button>
                    <span className="block font-mono text-xs text-muted-foreground">
                      {token.id}
                    </span>
                  </TableCell>
                  <TableCell>
                    <LifecycleBadge state={token.status} />
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
        {selected === null ? null : (
          <RecordCard>
            <strong className="text-sm">{selected.name}</strong>
            <pre className={JSON_PREVIEW}>{JSON.stringify(selected.scope, null, 2)}</pre>
            <form className="grid gap-4" onSubmit={onRotate}>
              <Field label="Replacement token ID" htmlFor="automation-replacement-id">
                <Input
                  id="automation-replacement-id"
                  name="replacementId"
                  required
                  pattern="atm_[A-Za-z0-9_-]{8,64}"
                  className="font-mono"
                />
              </Field>
              <Field label="Replacement expires at" htmlFor="automation-replacement-expires-at">
                <Input
                  id="automation-replacement-expires-at"
                  name="expiresAt"
                  type="datetime-local"
                  required
                />
              </Field>
              <Button type="submit" variant="secondary" className="justify-self-start">
                Rotate token
              </Button>
            </form>
            <Button
              variant="ghost"
              size="sm"
              className="w-fit text-destructive hover:text-destructive"
              disabled={selected.status !== "active"}
              onClick={onRevoke}
            >
              Revoke token
            </Button>
          </RecordCard>
        )}
      </CardContent>
    </Card>
  );
}

/** The one record a panel is working on, set apart from the forms around it. */
function RecordCard({ children }: { readonly children: ReactNode }) {
  return <article className="grid gap-3 rounded-lg border bg-muted/30 p-4">{children}</article>;
}

/** A set of checkboxes under one legend, laid out in columns. */
function CheckboxGroup({
  legend,
  children,
}: {
  readonly legend: string;
  readonly children: ReactNode;
}) {
  return (
    <fieldset className="m-0 grid min-w-0 gap-2 border-0 p-0">
      <legend className="mb-2 p-0 text-sm font-medium leading-none">{legend}</legend>
      <div className="grid grid-cols-[repeat(auto-fill,minmax(10rem,1fr))] gap-x-4 gap-y-2">
        {children}
      </div>
    </fieldset>
  );
}

/** One checkbox with its label; it submits with the form under `name`. */
function CheckboxOption({
  id,
  name,
  value,
  defaultChecked,
  children,
}: {
  readonly id: string;
  readonly name: string;
  readonly value: string;
  readonly defaultChecked?: boolean;
  readonly children: ReactNode;
}) {
  return (
    <div className="flex items-center gap-2">
      <Checkbox id={id} name={name} value={value} defaultChecked={defaultChecked === true} />
      <Label htmlFor={id} className="font-mono text-xs font-normal">
        {children}
      </Label>
    </div>
  );
}

function OneTimeValue({
  secret,
  onDismiss,
}: {
  readonly secret: OneTimeSecret | null;
  readonly onDismiss: () => void;
}) {
  if (secret === null) {
    return null;
  }
  return <OneTimeSecretCard label={secret.label} value={secret.value} onDismiss={onDismiss} />;
}

/**
 * A secret shown exactly once. It starts hidden, is hidden again whenever the
 * tab leaves the foreground, and dismisses itself after five minutes; the
 * developer reveals it, copies it, and confirms it is stored.
 */
function OneTimeSecretCard({
  label,
  value,
  onDismiss,
}: {
  readonly label: string;
  readonly value: string;
  readonly onDismiss: () => void;
}) {
  const [revealed, setRevealed] = useState(false);
  const [copyStatus, setCopyStatus] = useState("");
  const heading = useRef<HTMLElement>(null);
  const dismiss = useRef(onDismiss);
  dismiss.current = onDismiss;

  useEffect(() => {
    if (value.length === 0) {
      dismiss.current();
      return;
    }
    setRevealed(false);
    setCopyStatus("");
    heading.current?.focus();
    const timer = window.setTimeout(() => dismiss.current(), AUTO_DISMISS_MILLISECONDS);
    const hideWhenBackgrounded = () => {
      if (document.visibilityState === "hidden") {
        setRevealed(false);
      }
    };
    document.addEventListener("visibilitychange", hideWhenBackgrounded);
    return () => {
      window.clearTimeout(timer);
      document.removeEventListener("visibilitychange", hideWhenBackgrounded);
    };
  }, [value]);

  const copy = async () => {
    try {
      await navigator.clipboard.writeText(value);
      setCopyStatus("Copied. Clear your clipboard after storing the value securely.");
    } catch {
      setCopyStatus("Clipboard access was unavailable. Reveal the value and copy it manually.");
    }
  };

  return (
    <aside
      aria-labelledby="one-time-title"
      className="grid gap-3 rounded-xl border border-warning/50 bg-warning/10 p-5 text-foreground"
    >
      <div className="flex items-start gap-3">
        <ShieldAlert aria-hidden="true" className="mt-0.5 size-5 shrink-0 text-warning" />
        <div className="grid gap-1">
          <strong
            id="one-time-title"
            ref={heading}
            tabIndex={-1}
            className="text-base font-semibold outline-none"
          >
            Copy this {label} now. It will not be shown again.
          </strong>
          <p className="m-0 text-sm text-muted-foreground">
            The value is hidden again when this tab moves to the background and removed after five
            minutes.
          </p>
        </div>
      </div>
      {revealed ? (
        <code className="block break-all rounded-md border bg-card px-3 py-2 font-mono text-sm">
          {value}
        </code>
      ) : (
        <span className="block rounded-md border bg-card px-3 py-2 font-mono text-sm tracking-widest text-muted-foreground">
          <span aria-hidden="true">••••••••••••••••</span>
          <span className="sr-only">Secret value hidden</span>
        </span>
      )}
      <div className="flex flex-wrap gap-2">
        <Button variant="outline" onClick={() => setRevealed((shown) => !shown)}>
          {revealed ? <EyeOff aria-hidden="true" /> : <Eye aria-hidden="true" />}
          {revealed ? "Hide value" : "Reveal value"}
        </Button>
        <Button variant="outline" onClick={() => void copy()}>
          <Copy aria-hidden="true" />
          Copy value
        </Button>
        <Button onClick={onDismiss}>I have stored it securely</Button>
      </div>
      {copyStatus === "" ? null : (
        <p className="m-0 text-sm text-muted-foreground" aria-live="polite" aria-atomic="true">
          {copyStatus}
        </p>
      )}
    </aside>
  );
}

class FormInputError extends Error {}

function requiredText(data: FormData, name: string): string {
  const value = String(data.get(name) ?? "").trim();
  if (value === "") {
    throw new FormInputError(`${name} is required.`);
  }
  return value;
}

function checkedValues<T extends string>(data: FormData, name: string, allowed: readonly T[]): T[] {
  const values = data.getAll(name).map(String);
  if (values.length === 0 || !values.every((value): value is T => allowed.includes(value as T))) {
    throw new FormInputError(`Select at least one valid ${name}.`);
  }
  return values;
}

function serviceScope(data: FormData): ServiceCredentialScope {
  const collections = requiredText(data, "collections")
    .split(",")
    .map((value) => value.trim())
    .filter((value) => value !== "");
  if (collections.length === 0) {
    throw new FormInputError("Select at least one service collection.");
  }
  return {
    collections,
    operations: checkedValues(data, "operation", DOCUMENT_OPERATIONS),
  };
}

function automationScope(
  data: FormData,
  projectId: string,
  environmentId: string,
): AutomationScope {
  return {
    projectId,
    environmentId,
    permissions: checkedValues(data, "permission", AUTOMATION_PERMISSIONS),
  };
}

function nonNegativeInteger(data: FormData, name: string): number {
  const value = Number(requiredText(data, name));
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new FormInputError(`${name} must be a non-negative integer.`);
  }
  return value;
}

function positiveInteger(data: FormData, name: string): number {
  const value = nonNegativeInteger(data, name);
  if (value === 0) {
    throw new FormInputError(`${name} must be positive.`);
  }
  return value;
}

function dateTime(data: FormData, name: string): string {
  const value = new Date(requiredText(data, name));
  if (Number.isNaN(value.getTime())) {
    throw new FormInputError(`${name} must be a valid date and time.`);
  }
  return value.toISOString();
}

function failureFrom(error: unknown): ConsoleApiFailure {
  return error instanceof FormInputError
    ? { message: error.message, requestId: null }
    : toConsoleApiFailure(error);
}

function idempotencyKey(): string {
  return globalThis.crypto.randomUUID();
}
