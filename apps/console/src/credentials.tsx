import { type FormEvent, useCallback, useEffect, useState } from "react";

import type {
  AutomationPermission,
  AutomationScope,
  AutomationToken,
  FunctionSecret,
  JwtSigningKey,
  ProjectCredential,
  ServiceCredentialScope,
} from "@mako-cloud/management-sdk";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useManagementClient } from "./management.js";
import { LifecycleBadge } from "./projects.js";
import { confirmDestructiveAction, OneTimeSecretValue } from "./safety.js";

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
    <section aria-labelledby="credentials-title">
      <button type="button" className="back-link" onClick={onBack}>
        ← Project
      </button>
      <div className="section-heading">
        <div>
          <p className="eyebrow">Environment {environmentId}</p>
          <h1 id="credentials-title">Credentials and secrets</h1>
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      <OneTimeValue secret={oneTime} onDismiss={() => setOneTime(null)} />
      <div className="split-grid">
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
    <section className="panel" aria-labelledby="project-credentials-title">
      <h2 id="project-credentials-title">Project credentials</h2>
      <form onSubmit={onCreate}>
        <label>
          Credential kind
          <select name="kind" defaultValue="public">
            <option value="public">Public project key</option>
            <option value="service">Service credential</option>
          </select>
        </label>
        <label>
          Credential ID
          <input name="id" required maxLength={128} />
        </label>
        <label>
          Service collections (comma-separated)
          <input name="collections" defaultValue="*" />
        </label>
        <fieldset>
          <legend>Service operations</legend>
          <div className="checkbox-grid">
            {DOCUMENT_OPERATIONS.map((operation) => (
              <label key={operation}>
                <input type="checkbox" name="operation" value={operation} defaultChecked />
                {operation}
              </label>
            ))}
          </div>
        </fieldset>
        <button type="submit">Create credential</button>
      </form>
      <hr />
      <form className="inline-form" onSubmit={onInspect}>
        <label>
          Credential ID
          <input name="credentialId" required maxLength={128} />
        </label>
        <button type="submit" className="secondary">
          Inspect
        </button>
      </form>
      {credential === null ? null : (
        <article className="workflow-card">
          <div className="button-row spread">
            <strong>{credential.id}</strong>
            <LifecycleBadge state={credential.state} />
          </div>
          <p>{credential.kind} credential</p>
          {credential.serviceScope === undefined ? null : (
            <pre className="json-preview">{JSON.stringify(credential.serviceScope, null, 2)}</pre>
          )}
          <form onSubmit={onRotate}>
            <label>
              Replacement ID
              <input name="replacementId" required maxLength={128} />
            </label>
            <label>
              Overlap seconds
              <input
                name="overlapSeconds"
                type="number"
                min="0"
                max="2592000"
                defaultValue="300"
                required
              />
            </label>
            <button type="submit">Rotate credential</button>
          </form>
          <button
            type="button"
            className="danger-link"
            disabled={credential.state === "retired"}
            onClick={onRetire}
          >
            Retire credential
          </button>
        </article>
      )}
    </section>
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
    <section className="panel" aria-labelledby="signing-keys-title">
      <h2 id="signing-keys-title">JWT signing keys</h2>
      <p>Private signing material is never returned.</p>
      {keys === null ? (
        <p>Loading signing keys…</p>
      ) : keys.length === 0 ? (
        <div className="notice key-initialize" role="status">
          <p>
            No signing key exists for this environment yet. Application-user sessions cannot be
            issued until one is initialized.
          </p>
          <button type="button" onClick={onInitialize}>
            Initialize signing key
          </button>
        </div>
      ) : (
        <ul className="resource-list">
          {keys.map((key) => (
            <li className="resource-row" key={key.keyId}>
              <span>
                <strong>{key.keyId}</strong>
                <small>{new Date(key.createdAt).toLocaleString()}</small>
              </span>
              <LifecycleBadge state={key.state} />
            </li>
          ))}
        </ul>
      )}
      {initialized === null ? null : (
        <p className="notice success" role="status">
          Signing key <code>{initialized.keyId}</code> initialized ({initialized.state}) at{" "}
          {new Date(initialized.createdAt).toLocaleString()}.
        </p>
      )}
      {keys === null || keys.length === 0 ? null : (
        <form onSubmit={onRotate}>
          <label>
            Verification overlap seconds
            <input
              name="overlapSeconds"
              type="number"
              min="1"
              max="2592000"
              defaultValue="3600"
              required
            />
          </label>
          <button type="submit">Rotate signing key</button>
        </form>
      )}
    </section>
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
    <section className="panel" aria-labelledby="function-secrets-title">
      <h2 id="function-secrets-title">Function secrets</h2>
      <form className="inline-form" onSubmit={onCreate}>
        <label>
          Secret name
          <input name="name" required pattern="[A-Za-z_][A-Za-z0-9_]{0,127}" />
        </label>
        <button type="submit">Create</button>
      </form>
      <form className="inline-form" onSubmit={onInspect}>
        <label>
          Inspect by name
          <input name="name" required pattern="[A-Za-z_][A-Za-z0-9_]{0,127}" />
        </label>
        <button type="submit" className="secondary">
          Inspect
        </button>
      </form>
      {secret === null ? null : (
        <article className="workflow-card">
          <div className="button-row spread">
            <strong>
              {secret.name} v{secret.version}
            </strong>
            <LifecycleBadge state={secret.state} />
          </div>
          <div className="button-row">
            <button type="button" disabled={secret.state !== "active"} onClick={onRotate}>
              Rotate
            </button>
            <button
              type="button"
              className="danger-link"
              disabled={secret.state !== "active"}
              onClick={onRetire}
            >
              Retire
            </button>
          </div>
        </article>
      )}
    </section>
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
    <section className="panel" aria-labelledby="automation-title">
      <h2 id="automation-title">Automation tokens</h2>
      <form onSubmit={onCreate}>
        <label>
          Token name
          <input name="name" required maxLength={100} />
        </label>
        <label>
          Expires at
          <input name="expiresAt" type="datetime-local" required />
        </label>
        <fieldset>
          <legend>Permissions</legend>
          <div className="checkbox-grid">
            {AUTOMATION_PERMISSIONS.map((permission) => (
              <label key={permission}>
                <input type="checkbox" name="permission" value={permission} />
                {permission.replaceAll("_", " ")}
              </label>
            ))}
          </div>
        </fieldset>
        <button type="submit">Create scoped token</button>
      </form>
      {tokens === null ? (
        <p>Loading automation tokens…</p>
      ) : (
        <div className="resource-list">
          {tokens.map((token) => (
            <button
              type="button"
              className="resource-row"
              key={token.id}
              onClick={() => onSelect(token)}
            >
              <span>
                <strong>{token.name}</strong>
                <small>{token.id}</small>
              </span>
              <LifecycleBadge state={token.status} />
            </button>
          ))}
        </div>
      )}
      {selected === null ? null : (
        <article className="workflow-card">
          <strong>{selected.name}</strong>
          <pre className="json-preview">{JSON.stringify(selected.scope, null, 2)}</pre>
          <form onSubmit={onRotate}>
            <label>
              Replacement token ID
              <input name="replacementId" required pattern="atm_[A-Za-z0-9_-]{8,64}" />
            </label>
            <label>
              Replacement expires at
              <input name="expiresAt" type="datetime-local" required />
            </label>
            <button type="submit">Rotate token</button>
          </form>
          <button
            type="button"
            className="danger-link"
            disabled={selected.status !== "active"}
            onClick={onRevoke}
          >
            Revoke token
          </button>
        </article>
      )}
    </section>
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
  return <OneTimeSecretValue label={secret.label} value={secret.value} onDismiss={onDismiss} />;
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
