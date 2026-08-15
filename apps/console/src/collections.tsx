import { type FormEvent, useCallback, useEffect, useState } from "react";

import type {
  Collection,
  CollectionIndex,
  CreateCollectionIndexRequest,
  CreateCollectionRequest,
  SchemaMigration,
  SchemaMigrationState,
  SchemaPublicationResult,
} from "@mako-cloud/management-sdk";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useManagementClient } from "./management.js";
import { LifecycleBadge } from "./projects.js";
import { confirmDestructiveAction } from "./safety.js";

const DEFAULT_SCHEMA = JSON.stringify(
  {
    type: "object",
    properties: { id: { type: "string" } },
    required: ["id"],
    additionalProperties: false,
  },
  null,
  2,
);
const DEFAULT_PRIMARY_KEY = JSON.stringify({ kind: "field", field: "id" }, null, 2);

interface SchemaDraft {
  readonly schemaVersion: number;
  readonly jsonSchema: Record<string, unknown>;
  readonly primaryKey: CreateCollectionRequest["primaryKey"];
}

export function CollectionsScreen({
  projectId,
  environmentId,
  onBack,
  onOpen,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly onBack: () => void;
  readonly onOpen: (collectionId: string) => void;
}) {
  const client = useManagementClient();
  const [collections, setCollections] = useState<Collection[] | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const reload = useCallback(async () => {
    try {
      setCollections(await client.listCollections(projectId, environmentId));
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
    const form = event.currentTarget;
    const data = new FormData(form);
    try {
      const collection = await client.createCollection(
        projectId,
        environmentId,
        {
          id: requiredText(data, "id"),
          schemaVersion: positiveInteger(data, "schemaVersion"),
          jsonSchema: parseJsonObject(requiredText(data, "jsonSchema"), "JSON schema"),
          primaryKey: parsePrimaryKey(requiredText(data, "primaryKey")),
        },
        idempotencyKey(),
      );
      setFailure(null);
      onOpen(collection.id);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };

  return (
    <section aria-labelledby="collections-title">
      <button type="button" className="back-link" onClick={onBack}>
        ← Project
      </button>
      <div className="section-heading">
        <div>
          <p className="eyebrow">Environment {environmentId}</p>
          <h1 id="collections-title">Collections</h1>
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      <div className="split-grid">
        <section className="panel" aria-labelledby="collection-list-title">
          <h2 id="collection-list-title">Collection schemas</h2>
          {collections === null ? (
            <p>Loading collections…</p>
          ) : collections.length === 0 ? (
            <p>No collections have been created.</p>
          ) : (
            <div className="resource-list">
              {collections.map((collection) => (
                <button
                  type="button"
                  className="resource-row"
                  key={collection.id}
                  onClick={() => onOpen(collection.id)}
                >
                  <span>
                    <strong>{collection.id}</strong>
                    <small>Schema v{collection.schemaVersion}</small>
                  </span>
                  <LifecycleBadge state={collection.compatibility} />
                </button>
              ))}
            </div>
          )}
        </section>
        <section className="panel" aria-labelledby="create-collection-title">
          <h2 id="create-collection-title">Create collection</h2>
          <p>New collections remain default-deny until a policy is activated.</p>
          <form onSubmit={(event) => void create(event)}>
            <label>
              Collection ID
              <input name="id" required pattern="[a-z][a-z0-9_-]{0,62}" />
            </label>
            <label>
              Schema version
              <input name="schemaVersion" type="number" min="1" defaultValue="1" required />
            </label>
            <JsonField name="jsonSchema" label="JSON schema" defaultValue={DEFAULT_SCHEMA} />
            <JsonField
              name="primaryKey"
              label="Primary-key definition"
              defaultValue={DEFAULT_PRIMARY_KEY}
              rows={4}
            />
            <button type="submit">Create collection</button>
          </form>
        </section>
      </div>
    </section>
  );
}

export function CollectionScreen({
  projectId,
  environmentId,
  collectionId,
  onBack,
  onOpenPolicies,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly collectionId: string;
  readonly onBack: () => void;
  readonly onOpenPolicies: () => void;
}) {
  const client = useManagementClient();
  const [collection, setCollection] = useState<Collection | null>(null);
  const [publication, setPublication] = useState<SchemaPublicationResult | null>(null);
  const [migrationDraft, setMigrationDraft] = useState<SchemaDraft | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const reload = useCallback(async () => {
    try {
      setCollection(await client.getCollection(projectId, environmentId, collectionId));
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  }, [client, collectionId, environmentId, projectId]);
  useEffect(() => {
    void reload();
  }, [reload]);

  const publish = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    try {
      const draft: SchemaDraft = {
        schemaVersion: positiveInteger(data, "schemaVersion"),
        jsonSchema: parseJsonObject(requiredText(data, "jsonSchema"), "JSON schema"),
        primaryKey: parsePrimaryKey(requiredText(data, "primaryKey")),
      };
      const result = await client.publishCollectionSchema(
        projectId,
        environmentId,
        collectionId,
        draft,
        idempotencyKey(),
      );
      setPublication(result);
      setMigrationDraft(result.status === "migration_required" ? draft : null);
      setFailure(null);
      if (result.status === "published") {
        await reload();
      }
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };

  return (
    <section aria-labelledby="collection-title">
      <button type="button" className="back-link" onClick={onBack}>
        ← Collections
      </button>
      <div className="section-heading">
        <div>
          <p className="eyebrow">Collection</p>
          <h1 id="collection-title">{collectionId}</h1>
        </div>
        {collection === null ? null : (
          <div className="button-row">
            <LifecycleBadge state={collection.state} />
            <button type="button" onClick={onOpenPolicies}>
              Manage policies
            </button>
          </div>
        )}
      </div>
      <ApiFailureNotice failure={failure} />
      {collection === null ? (
        <p>Loading collection…</p>
      ) : (
        <>
          <div className="split-grid">
            <CollectionSchemaCard collection={collection} />
            <section className="panel" aria-labelledby="publish-schema-title">
              <h2 id="publish-schema-title">Publish schema version</h2>
              <p>Compatibility is checked before the active schema changes.</p>
              <form onSubmit={(event) => void publish(event)}>
                <label>
                  New schema version
                  <input
                    name="schemaVersion"
                    type="number"
                    min={collection.schemaVersion + 1}
                    defaultValue={collection.schemaVersion + 1}
                    required
                  />
                </label>
                <JsonField
                  name="jsonSchema"
                  label="JSON schema"
                  defaultValue={JSON.stringify(collection.jsonSchema, null, 2)}
                />
                <JsonField
                  name="primaryKey"
                  label="Primary-key definition"
                  defaultValue={JSON.stringify(collection.primaryKey, null, 2)}
                  rows={4}
                />
                <button type="submit">Check and publish</button>
              </form>
              <CompatibilityResult result={publication} />
            </section>
          </div>
          <MigrationPanel
            projectId={projectId}
            environmentId={environmentId}
            collectionId={collectionId}
            suggested={migrationDraft}
          />
          <IndexesPanel
            projectId={projectId}
            environmentId={environmentId}
            collectionId={collectionId}
          />
        </>
      )}
    </section>
  );
}

function CollectionSchemaCard({ collection }: { readonly collection: Collection }) {
  return (
    <section className="panel" aria-labelledby="active-schema-title">
      <h2 id="active-schema-title">Active JSON schema</h2>
      <dl className="metadata-list">
        <div>
          <dt>Schema version</dt>
          <dd>{collection.schemaVersion}</dd>
        </div>
        <div>
          <dt>Metadata version</dt>
          <dd>{collection.metadataVersion}</dd>
        </div>
        <div>
          <dt>Compatibility</dt>
          <dd>{collection.compatibility.replaceAll("_", " ")}</dd>
        </div>
      </dl>
      <h3>Primary key</h3>
      <JsonPreview value={collection.primaryKey} />
      <h3>Schema</h3>
      <JsonPreview value={collection.jsonSchema} />
    </section>
  );
}

function CompatibilityResult({ result }: { readonly result: SchemaPublicationResult | null }) {
  if (result === null) {
    return null;
  }
  const report = result.compatibility;
  return (
    <div
      className={`notice ${result.status === "migration_required" ? "warning" : "success"}`}
      role="status"
    >
      <strong>
        {result.status === "published"
          ? "Schema published."
          : "Migration required before this schema can be published."}
      </strong>
      {report === undefined ? null : (
        <>
          <p>{report.documentsChecked} stored documents checked.</p>
          {report.issues.length === 0 ? null : (
            <ul>
              {report.issues.map((issue) => (
                <li key={issue}>{issue}</li>
              ))}
            </ul>
          )}
        </>
      )}
    </div>
  );
}

function MigrationPanel({
  projectId,
  environmentId,
  collectionId,
  suggested,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly collectionId: string;
  readonly suggested: SchemaDraft | null;
}) {
  const client = useManagementClient();
  const [migration, setMigration] = useState<SchemaMigration | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const refresh = useCallback(async () => {
    if (migration === null) {
      return;
    }
    try {
      setMigration(
        await client.getSchemaMigration(projectId, environmentId, collectionId, migration.id),
      );
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  }, [client, collectionId, environmentId, migration, projectId]);
  useEffect(() => {
    if (migration?.state !== "running") {
      return;
    }
    const timer = window.setInterval(() => void refresh(), 5_000);
    return () => window.clearInterval(timer);
  }, [migration?.state, refresh]);

  const create = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    try {
      setMigration(
        await client.createSchemaMigration(
          projectId,
          environmentId,
          collectionId,
          {
            targetSchemaVersion: positiveInteger(data, "targetSchemaVersion"),
            targetJsonSchema: parseJsonObject(
              requiredText(data, "targetJsonSchema"),
              "Target JSON schema",
            ),
            targetPrimaryKey: parsePrimaryKey(requiredText(data, "targetPrimaryKey")),
            reason: requiredText(data, "reason"),
          },
          idempotencyKey(),
        ),
      );
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };
  const lookup = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    try {
      setMigration(
        await client.getSchemaMigration(
          projectId,
          environmentId,
          collectionId,
          requiredText(new FormData(event.currentTarget), "migrationId"),
        ),
      );
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };
  const update = async (state: SchemaMigrationState) => {
    if (migration === null) {
      return;
    }
    if (
      state === "cancelled" &&
      !confirmDestructiveAction({
        action: "Cancel",
        target: `schema migration ${migration.id}`,
        consequence:
          "The migration workflow will stop and the target schema will not be activated.",
      })
    ) {
      return;
    }
    try {
      setMigration(
        await client.updateSchemaMigration(
          projectId,
          environmentId,
          collectionId,
          migration.id,
          state,
        ),
      );
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };

  return (
    <section className="panel full-span stacked-section" aria-labelledby="migration-title">
      <h2 id="migration-title">Schema migration</h2>
      <p>Incompatible schema changes use an explicit, observable workflow.</p>
      <ApiFailureNotice failure={failure} />
      <div className="split-grid">
        <form key={suggested?.schemaVersion ?? "empty"} onSubmit={(event) => void create(event)}>
          <label>
            Target schema version
            <input
              name="targetSchemaVersion"
              type="number"
              min="1"
              defaultValue={suggested?.schemaVersion}
              required
            />
          </label>
          <JsonField
            name="targetJsonSchema"
            label="Target JSON schema"
            defaultValue={
              suggested === null ? DEFAULT_SCHEMA : JSON.stringify(suggested.jsonSchema, null, 2)
            }
          />
          <JsonField
            name="targetPrimaryKey"
            label="Target primary-key definition"
            defaultValue={
              suggested === null
                ? DEFAULT_PRIMARY_KEY
                : JSON.stringify(suggested.primaryKey, null, 2)
            }
            rows={4}
          />
          <label>
            Migration reason
            <input name="reason" required maxLength={500} />
          </label>
          <button type="submit">Plan migration</button>
        </form>
        <div>
          <form className="inline-form" onSubmit={(event) => void lookup(event)}>
            <label>
              Migration ID
              <input name="migrationId" required pattern="mig_[A-Za-z0-9_-]{8,64}" />
            </label>
            <button type="submit" className="secondary">
              Inspect
            </button>
          </form>
          {migration === null ? (
            <p>No migration selected.</p>
          ) : (
            <article className="workflow-card" aria-live="polite">
              <div className="button-row spread">
                <strong>{migration.id}</strong>
                <LifecycleBadge state={migration.state} />
              </div>
              <p>
                Schema v{migration.fromSchemaVersion} → v{migration.toSchemaVersion}
              </p>
              <p>{migration.reason}</p>
              {migration.compatibilityIssues.length === 0 ? null : (
                <ul>
                  {migration.compatibilityIssues.map((issue) => (
                    <li key={issue}>{issue}</li>
                  ))}
                </ul>
              )}
              <div className="button-row">
                <button type="button" onClick={() => void update("running")}>
                  Start / retry
                </button>
                <button
                  type="button"
                  className="secondary"
                  onClick={() => void update("completed")}
                >
                  Mark completed
                </button>
                <button
                  type="button"
                  className="danger-link"
                  onClick={() => void update("cancelled")}
                >
                  Cancel
                </button>
              </div>
            </article>
          )}
        </div>
      </div>
    </section>
  );
}

function IndexesPanel({
  projectId,
  environmentId,
  collectionId,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly collectionId: string;
}) {
  const client = useManagementClient();
  const [indexes, setIndexes] = useState<CollectionIndex[] | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const reload = useCallback(async () => {
    try {
      setIndexes(await client.listCollectionIndexes(projectId, environmentId, collectionId));
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  }, [client, collectionId, environmentId, projectId]);
  useEffect(() => {
    void reload();
  }, [reload]);
  useEffect(() => {
    if (indexes?.some((index) => ["building", "deleting"].includes(index.state)) !== true) {
      return;
    }
    const timer = window.setInterval(() => void reload(), 5_000);
    return () => window.clearInterval(timer);
  }, [indexes, reload]);

  const create = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const form = event.currentTarget;
    const data = new FormData(form);
    try {
      await client.createCollectionIndex(
        projectId,
        environmentId,
        collectionId,
        {
          name: requiredText(data, "name"),
          version: positiveInteger(data, "version"),
          kind: requiredText(data, "kind") === "unique" ? "unique" : "non_unique",
          fields: parseIndexFields(requiredText(data, "fields")),
        },
        idempotencyKey(),
      );
      form.reset();
      await reload();
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };
  const remove = async (index: CollectionIndex) => {
    if (
      !confirmDestructiveAction({
        action: "Remove",
        target: `index ${index.name} version ${index.version}`,
        consequence: "Queries can no longer use this index once deletion completes.",
      })
    ) {
      return;
    }
    try {
      await client.deleteCollectionIndex(
        projectId,
        environmentId,
        collectionId,
        index.name,
        index.version,
      );
      await reload();
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };

  return (
    <section className="panel full-span stacked-section" aria-labelledby="indexes-title">
      <div className="button-row spread">
        <div>
          <h2 id="indexes-title">Indexes</h2>
          <p>Online builds are fenced from queries until backfill and catch-up finish.</p>
        </div>
        <button type="button" className="secondary" onClick={() => void reload()}>
          Refresh
        </button>
      </div>
      <ApiFailureNotice failure={failure} />
      {indexes === null ? (
        <p>Loading indexes…</p>
      ) : indexes.length === 0 ? (
        <p>No indexes have been defined.</p>
      ) : (
        <div className="card-grid">
          {indexes.map((index) => (
            <IndexCard
              key={`${index.name}:${index.version}`}
              index={index}
              onDelete={() => void remove(index)}
            />
          ))}
        </div>
      )}
      <details>
        <summary>Create index</summary>
        <form onSubmit={(event) => void create(event)}>
          <label>
            Index name
            <input name="name" required pattern="[A-Za-z0-9_-]{1,128}" />
          </label>
          <label>
            Version
            <input name="version" type="number" min="1" defaultValue="1" required />
          </label>
          <label>
            Kind
            <select name="kind" defaultValue="non_unique">
              <option value="non_unique">Non-unique</option>
              <option value="unique">Unique</option>
            </select>
          </label>
          <label>
            Fields (one per line: path direction)
            <textarea name="fields" rows={4} required defaultValue="id ascending" />
          </label>
          <button type="submit">Start online build</button>
        </form>
      </details>
    </section>
  );
}

function IndexCard({
  index,
  onDelete,
}: {
  readonly index: CollectionIndex;
  readonly onDelete: () => void;
}) {
  return (
    <article className="resource-card index-card">
      <div className="button-row spread">
        <strong>
          {index.name} v{index.version}
        </strong>
        <LifecycleBadge state={index.state} />
      </div>
      <span>{index.kind.replaceAll("_", " ")}</span>
      <p>{index.fields.map((field) => `${field.path} ${field.direction}`).join(", ")}</p>
      {index.progress === undefined ? null : (
        <dl className="metadata-list compact">
          <div>
            <dt>Snapshot position</dt>
            <dd>{index.progress.capturedPosition}</dd>
          </div>
          <div>
            <dt>Caught up through</dt>
            <dd>{index.progress.caughtUpPosition}</dd>
          </div>
          <div>
            <dt>Backfill</dt>
            <dd>{index.progress.backfillComplete ? "complete" : "running"}</dd>
          </div>
        </dl>
      )}
      {index.failure === undefined ? null : (
        <div className="notice error" role="alert">
          <strong>{index.failure.code.replaceAll("_", " ")}</strong>
          <p>{index.failure.message}</p>
          <small>{index.failure.affectedValues} affected values</small>
        </div>
      )}
      <button
        type="button"
        className="danger-link"
        disabled={index.state === "deleting"}
        onClick={onDelete}
      >
        Remove index
      </button>
    </article>
  );
}

function JsonField({
  name,
  label,
  defaultValue,
  rows = 10,
}: {
  readonly name: string;
  readonly label: string;
  readonly defaultValue: string;
  readonly rows?: number;
}) {
  return (
    <label>
      {label}
      <textarea name={name} rows={rows} required defaultValue={defaultValue} spellCheck={false} />
    </label>
  );
}

function JsonPreview({ value }: { readonly value: unknown }) {
  return <pre className="json-preview">{JSON.stringify(value, null, 2)}</pre>;
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

function parseJsonObject(raw: string, label: string): Record<string, unknown> {
  let value: unknown;
  try {
    value = JSON.parse(raw) as unknown;
  } catch {
    throw new FormInputError(`${label} must be valid JSON.`);
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new FormInputError(`${label} must be a JSON object.`);
  }
  return value as Record<string, unknown>;
}

function parsePrimaryKey(raw: string): CreateCollectionRequest["primaryKey"] {
  const value = parseJsonObject(raw, "Primary-key definition");
  if (value.kind === "field" && typeof value.field === "string" && value.field !== "") {
    return { kind: "field", field: value.field };
  }
  if (
    value.kind === "composite" &&
    typeof value.key === "string" &&
    Array.isArray(value.fields) &&
    value.fields.length >= 2 &&
    value.fields.every((field) => typeof field === "string") &&
    typeof value.separator === "string" &&
    value.separator !== ""
  ) {
    return {
      kind: "composite",
      key: value.key,
      fields: value.fields as string[],
      separator: value.separator,
    };
  }
  throw new FormInputError("Primary-key definition must be a valid field or composite definition.");
}

function parseIndexFields(raw: string): CreateCollectionIndexRequest["fields"] {
  const fields = raw
    .split("\n")
    .map((line) => line.trim())
    .filter((line) => line !== "")
    .map((line) => {
      const [path, direction, extra] = line.split(/\s+/u);
      if (
        path === undefined ||
        path === "" ||
        (direction !== "ascending" && direction !== "descending") ||
        extra !== undefined
      ) {
        throw new FormInputError(
          `Invalid index field "${line}". Use one line per field, such as: ownerId ascending.`,
        );
      }
      const normalizedDirection: "ascending" | "descending" = direction;
      return { path, direction: normalizedDirection };
    });
  if (fields.length === 0) {
    throw new FormInputError("At least one index field is required.");
  }
  return fields;
}

function failureFrom(error: unknown): ConsoleApiFailure {
  return error instanceof FormInputError
    ? { message: error.message, requestId: null }
    : toConsoleApiFailure(error);
}

function idempotencyKey(): string {
  return globalThis.crypto.randomUUID();
}
