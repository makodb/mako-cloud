import type {
  AdminCreateUserRequest,
  ApplicationUserSummary,
  ApplicationUserView,
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
} from "@mako-cloud/ui";
import { Search, TriangleAlert, UserPlus, Users } from "lucide-react";
import { type FormEvent, type ReactNode, useCallback, useEffect, useState } from "react";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useManagementClient } from "./management.js";
import { LifecycleBadge } from "./projects.js";
import { confirmDestructiveAction } from "./safety.js";

const EMPTY_OBJECT = JSON.stringify({}, null, 2);

export function ApplicationUsersScreen({
  projectId,
  environmentId,
  onBack,
  onOpen,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly onBack: () => void;
  readonly onOpen: (userId: string) => void;
}) {
  const client = useManagementClient();
  const [users, setUsers] = useState<ApplicationUserSummary[] | null>(null);
  const [truncated, setTruncated] = useState(false);
  const [query, setQuery] = useState("");
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const search = useCallback(
    async (nextQuery: string) => {
      try {
        const result = await client.searchApplicationUsers(projectId, environmentId, {
          ...(nextQuery === "" ? {} : { query: nextQuery }),
          limit: 100,
        });
        setUsers(result.users);
        setTruncated(result.truncated);
        setFailure(null);
      } catch (error) {
        setFailure(failureFrom(error));
      }
    },
    [client, environmentId, projectId],
  );
  useEffect(() => {
    void search("");
  }, [search]);

  const submitSearch = (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    void search(query.trim());
  };
  const create = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    try {
      const input: AdminCreateUserRequest = {
        email: requiredText(data, "email"),
        trustedMetadata: parseJsonObject(requiredText(data, "trustedMetadata"), "Trusted metadata"),
        profileMetadata: parseJsonObject(requiredText(data, "profileMetadata"), "Profile metadata"),
      };
      const mode = requiredText(data, "mode");
      const user =
        mode === "create"
          ? await client.createApplicationUser(projectId, environmentId, input, idempotencyKey())
          : await client.inviteApplicationUser(projectId, environmentId, input, idempotencyKey());
      setFailure(null);
      onOpen(user.id);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };

  return (
    <section aria-labelledby="users-title" className="grid gap-6">
      <div className="grid gap-3">
        <BackButton onClick={onBack}>← Project</BackButton>
        <div className="grid gap-1">
          <Eyebrow>Environment {environmentId}</Eyebrow>
          <h1 id="users-title" className="text-2xl">
            Application users
          </h1>
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      <div className="grid items-start gap-6 xl:grid-cols-[minmax(0,3fr)_minmax(0,2fr)]">
        <Card aria-labelledby="user-search-title">
          <CardHeader>
            <CardTitle id="user-search-title">Search users</CardTitle>
          </CardHeader>
          <CardContent className="grid gap-4">
            <search>
              <form className="flex flex-wrap items-end gap-3" onSubmit={submitSearch}>
                <Field label="Email or user ID" htmlFor="user-search" className="min-w-64 flex-1">
                  <Input
                    id="user-search"
                    type="search"
                    value={query}
                    maxLength={320}
                    onChange={(event) => setQuery(event.currentTarget.value)}
                  />
                </Field>
                <Button type="submit" variant="secondary">
                  <Search aria-hidden="true" />
                  Search
                </Button>
              </form>
            </search>
            {users === null ? (
              <p className="m-0 text-sm text-muted-foreground">Loading application users…</p>
            ) : users.length === 0 ? (
              <EmptyState icon={<Users aria-hidden="true" />} title="No users match this search." />
            ) : (
              <Table aria-label="Application users">
                <TableHeader>
                  <TableRow className="hover:bg-transparent">
                    <TableHead scope="col">User</TableHead>
                    <TableHead scope="col">Status</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {users.map((user) => (
                    <TableRow key={user.id}>
                      <TableCell className="whitespace-normal">
                        <Button
                          variant="link"
                          className="h-auto p-0 font-medium"
                          onClick={() => onOpen(user.id)}
                        >
                          {user.email ?? user.id}
                        </Button>
                        <span className="block font-mono text-xs text-muted-foreground">
                          {user.id}
                        </span>
                      </TableCell>
                      <TableCell>
                        <LifecycleBadge state={user.status} />
                      </TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            )}
            {truncated ? (
              <Alert variant="warning" role="status">
                <TriangleAlert aria-hidden="true" />
                <AlertDescription>Only the first 100 matches are shown.</AlertDescription>
              </Alert>
            ) : null}
          </CardContent>
        </Card>
        <Card aria-labelledby="create-user-title">
          <CardHeader>
            <CardTitle id="create-user-title">Create or invite</CardTitle>
            <CardDescription>
              Trusted metadata is administrator-controlled and may affect document access.
            </CardDescription>
          </CardHeader>
          <CardContent>
            <form className="grid gap-4" onSubmit={(event) => void create(event)}>
              <Field label="Action" htmlFor="create-user-mode">
                <NativeSelect id="create-user-mode" name="mode" defaultValue="invite">
                  <option value="invite">Invite by email</option>
                  <option value="create">Create active user</option>
                </NativeSelect>
              </Field>
              <Field label="Email" htmlFor="create-user-email">
                <Input id="create-user-email" name="email" type="email" required maxLength={320} />
              </Field>
              <JsonField name="trustedMetadata" label="Trusted metadata" />
              <JsonField name="profileMetadata" label="Profile metadata" />
              <Button type="submit" className="justify-self-start">
                <UserPlus aria-hidden="true" />
                Create application user
              </Button>
            </form>
          </CardContent>
        </Card>
      </div>
    </section>
  );
}

export function ApplicationUserScreen({
  projectId,
  environmentId,
  userId,
  onBack,
}: {
  readonly projectId: string;
  readonly environmentId: string;
  readonly userId: string;
  readonly onBack: () => void;
}) {
  const client = useManagementClient();
  const [user, setUser] = useState<ApplicationUserView | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const reload = useCallback(async () => {
    try {
      setUser(await client.getApplicationUser(projectId, environmentId, userId));
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  }, [client, environmentId, projectId, userId]);
  useEffect(() => {
    void reload();
  }, [reload]);

  const updateMetadata = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    try {
      setUser(
        await client.updateApplicationUserMetadata(projectId, environmentId, userId, {
          trustedMetadata: parseJsonObject(
            requiredText(data, "trustedMetadata"),
            "Trusted metadata",
          ),
          profileMetadata: parseJsonObject(
            requiredText(data, "profileMetadata"),
            "Profile metadata",
          ),
        }),
      );
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };
  const lifecycle = async (action: "disable" | "restore" | "delete" | "revoke-all") => {
    if (
      action !== "restore" &&
      !confirmDestructiveAction({
        action:
          action === "delete"
            ? "Delete"
            : action === "disable"
              ? "Disable"
              : "Revoke all sessions for",
        target: `application user ${user?.email ?? userId}`,
        consequence:
          action === "delete"
            ? "The user record will enter its terminal deleted state and active access will be revoked."
            : action === "disable"
              ? "Sign-in and existing application access will be denied until the user is restored."
              : "Every active session for this user will be invalidated.",
      })
    ) {
      return;
    }
    try {
      const updated =
        action === "disable"
          ? await client.disableApplicationUser(projectId, environmentId, userId)
          : action === "restore"
            ? await client.restoreApplicationUser(projectId, environmentId, userId)
            : action === "delete"
              ? await client.deleteApplicationUser(projectId, environmentId, userId)
              : await client.revokeApplicationUserSessions(projectId, environmentId, userId);
      setUser(updated);
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };
  const revokeSession = async (sessionId: string) => {
    if (
      !confirmDestructiveAction({
        action: "Revoke",
        target: `session ${sessionId}`,
        consequence: "The session will no longer authorize application requests.",
      })
    ) {
      return;
    }
    try {
      setUser(
        await client.revokeApplicationUserSession(projectId, environmentId, userId, sessionId),
      );
      setFailure(null);
    } catch (error) {
      setFailure(failureFrom(error));
    }
  };

  return (
    <section aria-labelledby="user-title" className="grid gap-6">
      <div className="grid gap-3">
        <BackButton onClick={onBack}>← Application users</BackButton>
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div className="grid gap-1">
            <Eyebrow>Application user</Eyebrow>
            <h1 id="user-title" className="break-all text-2xl">
              {user?.email ?? userId}
            </h1>
            <code className="font-mono text-xs text-muted-foreground">{userId}</code>
          </div>
          {user === null ? null : <LifecycleBadge state={user.status} />}
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      {user === null ? (
        <p className="m-0 text-sm text-muted-foreground">Loading application user…</p>
      ) : (
        <div className="grid items-start gap-6 xl:grid-cols-2">
          <Card aria-labelledby="metadata-title">
            <CardHeader>
              <CardTitle id="metadata-title">User metadata</CardTitle>
              <CardDescription>
                Trusted metadata affects authorization and is separate from user-editable profile
                metadata.
              </CardDescription>
            </CardHeader>
            <CardContent>
              <form
                key={user.updatedAt}
                className="grid gap-4"
                onSubmit={(event) => void updateMetadata(event)}
              >
                <Field label="Trusted metadata" htmlFor="user-trusted-metadata">
                  <Textarea
                    id="user-trusted-metadata"
                    name="trustedMetadata"
                    rows={10}
                    required
                    defaultValue={JSON.stringify(user.trustedMetadata, null, 2)}
                    spellCheck={false}
                    className="min-h-40 font-mono text-xs leading-relaxed"
                  />
                </Field>
                <Field label="Profile metadata" htmlFor="user-profile-metadata">
                  <Textarea
                    id="user-profile-metadata"
                    name="profileMetadata"
                    rows={10}
                    required
                    defaultValue={JSON.stringify(user.profileMetadata, null, 2)}
                    spellCheck={false}
                    className="min-h-40 font-mono text-xs leading-relaxed"
                  />
                </Field>
                <Button type="submit" className="justify-self-start">
                  Save metadata
                </Button>
              </form>
            </CardContent>
          </Card>
          <Card aria-labelledby="user-lifecycle-title">
            <CardHeader>
              <CardTitle id="user-lifecycle-title">Access and lifecycle</CardTitle>
            </CardHeader>
            <CardContent className="grid gap-5">
              <dl className="m-0 grid gap-2 text-sm">
                <div className="grid grid-cols-[8rem_minmax(0,1fr)] gap-3">
                  <dt className="text-muted-foreground">Session epoch</dt>
                  <dd className="m-0 font-mono tabular-nums">{user.sessionEpoch}</dd>
                </div>
                <div className="grid grid-cols-[8rem_minmax(0,1fr)] gap-3">
                  <dt className="text-muted-foreground">Created</dt>
                  <dd className="m-0">{new Date(user.createdAt).toLocaleString()}</dd>
                </div>
                <div className="grid grid-cols-[8rem_minmax(0,1fr)] gap-3">
                  <dt className="text-muted-foreground">Updated</dt>
                  <dd className="m-0">{new Date(user.updatedAt).toLocaleString()}</dd>
                </div>
              </dl>
              <div className="flex flex-wrap gap-2">
                <Button
                  variant="outline"
                  disabled={user.status !== "active"}
                  onClick={() => void lifecycle("disable")}
                >
                  Disable user
                </Button>
                <Button
                  variant="outline"
                  disabled={user.status !== "disabled"}
                  onClick={() => void lifecycle("restore")}
                >
                  Restore user
                </Button>
                <Button
                  variant="outline"
                  disabled={!user.sessions.some((session) => session.status === "active")}
                  onClick={() => void lifecycle("revoke-all")}
                >
                  Revoke all sessions
                </Button>
                <Button
                  variant="destructive"
                  disabled={user.status === "deleted"}
                  onClick={() => void lifecycle("delete")}
                >
                  Delete user
                </Button>
              </div>
            </CardContent>
          </Card>
          <SessionsPanel user={user} onRevoke={(sessionId) => void revokeSession(sessionId)} />
        </div>
      )}
    </section>
  );
}

function SessionsPanel({
  user,
  onRevoke,
}: {
  readonly user: ApplicationUserView;
  readonly onRevoke: (sessionId: string) => void;
}) {
  return (
    <Card className="xl:col-span-2" aria-labelledby="sessions-title">
      <CardHeader>
        <CardTitle id="sessions-title">Sessions</CardTitle>
      </CardHeader>
      <CardContent className="grid gap-4">
        {user.sessions.length === 0 ? (
          <p className="m-0 text-sm text-muted-foreground">No retained sessions.</p>
        ) : (
          <Table aria-labelledby="sessions-title">
            <TableHeader>
              <TableRow className="hover:bg-transparent">
                <TableHead scope="col">Session</TableHead>
                <TableHead scope="col">Status</TableHead>
                <TableHead scope="col">Created</TableHead>
                <TableHead scope="col">Expires</TableHead>
                <TableHead scope="col">Action</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {user.sessions.map((session) => (
                <TableRow key={session.id}>
                  <TableCell>
                    <code className="font-mono text-xs">{session.id}</code>
                  </TableCell>
                  <TableCell>
                    <LifecycleBadge state={session.status} />
                  </TableCell>
                  <TableCell className="text-muted-foreground">
                    {new Date(session.createdAt).toLocaleString()}
                  </TableCell>
                  <TableCell className="text-muted-foreground">
                    {new Date(session.expiresAt).toLocaleString()}
                  </TableCell>
                  <TableCell>
                    <Button
                      variant="ghost"
                      size="sm"
                      className="text-destructive hover:text-destructive"
                      disabled={session.status !== "active"}
                      onClick={() => onRevoke(session.id)}
                    >
                      Revoke
                    </Button>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
        {user.sessionsTruncated ? (
          <Alert variant="warning" role="status">
            <TriangleAlert aria-hidden="true" />
            <AlertDescription>Only the bounded retained session set is shown.</AlertDescription>
          </Alert>
        ) : null}
      </CardContent>
    </Card>
  );
}

/** The quiet way back up the hierarchy, above the page title. */
function BackButton({
  onClick,
  children,
}: {
  readonly onClick: () => void;
  readonly children: ReactNode;
}) {
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

function JsonField({ name, label }: { readonly name: string; readonly label: string }) {
  const id = `create-user-${name}`;
  return (
    <Field label={label} htmlFor={id}>
      <Textarea
        id={id}
        name={name}
        rows={6}
        required
        defaultValue={EMPTY_OBJECT}
        spellCheck={false}
        className="min-h-24 font-mono text-xs leading-relaxed"
      />
    </Field>
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

function failureFrom(error: unknown): ConsoleApiFailure {
  return error instanceof FormInputError
    ? { message: error.message, requestId: null }
    : toConsoleApiFailure(error);
}

function idempotencyKey(): string {
  return globalThis.crypto.randomUUID();
}
