import { type FormEvent, useCallback, useEffect, useState } from "react";

import {
  type CreateStorageBucketRequest,
  ManagementApiError,
  type StorageBucket,
  type StorageBucketAccess,
  type StorageBucketRule,
  type StorageObject,
  type StorageObjectPage,
  type UpdateStorageBucketRequest,
} from "@mako-cloud/management-sdk";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useManagementClient } from "./management.js";
import { confirmDestructiveAction } from "./safety.js";

/** The platform ceiling for a single object; the API refuses larger limits. */
export const MAX_OBJECT_BYTES_CEILING = 16 * 1024 * 1024;
const DEFAULT_MAX_OBJECT_BYTES = 1024 * 1024;
const OBJECT_PAGE_SIZE = 100;
const BUCKET_ID_PATTERN = "[a-z][a-z0-9-]{1,62}";
const RULE_OPERATIONS = ["create", "read", "update", "delete"] as const;
type RuleOperation = (typeof RULE_OPERATIONS)[number];

/** The owner-only policy `docs/file-storage.md` describes, as a starting point. */
export const STARTER_RULES: readonly StorageBucketRule[] = [
  {
    id: "owner-creates",
    effect: "allow",
    operations: ["create"],
    expression: "new.owner_id == identity.user_id",
  },
  {
    id: "owner-changes",
    effect: "allow",
    operations: ["update"],
    expression: "old.owner_id == identity.user_id && new.owner_id == identity.user_id",
  },
  {
    id: "owner-reads-deletes",
    effect: "allow",
    operations: ["read", "delete"],
    expression: "old.owner_id == identity.user_id",
  },
];

// Buckets per environment and the objects they hold, over the management
// API's storage-bucket routes: the list with its totals and a create form,
// or one bucket's settings, totals, and objects.
export function StorageScreen({
  projectId,
  environmentId,
  bucketId,
  navigate,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly bucketId?: string | undefined;
  readonly navigate: (path: string) => void;
}) {
  const base = `/projects/${projectId}/environments/${environmentId}/storage`;
  return bucketId === undefined ? (
    <BucketListScreen
      projectId={projectId}
      environmentId={environmentId}
      onOpen={(id) => navigate(`${base}/${id}`)}
    />
  ) : (
    <BucketScreen
      projectId={projectId}
      environmentId={environmentId}
      bucketId={bucketId}
      onBack={() => navigate(base)}
    />
  );
}

