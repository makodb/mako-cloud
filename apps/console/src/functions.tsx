import {
  Alert,
  AlertDescription,
  AlertTitle,
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
  NativeSelect,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  Textarea,
} from "@mako-cloud/ui";
import { CircleCheck, Zap } from "lucide-react";
import { type FormEvent, type ReactNode, useCallback, useEffect, useId, useState } from "react";

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

/** A code snippet inline in prose or a cell: an identifier, a digest, a path. */
const CODE = "rounded bg-muted px-1 py-0.5 font-mono text-[0.85em]";
/** A block of JSON or a response body. */
const PRE = "m-0 max-h-72 overflow-auto rounded-md bg-muted p-3 font-mono text-xs leading-relaxed";

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
  const nameId = useId();
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
    <section aria-labelledby="functions-title" className="grid gap-6">
      <div className="grid gap-2">
        <BackLink onClick={onBack}>← Project</BackLink>
        <div className="grid gap-1">
          <Eyebrow>Environment {environmentId}</Eyebrow>
          <h1 id="functions-title" className="text-2xl">
            Edge functions
          </h1>
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      <div className="grid items-start gap-6 lg:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
        <Card aria-labelledby="function-list-title">
          <CardHeader>
            <CardTitle id="function-list-title">Functions</CardTitle>
          </CardHeader>
          <CardContent>
            {functions === null ? (
              <p className="m-0 text-sm text-muted-foreground">Loading functions…</p>
            ) : functions.length === 0 ? (
              <EmptyState
                icon={<Zap aria-hidden="true" />}
                title="No functions have been created."
                className="border-0 py-8"
              />
            ) : (
              <ul className="m-0 grid list-none gap-1 p-0">
                {functions.map((item) => (
                  <li key={item.name}>
                    <Button
                      variant="ghost"
                      className="h-auto w-full justify-between gap-3 px-3 py-2.5 text-left font-normal"
                      onClick={() => onOpen(item.name)}
                    >
                      <span className="grid min-w-0 gap-0.5">
                        <strong className="truncate font-mono text-sm font-semibold">
                          {item.name}
                        </strong>
                        <small className="text-xs text-muted-foreground">
                          {item.activeVersion === null
                            ? "No active deployment"
                            : `Active v${item.activeVersion}`}
                        </small>
                      </span>
                      <LifecycleBadge state={item.state} />
                    </Button>
                  </li>
                ))}
              </ul>
            )}
          </CardContent>
        </Card>
        <Card aria-labelledby="create-function-title">
          <CardHeader>
            <CardTitle id="create-function-title">Create function</CardTitle>
          </CardHeader>
          <CardContent>
            <form className="grid gap-4" onSubmit={(event) => void create(event)}>
              <Field label="Function name" htmlFor={nameId}>
                <Input
                  id={nameId}
                  name="name"
                  required
                  pattern="[a-z][a-z0-9\-]{0,62}"
                  className="font-mono"
                  spellCheck={false}
                />
              </Field>
              <ConfigurationFields />
              <div>
                <Button type="submit">Create function</Button>
              </div>
            </form>
          </CardContent>
        </Card>
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
  const deployId = useId();
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
    <section aria-labelledby="function-title" className="grid gap-6">
      <div className="grid gap-2">
        <BackLink onClick={onBack}>← Functions</BackLink>
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div className="grid gap-1">
            <Eyebrow>Edge function</Eyebrow>
            <h1 id="function-title" className="font-mono text-2xl">
              {functionName}
            </h1>
          </div>
          {item === null ? null : <LifecycleBadge state={item.state} />}
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      {item === null ? (
        <p className="m-0 text-sm text-muted-foreground">Loading function…</p>
      ) : (
        <>
          <Card aria-labelledby="configuration-title">
            <CardHeader>
              <CardTitle id="configuration-title">Configuration</CardTitle>
              <CardDescription>
                {item.configuration.verifyJwt
                  ? "JWT verification is required."
                  : "Public invocation is enabled."}
              </CardDescription>
              <CardAction>
                <DangerButton
                  disabled={item.state === "deleted"}
                  onClick={() => void deleteFunction()}
                >
                  Delete function
                </DangerButton>
              </CardAction>
            </CardHeader>
            <CardContent>
              <form
                key={item.updatedAt}
                className="grid gap-4"
                onSubmit={(event) => void configure(event)}
              >
                <ConfigurationFields configuration={item.configuration} />
                <div>
                  <Button type="submit">Save configuration</Button>
                </div>
              </form>
            </CardContent>
          </Card>
          <div className="grid items-start gap-6 lg:grid-cols-2">
            <BundleUploadPanel
              projectId={projectId}
              environmentId={environmentId}
              onReady={setArtifact}
              onFailure={setFailure}
            />
            <Card aria-labelledby="deployment-title">
              <CardHeader>
                <CardTitle id="deployment-title">Deploy immutable version</CardTitle>
              </CardHeader>
              <CardContent>
                <form
                  key={artifact?.digest ?? "manual"}
                  className="grid gap-4"
                  onSubmit={(event) => void deploy(event)}
                >
                  <Field label="Version" htmlFor={`${deployId}-version`}>
                    <Input
                      id={`${deployId}-version`}
                      name="version"
                      type="number"
                      min="1"
                      defaultValue={Math.max(
                        (deployments?.at(-1)?.version ?? 0) + 1,
                        item?.nextVersion ?? 0,
                      )}
                      required
                    />
                  </Field>
                  <Field label="Bundle digest" htmlFor={`${deployId}-digest`}>
                    <Input
                      id={`${deployId}-digest`}
                      name="bundleDigest"
                      required
                      pattern="sha256:[a-f0-9]{64}"
                      defaultValue={artifact?.digest}
                      className="font-mono"
                      spellCheck={false}
                    />
                  </Field>
                  <Field label="Entrypoint" htmlFor={`${deployId}-entrypoint`}>
                    <Input
                      id={`${deployId}-entrypoint`}
                      name="entrypoint"
                      required
                      defaultValue={artifact?.entrypoint}
                      className="font-mono"
                      spellCheck={false}
                    />
                  </Field>
                  <Field label="Runtime version" htmlFor={`${deployId}-runtime`}>
                    <Input
                      id={`${deployId}-runtime`}
                      name="runtimeVersion"
                      required
                      placeholder="deno-compatible-v1"
                      className="font-mono"
                      spellCheck={false}
                    />
                  </Field>
                  <div>
                    <Button type="submit">Validate and deploy</Button>
                  </div>
                </form>
              </CardContent>
            </Card>
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
          <div className="grid items-start gap-6 lg:grid-cols-2">
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
  const id = useId();
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
    <Card aria-labelledby="bundle-upload-title">
      <CardHeader>
        <CardTitle id="bundle-upload-title">Upload bundle</CardTitle>
        <CardDescription>
          Files are validated, deterministically archived, and stored by SHA-256 digest.
        </CardDescription>
      </CardHeader>
      <CardContent className="grid gap-4">
        <form className="grid gap-4" onSubmit={(event) => void upload(event)}>
          <Field label="Upload type" htmlFor={`${id}-kind`}>
            <NativeSelect
              id={`${id}-kind`}
              value={kind}
              onChange={(event) => setKind(event.currentTarget.value as "source" | "prebuilt")}
            >
              <option value="source">TypeScript/JavaScript source</option>
              <option value="prebuilt">Prebuilt bundle</option>
            </NativeSelect>
          </Field>
          <Field
            label={kind === "source" ? "Source files" : "Prebuilt bundle file"}
            htmlFor={`${id}-files`}
          >
            <Input
              id={`${id}-files`}
              name="files"
              type="file"
              multiple={kind === "source"}
              required
            />
          </Field>
          <Field label="Entrypoint path" htmlFor={`${id}-entrypoint`}>
            <Input
              id={`${id}-entrypoint`}
              name="entrypoint"
              required
              placeholder="index.ts"
              className="font-mono"
              spellCheck={false}
            />
          </Field>
          {kind === "prebuilt" ? null : (
            <Field
              label="Dependency mappings (JSON: import specifier to uploaded path)"
              htmlFor={`${id}-dependencies`}
            >
              <Textarea
                id={`${id}-dependencies`}
                name="dependencies"
                rows={5}
                required
                defaultValue="{}"
                spellCheck={false}
                className="font-mono text-xs"
              />
            </Field>
          )}
          <div>
            <Button type="submit">Upload and validate</Button>
          </div>
        </form>
        {diagnostics.length === 0 ? null : (
          <Alert variant="destructive" role="alert">
            <AlertDescription className="block">
              <ul className="m-0 grid list-none gap-1 p-0">
                {diagnostics.map((diagnostic) => (
                  <li key={JSON.stringify(diagnostic)}>
                    <strong className="font-mono">{diagnostic.code}</strong>: {diagnostic.message}
                    {diagnostic.path === undefined ? null : ` (${diagnostic.path}`}
                    {diagnostic.line === undefined ? null : `:${diagnostic.line}`}
                    {diagnostic.path === undefined ? null : ")"}
                  </li>
                ))}
              </ul>
            </AlertDescription>
          </Alert>
        )}
        {artifact === null ? null : (
          <Alert variant="positive" role="status">
            <CircleCheck aria-hidden="true" />
            <AlertTitle>Immutable bundle ready</AlertTitle>
            <AlertDescription className="block">
              <p className="m-0">
                {artifact.format.replaceAll("_", " ")} · {artifact.moduleCount} module(s) ·{" "}
                {artifact.sizeBytes.toLocaleString()} bytes
              </p>
              <code className="mt-1 block font-mono text-xs break-all">{artifact.digest}</code>
            </AlertDescription>
          </Alert>
        )}
      </CardContent>
    </Card>
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
    <Card aria-labelledby="versions-title">
      <CardHeader>
        <CardTitle id="versions-title">Deployment versions</CardTitle>
      </CardHeader>
      <CardContent>
        {deployments === null ? (
          <p className="m-0 text-sm text-muted-foreground">Loading deployments…</p>
        ) : deployments.length === 0 ? (
          <p className="m-0 text-sm text-muted-foreground">No versions have been deployed.</p>
        ) : (
          <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
            {deployments.map((deployment) => (
              <article
                className="resource-card grid gap-3 rounded-lg border bg-card p-4 text-sm text-card-foreground"
                key={deployment.version}
              >
                <div className="flex items-center justify-between gap-3">
                  <strong className="font-semibold">Version {deployment.version}</strong>
                  <LifecycleBadge
                    state={activeVersion === deployment.version ? "active" : deployment.state}
                  />
                </div>
                <small className="font-mono text-xs text-muted-foreground">
                  {deployment.runtimeVersion}
                </small>
                <code className="block font-mono text-xs break-all">{deployment.bundleDigest}</code>
                <p className="m-0 text-muted-foreground">
                  {deployment.bundleFormat.replaceAll("_", " ")} ·{" "}
                  {deployment.bundleSizeBytes.toLocaleString()} bytes
                </p>
                {deployment.diagnostic === null ? null : (
                  <Alert variant="destructive" role="alert">
                    <AlertDescription className="block">{deployment.diagnostic}</AlertDescription>
                  </Alert>
                )}
                <details>
                  <summary className="cursor-pointer text-muted-foreground hover:text-foreground">
                    Version configuration
                  </summary>
                  <pre className={`mt-2 ${PRE}`}>
                    {JSON.stringify(deployment.configuration, null, 2)}
                  </pre>
                </details>
                <div className="flex flex-wrap gap-2">
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={deployment.state === "deleting"}
                    onClick={() => onCheckHealth(deployment.version)}
                  >
                    Check health
                  </Button>
                  <Button
                    size="sm"
                    disabled={
                      deployment.state !== "healthy" || activeVersion === deployment.version
                    }
                    onClick={() => onPromote(deployment.version)}
                  >
                    Promote
                  </Button>
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={
                      deployment.state !== "healthy" || activeVersion === deployment.version
                    }
                    onClick={() => onRollback(deployment.version)}
                  >
                    Roll back
                  </Button>
                  <DangerButton
                    disabled={
                      activeVersion === deployment.version || deployment.state === "deleting"
                    }
                    onClick={() => onDelete(deployment.version)}
                  >
                    Delete version
                  </DangerButton>
                </div>
              </article>
            ))}
          </div>
        )}
      </CardContent>
    </Card>
  );
}

