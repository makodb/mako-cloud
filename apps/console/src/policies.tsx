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
  Eyebrow,
  Field,
  Input,
  Textarea,
} from "@mako-cloud/ui";
import { AlertTriangle, CheckCircle2, ShieldCheck, XCircle } from "lucide-react";
import { type FormEvent, type ReactNode, useCallback, useEffect, useState } from "react";

import type {
  ActivePolicy,
  CreatePolicyDraftRequest,
  PolicyExample,
  PolicyExampleResult,
  PolicySet,
  PolicyValidation,
} from "@mako-cloud/management-sdk";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useManagementClient } from "./management.js";
import { LifecycleBadge } from "./projects.js";
import { confirmDestructiveAction } from "./safety.js";

const DEFAULT_RULES = JSON.stringify(
  [
    {
      id: "owner-read",
      effect: "allow",
      operations: ["read"],
      // Expressions name the documents old/new and the caller identity.user_id;
      // oldDocument and userId are the example-context JSON's names, and a
      // starter written with them failed validation (unknown_identifier).
      expression: "old.ownerId == identity.user_id",
    },
  ],
  null,
  2,
);

const DEFAULT_EXAMPLES = JSON.stringify(
  [
    {
      operation: "read",
      identity: {
        userId: "usr_example01",
        role: "authenticated",
        trustedClaims: {},
      },
      oldDocument: { id: "doc-1", ownerId: "usr_example01" },
      requestMetadata: {},
    },
  ],
  null,
  2,
);

/** JSON that is written: the code face inside a textarea, sized by its content within bounds. */
const JSON_FIELD = "min-h-56 max-h-[40rem] font-mono text-xs leading-5 md:text-xs";
/** JSON that is read. */
const JSON_BLOCK =
  "m-0 overflow-x-auto rounded-md border bg-muted/40 p-3 font-mono text-xs leading-5";

interface EvaluationResultView {
  readonly id: string;
  readonly result: PolicyExampleResult;
}