function BucketListScreen({
  projectId,
  environmentId,
  onOpen,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly onOpen: (bucketId: string) => void;
}) {
  const client = useManagementClient();
  const [buckets, setBuckets] = useState<StorageBucket[] | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const [deleting, setDeleting] = useState<string | null>(null);
  const reload = useCallback(async () => {
    try {
      setBuckets(await client.listStorageBuckets(projectId, environmentId));
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
    setCreating(true);
    setStatus(null);
    try {
      const input: CreateStorageBucketRequest = {
        id: requiredText(data, "id"),
        ...bucketSettings(data),
      };
      const bucket = await client.createStorageBucket(
        projectId,
        environmentId,
        input,
        idempotencyKey(),
      );
      setFailure(null);
      setStatus(`Bucket ${bucket.id} created.`);
      form.reset();
      await reload();
    } catch (error) {
      setFailure(failureFrom(error));
    } finally {
      setCreating(false);
    }
  };

  const remove = async (bucket: StorageBucket) => {
    if (
      !confirmDestructiveAction({
        action: "Delete bucket",
        target: bucket.id,
        consequence:
          "The bucket and its access rules are removed. A bucket that still holds objects is refused until their loss is confirmed.",
      })
    ) {
      return;
    }
    const confirmation = `delete:${bucket.id}`;
    setDeleting(bucket.id);
    setStatus(null);
    try {
      let removal: { objectCount: number; totalBytes: number };
      try {
        removal = await client.deleteStorageBucket(
          projectId,
          environmentId,
          bucket.id,
          confirmation,
        );
      } catch (error) {
        if (!(error instanceof ManagementApiError && error.status === 409)) {
          throw error;
        }
        // The bucket still holds objects: deleting them is a second, explicit
        // decision, taken with the count and size in front of the developer.
        if (
          !confirmDestructiveAction({
            action: "Delete bucket",
            target: `${bucket.id} and its ${bucket.objectCount.toLocaleString("en-US")} objects (${formatBytes(bucket.totalBytes)})`,
            consequence:
              "Every object in the bucket is permanently removed along with it. This cannot be undone.",
          })
        ) {
          return;
        }
        removal = await client.deleteStorageBucket(
          projectId,
          environmentId,
          bucket.id,
          confirmation,
          true,
        );
      }
      setFailure(null);
      setStatus(
        removal.objectCount === 0
          ? `Bucket ${bucket.id} deleted.`
          : `Bucket ${bucket.id} deleted with ${removal.objectCount.toLocaleString("en-US")} objects (${formatBytes(removal.totalBytes)}).`,
      );
      await reload();
    } catch (error) {
      setFailure(failureFrom(error));
    } finally {
      setDeleting(null);
    }
  };

  return (
    <section aria-labelledby="storage-title" className="storage-screen">
      <div className="section-heading">
        <div>
          <p className="eyebrow">Environment {environmentId}</p>
          <h1 id="storage-title">Storage</h1>
          <p>
            Buckets hold application files for project <code>{projectId}</code>, governed by the
            same policy language as documents and metered like every other resource.
          </p>
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      {status === null ? null : (
        <p className="notice success" role="status">
          {status}
        </p>
      )}
      <div className="split-grid">
        <section className="panel" aria-labelledby="bucket-list-title">
          <h2 id="bucket-list-title">Buckets</h2>
          {buckets === null ? (
            <p aria-live="polite">Loading buckets…</p>
          ) : buckets.length === 0 ? (
            <p>No buckets have been created in this environment.</p>
          ) : (
            <div className="table-scroll">
              <table className="bucket-table">
                <thead>
                  <tr>
                    <th scope="col">Bucket</th>
                    <th scope="col">Access</th>
                    <th scope="col" className="numeric">
                      Objects
                    </th>
                    <th scope="col" className="numeric">
                      Bytes
                    </th>
                    <th scope="col" className="numeric">
                      Max object size
                    </th>
                    <th scope="col">Content types</th>
                    <th scope="col">Updated</th>
                    <th scope="col">
                      <span className="visually-hidden">Actions</span>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {buckets.map((bucket) => (
                    <tr key={bucket.id} data-bucket-id={bucket.id}>
                      <th scope="row">
                        <code>{bucket.id}</code>
                      </th>
                      <td>{bucket.access}</td>
                      <td className="numeric">{bucket.objectCount.toLocaleString("en-US")}</td>
                      <td className="numeric">{formatBytes(bucket.totalBytes)}</td>
                      <td className="numeric">{formatBytes(bucket.maxObjectBytes)}</td>
                      <td>{formatContentTypes(bucket.allowedContentTypes)}</td>
                      <td>
                        <time dateTime={bucket.updatedAt}>
                          {new Date(bucket.updatedAt).toLocaleString()}
                        </time>
                      </td>
                      <td>
                        <div className="button-row">
                          <button
                            type="button"
                            className="secondary"
                            aria-label={`Open bucket ${bucket.id}`}
                            onClick={() => onOpen(bucket.id)}
                          >
                            Open
                          </button>
                          <button
                            type="button"
                            className="danger"
                            aria-label={`Delete bucket ${bucket.id}`}
                            disabled={deleting !== null}
                            onClick={() => void remove(bucket)}
                          >
                            {deleting === bucket.id ? "Deleting…" : "Delete"}
                          </button>
                        </div>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </section>
        <section className="panel" aria-labelledby="create-bucket-title">
          <h2 id="create-bucket-title">Create bucket</h2>
          <p>
            A policy bucket with no rules refuses every request; rules that do not compile are
            refused before the bucket is installed.
          </p>
          <form onSubmit={(event) => void create(event)}>
            <label>
              Bucket ID
              <input name="id" required pattern={BUCKET_ID_PATTERN} spellCheck={false} />
            </label>
            <BucketSettingsFields />
            <button type="submit" disabled={creating}>
              {creating ? "Creating bucket…" : "Create bucket"}
            </button>
          </form>
        </section>
      </div>
    </section>
  );
}

function BucketScreen({
  projectId,
  environmentId,
  bucketId,
  onBack,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly bucketId: string;
  readonly onBack: () => void;
}) {
  const client = useManagementClient();
  const [bucket, setBucket] = useState<StorageBucket | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [prefix, setPrefix] = useState("");
  const [page, setPage] = useState<StorageObjectPage | null>(null);
  const [loadingObjects, setLoadingObjects] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [deletingPath, setDeletingPath] = useState<string | null>(null);

  const reloadBucket = useCallback(async () => {
    try {
      setBucket(await client.getStorageBucket(projectId, environmentId, bucketId));
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  }, [bucketId, client, environmentId, projectId]);
  useEffect(() => {
    void reloadBucket();
  }, [reloadBucket]);

  useEffect(() => {
    let cancelled = false;
    setLoadingObjects(true);
    client
      .listStorageObjects(projectId, environmentId, bucketId, objectQuery(prefix))
      .then((next) => {
        if (!cancelled) {
          setPage(next);
          setFailure(null);
        }
      })
      .catch((error: unknown) => {
        if (!cancelled) {
          setFailure(failureFrom(error));
        }
      })
      .finally(() => {
        if (!cancelled) {
          setLoadingObjects(false);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [bucketId, client, environmentId, prefix, projectId]);

  const save = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    setSaving(true);
    setStatus(null);
    try {
      const input: UpdateStorageBucketRequest = bucketSettings(data);
      setBucket(
        await client.updateStorageBucket(
          projectId,
          environmentId,
          bucketId,
          input,
          idempotencyKey(),
        ),
      );
      setFailure(null);
      setStatus("Bucket settings saved.");
    } catch (error) {
      setFailure(failureFrom(error));
    } finally {
      setSaving(false);
    }
  };

  const applyPrefix = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    setPrefix(String(data.get("prefix") ?? "").trim());
  };

  const loadMore = async () => {
    if (page === null || page.nextCursor === null) {
      return;
    }
    setLoadingMore(true);
    try {
      const next = await client.listStorageObjects(
        projectId,
        environmentId,
        bucketId,
        objectQuery(prefix, page.nextCursor),
      );
      setPage({ ...next, items: [...page.items, ...next.items] });
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    } finally {
      setLoadingMore(false);
    }
  };

  const removeObject = async (object: StorageObject) => {
    if (
      !confirmDestructiveAction({
        action: "Delete object",
        target: object.path,
        consequence: `The object's bytes and metadata are removed from bucket ${bucketId}. This cannot be undone.`,
      })
    ) {
      return;
    }
    setDeletingPath(object.path);
    setStatus(null);
    try {
      await client.deleteStorageObject(projectId, environmentId, bucketId, object.path);
      setPage((current) =>
        current === null
          ? current
          : { ...current, items: current.items.filter((item) => item.path !== object.path) },
      );
      setFailure(null);
      setStatus(`Object ${object.path} deleted.`);
      await reloadBucket();
    } catch (error) {
      setFailure(failureFrom(error));
    } finally {
      setDeletingPath(null);
    }
  };

  return (
    <section aria-labelledby="bucket-title" className="storage-screen">
      <button type="button" className="back-link" onClick={onBack}>
        ← Buckets
      </button>
      <div className="section-heading">
        <div>
          <p className="eyebrow">Bucket</p>
          <h1 id="bucket-title">{bucketId}</h1>
          <p>
            Objects are served under{" "}
            <code>
              /v1/projects/{projectId}/environments/{environmentId}/storage/{bucketId}/objects/
            </code>
          </p>
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      {status === null ? null : (
        <p className="notice success" role="status">
          {status}
        </p>
      )}
      {bucket === null ? (
        <p aria-live="polite">Loading bucket…</p>
      ) : (
        <div className="split-grid">
          <section className="panel" aria-labelledby="bucket-totals-title">
            <h2 id="bucket-totals-title">Totals</h2>
            <dl className="definition-grid bucket-totals">
              <div>
                <dt>Objects</dt>
                <dd data-field="objectCount">{bucket.objectCount.toLocaleString("en-US")}</dd>
              </div>
              <div>
                <dt>Stored bytes</dt>
                <dd data-field="totalBytes">{formatBytes(bucket.totalBytes)}</dd>
              </div>
              <div>
                <dt>Access</dt>
                <dd data-field="access">{bucket.access}</dd>
              </div>
              <div>
                <dt>Max object size</dt>
                <dd data-field="maxObjectBytes">{formatBytes(bucket.maxObjectBytes)}</dd>
              </div>
              <div>
                <dt>Version</dt>
                <dd data-field="version">{bucket.version}</dd>
              </div>
              <div>
                <dt>Created</dt>
                <dd>
                  <time dateTime={bucket.createdAt}>
                    {new Date(bucket.createdAt).toLocaleString()}
                  </time>
                </dd>
              </div>
              <div>
                <dt>Updated</dt>
                <dd>
                  <time dateTime={bucket.updatedAt}>
                    {new Date(bucket.updatedAt).toLocaleString()}
                  </time>
                </dd>
              </div>
            </dl>
          </section>
          <section className="panel" aria-labelledby="bucket-settings-title">
            <h2 id="bucket-settings-title">Settings</h2>
            <p>Changes apply to every request that follows; stored objects are kept as they are.</p>
            <form key={`settings-${bucket.version}`} onSubmit={(event) => void save(event)}>
              <BucketSettingsFields bucket={bucket} />
              <button type="submit" disabled={saving}>
                {saving ? "Saving…" : "Save"}
              </button>
            </form>
          </section>
          <section className="panel full-span" aria-labelledby="bucket-objects-title">
            <div className="button-row spread">
              <h2 id="bucket-objects-title">Objects</h2>
              <small>
                {page === null
                  ? ""
                  : `Showing ${page.items.length.toLocaleString("en-US")} of ${bucket.objectCount.toLocaleString("en-US")} objects.`}
              </small>
            </div>
            <form className="object-filter" onSubmit={applyPrefix}>
              <label>
                Path prefix
                <input
                  name="prefix"
                  defaultValue={prefix}
                  placeholder="avatars/"
                  spellCheck={false}
                />
              </label>
              <button type="submit" className="secondary">
                Apply prefix
              </button>
            </form>
            {loadingObjects && page === null ? (
              <p aria-live="polite">Loading objects…</p>
            ) : page === null || page.items.length === 0 ? (
              <p className="object-empty">
                {prefix === ""
                  ? "The bucket holds no objects."
                  : `No objects start with ${prefix}.`}
              </p>
            ) : (
              <ObjectTable
                objects={page.items}
                deletingPath={deletingPath}
                onDelete={(object) => void removeObject(object)}
              />
            )}
            <div className="button-row spread">
              <small>
                {page === null || page.nextCursor === null
                  ? "Every matching object is listed."
                  : "More objects match."}
              </small>
              <button
                type="button"
                className="secondary"
                disabled={
                  page === null || page.nextCursor === null || loadingObjects || loadingMore
                }
                onClick={() => void loadMore()}
              >
                {loadingMore ? "Loading…" : "Load more"}
              </button>
            </div>
          </section>
        </div>
      )}
    </section>
  );
}

function ObjectTable({
  objects,
  deletingPath,
  onDelete,
}: {
  readonly objects: readonly StorageObject[];
  readonly deletingPath: string | null;
  readonly onDelete: (object: StorageObject) => void;
}) {
  return (
    <div className="table-scroll">
      <table className="object-table">
        <thead>
          <tr>
            <th scope="col">Path</th>
            <th scope="col">Content type</th>
            <th scope="col" className="numeric">
              Size
            </th>
            <th scope="col">Owner</th>
            <th scope="col">Updated</th>
            <th scope="col">
              <span className="visually-hidden">Actions</span>
            </th>
          </tr>
        </thead>
        <tbody>
          {objects.map((object) => (
            <tr key={object.path} data-object-path={object.path}>
              <th scope="row">
                <code className="object-path">{object.path}</code>
              </th>
              <td>{object.contentType}</td>
              <td className="numeric">{formatBytes(object.sizeBytes)}</td>
              <td>{object.ownerId === null ? <em>none</em> : <code>{object.ownerId}</code>}</td>
              <td>
                <time dateTime={object.updatedAt}>
                  {new Date(object.updatedAt).toLocaleString()}
                </time>
              </td>
              <td>
                <button
                  type="button"
                  className="danger"
                  aria-label={`Delete object ${object.path}`}
                  disabled={deletingPath !== null}
                  onClick={() => onDelete(object)}
                >
                  {deletingPath === object.path ? "Deleting…" : "Delete"}
                </button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** The editable bucket settings, shared by the create form and the bucket's settings form. */
function BucketSettingsFields({ bucket }: { readonly bucket?: StorageBucket }) {
  return (
    <>
      <label>
        Access
        <select name="access" defaultValue={bucket?.access ?? "policy"}>
          <option value="policy">policy · every request is evaluated against the rules</option>
          <option value="public">public · reads need no session; writes are still evaluated</option>
        </select>
      </label>
      <label>
        Max object bytes
        <input
          name="maxObjectBytes"
          type="number"
          min="1"
          max={MAX_OBJECT_BYTES_CEILING}
          step="1"
          required
          defaultValue={bucket?.maxObjectBytes ?? DEFAULT_MAX_OBJECT_BYTES}
        />
      </label>
      <small className="field-hint">
        Up to {formatBytes(MAX_OBJECT_BYTES_CEILING)} (
        {MAX_OBJECT_BYTES_CEILING.toLocaleString("en-US")} bytes).
      </small>
      <label>
        Allowed content types
        <input
          name="allowedContentTypes"
          placeholder="image/*, application/pdf"
          spellCheck={false}
          defaultValue={bucket?.allowedContentTypes.join(", ") ?? ""}
        />
      </label>
      <small className="field-hint">
        Media types or <code>type/*</code> patterns, separated by commas or spaces; leave empty to
        allow every type.
      </small>
      <label>
        Rules (JSON)
        <textarea
          name="rules"
          rows={12}
          spellCheck={false}
          defaultValue={JSON.stringify(bucket?.rules ?? STARTER_RULES, null, 2)}
        />
      </label>
      <small className="field-hint">
        Expressions see the object document — <code>new.*</code> on create and update,{" "}
        <code>old.*</code> on read, update, and delete — plus <code>identity.*</code>,{" "}
        <code>claims.*</code>, and <code>request.*</code>.
      </small>
    </>
  );
}

function bucketSettings(data: FormData): {
  access: StorageBucketAccess;
  maxObjectBytes: number;
  allowedContentTypes: string[];
  rules: StorageBucketRule[];
} {
  return {
    access: parseAccess(requiredText(data, "access")),
    maxObjectBytes: parseMaxObjectBytes(requiredText(data, "maxObjectBytes")),
    allowedContentTypes: parseContentTypes(String(data.get("allowedContentTypes") ?? "")),
    rules: parseRules(String(data.get("rules") ?? "")),
  };
}

function objectQuery(
  prefix: string,
  cursor?: string,
): { readonly prefix?: string; readonly limit?: number; readonly cursor?: string } {
  return {
    limit: OBJECT_PAGE_SIZE,
    ...(prefix === "" ? {} : { prefix }),
    ...(cursor === undefined ? {} : { cursor }),
  };
}

/** Bytes as a developer reads them: whole bytes below a KiB, one decimal above. */
export function formatBytes(value: number): string {
  const units = ["B", "KiB", "MiB", "GiB", "TiB"] as const;
  let scaled = value;
  let index = 0;
  while (scaled >= 1024 && index < units.length - 1) {
    scaled /= 1024;
    index += 1;
  }
  return index === 0
    ? `${value.toLocaleString("en-US")} B`
    : `${scaled.toFixed(1)} ${units[index] ?? "B"}`;
}

function formatContentTypes(types: readonly string[]): string {
  return types.length === 0 ? "any" : types.join(", ");
}

/** Comma- or whitespace-separated media types, deduplicated in order. */
export function parseContentTypes(raw: string): string[] {
  return Array.from(
    new Set(
      raw
        .split(/[\s,]+/u)
        .map((type) => type.trim())
        .filter((type) => type !== ""),
    ),
  );
}

function parseAccess(raw: string): StorageBucketAccess {
  if (raw === "policy" || raw === "public") {
    return raw;
  }
  throw new FormInputError("Access must be policy or public.");
}

function parseMaxObjectBytes(raw: string): number {
  const value = Number(raw);
  if (!Number.isSafeInteger(value) || value < 1) {
    throw new FormInputError("Max object bytes must be a positive integer.");
  }
  if (value > MAX_OBJECT_BYTES_CEILING) {
    throw new FormInputError(
      `Max object bytes cannot exceed the platform ceiling of ${formatBytes(MAX_OBJECT_BYTES_CEILING)} (${MAX_OBJECT_BYTES_CEILING.toLocaleString("en-US")} bytes).`,
    );
  }
  return value;
}

/**
 * The rules textarea as a JSON array of rules. Shape is checked here so a
 * malformed document never reaches the API; whether the expressions compile
 * is the API's verdict, shown verbatim when it refuses.
 */
export function parseRules(raw: string): StorageBucketRule[] {
  const trimmed = raw.trim();
  if (trimmed === "") {
    return [];
  }
  let value: unknown;
  try {
    value = JSON.parse(trimmed) as unknown;
  } catch {
    throw new FormInputError("Rules must be valid JSON.");
  }
  if (!Array.isArray(value)) {
    throw new FormInputError("Rules must be a JSON array of rule objects.");
  }
  return value.map((entry: unknown, index) => parseRule(entry, index));
}

function parseRule(entry: unknown, index: number): StorageBucketRule {
  const position = `Rule ${index + 1}`;
  if (typeof entry !== "object" || entry === null || Array.isArray(entry)) {
    throw new FormInputError(`${position} must be an object.`);
  }
  const rule = entry as Record<string, unknown>;
  const { id, effect, operations, expression } = rule;
  if (typeof id !== "string" || id.trim() === "") {
    throw new FormInputError(`${position} needs a non-empty string "id".`);
  }
  if (effect !== "allow" && effect !== "deny") {
    throw new FormInputError(`${position} (${id}) needs an "effect" of "allow" or "deny".`);
  }
  if (
    !Array.isArray(operations) ||
    operations.length === 0 ||
    !operations.every((operation): operation is RuleOperation =>
      (RULE_OPERATIONS as readonly unknown[]).includes(operation),
    )
  ) {
    throw new FormInputError(
      `${position} (${id}) needs "operations" listing one or more of ${RULE_OPERATIONS.join(", ")}.`,
    );
  }
  if (typeof expression !== "string" || expression.trim() === "") {
    throw new FormInputError(`${position} (${id}) needs a non-empty string "expression".`);
  }
  const unknown = Object.keys(rule).filter(
    (key) => !["id", "effect", "operations", "expression"].includes(key),
  );
  if (unknown.length > 0) {
    throw new FormInputError(`${position} (${id}) has unknown fields: ${unknown.join(", ")}.`);
  }
  return { id, effect, operations: [...operations], expression };
}

class FormInputError extends Error {}

function requiredText(data: FormData, name: string): string {
  const value = String(data.get(name) ?? "").trim();
  if (value === "") {
    throw new FormInputError(`${name} is required.`);
  }
  return value;
}

function failureFrom(error: unknown): ConsoleApiFailure {
  return error instanceof FormInputError
    ? { message: error.message, requestId: null }
    : toConsoleApiFailure(error);
}

function idempotencyKey(): string {
  return globalThis.crypto.randomUUID();
}
