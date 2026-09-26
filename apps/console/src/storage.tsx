import {
  Alert,
  AlertDescription,
  Badge,
  Button,
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
  EmptyState,
  Eyebrow,
  Field,
  Input,
  NativeSelect,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  Textarea,
  cn,
} from "@mako-cloud/ui";
import { HardDrive } from "lucide-react";
import { type FormEvent, type ReactNode, useCallback, useEffect, useId, useState } from "react";

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
const BUCKET_ID_PATTERN = "[a-z][a-z0-9\\-]{1,62}";
const RULE_OPERATIONS = ["create", "read", "update", "delete"] as const;
type RuleOperation = (typeof RULE_OPERATIONS)[number];

/** A code snippet inline in prose or a cell: an identifier, a path, a pattern. */
const CODE = "rounded bg-muted px-1 py-0.5 font-mono text-[0.85em]";
/** A term in the totals list. */
const TERM = "m-0 text-xs font-medium tracking-wide text-muted-foreground uppercase";
/** Its definition. */
const DEFINITION = "m-0 mt-0.5 text-sm font-medium break-words tabular-nums";
/** The destructive action in a row: outlined, so the table stays quiet until it is needed. */
const DANGER_OUTLINE =
  "border-destructive/40 text-destructive hover:bg-destructive/10 hover:text-destructive";

