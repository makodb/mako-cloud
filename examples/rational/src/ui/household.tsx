import {
  Alert,
  AlertDescription,
  Badge,
  Button,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  Field,
  Input,
  NativeSelect,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@mako-cloud/ui";
import { CircleAlert, House, UserPlus } from "lucide-react";
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
 *
 * It lives under Settings as "Members"; the household's own name and currency
 * are the Household page's, next door.
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
    <section aria-labelledby="household-title" className="grid gap-6">
      <div className="grid gap-1">
        <h1 id="household-title" className="text-2xl">
          Members
        </h1>
        <p className="text-sm text-muted-foreground">
          Who shares this space, and what each of them may do in it.
        </p>
      </div>
      {app.householdsAvailable ? null : (
        <p
          className="text-sm text-muted-foreground"
          role="status"
          data-testid="households-unavailable"
        >
          This environment has no <code>households</code> function deployed, so membership can be
          seen but not changed here.
        </p>
      )}
      {error === null ? null : (
        <Alert variant="destructive" data-testid="household-error">
          <CircleAlert />
          <AlertDescription className="block">{error}</AlertDescription>
        </Alert>
      )}

      {state.invitations.length === 0 ? null : (
        <Card aria-labelledby="invitations-title">
          <CardHeader>
            <CardTitle id="invitations-title">Invitations</CardTitle>
          </CardHeader>
          <CardContent>
            <ul className="m-0 grid list-none gap-2 p-0 text-sm" aria-label="Invitations">
              {state.invitations.map((invitation) => (
                <li
                  key={invitation.id}
                  data-testid={`invitation-${invitation.household_id}`}
                  className="flex flex-wrap items-center justify-between gap-3 rounded-md bg-accent/60 px-3 py-2"
                >
                  <span>
                    {state.households.find((candidate) => candidate.id === invitation.household_id)
                      ?.name ?? invitation.household_id}{" "}
                    · {invitation.role}
                  </span>
                  <Button
                    size="sm"
                    disabled={busy || !app.householdsAvailable}
                    onClick={() => void run(() => app.acceptInvitation(invitation.household_id))}
                  >
                    Accept
                  </Button>
                </li>
              ))}
            </ul>
          </CardContent>
        </Card>
      )}

      {householdId === null ? (
        <p className="text-sm text-muted-foreground" role="status" data-testid="household-empty">
          You are not a member of any household yet. Create one to start.
        </p>
      ) : (
        <Card aria-labelledby="members-title">
          <CardHeader>
            <CardTitle id="members-title">
              Members of {household?.name ?? householdId}
              {role === null ? null : (
                <small className="font-normal text-muted-foreground"> · you are {role}</small>
              )}
            </CardTitle>
          </CardHeader>
          <CardContent className="grid gap-5">
            <Table aria-label="Members">
              <TableHeader>
                <TableRow>
                  <TableHead scope="col">Member</TableHead>
                  <TableHead scope="col">Role</TableHead>
                  <TableHead scope="col">Status</TableHead>
                  <TableHead scope="col">
                    <span className="sr-only">Actions</span>
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {members.map((member) => (
                  <TableRow
                    key={member.id}
                    data-testid={`member-${member.user_id === "" ? (member.email ?? "") : member.user_id}`}
                  >
                    <TableCell className="font-medium">{member.email ?? member.user_id}</TableCell>
                    <TableCell>
                      {role === "owner" && member.user_id !== "" && member.status === "active" ? (
                        <div className="max-w-36">
                          <NativeSelect
                            size="sm"
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
                          </NativeSelect>
                        </div>
                      ) : (
                        member.role
                      )}
                    </TableCell>
                    <TableCell>
                      <Badge variant={member.status === "active" ? "positive" : "warning"}>
                        {member.status}
                      </Badge>
                    </TableCell>
                    <TableCell className="text-right">
                      {role === "owner" &&
                      member.user_id !== "" &&
                      member.user_id !== household?.owner_id ? (
                        <Button
                          variant="link"
                          size="sm"
                          className="h-auto p-0 text-destructive"
                          disabled={busy}
                          onClick={() =>
                            void run(() => app.removeMember(householdId, member.user_id))
                          }
                        >
                          Remove
                        </Button>
                      ) : null}
                    </TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
            {role === "owner" ? (
              <InviteForm
                busy={busy || !app.householdsAvailable}
                onInvite={(email, invitedRole) =>
                  run(() => app.inviteMember(householdId, email, invitedRole))
                }
              />
            ) : (
              <p className="text-sm text-muted-foreground">
                Only the household's owner can invite or remove members.
              </p>
            )}
          </CardContent>
        </Card>
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
    <form
      className="flex flex-wrap items-end gap-3 border-t pt-5"
      onSubmit={(event) => void submit(event)}
      aria-label="Invite a member"
    >
      <Field label="Email" htmlFor="invite-email" className="min-w-64 flex-1">
        <Input
          id="invite-email"
          name="invite-email"
          type="email"
          required
          value={email}
          onChange={(event) => setEmail(event.target.value)}
        />
      </Field>
      <Field label="Role" htmlFor="invite-role" className="w-36">
        <NativeSelect
          id="invite-role"
          name="invite-role"
          value={role}
          onChange={(event) => setRole(event.target.value as HouseholdRole)}
        >
          {HOUSEHOLD_ROLES.map((candidate) => (
            <option key={candidate} value={candidate}>
              {candidate}
            </option>
          ))}
        </NativeSelect>
      </Field>
      <Button type="submit" disabled={busy}>
        <UserPlus />
        Send invitation
      </Button>
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
    <Card aria-labelledby="new-household-title">
      <CardHeader>
        <CardTitle id="new-household-title">New household</CardTitle>
      </CardHeader>
      <CardContent>
        <form
          className="flex flex-wrap items-end gap-3"
          onSubmit={(event) => void submit(event)}
          aria-label="New household"
        >
          <Field label="Name" htmlFor="new-household-name" className="min-w-64 flex-1">
            <Input
              id="new-household-name"
              name="household-name"
              required
              value={name}
              onChange={(event) => setName(event.target.value)}
            />
          </Field>
          <Field label="Currency" htmlFor="new-household-currency" className="w-28">
            <Input
              id="new-household-currency"
              name="household-currency"
              required
              maxLength={3}
              className="uppercase"
              value={currency}
              onChange={(event) => setCurrency(event.target.value.toUpperCase())}
            />
          </Field>
          <Button type="submit" variant="secondary" disabled={busy}>
            <House />
            Create household
          </Button>
        </form>
      </CardContent>
    </Card>
  );
}
