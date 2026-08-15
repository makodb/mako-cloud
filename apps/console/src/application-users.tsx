import { type FormEvent, useCallback, useEffect, useState } from "react";

import type {
  AdminCreateUserRequest,
  ApplicationUserSummary,
  ApplicationUserView,
} from "@mako-cloud/management-sdk";

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
    <section aria-labelledby="users-title">
      <button type="button" className="back-link" onClick={onBack}>
        ← Project
      </button>
      <div className="section-heading">
        <div>
          <p className="eyebrow">Environment {environmentId}</p>
          <h1 id="users-title">Application users</h1>
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      <div className="split-grid">
        <section className="panel" aria-labelledby="user-search-title">
          <h2 id="user-search-title">Search users</h2>
          <search>
            <form className="inline-form" onSubmit={submitSearch}>
              <label>
                Email or user ID
                <input
                  type="search"
                  value={query}
                  maxLength={320}
                  onChange={(event) => setQuery(event.currentTarget.value)}
                />
              </label>
              <button type="submit">Search</button>
            </form>
          </search>
          {users === null ? (
            <p>Loading application users…</p>
          ) : users.length === 0 ? (
            <p>No users match this search.</p>
          ) : (
            <div className="resource-list">
              {users.map((user) => (
                <button
                  type="button"
                  className="resource-row"
                  key={user.id}
                  onClick={() => onOpen(user.id)}
                >
                  <span>
                    <strong>{user.email ?? user.id}</strong>
                    <small>{user.id}</small>
                  </span>
                  <LifecycleBadge state={user.status} />
                </button>
              ))}
            </div>
          )}
          {truncated ? (
            <p className="notice warning">Only the first 100 matches are shown.</p>
          ) : null}
        </section>
        <section className="panel" aria-labelledby="create-user-title">
          <h2 id="create-user-title">Create or invite</h2>
          <p>Trusted metadata is administrator-controlled and may affect document access.</p>
          <form onSubmit={(event) => void create(event)}>
            <label>
              Action
              <select name="mode" defaultValue="invite">
                <option value="invite">Invite by email</option>
                <option value="create">Create active user</option>
              </select>
            </label>
            <label>
              Email
              <input name="email" type="email" required maxLength={320} />
            </label>
            <JsonField name="trustedMetadata" label="Trusted metadata" />
            <JsonField name="profileMetadata" label="Profile metadata" />
            <button type="submit">Create application user</button>
          </form>
        </section>
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
    <section aria-labelledby="user-title">
      <button type="button" className="back-link" onClick={onBack}>
        ← Application users
      </button>
      <div className="section-heading">
        <div>
          <p className="eyebrow">Application user</p>
          <h1 id="user-title">{user?.email ?? userId}</h1>
          <code>{userId}</code>
        </div>
        {user === null ? null : <LifecycleBadge state={user.status} />}
      </div>
      <ApiFailureNotice failure={failure} />
      {user === null ? (
        <p>Loading application user…</p>
      ) : (
        <div className="split-grid">
          <section className="panel" aria-labelledby="metadata-title">
            <h2 id="metadata-title">User metadata</h2>
            <p>
              Trusted metadata affects authorization and is separate from user-editable profile
              metadata.
            </p>
            <form key={user.updatedAt} onSubmit={(event) => void updateMetadata(event)}>
              <label>
                Trusted metadata
                <textarea
                  name="trustedMetadata"
                  rows={10}
                  required
                  defaultValue={JSON.stringify(user.trustedMetadata, null, 2)}
                  spellCheck={false}
                />
              </label>
              <label>
                Profile metadata
                <textarea
                  name="profileMetadata"
                  rows={10}
                  required
                  defaultValue={JSON.stringify(user.profileMetadata, null, 2)}
                  spellCheck={false}
                />
              </label>
              <button type="submit">Save metadata</button>
            </form>
          </section>
          <section className="panel" aria-labelledby="user-lifecycle-title">
            <h2 id="user-lifecycle-title">Access and lifecycle</h2>
            <dl className="metadata-list">
              <div>
                <dt>Session epoch</dt>
                <dd>{user.sessionEpoch}</dd>
              </div>
              <div>
                <dt>Created</dt>
                <dd>{new Date(user.createdAt).toLocaleString()}</dd>
              </div>
              <div>
                <dt>Updated</dt>
                <dd>{new Date(user.updatedAt).toLocaleString()}</dd>
              </div>
            </dl>
            <div className="button-row">
              <button
                type="button"
                disabled={user.status !== "active"}
                onClick={() => void lifecycle("disable")}
              >
                Disable user
              </button>
              <button
                type="button"
                className="secondary"
                disabled={user.status !== "disabled"}
                onClick={() => void lifecycle("restore")}
              >
                Restore user
              </button>
              <button
                type="button"
                className="secondary"
                disabled={!user.sessions.some((session) => session.status === "active")}
                onClick={() => void lifecycle("revoke-all")}
              >
                Revoke all sessions
              </button>
              <button
                type="button"
                className="danger"
                disabled={user.status === "deleted"}
                onClick={() => void lifecycle("delete")}
              >
                Delete user
              </button>
            </div>
          </section>
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
    <section className="panel full-span" aria-labelledby="sessions-title">
      <h2 id="sessions-title">Sessions</h2>
      {user.sessions.length === 0 ? (
        <p>No retained sessions.</p>
      ) : (
        <div className="table-scroll">
          <table>
            <thead>
              <tr>
                <th scope="col">Session</th>
                <th scope="col">Status</th>
                <th scope="col">Created</th>
                <th scope="col">Expires</th>
                <th scope="col">Action</th>
              </tr>
            </thead>
            <tbody>
              {user.sessions.map((session) => (
                <tr key={session.id}>
                  <td>
                    <code>{session.id}</code>
                  </td>
                  <td>
                    <LifecycleBadge state={session.status} />
                  </td>
                  <td>{new Date(session.createdAt).toLocaleString()}</td>
                  <td>{new Date(session.expiresAt).toLocaleString()}</td>
                  <td>
                    <button
                      type="button"
                      className="danger-link"
                      disabled={session.status !== "active"}
                      onClick={() => onRevoke(session.id)}
                    >
                      Revoke
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {user.sessionsTruncated ? (
        <p className="notice warning">Only the bounded retained session set is shown.</p>
      ) : null}
    </section>
  );
}

function JsonField({ name, label }: { readonly name: string; readonly label: string }) {
  return (
    <label>
      {label}
      <textarea name={name} rows={6} required defaultValue={EMPTY_OBJECT} spellCheck={false} />
    </label>
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