/** The owner-only policy the User Book's file-storage chapter describes, as a starting point. */
export const STARTER_RULES: readonly StorageBucketRule[] = [
  {
    // The uploader owns what they upload, so owner_id alone let anyone take a
    // name inside someone else's folder; folder is the path's first segment.
    id: "owner-creates",
    effect: "allow",
    operations: ["create"],
    expression: "new.owner_id == identity.user_id && new.folder == identity.user_id",
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
  const idField = useId();
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
    <section aria-labelledby="storage-title" className="grid gap-6">
      <div className="grid gap-1">
        <Eyebrow>Environment {environmentId}</Eyebrow>
        <h1 id="storage-title" className="text-2xl">
          Storage
        </h1>
        <p className="m-0 max-w-3xl text-sm text-muted-foreground">
          Buckets hold application files for project <code className={CODE}>{projectId}</code>,
          governed by the same policy language as documents and metered like every other resource.
        </p>
      </div>
      <ApiFailureNotice failure={failure} />
      {status === null ? null : <StatusNotice>{status}</StatusNotice>}
      <div className="grid items-start gap-6">
        <Card aria-labelledby="bucket-list-title">
          <CardHeader>
            <CardTitle id="bucket-list-title">Buckets</CardTitle>
          </CardHeader>
          <CardContent>
            {buckets === null ? (
              <p className="m-0 text-sm text-muted-foreground" aria-live="polite">
                Loading buckets…
              </p>
            ) : buckets.length === 0 ? (
              <EmptyState
                icon={<HardDrive aria-hidden="true" />}
                title="No buckets have been created in this environment."
                className="border-0 py-8"
              />
            ) : (
              <Table>
                <TableHeader>
                  <TableRow className="hover:bg-transparent">
                    <TableHead scope="col">Bucket</TableHead>
                    <TableHead scope="col">Access</TableHead>
                    <TableHead scope="col" className="text-right">
                      Objects
                    </TableHead>
                    <TableHead scope="col" className="text-right">
                      Bytes
                    </TableHead>
                    <TableHead scope="col" className="text-right">
                      Max object size
                    </TableHead>
                    <TableHead scope="col">Content types</TableHead>
                    <TableHead scope="col">Updated</TableHead>
                    <TableHead scope="col" className="text-right">
                      <span className="sr-only">Actions</span>
                    </TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {buckets.map((bucket) => (
                    <TableRow key={bucket.id} data-bucket-id={bucket.id}>
                      <TableHead scope="row">
                        <code className={CODE}>{bucket.id}</code>
                      </TableHead>
                      <TableCell>
                        <Badge variant="outline">{bucket.access}</Badge>
                      </TableCell>
                      <TableCell className="text-right tabular-nums">
                        {bucket.objectCount.toLocaleString("en-US")}
                      </TableCell>
                      <TableCell className="text-right tabular-nums">
                        {formatBytes(bucket.totalBytes)}
                      </TableCell>
                      <TableCell className="text-right tabular-nums">
                        {formatBytes(bucket.maxObjectBytes)}
                      </TableCell>
                      <TableCell className="font-mono text-xs">
                        {formatContentTypes(bucket.allowedContentTypes)}
                      </TableCell>
                      <TableCell className="text-muted-foreground">
                        <Timestamp value={bucket.updatedAt} />
                      </TableCell>
                      <TableCell className="text-right">
                        <div className="flex justify-end gap-1">
                          <Button
                            variant="outline"
                            size="sm"
                            aria-label={`Open bucket ${bucket.id}`}
                            onClick={() => onOpen(bucket.id)}
                          >
                            Open
                          </Button>
                          <Button
                            variant="outline"
                            size="sm"
                            className={DANGER_OUTLINE}
                            aria-label={`Delete bucket ${bucket.id}`}
                            disabled={deleting !== null}
                            onClick={() => void remove(bucket)}
                          >
                            {deleting === bucket.id ? "Deleting…" : "Delete"}
                          </Button>
                        </div>
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            )}
          </CardContent>
        </Card>
        <Card aria-labelledby="create-bucket-title" className="w-full max-w-3xl">
          <CardHeader>
            <CardTitle id="create-bucket-title">Create bucket</CardTitle>
            <CardDescription>
              A policy bucket with no rules refuses every request; rules that do not compile are
              refused before the bucket is installed.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <form className="grid gap-4" onSubmit={(event) => void create(event)}>
              <Field label="Bucket ID" htmlFor={idField}>
                <Input
                  id={idField}
                  name="id"
                  required
                  pattern={BUCKET_ID_PATTERN}
                  spellCheck={false}
                  className="font-mono"
                />
              </Field>
              <BucketSettingsFields />
              <div>
                <Button type="submit" disabled={creating}>
                  {creating ? "Creating bucket…" : "Create bucket"}
                </Button>
              </div>
            </form>
          </CardContent>
        </Card>
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
  const prefixId = useId();
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
    <section aria-labelledby="bucket-title" className="grid gap-6">
      <div className="grid gap-2">
        <Button
          variant="ghost"
          size="sm"
          className="-ml-2 w-fit text-muted-foreground hover:text-foreground"
          onClick={onBack}
        >
          ← Buckets
        </Button>
        <div className="grid gap-1">
          <Eyebrow>Bucket</Eyebrow>
          <h1 id="bucket-title" className="font-mono text-2xl">
            {bucketId}
          </h1>
          <p className="m-0 text-sm text-muted-foreground">
            Objects are served under{" "}
            <code className={cn(CODE, "break-all")}>
              /v1/projects/{projectId}/environments/{environmentId}/storage/{bucketId}/objects/
            </code>
          </p>
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      {status === null ? null : <StatusNotice>{status}</StatusNotice>}
      {bucket === null ? (
        <p className="m-0 text-sm text-muted-foreground" aria-live="polite">
          Loading bucket…
        </p>
      ) : (
        <div className="grid items-start gap-6 lg:grid-cols-2">
          <Card aria-labelledby="bucket-totals-title">
            <CardHeader>
              <CardTitle id="bucket-totals-title">Totals</CardTitle>
            </CardHeader>
            <CardContent>
              <dl className="m-0 grid gap-x-6 gap-y-4 sm:grid-cols-2">
                <div>
                  <dt className={TERM}>Objects</dt>
                  <dd className={DEFINITION} data-field="objectCount">
                    {bucket.objectCount.toLocaleString("en-US")}
                  </dd>
                </div>
                <div>
                  <dt className={TERM}>Stored bytes</dt>
                  <dd className={DEFINITION} data-field="totalBytes">
                    {formatBytes(bucket.totalBytes)}
                  </dd>
                </div>
                <div>
                  <dt className={TERM}>Access</dt>
                  <dd className={DEFINITION} data-field="access">
                    {bucket.access}
                  </dd>
                </div>
                <div>
                  <dt className={TERM}>Max object size</dt>
                  <dd className={DEFINITION} data-field="maxObjectBytes">
                    {formatBytes(bucket.maxObjectBytes)}
                  </dd>
                </div>
                <div>
                  <dt className={TERM}>Version</dt>
                  <dd className={DEFINITION} data-field="version">
                    {bucket.version}
                  </dd>
                </div>
                <div>
                  <dt className={TERM}>Created</dt>
                  <dd className={DEFINITION}>
                    <Timestamp value={bucket.createdAt} />
                  </dd>
                </div>
                <div>
                  <dt className={TERM}>Updated</dt>
                  <dd className={DEFINITION}>
                    <Timestamp value={bucket.updatedAt} />
                  </dd>
                </div>
              </dl>
            </CardContent>
          </Card>
          <Card aria-labelledby="bucket-settings-title">
            <CardHeader>
              <CardTitle id="bucket-settings-title">Settings</CardTitle>
              <CardDescription>
                Changes apply to every request that follows; stored objects are kept as they are.
              </CardDescription>
            </CardHeader>
            <CardContent>
              <form
                key={`settings-${bucket.version}`}
                className="grid gap-4"
                onSubmit={(event) => void save(event)}
              >
                <BucketSettingsFields bucket={bucket} />
                <div>
                  <Button type="submit" disabled={saving}>
                    {saving ? "Saving…" : "Save"}
                  </Button>
                </div>
              </form>
            </CardContent>
          </Card>
          <Card aria-labelledby="bucket-objects-title" className="lg:col-span-2">
            <CardHeader>
              <CardTitle id="bucket-objects-title">Objects</CardTitle>
              <CardDescription>
                {page === null
                  ? ""
                  : `Showing ${page.items.length.toLocaleString("en-US")} of ${bucket.objectCount.toLocaleString("en-US")} objects.`}
              </CardDescription>
            </CardHeader>
            <CardContent className="grid gap-4">
              <form className="flex flex-wrap items-end gap-3" onSubmit={applyPrefix}>
                <Field label="Path prefix" htmlFor={prefixId} className="w-72">
                  <Input
                    id={prefixId}
                    name="prefix"
                    defaultValue={prefix}
                    placeholder="avatars/"
                    spellCheck={false}
                    className="font-mono"
                  />
                </Field>
                <Button type="submit" variant="outline">
                  Apply prefix
                </Button>
              </form>
              {loadingObjects && page === null ? (
                <p className="m-0 text-sm text-muted-foreground" aria-live="polite">
                  Loading objects…
                </p>
              ) : page === null || page.items.length === 0 ? (
                <p className="m-0 text-sm text-muted-foreground">
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
              <div className="flex flex-wrap items-center justify-between gap-3">
                <small className="text-xs text-muted-foreground">
                  {page === null || page.nextCursor === null
                    ? "Every matching object is listed."
                    : "More objects match."}
                </small>
                <Button
                  variant="outline"
                  size="sm"
                  disabled={
                    page === null || page.nextCursor === null || loadingObjects || loadingMore
                  }
                  onClick={() => void loadMore()}
                >
                  {loadingMore ? "Loading…" : "Load more"}
                </Button>
              </div>
            </CardContent>
          </Card>
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
    <Table>
      <TableHeader>
        <TableRow className="hover:bg-transparent">
          <TableHead scope="col">Path</TableHead>
          <TableHead scope="col">Content type</TableHead>
          <TableHead scope="col" className="text-right">
            Size
          </TableHead>
          <TableHead scope="col">Owner</TableHead>
          <TableHead scope="col">Updated</TableHead>
          <TableHead scope="col" className="text-right">
            <span className="sr-only">Actions</span>
          </TableHead>
        </TableRow>
      </TableHeader>
      <TableBody>
        {objects.map((object) => (
          <TableRow key={object.path} data-object-path={object.path}>
            <TableHead scope="row" className="whitespace-normal">
              <code className={cn(CODE, "break-all")}>{object.path}</code>
            </TableHead>
            <TableCell className="font-mono text-xs">{object.contentType}</TableCell>
            <TableCell className="text-right tabular-nums">
              {formatBytes(object.sizeBytes)}
            </TableCell>
            <TableCell>
              {object.ownerId === null ? (
                <em className="text-muted-foreground">none</em>
              ) : (
                <code className={CODE}>{object.ownerId}</code>
              )}
            </TableCell>
            <TableCell className="text-muted-foreground">
              <Timestamp value={object.updatedAt} />
            </TableCell>
            <TableCell className="text-right">
              <Button
                variant="outline"
                size="sm"
                className={DANGER_OUTLINE}
                aria-label={`Delete object ${object.path}`}
                disabled={deletingPath !== null}
                onClick={() => onDelete(object)}
              >
                {deletingPath === object.path ? "Deleting…" : "Delete"}
              </Button>
            </TableCell>
          </TableRow>
        ))}
      </TableBody>
    </Table>
  );
}

/** The editable bucket settings, shared by the create form and the bucket's settings form. */
function BucketSettingsFields({ bucket }: { readonly bucket?: StorageBucket }) {
  const id = useId();
  return (
    <>
      <Field label="Access" htmlFor={`${id}-access`}>
        <NativeSelect id={`${id}-access`} name="access" defaultValue={bucket?.access ?? "policy"}>
          <option value="policy">policy · every request is evaluated against the rules</option>
          <option value="public">public · reads need no session; writes are still evaluated</option>
        </NativeSelect>
      </Field>
      <Field label="Max object bytes" htmlFor={`${id}-max-bytes`}>
        <Input
          id={`${id}-max-bytes`}
          name="maxObjectBytes"
          type="number"
          min="1"
          max={MAX_OBJECT_BYTES_CEILING}
          step="1"
          required
          defaultValue={bucket?.maxObjectBytes ?? DEFAULT_MAX_OBJECT_BYTES}
        />
        <p className="m-0 text-sm text-muted-foreground">
          Up to {formatBytes(MAX_OBJECT_BYTES_CEILING)} (
          {MAX_OBJECT_BYTES_CEILING.toLocaleString("en-US")} bytes).
        </p>
      </Field>
      <Field label="Allowed content types" htmlFor={`${id}-content-types`}>
        <Input
          id={`${id}-content-types`}
          name="allowedContentTypes"
          placeholder="image/*, application/pdf"
          spellCheck={false}
          defaultValue={bucket?.allowedContentTypes.join(", ") ?? ""}
          className="font-mono"
        />
        <p className="m-0 text-sm text-muted-foreground">
          Media types or <code className={CODE}>type/*</code> patterns, separated by commas or
          spaces; leave empty to allow every type.
        </p>
      </Field>
      <Field label="Rules (JSON)" htmlFor={`${id}-rules`}>
        <Textarea
          id={`${id}-rules`}
          name="rules"
          rows={12}
          spellCheck={false}
          defaultValue={JSON.stringify(bucket?.rules ?? STARTER_RULES, null, 2)}
          className="font-mono text-xs"
        />
        <p className="m-0 text-sm text-muted-foreground">
          Expressions see the object document — <code className={CODE}>new.*</code> on create and
          update, <code className={CODE}>old.*</code> on read, update, and delete — plus{" "}
          <code className={CODE}>identity.*</code>, <code className={CODE}>claims.*</code>, and{" "}
          <code className={CODE}>request.*</code>.
        </p>
      </Field>
    </>
  );
}

/** What the last action did, in the page's own words. */
function StatusNotice({ children }: { readonly children: ReactNode }) {
  return (
    <Alert variant="positive" role="status">
      <AlertDescription className="block text-foreground">{children}</AlertDescription>
    </Alert>
  );
}

function Timestamp({ value }: { readonly value: string }) {
  return (
    <time dateTime={value} className="whitespace-nowrap tabular-nums">
      {new Date(value).toLocaleString()}
    </time>
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
