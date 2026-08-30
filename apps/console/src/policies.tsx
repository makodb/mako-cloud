import { type FormEvent, useCallback, useEffect, useState } from "react";

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
      expression: "oldDocument.ownerId == identity.userId",
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
  const reloadActive = useCallback(async () => {
    try {
      const next = await client.getActiveCollectionPolicy(projectId, environmentId, collectionId);
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
    <section aria-labelledby="policy-title">
      <button type="button" className="back-link" onClick={onBack}>
        ← Collection
      </button>
      <div className="section-heading">
        <div>
          <p className="eyebrow">Collection {collectionId}</p>
          <h1 id="policy-title">Document policies</h1>
        </div>
        {active === null ? null : (
          <span className="status">Authorization epoch {active.authorizationEpoch}</span>
        )}
      </div>
      <ApiFailureNotice failure={failure} />
      <DefaultDenyNotice active={active} />
      <div className="split-grid stacked-section">
        <section className="panel" aria-labelledby="draft-title">
          <h2 id="draft-title">Create immutable draft</h2>
          <p>Rules use the typed, CEL-compatible policy expression environment.</p>
          <form onSubmit={(event) => void createDraft(event)}>
            <label>
              Policy version
              <input
                name="version"
                type="number"
                min="1"
                defaultValue={(active?.policy?.version ?? 0) + 1}
                required
              />
            </label>
            <label>
              Rules (JSON)
              <textarea
                name="rules"
                rows={18}
                required
                defaultValue={DEFAULT_RULES}
                spellCheck={false}
              />
            </label>
            <button type="submit">Create draft</button>
          </form>
        </section>
        <section className="panel" aria-labelledby="versions-title">
          <h2 id="versions-title">Policy versions</h2>
          <form className="inline-form" onSubmit={(event) => void inspectVersion(event)}>
            <label>
              Inspect version
              <input
                name="version"
                type="number"
                min="1"
                defaultValue={active?.policy?.version}
                required
              />
            </label>
            <button type="submit" className="secondary">
              Load
            </button>
          </form>
          <PolicyVersionCard policy={selected} />
          {selected === null ? null : (
            <div className="button-row">
              <button type="button" onClick={() => void validate()}>
                Validate against schema
              </button>
              <button
                type="button"
                className="secondary"
                disabled={selected.state !== "validated"}
                onClick={() => void changeActive("activate")}
              >
                Activate
              </button>
              <button
                type="button"
                className="secondary"
                disabled={selected.state === "draft"}
                onClick={() => void changeActive("rollback")}
              >
                Roll back to this version
              </button>
            </div>
          )}
          <ValidationResult validation={validation} />
        </section>
      </div>
      <section className="panel full-span stacked-section" aria-labelledby="test-policy-title">
        <h2 id="test-policy-title">Example evaluation</h2>
        <p>Examples run without changing the active policy.</p>
        {selected === null ? (
          <p>Select or create a policy version before testing.</p>
        ) : (
          <form onSubmit={(event) => void testExamples(event)}>
            <label>
              Example contexts (JSON)
              <textarea
                name="examples"
                rows={16}
                required
                defaultValue={DEFAULT_EXAMPLES}
                spellCheck={false}
              />
            </label>
            <button type="submit">Evaluate policy v{selected.version}</button>
          </form>
        )}
        <EvaluationTraces results={testResults} />
      </section>
    </section>
  );
}

function DefaultDenyNotice({ active }: { readonly active: ActivePolicy | null }) {
  if (active === null) {
    return <p>Loading active policy state…</p>;
  }
  return (
    <div className={`notice ${active.defaultDeny ? "warning" : "success"}`} role="status">
      <strong>
        {active.defaultDeny ? "Default deny is active." : "An active policy is installed."}
      </strong>
      <p>
        {active.defaultDeny
          ? "All create, read, update, and delete operations are denied until a validated policy is activated."
          : `Policy v${active.policy?.version ?? "unknown"} controls access. Activation or rollback advances the authorization epoch.`}
      </p>
    </div>
  );
}

function PolicyVersionCard({ policy }: { readonly policy: PolicySet | null }) {
  if (policy === null) {
    return <p>No policy version selected.</p>;
  }
  return (
    <article className="workflow-card">
      <div className="button-row spread">
        <strong>Policy v{policy.version}</strong>
        <LifecycleBadge state={policy.state} />
      </div>
      <pre className="json-preview">{JSON.stringify(policy.rules, null, 2)}</pre>
    </article>
  );
}

function ValidationResult({ validation }: { readonly validation: PolicyValidation | null }) {
  if (validation === null) {
    return null;
  }
  return (
    <div className={`notice ${validation.valid ? "success" : "error"}`} role="status">
      <strong>{validation.valid ? "Schema-aware validation passed." : "Validation failed."}</strong>
      {validation.policy.diagnostics.length === 0 ? (
        <p>No diagnostics.</p>
      ) : (
        <ul className="diagnostic-list">
          {validation.policy.diagnostics.map((diagnostic) => (
            <li key={JSON.stringify(diagnostic)}>
              <strong>{diagnostic.severity}</strong> {diagnostic.code}: {diagnostic.message}
              {diagnostic.span === undefined
                ? null
                : ` (line ${diagnostic.span.line}, column ${diagnostic.span.column})`}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function EvaluationTraces({ results }: { readonly results: EvaluationResultView[] | null }) {
  if (results === null) {
    return null;
  }
  return (
    <div className="card-grid stacked-section" aria-live="polite">
      {results.map(({ id, result }, index) => (
        <article className="resource-card" key={id}>
          <div className="button-row spread">
            <strong>Example {index + 1}</strong>
            <LifecycleBadge state={result.allowed ? "allowed" : "denied"} />
          </div>
          <p>Result code: {result.code}</p>
          <p>Evaluated rules: {result.evaluatedRules}</p>
          <p>
            Matched rules:{" "}
            {result.matchedRuleIds.length === 0 ? "none" : result.matchedRuleIds.join(", ")}
          </p>
        </article>
      ))}
    </div>
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
