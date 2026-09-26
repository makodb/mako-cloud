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
  Eyebrow,
  Field,
  Input,
  NativeSelect,
  Textarea,
} from "@mako-cloud/ui";
import { AlertTriangle, CheckCircle2, Plus, RefreshCw, ShieldCheck } from "lucide-react";
import { type FormEvent, type ReactNode, useCallback, useEffect, useState } from "react";

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

/** An identifier inside an eyebrow keeps its case and typeface. */
const IDENTIFIER = "font-mono normal-case tracking-normal";

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
    <section aria-labelledby="collections-title" className="grid gap-6">
      <div className="grid gap-3">
        <BackButton onClick={onBack}>← Project</BackButton>
        <div>
          <Eyebrow>
            Environment <span className={IDENTIFIER}>{environmentId}</span>
          </Eyebrow>
          <h1 id="collections-title" className="m-0 text-2xl font-semibold tracking-tight">
            Collections
          </h1>
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      <div className="grid items-start gap-6 lg:grid-cols-2">
        <Card aria-labelledby="collection-list-title">
          <CardHeader>
            <CardTitle id="collection-list-title">Collection schemas</CardTitle>
          </CardHeader>
          <CardContent>
            {collections === null ? (
              <p className="m-0 text-sm text-muted-foreground">Loading collections…</p>
            ) : collections.length === 0 ? (
              <p className="m-0 text-sm text-muted-foreground">No collections have been created.</p>
            ) : (
              <ul className="m-0 list-none divide-y overflow-hidden rounded-md border p-0">
                {collections.map((collection) => (
                  <li key={collection.id}>
                    <Button
                      variant="ghost"
                      className="h-auto w-full justify-between gap-3 rounded-none px-3 py-2.5 text-left font-normal"
                      onClick={() => onOpen(collection.id)}
                    >
                      <span className="grid min-w-0 gap-0.5">
                        <strong className="truncate font-mono text-sm font-medium">
                          {collection.id}
                        </strong>
                        <small className="text-xs text-muted-foreground">
                          Schema v{collection.schemaVersion}
                        </small>
                      </span>
                      <LifecycleBadge state={collection.compatibility} />
                    </Button>
                  </li>
                ))}
              </ul>
            )}
          </CardContent>
        </Card>
        <Card aria-labelledby="create-collection-title">
          <CardHeader>
            <CardTitle id="create-collection-title">Create collection</CardTitle>
            <CardDescription>
              New collections remain default-deny until a policy is activated.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <form className="grid gap-4" onSubmit={(event) => void create(event)}>
              <Field label="Collection ID" htmlFor="create-collection-id">
                <Input
                  id="create-collection-id"
                  name="id"
                  required
                  pattern="[a-z][a-z0-9_\-]{0,62}"
                  className="font-mono"
                />
              </Field>
              <Field label="Schema version" htmlFor="create-collection-schema-version">
                <Input
                  id="create-collection-schema-version"
                  name="schemaVersion"
                  type="number"
                  min="1"
                  defaultValue="1"
                  required
                  className="max-w-40"
                />
              </Field>
              <JsonField
                id="create-collection-json-schema"
                name="jsonSchema"
                label="JSON schema"
                defaultValue={DEFAULT_SCHEMA}
              />
              <JsonField
                id="create-collection-primary-key"
                name="primaryKey"
                label="Primary-key definition"
                defaultValue={DEFAULT_PRIMARY_KEY}
                rows={4}
              />
              <div>
                <Button type="submit">
                  <Plus aria-hidden="true" />
                  Create collection
                </Button>
              </div>
            </form>
          </CardContent>
        </Card>
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
      // Even an additive change is a new version every replicating app must
      // move to, which a compatibility check alone did not make clear.
      const current = collection?.schemaVersion;
      if (
        current !== undefined &&
        !confirmDestructiveAction({
          action: "Publish",
          target: `schema version ${draft.schemaVersion} of ${collectionId}`,
          consequence: `If it is compatible it becomes active at once. Apps replicating ${collectionId} at version ${current} are then refused until they update to version ${draft.schemaVersion}; their unsynced changes stay on the device and are sent after the update.`,
        })
      ) {
        return;
      }
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
    <section aria-labelledby="collection-title" className="grid gap-6">
      <div className="grid gap-3">
        <BackButton onClick={onBack}>← Collections</BackButton>
        <div className="flex flex-wrap items-end justify-between gap-4">
          <div>
            <Eyebrow>Collection</Eyebrow>
            <h1 id="collection-title" className="m-0 text-2xl font-semibold tracking-tight">
              {collectionId}
            </h1>
          </div>
          {collection === null ? null : (
            <div className="flex flex-wrap items-center gap-3">
              <LifecycleBadge state={collection.state} />
              <Button onClick={onOpenPolicies}>
                <ShieldCheck aria-hidden="true" />
                Manage policies
              </Button>
            </div>
          )}
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      {collection === null ? (
        <p className="m-0 text-sm text-muted-foreground">Loading collection…</p>
      ) : (
        <>
          <div className="grid items-start gap-6 lg:grid-cols-2">
            <CollectionSchemaCard collection={collection} />
            <Card aria-labelledby="publish-schema-title">
              <CardHeader>
                <CardTitle id="publish-schema-title">Publish schema version</CardTitle>
                <CardDescription>
                  Compatibility is checked before the active schema changes. Apps replicating this
                  collection are bound to version {collection.schemaVersion}: once a newer version
                  is active they must update to it before they can sync again. Changes they have not
                  synced yet stay on the device until then.
                </CardDescription>
              </CardHeader>
              <CardContent className="grid gap-4">
                <form className="grid gap-4" onSubmit={(event) => void publish(event)}>
                  <Field label="New schema version" htmlFor="publish-schema-version">
                    <Input
                      id="publish-schema-version"
                      name="schemaVersion"
                      type="number"
                      min={collection.schemaVersion + 1}
                      defaultValue={collection.schemaVersion + 1}
                      required
                      className="max-w-40"
                    />
                  </Field>
                  <JsonField
                    id="publish-json-schema"
                    name="jsonSchema"
                    label="JSON schema"
                    defaultValue={JSON.stringify(collection.jsonSchema, null, 2)}
                  />
                  <JsonField
                    id="publish-primary-key"
                    name="primaryKey"
                    label="Primary-key definition"
                    defaultValue={JSON.stringify(collection.primaryKey, null, 2)}
                    rows={4}
                  />
                  <div>
                    <Button type="submit">Check and publish</Button>
                  </div>
                </form>
                <CompatibilityResult result={publication} />
              </CardContent>
            </Card>
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
    <Card aria-labelledby="active-schema-title">
      <CardHeader>
        <CardTitle id="active-schema-title">Active JSON schema</CardTitle>
      </CardHeader>
      <CardContent className="grid gap-4">
        <dl className="m-0 grid gap-3 sm:grid-cols-3">
          <MetaItem term="Schema version">{collection.schemaVersion}</MetaItem>
          <MetaItem term="Metadata version">{collection.metadataVersion}</MetaItem>
          <MetaItem term="Compatibility">{collection.compatibility.replaceAll("_", " ")}</MetaItem>
        </dl>
        <div className="grid gap-2">
          <h3 className="m-0 text-sm font-medium">Primary key</h3>
          <JsonPreview value={collection.primaryKey} />
        </div>
        <div className="grid gap-2">
          <h3 className="m-0 text-sm font-medium">Schema</h3>
          <JsonPreview value={collection.jsonSchema} />
        </div>
      </CardContent>
    </Card>
  );
}