function TestInvocationPanel({
  response,
  onSubmit,
}: {
  readonly response: FunctionTestResponse | null;
  readonly onSubmit: (event: FormEvent<HTMLFormElement>) => void;
}) {
  const id = useId();
  return (
    <Card aria-labelledby="test-title">
      <CardHeader>
        <CardTitle id="test-title">Test invocation</CardTitle>
      </CardHeader>
      <CardContent className="grid gap-4">
        <form className="grid gap-4" onSubmit={onSubmit}>
          <div className="grid gap-4 sm:grid-cols-[minmax(0,1fr)_minmax(8rem,10rem)]">
            <Field label="Version (blank uses active)" htmlFor={`${id}-version`}>
              <Input id={`${id}-version`} name="version" type="number" min="1" />
            </Field>
            <Field label="Method" htmlFor={`${id}-method`}>
              <NativeSelect id={`${id}-method`} name="method" defaultValue="POST">
                {["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"].map((method) => (
                  <option value={method} key={method}>
                    {method}
                  </option>
                ))}
              </NativeSelect>
            </Field>
          </div>
          <Field label="Path" htmlFor={`${id}-path`}>
            <Input
              id={`${id}-path`}
              name="path"
              required
              defaultValue="/"
              maxLength={2048}
              className="font-mono"
              spellCheck={false}
            />
          </Field>
          <Field label="Headers (JSON)" htmlFor={`${id}-headers`}>
            <Textarea
              id={`${id}-headers`}
              name="headers"
              rows={4}
              required
              defaultValue="{}"
              spellCheck={false}
              className="font-mono text-xs"
            />
          </Field>
          <Field label="Body" htmlFor={`${id}-body`}>
            <Textarea id={`${id}-body`} name="body" rows={5} className="font-mono text-xs" />
          </Field>
          <div>
            <Button type="submit">Invoke test</Button>
          </div>
        </form>
        {response === null ? null : (
          <article className="grid gap-2 rounded-lg border bg-muted/40 p-4" aria-live="polite">
            <div className="flex flex-wrap items-center justify-between gap-3 text-sm">
              <strong className="font-semibold">HTTP {response.status}</strong>
              <code className={CODE}>{response.correlationId}</code>
            </div>
            <pre className={PRE}>{JSON.stringify(response.headers, null, 2)}</pre>
            <pre className={PRE}>{decodeBase64Text(response.body)}</pre>
          </article>
        )}
      </CardContent>
    </Card>
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
    <Card aria-labelledby="function-metrics-title">
      <CardHeader>
        <CardTitle id="function-metrics-title">Metrics</CardTitle>
      </CardHeader>
      <CardContent>
        {page === null ? (
          <p className="m-0 text-sm text-muted-foreground">Loading metrics…</p>
        ) : metrics.length === 0 ? (
          <p className="m-0 text-sm text-muted-foreground">
            No retained metrics for this function.
          </p>
        ) : (
          <ul className="m-0 grid list-none gap-3 p-0 text-sm">
            {metrics.map(({ timestamp, metric }, position) => (
              <li
                key={`${timestamp}:${metric.version}:${metric.region}:${position}`}
                className="grid gap-0.5 border-b pb-3 last:border-0 last:pb-0"
              >
                <strong className="font-mono text-xs font-semibold">
                  v{metric.version} in {metric.region}
                </strong>
                <p className="m-0 text-muted-foreground tabular-nums">
                  {metric.invocationCount.toLocaleString()} calls ·{" "}
                  {metric.errorCount.toLocaleString()} errors ·{" "}
                  {metric.latencyMilliseconds.toLocaleString()} ms latency ·{" "}
                  {metric.computeMilliseconds.toLocaleString()} ms compute
                </p>
              </li>
            ))}
          </ul>
        )}
      </CardContent>
    </Card>
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
    <Card aria-labelledby="function-logs-title">
      <CardHeader>
        <CardTitle id="function-logs-title">Sanitized logs</CardTitle>
      </CardHeader>
      <CardContent className="grid gap-4">
        {logs === null ? (
          <p className="m-0 text-sm text-muted-foreground">Loading logs…</p>
        ) : logs.items.length === 0 ? (
          <p className="m-0 text-sm text-muted-foreground">No retained log entries.</p>
        ) : (
          <Table>
            <TableHeader>
              <TableRow className="hover:bg-transparent">
                <TableHead scope="col">Time</TableHead>
                <TableHead scope="col">Level</TableHead>
                <TableHead scope="col">Version / region</TableHead>
                <TableHead scope="col">Message</TableHead>
                <TableHead scope="col">Correlation</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {/* One invocation writes several lines with the same time,
                  correlation id, and version, so only the position tells
                  them apart. */}
              {logs.items.map((entry, position) => (
                <TableRow
                  key={`${entry.timestamp}:${entry.correlationId}:${entry.version}:${position}`}
                >
                  <TableCell className="align-top text-muted-foreground tabular-nums">
                    {new Date(entry.timestamp).toLocaleString()}
                  </TableCell>
                  <TableCell className="align-top font-mono text-xs uppercase">
                    {entry.level}
                  </TableCell>
                  <TableCell className="align-top font-mono text-xs">
                    v{entry.version} / {entry.region}
                  </TableCell>
                  <TableCell className="min-w-72 align-top font-mono text-xs whitespace-pre-wrap break-words">
                    {entry.message}
                  </TableCell>
                  <TableCell className="align-top">
                    <code className={CODE}>{entry.correlationId}</code>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
        <div>
          <Button
            variant="outline"
            size="sm"
            disabled={logs?.nextCursor == null}
            onClick={onLoadMore}
          >
            Load more logs
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}

function ConfigurationFields({
  configuration,
}: {
  readonly configuration?: FunctionConfiguration;
}) {
  const id = useId();
  const limits = configuration?.limits ?? DEFAULT_LIMITS;
  return (
    <>
      <div className="flex items-center gap-2">
        <Checkbox
          id={`${id}-verify-jwt`}
          name="verifyJwt"
          defaultChecked={configuration?.verifyJwt ?? true}
        />
        <Label htmlFor={`${id}-verify-jwt`} className="font-normal">
          Require a valid project JWT
        </Label>
      </div>
      <Field label="Regions (comma-separated)" htmlFor={`${id}-regions`}>
        <Input
          id={`${id}-regions`}
          name="regions"
          required
          defaultValue={configuration?.regions.join(",") ?? "local"}
          className="font-mono"
          spellCheck={false}
        />
      </Field>
      <Field label="Attached secret names (comma-separated)" htmlFor={`${id}-secrets`}>
        <Input
          id={`${id}-secrets`}
          name="secretNames"
          defaultValue={configuration?.secretNames.join(",") ?? ""}
          className="font-mono"
          spellCheck={false}
        />
      </Field>
      <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
        <NumberField
          id={`${id}-cpu`}
          name="cpuMilliseconds"
          label="CPU milliseconds"
          value={limits.cpuMilliseconds}
        />
        <NumberField
          id={`${id}-wall`}
          name="wallMilliseconds"
          label="Wall milliseconds"
          value={limits.wallMilliseconds}
        />
        <NumberField
          id={`${id}-memory`}
          name="memoryBytes"
          label="Memory bytes"
          value={limits.memoryBytes}
        />
        <NumberField
          id={`${id}-request`}
          name="requestBytes"
          label="Request bytes"
          value={limits.requestBytes}
        />
        <NumberField
          id={`${id}-response`}
          name="responseBytes"
          label="Response bytes"
          value={limits.responseBytes}
        />
        <NumberField
          id={`${id}-concurrency`}
          name="concurrency"
          label="Concurrency"
          value={limits.concurrency}
        />
      </div>
    </>
  );
}

function NumberField({
  id,
  name,
  label,
  value,
}: {
  readonly id: string;
  readonly name: string;
  readonly label: string;
  readonly value: number;
}) {
  return (
    <Field label={label} htmlFor={id}>
      <Input id={id} name={name} type="number" min="1" defaultValue={value} required />
    </Field>
  );
}

/** The quiet way back to the list this page came from. */
function BackLink({ onClick, children }: { readonly onClick: () => void; children: ReactNode }) {
  return (
    <Button
      variant="ghost"
      size="sm"
      className="-ml-2 w-fit text-muted-foreground hover:text-foreground"
      onClick={onClick}
    >
      {children}
    </Button>
  );
}

/** A destructive action that is not the page's main one: a text button in the destructive colour. */
function DangerButton({
  disabled,
  onClick,
  children,
}: {
  readonly disabled?: boolean;
  readonly onClick: () => void;
  readonly children: ReactNode;
}) {
  return (
    <Button
      variant="ghost"
      size="sm"
      className="text-destructive hover:bg-destructive/10 hover:text-destructive"
      disabled={disabled}
      onClick={onClick}
    >
      {children}
    </Button>
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
