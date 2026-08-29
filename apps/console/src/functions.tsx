import { type FormEvent, useCallback, useEffect, useState } from "react";

import type {
  CreateFunctionRequest,
  Function as EdgeFunction,
  FunctionBundleArtifact,
  FunctionBundleUploadRequest,
  FunctionConfiguration,
  FunctionDeployment,
  FunctionLogPage,
  FunctionTestResponse,
  ObservabilityPage,
} from "@mako-cloud/management-sdk";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { FunctionSchedulesPanel } from "./function-schedules.js";
import { useManagementClient } from "./management.js";
import { LifecycleBadge } from "./projects.js";
import { confirmDestructiveAction } from "./safety.js";

const DEFAULT_LIMITS = {
  cpuMilliseconds: 100,
  wallMilliseconds: 1_000,
  memoryBytes: 128 * 1024 * 1024,
  requestBytes: 1024 * 1024,
  responseBytes: 1024 * 1024,
  concurrency: 10,
} as const;

export function FunctionsScreen({
  projectId,
  environmentId,
  onBack,
  onOpen,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly onBack: () => void;
  readonly onOpen: (functionName: string) => void;
}) {
  const client = useManagementClient();
  const [functions, setFunctions] = useState<EdgeFunction[] | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const reload = useCallback(async () => {
    try {
      setFunctions(await client.listFunctions(projectId, environmentId));
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  }, [client, environmentId, projectId]);
  useEffect(() => {
    void reload();
  }, [reload]);

  const create = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    try {
      const input: CreateFunctionRequest = {
        name: requiredText(data, "name"),
        configuration: configurationFrom(data),
      };
      const created = await client.createFunction(
        projectId,
        environmentId,
        input,
        idempotencyKey(),
      );
      setFailure(null);
      onOpen(created.name);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };

  return (
    <section aria-labelledby="functions-title">
      <button type="button" className="back-link" onClick={onBack}>
        ← Project
      </button>
      <div className="section-heading">
        <div>
          <p className="eyebrow">Environment {environmentId}</p>
          <h1 id="functions-title">Edge functions</h1>
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      <div className="split-grid">
        <section className="panel" aria-labelledby="function-list-title">
          <h2 id="function-list-title">Functions</h2>
          {functions === null ? (
            <p>Loading functions…</p>
          ) : functions.length === 0 ? (
            <p>No functions have been created.</p>
          ) : (
            <div className="resource-list">
              {functions.map((item) => (
                <button
                  type="button"
                  className="resource-row"
                  key={item.name}
                  onClick={() => onOpen(item.name)}
                >
                  <span>
                    <strong>{item.name}</strong>
                    <small>
                      {item.activeVersion === null
                        ? "No active deployment"
                        : `Active v${item.activeVersion}`}
                    </small>
                  </span>
                  <LifecycleBadge state={item.state} />
                </button>
              ))}
            </div>
          )}
        </section>
        <section className="panel" aria-labelledby="create-function-title">
          <h2 id="create-function-title">Create function</h2>
          <form onSubmit={(event) => void create(event)}>
            <label>
              Function name
              <input name="name" required pattern="[a-z][a-z0-9-]{0,62}" />
            </label>
            <ConfigurationFields />
            <button type="submit">Create function</button>
          </form>
        </section>
      </div>
    </section>
  );
}

export function FunctionScreen({
  projectId,
  environmentId,
  functionName,
  onBack,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly functionName: string;
  readonly onBack: () => void;
}) {
  const client = useManagementClient();
  const [item, setItem] = useState<EdgeFunction | null>(null);
  const [deployments, setDeployments] = useState<FunctionDeployment[] | null>(null);
  const [artifact, setArtifact] = useState<FunctionBundleArtifact | null>(null);
  const [logs, setLogs] = useState<FunctionLogPage | null>(null);
  const [metrics, setMetrics] = useState<ObservabilityPage | null>(null);
  const [testResponse, setTestResponse] = useState<FunctionTestResponse | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const reload = useCallback(async () => {
    try {
      const [nextFunction, nextDeployments, nextLogs, nextMetrics] = await Promise.all([
        client.getFunction(projectId, environmentId, functionName),
        client.listFunctionDeployments(projectId, environmentId, functionName),
        client.queryFunctionLogs(projectId, environmentId, functionName, { limit: 100 }),
        client.queryFunctionMetrics(projectId, environmentId, { limit: 100 }),
      ]);
      setItem(nextFunction);
      setDeployments(nextDeployments);
      setLogs(nextLogs);
      setMetrics(nextMetrics);
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  }, [client, environmentId, functionName, projectId]);
  useEffect(() => {
    void reload();
  }, [reload]);

  const configure = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    try {
      setItem(
        await client.updateFunctionConfiguration(
          projectId,
          environmentId,
          functionName,
          configurationFrom(new FormData(event.currentTarget)),
        ),
      );
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };
  const deploy = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    try {
      await client.createFunctionDeployment(
        projectId,
        environmentId,
        functionName,
        {
          version: positiveInteger(data, "version"),
          bundleDigest: requiredText(data, "bundleDigest"),
          entrypoint: requiredText(data, "entrypoint"),
          runtimeVersion: requiredText(data, "runtimeVersion"),
        },
        idempotencyKey(),
      );
      await reload();
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };
  const switchVersion = async (version: number, action: "promote" | "rollback") => {
    if (
      action === "rollback" &&
      !confirmDestructiveAction({
        action: "Roll back",
        target: `${functionName} to version ${version}`,
        consequence: "New invocations will switch to the selected immutable version.",
      })
    ) {
      return;
    }
    try {
      const next =
        action === "promote"
          ? await client.promoteFunctionDeployment(
              projectId,
              environmentId,
              functionName,
              version,
              idempotencyKey(),
            )
          : await client.rollbackFunctionDeployment(
              projectId,
              environmentId,
              functionName,
              version,
              idempotencyKey(),
            );
      setItem(next);
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };
  const removeVersion = async (version: number) => {
    if (
      !confirmDestructiveAction({
        action: "Delete",
        target: `${functionName} version ${version}`,
        consequence: "The immutable version will no longer be available for tests or rollback.",
      })
    ) {
      return;
    }
    try {
      await client.deleteFunctionDeployment(projectId, environmentId, functionName, version);
      await reload();
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };
  const checkHealth = async (version: number) => {
    try {
      await client.checkFunctionDeploymentHealth(projectId, environmentId, functionName, version);
      await reload();
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };
  const test = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    const version = optionalPositiveInteger(data, "version");
    try {
      setTestResponse(
        await client.testFunctionInvocation(projectId, environmentId, functionName, {
          ...(version === null ? {} : { version }),
          method: requiredText(data, "method"),
          path: requiredText(data, "path"),
          headers: parseStringRecord(requiredText(data, "headers"), "Headers"),
          body: utf8Base64(String(data.get("body") ?? "")),
        }),
      );
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };
  const loadMoreLogs = async () => {
    if (logs?.nextCursor === null || logs === null) {
      return;
    }
    try {
      const page = await client.queryFunctionLogs(projectId, environmentId, functionName, {
        cursor: logs.nextCursor,
        limit: 100,
      });
      setLogs({ items: [...logs.items, ...page.items], nextCursor: page.nextCursor });
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };
  const deleteFunction = async () => {
    if (
      !confirmDestructiveAction({
        action: "Delete",
        target: `function ${functionName}`,
        consequence: "The stable function route and future invocations will be disabled.",
      })
    ) {
      return;
    }
    try {
      setItem(await client.deleteFunction(projectId, environmentId, functionName));
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };

  return (
    <section aria-labelledby="function-title">
      <button type="button" className="back-link" onClick={onBack}>
        ← Functions
      </button>
      <div className="section-heading">
        <div>
          <p className="eyebrow">Edge function</p>
          <h1 id="function-title">{functionName}</h1>
        </div>
        {item === null ? null : <LifecycleBadge state={item.state} />}
      </div>
      <ApiFailureNotice failure={failure} />
      {item === null ? (
        <p>Loading function…</p>
      ) : (
        <>
          <section className="panel" aria-labelledby="configuration-title">
            <div className="button-row spread">
              <div>
                <h2 id="configuration-title">Configuration</h2>
                <p>
                  {item.configuration.verifyJwt
                    ? "JWT verification is required."
                    : "Public invocation is enabled."}
                </p>
              </div>
              <button
                type="button"
                className="danger-link"
                disabled={item.state === "deleted"}
                onClick={() => void deleteFunction()}
              >
                Delete function
              </button>
            </div>
            <form key={item.updatedAt} onSubmit={(event) => void configure(event)}>
              <ConfigurationFields configuration={item.configuration} />
              <button type="submit">Save configuration</button>
            </form>
          </section>
          <div className="split-grid stacked-section">
            <BundleUploadPanel
              projectId={projectId}
              environmentId={environmentId}
              onReady={setArtifact}
              onFailure={setFailure}
            />
            <section className="panel" aria-labelledby="deployment-title">
              <h2 id="deployment-title">Deploy immutable version</h2>
              <form key={artifact?.digest ?? "manual"} onSubmit={(event) => void deploy(event)}>
                <label>
                  Version
                  <input
                    name="version"
                    type="number"
                    min="1"
                    defaultValue={(deployments?.at(-1)?.version ?? 0) + 1}
                    required
                  />
                </label>
                <label>
                  Bundle digest
                  <input
                    name="bundleDigest"
                    required
                    pattern="sha256:[a-f0-9]{64}"
                    defaultValue={artifact?.digest}
                  />
                </label>
                <label>
                  Entrypoint
                  <input name="entrypoint" required defaultValue={artifact?.entrypoint} />
                </label>
                <label>
                  Runtime version
                  <input name="runtimeVersion" required placeholder="deno-compatible-v1" />
                </label>
                <button type="submit">Validate and deploy</button>
              </form>
            </section>
          </div>
          <DeploymentsPanel
            deployments={deployments}
            activeVersion={item.activeVersion}
            onPromote={(version) => void switchVersion(version, "promote")}
            onRollback={(version) => void switchVersion(version, "rollback")}
            onCheckHealth={(version) => void checkHealth(version)}
            onDelete={(version) => void removeVersion(version)}
          />
          <FunctionSchedulesPanel
            projectId={projectId}
            environmentId={environmentId}
            functionName={functionName}
            activeVersion={item.activeVersion}
          />
          <div className="split-grid stacked-section">
            <TestInvocationPanel response={testResponse} onSubmit={test} />
            <FunctionMetricsPanel page={metrics} functionName={functionName} />
          </div>
          <FunctionLogsPanel logs={logs} onLoadMore={() => void loadMoreLogs()} />
        </>
      )}
    </section>
  );
}

function BundleUploadPanel({
  projectId,
  environmentId,
  onReady,
  onFailure,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly onReady: (artifact: FunctionBundleArtifact) => void;
  readonly onFailure: (failure: ConsoleApiFailure | null) => void;
}) {
  const client = useManagementClient();
  const [kind, setKind] = useState<"source" | "prebuilt">("source");
  const [diagnostics, setDiagnostics] = useState<
    readonly { code: string; message: string; path?: string; line?: number }[]
  >([]);
  const [artifact, setArtifact] = useState<FunctionBundleArtifact | null>(null);
  const upload = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    try {
      const files = data.getAll("files").filter((value): value is File => value instanceof File);
      if (files.length === 0 || files.some((file) => file.size === 0)) {
        throw new FormInputError("Select at least one non-empty source or bundle file.");
      }
      const entrypoint = requiredText(data, "entrypoint");
      const input: FunctionBundleUploadRequest =
        kind === "source"
          ? {
              kind: "source",
              entrypoint,
              files: await Promise.all(
                files.map(async (file) => ({
                  path: file.webkitRelativePath || file.name,
                  contentBase64: await fileBase64(file),
                })),
              ),
              dependencies: parseStringRecord(requiredText(data, "dependencies"), "Dependencies"),
            }
          : {
              kind: "prebuilt",
              entrypoint,
              bundleBase64: await fileBase64(files[0] as File),
            };
      const result = await client.uploadFunctionBundle(
        projectId,
        environmentId,
        input,
        idempotencyKey(),
      );
      setDiagnostics(result.diagnostics);
      setArtifact(result.artifact ?? null);
      if (result.artifact !== undefined) {
        onReady(result.artifact);
      }
      onFailure(null);
    } catch (error) {
      onFailure(failureFrom(error));
    }
  };
  return (
    <section className="panel" aria-labelledby="bundle-upload-title">
      <h2 id="bundle-upload-title">Upload bundle</h2>
      <p>Files are validated, deterministically archived, and stored by SHA-256 digest.</p>
      <form onSubmit={(event) => void upload(event)}>
        <label>
          Upload type
          <select
            value={kind}
            onChange={(event) => setKind(event.currentTarget.value as "source" | "prebuilt")}
          >
            <option value="source">TypeScript/JavaScript source</option>
            <option value="prebuilt">Prebuilt bundle</option>
          </select>
        </label>
        <label>
          {kind === "source" ? "Source files" : "Prebuilt bundle file"}
          <input name="files" type="file" multiple={kind === "source"} required />
        </label>
        <label>
          Entrypoint path
          <input name="entrypoint" required placeholder="index.ts" />
        </label>
        {kind === "prebuilt" ? null : (
          <label>
            Dependency mappings (JSON: import specifier to uploaded path)
            <textarea name="dependencies" rows={5} required defaultValue="{}" spellCheck={false} />
          </label>
        )}
        <button type="submit">Upload and validate</button>
      </form>
      {diagnostics.length === 0 ? null : (
        <ul className="diagnostic-list notice error" role="alert">
          {diagnostics.map((diagnostic) => (
            <li key={JSON.stringify(diagnostic)}>
              <strong>{diagnostic.code}</strong>: {diagnostic.message}
              {diagnostic.path === undefined ? null : ` (${diagnostic.path}`}
              {diagnostic.line === undefined ? null : `:${diagnostic.line}`}
              {diagnostic.path === undefined ? null : ")"}
            </li>
          ))}
        </ul>
      )}
      {artifact === null ? null : (
        <div className="notice success" role="status">
          <strong>Immutable bundle ready</strong>
          <p>
            {artifact.format.replaceAll("_", " ")} · {artifact.moduleCount} module(s) ·{" "}
            {artifact.sizeBytes.toLocaleString()} bytes
          </p>
          <code>{artifact.digest}</code>
        </div>
      )}
    </section>
  );
}

function DeploymentsPanel({
  deployments,
  activeVersion,
  onPromote,
  onRollback,
  onCheckHealth,
  onDelete,
}: {
  readonly deployments: FunctionDeployment[] | null;
  readonly activeVersion: number | null;
  readonly onPromote: (version: number) => void;
  readonly onRollback: (version: number) => void;
  readonly onCheckHealth: (version: number) => void;
  readonly onDelete: (version: number) => void;
}) {
  return (
    <section className="panel full-span stacked-section" aria-labelledby="versions-title">
      <h2 id="versions-title">Deployment versions</h2>
      {deployments === null ? (
        <p>Loading deployments…</p>
      ) : deployments.length === 0 ? (
        <p>No versions have been deployed.</p>
      ) : (
        <div className="card-grid">
          {deployments.map((deployment) => (
            <article className="resource-card" key={deployment.version}>
              <div className="button-row spread">
                <strong>Version {deployment.version}</strong>
                <LifecycleBadge
                  state={activeVersion === deployment.version ? "active" : deployment.state}
                />
              </div>
              <small>{deployment.runtimeVersion}</small>
              <code>{deployment.bundleDigest}</code>
              <p>
                {deployment.bundleFormat.replaceAll("_", " ")} ·{" "}
                {deployment.bundleSizeBytes.toLocaleString()} bytes
              </p>
              {deployment.diagnostic === null ? null : (
                <p className="notice error" role="alert">
                  {deployment.diagnostic}
                </p>
              )}
              <details>
                <summary>Version configuration</summary>
                <pre className="json-preview">
                  {JSON.stringify(deployment.configuration, null, 2)}
                </pre>
              </details>
              <div className="button-row">
                <button
                  type="button"
                  className="secondary"
                  disabled={deployment.state === "deleting"}
                  onClick={() => onCheckHealth(deployment.version)}
                >
                  Check health
                </button>
                <button
                  type="button"
                  disabled={deployment.state !== "healthy" || activeVersion === deployment.version}
                  onClick={() => onPromote(deployment.version)}
                >
                  Promote
                </button>
                <button
                  type="button"
                  className="secondary"
                  disabled={deployment.state !== "healthy" || activeVersion === deployment.version}
                  onClick={() => onRollback(deployment.version)}
                >
                  Roll back
                </button>
                <button
                  type="button"
                  className="danger-link"
                  disabled={activeVersion === deployment.version || deployment.state === "deleting"}
                  onClick={() => onDelete(deployment.version)}
                >
                  Delete version
                </button>
              </div>
            </article>
          ))}
        </div>
      )}
    </section>
  );
}

function TestInvocationPanel({
  response,
  onSubmit,
}: {
  readonly response: FunctionTestResponse | null;
  readonly onSubmit: (event: FormEvent<HTMLFormElement>) => void;
}) {
  return (
    <section className="panel" aria-labelledby="test-title">
      <h2 id="test-title">Test invocation</h2>
      <form onSubmit={onSubmit}>
        <label>
          Version (blank uses active)
          <input name="version" type="number" min="1" />
        </label>
        <label>
          Method
          <select name="method" defaultValue="POST">
            {["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"].map((method) => (
              <option value={method} key={method}>
                {method}
              </option>
            ))}
          </select>
        </label>
        <label>
          Path
          <input name="path" required defaultValue="/" maxLength={2048} />
        </label>
        <label>
          Headers (JSON)
          <textarea name="headers" rows={4} required defaultValue="{}" spellCheck={false} />
        </label>
        <label>
          Body
          <textarea name="body" rows={5} />
        </label>
        <button type="submit">Invoke test</button>
      </form>
      {response === null ? null : (
        <article className="workflow-card" aria-live="polite">
          <div className="button-row spread">
            <strong>HTTP {response.status}</strong>
            <code>{response.correlationId}</code>
          </div>
          <pre className="json-preview">{JSON.stringify(response.headers, null, 2)}</pre>
          <pre className="json-preview">{decodeBase64Text(response.body)}</pre>
        </article>
      )}
    </section>
  );
}

function FunctionMetricsPanel({
  page,
  functionName,
}: {
  readonly page: ObservabilityPage | null;
  readonly functionName: string;
}) {
  const metrics =
    page?.items.flatMap((record) =>
      record.payload.kind === "function_metric" && record.payload.functionName === functionName
        ? [{ timestamp: record.timestamp, metric: record.payload }]
        : [],
    ) ?? [];
  return (
    <section className="panel" aria-labelledby="function-metrics-title">
      <h2 id="function-metrics-title">Metrics</h2>
      {page === null ? (
        <p>Loading metrics…</p>
      ) : metrics.length === 0 ? (
        <p>No retained metrics for this function.</p>
      ) : (
        <ul className="signal-list">
          {metrics.map(({ timestamp, metric }) => (
            <li key={`${timestamp}:${metric.version}:${metric.region}`}>
              <strong>
                v{metric.version} in {metric.region}
              </strong>
              <p>
                {metric.invocationCount} calls · {metric.errorCount} errors ·{" "}
                {metric.latencyMilliseconds} ms latency · {metric.computeMilliseconds} ms compute
              </p>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}

function FunctionLogsPanel({
  logs,
  onLoadMore,
}: {
  readonly logs: FunctionLogPage | null;
  readonly onLoadMore: () => void;
}) {
  return (
    <section className="panel full-span stacked-section" aria-labelledby="function-logs-title">
      <h2 id="function-logs-title">Sanitized logs</h2>
      {logs === null ? (
        <p>Loading logs…</p>
      ) : logs.items.length === 0 ? (
        <p>No retained log entries.</p>
      ) : (
        <div className="table-scroll">
          <table>
            <thead>
              <tr>
                <th scope="col">Time</th>
                <th scope="col">Level</th>
                <th scope="col">Version / region</th>
                <th scope="col">Message</th>
                <th scope="col">Correlation</th>
              </tr>
            </thead>
            <tbody>
              {logs.items.map((entry) => (
                <tr key={`${entry.timestamp}:${entry.correlationId}:${entry.version}`}>
                  <td>{new Date(entry.timestamp).toLocaleString()}</td>
                  <td>{entry.level}</td>
                  <td>
                    v{entry.version} / {entry.region}
                  </td>
                  <td>{entry.message}</td>
                  <td>
                    <code>{entry.correlationId}</code>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      <button
        type="button"
        className="secondary"
        disabled={logs?.nextCursor == null}
        onClick={onLoadMore}
      >
        Load more logs
      </button>
    </section>
  );
}

function ConfigurationFields({
  configuration,
}: {
  readonly configuration?: FunctionConfiguration;
}) {
  const limits = configuration?.limits ?? DEFAULT_LIMITS;
  return (
    <>
      <label className="checkbox-label">
        <input name="verifyJwt" type="checkbox" defaultChecked={configuration?.verifyJwt ?? true} />
        Require a valid project JWT
      </label>
      <label>
        Regions (comma-separated)
        <input name="regions" required defaultValue={configuration?.regions.join(",") ?? "local"} />
      </label>
      <label>
        Attached secret names (comma-separated)
        <input name="secretNames" defaultValue={configuration?.secretNames.join(",") ?? ""} />
      </label>
      <div className="limit-grid">
        <NumberField
          name="cpuMilliseconds"
          label="CPU milliseconds"
          value={limits.cpuMilliseconds}
        />
        <NumberField
          name="wallMilliseconds"
          label="Wall milliseconds"
          value={limits.wallMilliseconds}
        />
        <NumberField name="memoryBytes" label="Memory bytes" value={limits.memoryBytes} />
        <NumberField name="requestBytes" label="Request bytes" value={limits.requestBytes} />
        <NumberField name="responseBytes" label="Response bytes" value={limits.responseBytes} />
        <NumberField name="concurrency" label="Concurrency" value={limits.concurrency} />
      </div>
    </>
  );
}

function NumberField({
  name,
  label,
  value,
}: {
  readonly name: string;
  readonly label: string;
  readonly value: number;
}) {
  return (
    <label>
      {label}
      <input name={name} type="number" min="1" defaultValue={value} required />
    </label>
  );
}

class FormInputError extends Error {}

function configurationFrom(data: FormData): FunctionConfiguration {
  return {
    verifyJwt: data.get("verifyJwt") === "on",
    regions: commaList(requiredText(data, "regions"), "regions"),
    secretNames: optionalCommaList(data, "secretNames"),
    limits: {
      cpuMilliseconds: positiveInteger(data, "cpuMilliseconds"),
      wallMilliseconds: positiveInteger(data, "wallMilliseconds"),
      memoryBytes: positiveInteger(data, "memoryBytes"),
      requestBytes: positiveInteger(data, "requestBytes"),
      responseBytes: positiveInteger(data, "responseBytes"),
      concurrency: positiveInteger(data, "concurrency"),
    },
  };
}

function requiredText(data: FormData, name: string): string {
  const value = String(data.get(name) ?? "").trim();
  if (value === "") {
    throw new FormInputError(`${name} is required.`);
  }
  return value;
}

function positiveInteger(data: FormData, name: string): number {
  const value = Number(requiredText(data, name));
  if (!Number.isSafeInteger(value) || value < 1) {
    throw new FormInputError(`${name} must be a positive integer.`);
  }
  return value;
}

function optionalPositiveInteger(data: FormData, name: string): number | null {
  const raw = String(data.get(name) ?? "").trim();
  if (raw === "") {
    return null;
  }
  const value = Number(raw);
  if (!Number.isSafeInteger(value) || value < 1) {
    throw new FormInputError(`${name} must be a positive integer.`);
  }
  return value;
}

function commaList(value: string, label: string): string[] {
  const items = value
    .split(",")
    .map((item) => item.trim())
    .filter((item) => item !== "");
  if (items.length === 0 || new Set(items).size !== items.length) {
    throw new FormInputError(`${label} must contain unique comma-separated values.`);
  }
  return items;
}

function optionalCommaList(data: FormData, name: string): string[] {
  const value = String(data.get(name) ?? "").trim();
  return value === "" ? [] : commaList(value, name);
}

function parseStringRecord(raw: string, label: string): Record<string, string> {
  let value: unknown;
  try {
    value = JSON.parse(raw) as unknown;
  } catch {
    throw new FormInputError(`${label} must be valid JSON.`);
  }
  if (
    typeof value !== "object" ||
    value === null ||
    Array.isArray(value) ||
    !Object.values(value).every((entry) => typeof entry === "string")
  ) {
    throw new FormInputError(`${label} must be a JSON object with string values.`);
  }
  return value as Record<string, string>;
}

async function fileBase64(file: File): Promise<string> {
  if (file.size > 10 * 1024 * 1024) {
    throw new FormInputError("Each selected file must be at most 10 MiB.");
  }
  return bytesBase64(new Uint8Array(await file.arrayBuffer()));
}

function utf8Base64(value: string): string {
  return bytesBase64(new TextEncoder().encode(value));
}

function bytesBase64(bytes: Uint8Array): string {
  let binary = "";
  for (let offset = 0; offset < bytes.length; offset += 32_768) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + 32_768));
  }
  return globalThis.btoa(binary);
}

function decodeBase64Text(value: string): string {
  try {
    const binary = globalThis.atob(value);
    const bytes = Uint8Array.from(binary, (character) => character.charCodeAt(0));
    return new TextDecoder().decode(bytes);
  } catch {
    return "Response body is not valid base64 text.";
  }
}

function failureFrom(error: unknown): ConsoleApiFailure {
  return error instanceof FormInputError
    ? { message: error.message, requestId: null }
    : toConsoleApiFailure(error);
}

function idempotencyKey(): string {
  return globalThis.crypto.randomUUID();
}