function CompatibilityResult({ result }: { readonly result: SchemaPublicationResult | null }) {
  if (result === null) {
    return null;
  }
  const report = result.compatibility;
  const migrationRequired = result.status === "migration_required";
  return (
    <Alert variant={migrationRequired ? "warning" : "positive"} role="status">
      {migrationRequired ? (
        <AlertTriangle aria-hidden="true" />
      ) : (
        <CheckCircle2 aria-hidden="true" />
      )}
      <AlertTitle className="line-clamp-none">
        {result.status === "published"
          ? "Schema published."
          : "Migration required before this schema can be published."}
      </AlertTitle>
      {report === undefined ? null : (
        <AlertDescription>
          <p className="m-0">{report.documentsChecked} stored documents checked.</p>
          {report.issues.length === 0 ? null : (
            <ul className="m-0 list-disc pl-4">
              {report.issues.map((issue) => (
                <li key={issue}>{issue}</li>
              ))}
            </ul>
          )}
        </AlertDescription>
      )}
    </Alert>
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
    <Card aria-labelledby="migration-title">
      <CardHeader>
        <CardTitle id="migration-title">Schema migration</CardTitle>
        <CardDescription>
          Incompatible schema changes use an explicit, observable workflow.
        </CardDescription>
      </CardHeader>
      <CardContent className="grid gap-4">
        <ApiFailureNotice failure={failure} />
        <div className="grid items-start gap-6 lg:grid-cols-2">
          <form
            key={suggested?.schemaVersion ?? "empty"}
            className="grid gap-4"
            onSubmit={(event) => void create(event)}
          >
            <Field label="Target schema version" htmlFor="migration-target-schema-version">
              <Input
                id="migration-target-schema-version"
                name="targetSchemaVersion"
                type="number"
                min="1"
                defaultValue={suggested?.schemaVersion}
                required
                className="max-w-40"
              />
            </Field>
            <JsonField
              id="migration-target-json-schema"
              name="targetJsonSchema"
              label="Target JSON schema"
              defaultValue={
                suggested === null ? DEFAULT_SCHEMA : JSON.stringify(suggested.jsonSchema, null, 2)
              }
            />
            <JsonField
              id="migration-target-primary-key"
              name="targetPrimaryKey"
              label="Target primary-key definition"
              defaultValue={
                suggested === null
                  ? DEFAULT_PRIMARY_KEY
                  : JSON.stringify(suggested.primaryKey, null, 2)
              }
              rows={4}
            />
            <Field label="Migration reason" htmlFor="migration-reason">
              <Input id="migration-reason" name="reason" required maxLength={500} />
            </Field>
            <div>
              <Button type="submit">Plan migration</Button>
            </div>
          </form>
          <div className="grid gap-4">
            <form
              className="flex flex-wrap items-end gap-2"
              onSubmit={(event) => void lookup(event)}
            >
              <Field label="Migration ID" htmlFor="migration-lookup-id" className="min-w-0 flex-1">
                <Input
                  id="migration-lookup-id"
                  name="migrationId"
                  required
                  pattern="mig_[A-Za-z0-9_\-]{8,64}"
                  className="font-mono"
                />
              </Field>
              <Button type="submit" variant="outline">
                Inspect
              </Button>
            </form>
            {migration === null ? (
              <p className="m-0 text-sm text-muted-foreground">No migration selected.</p>
            ) : (
              <Card as="article" aria-live="polite" className="gap-3 py-4">
                <CardContent className="grid gap-3 px-4">
                  <div className="flex flex-wrap items-center justify-between gap-2">
                    <strong className="font-mono text-sm font-medium">{migration.id}</strong>
                    <LifecycleBadge state={migration.state} />
                  </div>
                  <p className="m-0 text-sm">
                    Schema v{migration.fromSchemaVersion} → v{migration.toSchemaVersion}
                  </p>
                  <p className="m-0 text-sm text-muted-foreground">{migration.reason}</p>
                  {migration.compatibilityIssues.length === 0 ? null : (
                    <ul className="m-0 list-disc pl-4 text-sm">
                      {migration.compatibilityIssues.map((issue) => (
                        <li key={issue}>{issue}</li>
                      ))}
                    </ul>
                  )}
                  <div className="flex flex-wrap items-center gap-2">
                    <Button size="sm" onClick={() => void update("running")}>
                      Start / retry
                    </Button>
                    <Button size="sm" variant="outline" onClick={() => void update("completed")}>
                      Mark completed
                    </Button>
                    <Button
                      size="sm"
                      variant="ghost"
                      className="text-destructive hover:bg-destructive/10 hover:text-destructive"
                      onClick={() => void update("cancelled")}
                    >
                      Cancel
                    </Button>
                  </div>
                </CardContent>
              </Card>
            )}
          </div>
        </div>
      </CardContent>
    </Card>
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
    <Card aria-labelledby="indexes-title">
      <CardHeader>
        <CardTitle id="indexes-title">Indexes</CardTitle>
        <CardDescription>
          Online builds are fenced from queries until backfill and catch-up finish.
        </CardDescription>
        <CardAction>
          <Button variant="outline" size="sm" onClick={() => void reload()}>
            <RefreshCw aria-hidden="true" />
            Refresh
          </Button>
        </CardAction>
      </CardHeader>
      <CardContent className="grid gap-4">
        <ApiFailureNotice failure={failure} />
        {indexes === null ? (
          <p className="m-0 text-sm text-muted-foreground">Loading indexes…</p>
        ) : indexes.length === 0 ? (
          <p className="m-0 text-sm text-muted-foreground">No indexes have been defined.</p>
        ) : (
          <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-3">
            {indexes.map((index) => (
              <IndexCard
                key={`${index.name}:${index.version}`}
                index={index}
                onDelete={() => void remove(index)}
              />
            ))}
          </div>
        )}
        <details className="group rounded-md border">
          <summary className="cursor-pointer select-none px-4 py-2.5 text-sm font-medium marker:text-muted-foreground">
            Create index
          </summary>
          <form
            className="grid gap-4 border-t px-4 py-4 sm:grid-cols-2"
            onSubmit={(event) => void create(event)}
          >
            <Field label="Index name" htmlFor="create-index-name">
              <Input
                id="create-index-name"
                name="name"
                required
                pattern="[A-Za-z0-9_\-]{1,128}"
                className="font-mono"
              />
            </Field>
            <Field label="Version" htmlFor="create-index-version">
              <Input
                id="create-index-version"
                name="version"
                type="number"
                min="1"
                defaultValue="1"
                required
                className="max-w-40"
              />
            </Field>
            <Field label="Kind" htmlFor="create-index-kind">
              <NativeSelect id="create-index-kind" name="kind" defaultValue="non_unique">
                <option value="non_unique">Non-unique</option>
                <option value="unique">Unique</option>
              </NativeSelect>
            </Field>
            <Field
              label="Fields (one per line: path direction)"
              htmlFor="create-index-fields"
              className="sm:col-span-2"
            >
              <Textarea
                id="create-index-fields"
                name="fields"
                rows={4}
                required
                defaultValue="id ascending"
                className="min-h-24 font-mono text-xs leading-5 md:text-xs"
              />
            </Field>
            <div className="sm:col-span-2">
              <Button type="submit">Start online build</Button>
            </div>
          </form>
        </details>
      </CardContent>
    </Card>
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
    <Card as="article" className="gap-3 py-4">
      <CardContent className="grid gap-3 px-4">
        <div className="flex flex-wrap items-center justify-between gap-2">
          <strong className="font-mono text-sm font-medium">
            {index.name} v{index.version}
          </strong>
          <LifecycleBadge state={index.state} />
        </div>
        <span className="text-xs text-muted-foreground">{index.kind.replaceAll("_", " ")}</span>
        <p className="m-0 font-mono text-xs">
          {index.fields.map((field) => `${field.path} ${field.direction}`).join(", ")}
        </p>
        {index.progress === undefined ? null : (
          <dl className="m-0 grid gap-2 text-xs">
            <MetaItem term="Snapshot position" compact>
              {index.progress.capturedPosition}
            </MetaItem>
            <MetaItem term="Caught up through" compact>
              {index.progress.caughtUpPosition}
            </MetaItem>
            <MetaItem term="Backfill" compact>
              {index.progress.backfillComplete ? "complete" : "running"}
            </MetaItem>
          </dl>
        )}
        {index.failure === undefined ? null : (
          <Alert variant="destructive" role="alert">
            <AlertTriangle aria-hidden="true" />
            <AlertTitle>{index.failure.code.replaceAll("_", " ")}</AlertTitle>
            <AlertDescription>
              <p className="m-0">{index.failure.message}</p>
              <small>{index.failure.affectedValues} affected values</small>
            </AlertDescription>
          </Alert>
        )}
        <div>
          <Button
            size="sm"
            variant="ghost"
            className="-ml-2 text-destructive hover:bg-destructive/10 hover:text-destructive"
            disabled={index.state === "deleting"}
            onClick={onDelete}
          >
            Remove index
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}

/** The quiet link back up the hierarchy that sits above every page title. */
function BackButton({ onClick, children }: { readonly onClick: () => void; children: ReactNode }) {
  return (
    <div>
      <Button
        variant="ghost"
        size="sm"
        className="-ml-2 text-muted-foreground hover:text-foreground"
        onClick={onClick}
      >
        {children}
      </Button>
    </div>
  );
}

/** One term and its value in a metadata list. */
function MetaItem({
  term,
  compact = false,
  children,
}: {
  readonly term: string;
  readonly compact?: boolean;
  readonly children: ReactNode;
}) {
  return (
    <div className={compact ? "flex items-baseline justify-between gap-3" : "grid gap-0.5"}>
      <dt
        className={
          compact
            ? "text-muted-foreground"
            : "text-xs font-medium uppercase tracking-wider text-muted-foreground"
        }
      >
        {term}
      </dt>
      <dd className={compact ? "m-0 font-mono" : "m-0 text-sm"}>{children}</dd>
    </div>
  );
}

function JsonField({
  id,
  name,
  label,
  defaultValue,
  rows = 10,
}: {
  readonly id: string;
  readonly name: string;
  readonly label: string;
  readonly defaultValue: string;
  readonly rows?: number;
}) {
  return (
    <Field label={label} htmlFor={id}>
      <Textarea
        id={id}
        name={name}
        rows={rows}
        required
        defaultValue={defaultValue}
        spellCheck={false}
        className={`${rows <= 4 ? "min-h-24" : "min-h-40"} max-h-[32rem] font-mono text-xs leading-5 md:text-xs`}
      />
    </Field>
  );
}

function JsonPreview({ value }: { readonly value: unknown }) {
  return (
    <pre className="m-0 overflow-x-auto rounded-md border bg-muted/40 p-3 font-mono text-xs leading-5">
      {JSON.stringify(value, null, 2)}
    </pre>
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
