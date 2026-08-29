import { type FormEvent, useState } from "react";

import type { AppState, RationalApp } from "../data/rational.js";
import type { ScopeSession } from "../data/scope.js";
import {
  type DirectoryCollectionId,
  HOUSEHOLD_ROLES,
  type HouseholdRole,
  type Membership,
} from "../model/types.js";
import { useQuery } from "./hooks.js";

/**
 * Households, members, and invitations. Everything on this screen is a call
 * to the `households` edge function: membership lives in the claims a token
 * carries, so only trusted code may change it, and the app's own writes to
 * the `memberships` collection would be refused by its policy.
 */
export function HouseholdScreen({ app, state }: { app: RationalApp; state: AppState }) {
  const session = app.directory?.session ?? null;
  const householdId = state.currentHouseholdId;
  const role = app.roleIn(householdId);
  const household = state.households.find((candidate) => candidate.id === householdId);
  const members = useMembers(session, householdId);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const run = async (action: () => Promise<unknown>) => {
    setError(null);
    setBusy(true);
    try {
      await action();
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "That change was refused.");
    } finally {
      setBusy(false);
    }
  };

  return (
    <section aria-labelledby="household-title">
      <div className="heading">
        <h1 id="household-title">Household</h1>
      </div>
      {app.householdsAvailable ? null : (
        <p className="hint" role="status" data-testid="households-unavailable">
          This environment has no <code>households</code> function deployed, so membership can be
          seen but not changed here.
        </p>
      )}
      {error === null ? null : (
        <p className="error" role="alert" data-testid="household-error">
          {error}
        </p>
      )}

      {state.invitations.length === 0 ? null : (
        <section aria-labelledby="invitations-title" className="panel">
          <h2 id="invitations-title">Invitations</h2>
          <ul className="list" aria-label="Invitations">
            {state.invitations.map((invitation) => (
              <li key={invitation.id} data-testid={`invitation-${invitation.household_id}`}>
                <span>
                  {state.households.find((candidate) => candidate.id === invitation.household_id)
                    ?.name ?? invitation.household_id}{" "}
                  · {invitation.role}
                </span>
                <button
                  type="button"
                  disabled={busy || !app.householdsAvailable}
                  onClick={() => void run(() => app.acceptInvitation(invitation.household_id))}
                >
                  Accept
                </button>
              </li>
            ))}
          </ul>
        </section>
      )}

      {householdId === null ? (
        <p role="status" data-testid="household-empty">
          You are not a member of any household yet. Create one to start.
        </p>
      ) : (
        <section aria-labelledby="members-title" className="panel">
          <h2 id="members-title">
            Members of {household?.name ?? householdId}
            {role === null ? null : <small> · you are {role}</small>}
          </h2>
          <table className="list" aria-label="Members">
            <thead>
              <tr>
                <th scope="col">Member</th>
                <th scope="col">Role</th>
                <th scope="col">Status</th>
                <th scope="col">
                  <span className="visually-hidden">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {members.map((member) => (
                <tr
                  key={member.id}
                  data-testid={`member-${member.user_id === "" ? (member.email ?? "") : member.user_id}`}
                >
                  <td>{member.email ?? member.user_id}</td>
                  <td>
                    {role === "owner" && member.user_id !== "" && member.status === "active" ? (
                      <select
                        aria-label={`Role of ${member.email ?? member.user_id}`}
                        value={member.role}
                        disabled={busy}
                        onChange={(event) =>
                          void run(() =>
                            app.changeMemberRole(
                              householdId,
                              member.user_id,
                              event.target.value as HouseholdRole,
                            ),
                          )
                        }
                      >
                        {HOUSEHOLD_ROLES.map((candidate) => (
                          <option key={candidate} value={candidate}>
                            {candidate}
                          </option>
                        ))}
                      </select>
                    ) : (
                      member.role
                    )}
                  </td>
                  <td>{member.status}</td>
                  <td className="actions">
                    {role === "owner" &&
                    member.user_id !== "" &&
                    member.user_id !== household?.owner_id ? (
                      <button
                        type="button"
                        className="link"
                        disabled={busy}
                        onClick={() =>
                          void run(() => app.removeMember(householdId, member.user_id))
                        }
                      >
                        Remove
                      </button>
                    ) : null}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {role === "owner" ? (
            <InviteForm
              busy={busy || !app.householdsAvailable}
              onInvite={(email, invitedRole) =>
                run(() => app.inviteMember(householdId, email, invitedRole))
              }
            />
          ) : (
            <p className="hint">Only the household's owner can invite or remove members.</p>
          )}
        </section>
      )}

      <CreateHouseholdForm
        busy={busy || !app.householdsAvailable}
        onCreate={(name, currency) => run(() => app.createHousehold(name, currency))}
      />
    </section>
  );
}

function useMembers(
  session: ScopeSession<DirectoryCollectionId> | null,
  householdId: string | null,
): readonly Membership[] {
  const members = useQuery(
    householdId === null
      ? null
      : (session?.collection("memberships")?.find({ selector: { household_id: householdId } }) ??
          null),
  );
  return members
    .filter((member) => member.status === "active" || member.status === "invited")
    .sort((left, right) =>
      (left.email ?? left.user_id).localeCompare(right.email ?? right.user_id),
    );
}

function InviteForm({
  busy,
  onInvite,
}: {
  busy: boolean;
  onInvite: (email: string, role: HouseholdRole) => Promise<void>;
}) {
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<HouseholdRole>("editor");
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    await onInvite(email, role);
    setEmail("");
  };
  return (
    <form className="inline" onSubmit={(event) => void submit(event)} aria-label="Invite a member">
      <label>
        Email
        <input
          name="invite-email"
          type="email"
          required
          value={email}
          onChange={(event) => setEmail(event.target.value)}
        />
      </label>
      <label>
        Role
        <select
          name="invite-role"
          value={role}
          onChange={(event) => setRole(event.target.value as HouseholdRole)}
        >
          {HOUSEHOLD_ROLES.map((candidate) => (
            <option key={candidate} value={candidate}>
              {candidate}
            </option>
          ))}
        </select>
      </label>
      <button type="submit" disabled={busy}>
        Send invitation
      </button>
    </form>
  );
}

function CreateHouseholdForm({
  busy,
  onCreate,
}: {
  busy: boolean;
  onCreate: (name: string, currency: string) => Promise<void>;
}) {
  const [name, setName] = useState("");
  const [currency, setCurrency] = useState("USD");
  const submit = async (event: FormEvent) => {
    event.preventDefault();
    await onCreate(name, currency);
    setName("");
  };
  return (
    <section aria-labelledby="new-household-title" className="panel">
      <h2 id="new-household-title">New household</h2>
      <form className="inline" onSubmit={(event) => void submit(event)} aria-label="New household">
        <label>
          Name
          <input
            name="household-name"
            required
            value={name}
            onChange={(event) => setName(event.target.value)}
          />
        </label>
        <label>
          Currency
          <input
            name="household-currency"
            required
            maxLength={3}
            value={currency}
            onChange={(event) => setCurrency(event.target.value.toUpperCase())}
          />
        </label>
        <button type="submit" disabled={busy}>
          Create household
        </button>
      </form>
    </section>
  );
}