export function PolicyScreen({
  projectId,
  environmentId,
  collectionId,
  onBack,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly collectionId: string;
  readonly onBack: () => void;
}) {
  const client = useManagementClient();
  const [active, setActive] = useState<ActivePolicy | null>(null);
  const [selected, setSelected] = useState<PolicySet | null>(null);
  const [validation, setValidation] = useState<PolicyValidation | null>(null);
  const [testResults, setTestResults] = useState<EvaluationResultView[] | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  // The highest version that exists, drafts included. Only the active version
  // is listed, so the versions after it are probed until one is free.
  const [highestVersion, setHighestVersion] = useState(0);
  // Until the probing below finishes, the next free version is unknown: the
  // form used to offer version 1 meanwhile, and a draft submitted then was
  // refused because that version already existed.
  const [versionsKnown, setVersionsKnown] = useState(false);
  const reloadActive = useCallback(async () => {
    try {
      const next = await client.getActiveCollectionPolicy(projectId, environmentId, collectionId);
      let highest = next.policy?.version ?? 0;
      for (let probe = highest + 1; probe <= highest + 50; probe += 1) {
        const found = await client
          .getCollectionPolicy(projectId, environmentId, collectionId, probe)
          .then(
            () => true,
            () => false,
          );
        if (!found) break;
        highest = probe;
      }
      setHighestVersion(highest);
      setVersionsKnown(true);
      setActive(next);
      if (selected === null && next.policy !== undefined) {
        setSelected(next.policy);
      }
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  }, [client, collectionId, environmentId, projectId, selected]);
  useEffect(() => {
    void reloadActive();
  }, [reloadActive]);

  // The next version to offer: above every version that exists and any draft made here.
  // It is keyed into the input below, whose default was otherwise fixed at
  // first render -- before the active policy loaded -- and offered version 1
  // on a collection that already had it.
  const nextVersion =
    Math.max(highestVersion, active?.policy?.version ?? 0, selected?.version ?? 0) + 1;
  const createDraft = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    try {
      const policy = await client.createCollectionPolicyDraft(
        projectId,
        environmentId,
        collectionId,
        {
          version: positiveInteger(data, "version"),
          rules: parsePolicyRules(requiredText(data, "rules")),
        },
        idempotencyKey(),
      );
      setSelected(policy);
      setValidation(null);
      setTestResults(null);
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };
  const inspectVersion = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    try {
      setSelected(
        await client.getCollectionPolicy(
          projectId,
          environmentId,
          collectionId,
          positiveInteger(new FormData(event.currentTarget), "version"),
        ),
      );
      setValidation(null);
      setTestResults(null);
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };
  const validate = async () => {
    if (selected === null) {
      return;
    }
    try {
      const result = await client.validateCollectionPolicy(
        projectId,
        environmentId,
        collectionId,
        selected.version,
      );
      setValidation(result);
      setSelected(result.policy);
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };
  const testExamples = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    if (selected === null) {
      return;
    }
    try {
      const results = await client.testCollectionPolicy(
        projectId,
        environmentId,
        collectionId,
        selected.version,
        parsePolicyExamples(requiredText(new FormData(event.currentTarget), "examples")),
      );
      setTestResults(results.map((result) => ({ id: globalThis.crypto.randomUUID(), result })));
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };
  const changeActive = async (action: "activate" | "rollback") => {
    if (selected === null) {
      return;
    }
    if (
      !confirmDestructiveAction({
        action: action === "activate" ? "Activate" : "Roll back to",
        target: `policy version ${selected.version}`,
        consequence:
          "Document authorization changes immediately and the authorization epoch will advance.",
      })
    ) {
      return;
    }
    try {
      const next =
        action === "activate"
          ? await client.activateCollectionPolicy(
              projectId,
              environmentId,
              collectionId,
              selected.version,
              idempotencyKey(),
            )
          : await client.rollbackCollectionPolicy(
              projectId,
              environmentId,
              collectionId,
              selected.version,
              idempotencyKey(),
            );
      setActive(next);
      setSelected(next.policy ?? selected);
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };

  return (
    <section aria-labelledby="policy-title" className="grid gap-6">
      <div className="grid gap-3">
        <div>
          <Button
            variant="ghost"
            size="sm"
            className="-ml-2 text-muted-foreground hover:text-foreground"
            onClick={onBack}
          >
            ← Collection
          </Button>
        </div>
        <div className="flex flex-wrap items-end justify-between gap-4">
          <div>
            <Eyebrow>
              Collection{" "}
              <span className="font-mono normal-case tracking-normal">{collectionId}</span>
            </Eyebrow>
            <h1 id="policy-title" className="m-0 text-2xl font-semibold tracking-tight">
              Document policies
            </h1>
          </div>
          {active === null ? null : (
            <Badge variant="outline" className="tabular-nums">
              Authorization epoch {active.authorizationEpoch}
            </Badge>
          )}
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      <DefaultDenyNotice active={active} />
      <div className="grid items-start gap-6 lg:grid-cols-2">
        <Card aria-labelledby="draft-title">
          <CardHeader>
            <CardTitle id="draft-title">Create immutable draft</CardTitle>
            <CardDescription>
              Rules use the typed, CEL-compatible policy expression environment.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <form className="grid gap-4" onSubmit={(event) => void createDraft(event)}>
              <Field label="Policy version" htmlFor="draft-version">
                <Input
                  key={versionsKnown ? nextVersion : "unknown"}
                  id="draft-version"
                  name="version"
                  type="number"
                  min="1"
                  defaultValue={versionsKnown ? nextVersion : undefined}
                  placeholder={versionsKnown ? undefined : "Finding the next version…"}
                  disabled={!versionsKnown}
                  required
                  className="max-w-40"
                />
              </Field>
              <Field label="Rules (JSON)" htmlFor="draft-rules">
                <Textarea
                  id="draft-rules"
                  name="rules"
                  rows={18}
                  required
                  defaultValue={DEFAULT_RULES}
                  spellCheck={false}
                  className={JSON_FIELD}
                />
              </Field>
              <div>
                <Button type="submit" disabled={!versionsKnown}>
                  Create draft
                </Button>
              </div>
            </form>
          </CardContent>
        </Card>
        <Card aria-labelledby="versions-title">
          <CardHeader>
            <CardTitle id="versions-title">Policy versions</CardTitle>
          </CardHeader>
          <CardContent className="grid gap-4">
            <form
              className="flex flex-wrap items-end gap-2"
              onSubmit={(event) => void inspectVersion(event)}
            >
              <Field label="Inspect version" htmlFor="inspect-version">
                <Input
                  key={active?.policy?.version ?? 0}
                  id="inspect-version"
                  name="version"
                  type="number"
                  min="1"
                  defaultValue={active?.policy?.version}
                  required
                  className="w-32"
                />
              </Field>
              <Button type="submit" variant="outline">
                Load
              </Button>
            </form>
            <PolicyVersionCard policy={selected} />
            {selected === null ? null : (
              <div className="flex flex-wrap items-center gap-2">
                <Button onClick={() => void validate()}>
                  <ShieldCheck aria-hidden="true" />
                  Validate against schema
                </Button>
                <Button
                  variant="outline"
                  disabled={selected.state !== "validated"}
                  onClick={() => void changeActive("activate")}
                >
                  Activate
                </Button>
                <Button
                  variant="outline"
                  disabled={selected.state === "draft"}
                  onClick={() => void changeActive("rollback")}
                >
                  Roll back to this version
                </Button>
              </div>
            )}
            <ValidationResult validation={validation} />
          </CardContent>
        </Card>
      </div>
      <Card aria-labelledby="test-policy-title">
        <CardHeader>
          <CardTitle id="test-policy-title">Example evaluation</CardTitle>
          <CardDescription>Examples run without changing the active policy.</CardDescription>
        </CardHeader>
        <CardContent className="grid gap-4">
          {selected === null ? (
            <p className="m-0 text-sm text-muted-foreground">
              Select or create a policy version before testing.
            </p>
          ) : (
            <form className="grid gap-4" onSubmit={(event) => void testExamples(event)}>
              <Field label="Example contexts (JSON)" htmlFor="example-contexts">
                <Textarea
                  id="example-contexts"
                  name="examples"
                  rows={16}
                  required
                  defaultValue={DEFAULT_EXAMPLES}
                  spellCheck={false}
                  className={JSON_FIELD}
                />
              </Field>
              <div>
                <Button type="submit">Evaluate policy v{selected.version}</Button>
              </div>
            </form>
          )}
          <EvaluationTraces results={testResults} />
        </CardContent>
      </Card>
    </section>
  );
}

function DefaultDenyNotice({ active }: { readonly active: ActivePolicy | null }) {
  if (active === null) {
    return <p className="m-0 text-sm text-muted-foreground">Loading active policy state…</p>;
  }
  return (
    <Alert variant={active.defaultDeny ? "warning" : "positive"} role="status">
      {active.defaultDeny ? (
        <AlertTriangle aria-hidden="true" />
      ) : (
        <CheckCircle2 aria-hidden="true" />
      )}
      <AlertTitle>
        {active.defaultDeny ? "Default deny is active." : "An active policy is installed."}
      </AlertTitle>
      <AlertDescription>
        <p className="m-0">
          {active.defaultDeny
            ? "All create, read, update, and delete operations are denied until a validated policy is activated."
            : `Policy v${active.policy?.version ?? "unknown"} controls access. Activation or rollback advances the authorization epoch.`}
        </p>
      </AlertDescription>
    </Alert>
  );
}

function PolicyVersionCard({ policy }: { readonly policy: PolicySet | null }) {
  if (policy === null) {
    return <p className="m-0 text-sm text-muted-foreground">No policy version selected.</p>;
  }
  return (
    <Card as="article" className="gap-3 py-4">
      <CardContent className="grid gap-3 px-4">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <strong className="text-sm font-medium">Policy v{policy.version}</strong>
          <LifecycleBadge state={policy.state} />
        </div>
        <pre className={JSON_BLOCK}>{JSON.stringify(policy.rules, null, 2)}</pre>
      </CardContent>
    </Card>
  );
}

function ValidationResult({ validation }: { readonly validation: PolicyValidation | null }) {
  if (validation === null) {
    return null;
  }
  return (
    <Alert variant={validation.valid ? "positive" : "destructive"} role="status">
      {validation.valid ? <CheckCircle2 aria-hidden="true" /> : <XCircle aria-hidden="true" />}
      <AlertTitle>
        {validation.valid ? "Schema-aware validation passed." : "Validation failed."}
      </AlertTitle>
      <AlertDescription>
        {validation.policy.diagnostics.length === 0 ? (
          <p className="m-0">No diagnostics.</p>
        ) : (
          <ul className="m-0 list-disc pl-4">
            {validation.policy.diagnostics.map((diagnostic) => (
              <li key={JSON.stringify(diagnostic)}>
                <strong>{diagnostic.severity}</strong>{" "}
                <span className="font-mono">{diagnostic.code}</span>: {diagnostic.message}
                {diagnostic.span === undefined
                  ? null
                  : ` (line ${diagnostic.span.line}, column ${diagnostic.span.column})`}
              </li>
            ))}
          </ul>
        )}
      </AlertDescription>
    </Alert>
  );
}

function EvaluationTraces({ results }: { readonly results: EvaluationResultView[] | null }) {
  if (results === null) {
    return null;
  }
  return (
    <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3" aria-live="polite">
      {results.map(({ id, result }, index) => (
        <Card as="article" key={id} className="gap-3 py-4">
          <CardContent className="grid gap-2 px-4 text-sm">
            <div className="flex flex-wrap items-center justify-between gap-2">
              <strong className="font-medium">Example {index + 1}</strong>
              <LifecycleBadge state={result.allowed ? "allowed" : "denied"} />
            </div>
            <TraceLine>
              Result code: <span className="font-mono">{result.code}</span>
            </TraceLine>
            <TraceLine>Evaluated rules: {result.evaluatedRules}</TraceLine>
            <TraceLine>
              Matched rules:{" "}
              {result.matchedRuleIds.length === 0 ? (
                "none"
              ) : (
                <span className="font-mono">{result.matchedRuleIds.join(", ")}</span>
              )}
            </TraceLine>
          </CardContent>
        </Card>
      ))}
    </div>
  );
}

function TraceLine({ children }: { readonly children: ReactNode }) {
  return <p className="m-0 text-muted-foreground">{children}</p>;
}

class FormInputError extends Error {}

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

function parseJsonArray(raw: string, label: string): unknown[] {
  let value: unknown;
  try {
    value = JSON.parse(raw) as unknown;
  } catch {
    throw new FormInputError(`${label} must be valid JSON.`);
  }
  if (!Array.isArray(value) || value.length === 0) {
    throw new FormInputError(`${label} must be a non-empty JSON array.`);
  }
  return value;
}

function parsePolicyRules(raw: string): CreatePolicyDraftRequest["rules"] {
  return parseJsonArray(raw, "Policy rules").map((candidate, index) => {
    const rule = objectValue(candidate, `Rule ${index + 1}`);
    const operations = rule.operations;
    if (
      typeof rule.id !== "string" ||
      rule.id === "" ||
      (rule.effect !== "allow" && rule.effect !== "deny") ||
      !Array.isArray(operations) ||
      operations.length === 0 ||
      !operations.every(
        (operation) =>
          operation === "create" ||
          operation === "read" ||
          operation === "update" ||
          operation === "delete",
      ) ||
      typeof rule.expression !== "string" ||
      rule.expression === ""
    ) {
      throw new FormInputError(
        `Rule ${index + 1} has an invalid id, effect, operations, or expression.`,
      );
    }
    return {
      id: rule.id,
      effect: rule.effect,
      operations: operations as CreatePolicyDraftRequest["rules"][number]["operations"],
      expression: rule.expression,
    };
  });
}

function parsePolicyExamples(raw: string): PolicyExample[] {
  return parseJsonArray(raw, "Policy examples").map((candidate, index) => {
    const example = objectValue(candidate, `Example ${index + 1}`);
    const identity = objectValue(example.identity, `Example ${index + 1} identity`);
    if (
      !["create", "read", "update", "delete"].includes(String(example.operation)) ||
      typeof identity.role !== "string" ||
      identity.role === "" ||
      (identity.userId !== undefined && typeof identity.userId !== "string") ||
      (identity.email !== undefined && typeof identity.email !== "string") ||
      (identity.emailVerified !== undefined && typeof identity.emailVerified !== "boolean")
    ) {
      throw new FormInputError(`Example ${index + 1} has an invalid operation or identity.`);
    }
    const trustedClaims = objectValue(identity.trustedClaims, `Example ${index + 1} trustedClaims`);
    const parsed: PolicyExample = {
      operation: example.operation as PolicyExample["operation"],
      identity: {
        role: identity.role,
        trustedClaims,
        ...(typeof identity.userId === "string" ? { userId: identity.userId } : {}),
        // What `identity.email` and `identity.email_verified` read for this
        // caller, so a rule that scopes a document to an address can be
        // simulated rather than only deployed and hoped for.
        ...(typeof identity.email === "string" ? { email: identity.email } : {}),
        ...(typeof identity.emailVerified === "boolean"
          ? { emailVerified: identity.emailVerified }
          : {}),
      },
      ...(example.oldDocument === undefined
        ? {}
        : { oldDocument: objectValue(example.oldDocument, `Example ${index + 1} oldDocument`) }),
      ...(example.newDocument === undefined
        ? {}
        : { newDocument: objectValue(example.newDocument, `Example ${index + 1} newDocument`) }),
      ...(example.requestMetadata === undefined
        ? {}
        : {
            requestMetadata: stringRecord(
              example.requestMetadata,
              `Example ${index + 1} requestMetadata`,
            ),
          }),
    };
    return parsed;
  });
}

function objectValue(value: unknown, label: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new FormInputError(`${label} must be a JSON object.`);
  }
  return value as Record<string, unknown>;
}

function stringRecord(value: unknown, label: string): Record<string, string> {
  const record = objectValue(value, label);
  if (!Object.values(record).every((entry) => typeof entry === "string")) {
    throw new FormInputError(`${label} values must all be strings.`);
  }
  return record as Record<string, string>;
}

function failureFrom(error: unknown): ConsoleApiFailure {
  return error instanceof FormInputError
    ? { message: error.message, requestId: null }
    : toConsoleApiFailure(error);
}

function idempotencyKey(): string {
  return globalThis.crypto.randomUUID();
}
