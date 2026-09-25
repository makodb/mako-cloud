import {
  type InvitationIssue,
  ManagementApiError,
  type Team,
  type TeamMembership,
  type TeamRole,
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
  Eyebrow,
  Field,
  Input,
  Label,
  NativeSelect,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@mako-cloud/ui";
import { type FormEvent, useCallback, useEffect, useId, useState } from "react";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useDeveloperAuth } from "./auth.js";
import { useManagementClient } from "./management.js";
import { FirstProjectPanel, ProjectsPanel } from "./projects.js";
import { confirmDestructiveAction, OneTimeSecretValue } from "./safety.js";

const ROLES: readonly TeamRole[] = ["owner", "administrator", "developer", "viewer"];

export function TeamsScreen({
  onOpen,
  onOpenProject,
}: {
  readonly onOpen: (teamId: string) => void;
  readonly onOpenProject: (projectId: string) => void;
}) {
  const client = useManagementClient();
  const [teams, setTeams] = useState<Team[] | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const reload = useCallback(async () => {
    try {
      setTeams(await client.listTeams());
      setFailure(null);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  }, [client]);
  useEffect(() => {
    void reload();
  }, [reload]);

  // The personal space is an implicit one-member team that holds the caller's
  // individual projects; it leads the screen and is never listed as a team.
  const personalSpace = teams?.find((team) => team.kind === "personal");
  const joinedTeams = teams?.filter((team) => team.kind === "team") ?? [];

  return (
    <div className="grid gap-8">
      <section className="grid gap-4" aria-labelledby="personal-space-title">
        <div className="grid gap-1">
          <Eyebrow>Projects</Eyebrow>
          <h1 id="personal-space-title" className="text-2xl">
            Personal projects
          </h1>
        </div>
        <ApiFailureNotice failure={failure} />
        {teams === null ? (
          <p className="m-0 text-sm text-muted-foreground" aria-live="polite">
            Loading your projects…
          </p>
        ) : personalSpace === undefined ? (
          <FirstProjectPanel onCreated={reload} />
        ) : (
          <ProjectsPanel teamId={personalSpace.id} scope="personal" onOpen={onOpenProject} />
        )}
      </section>
      <section className="grid gap-4" aria-labelledby="teams-title">
        <div className="grid gap-1">
          <Eyebrow>Workspace</Eyebrow>
          <h2 id="teams-title" className="text-lg">
            Teams
          </h2>
        </div>
        {teams === null ? (
          <p className="m-0 text-sm text-muted-foreground" aria-live="polite">
            Loading teams…
          </p>
        ) : joinedTeams.length === 0 ? (
          <div className="rounded-xl border border-dashed px-6 py-8 text-center text-sm text-muted-foreground">
            No teams are available for this account.
          </div>
        ) : (
          <div className="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
            {joinedTeams.map((team) => (
              <TeamCard key={team.id} team={team} onOpen={() => onOpen(team.id)} />
            ))}
          </div>
        )}
      </section>
    </div>
  );
}

/** A team in a grid: its name, its state, and a click that opens it. */
function TeamCard({ team, onOpen }: { readonly team: Team; readonly onOpen: () => void }) {
  return (
    <Button
      variant="outline"
      // `resource-card` is the hook the browser suite selects on.
      className="resource-card h-auto flex-col items-start gap-1 px-4 py-3 text-left whitespace-normal"
      onClick={onOpen}
    >
      <strong className="text-sm font-semibold">{team.name}</strong>
      <span className="text-xs font-normal text-muted-foreground">
        {team.state.replaceAll("_", " ")}
      </span>
    </Button>
  );
}

export function TeamScreen({
  teamId,
  onOpen,
  onOpenProject,
}: {
  readonly teamId: string;
  readonly onOpen: (teamId: string) => void;
  readonly onOpenProject: (projectId: string) => void;
}) {
  const client = useManagementClient();
  const { state } = useDeveloperAuth();
  const [team, setTeam] = useState<Team | null>(null);
  const [teams, setTeams] = useState<Team[]>([]);
  const [members, setMembers] = useState<TeamMembership[] | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  // Refused or unknown: a member who was removed, or a mistyped link.
  const [unavailable, setUnavailable] = useState(false);
  const reload = useCallback(async () => {
    setFailure(null);
    try {
      const [selected, available] = await Promise.all([client.getTeam(teamId), client.listTeams()]);
      setUnavailable(false);
      // A personal space has exactly one member and refuses membership changes,
      // so there is nothing to manage and nothing to fetch.
      const membershipItems = selected.kind === "personal" ? [] : await client.listMembers(teamId);
      setTeam(selected);
      setTeams(available);
      setMembers(membershipItems);
    } catch (error) {
      if (error instanceof ManagementApiError && (error.status === 403 || error.status === 404)) {
        setUnavailable(true);
        return;
      }
      setFailure(toConsoleApiFailure(error));
    }
  }, [client, teamId]);
  useEffect(() => {
    void reload();
  }, [reload]);

  const developerId = state.status === "authenticated" ? state.session.profile.id : "";
  const currentRole = members?.find((member) => member.developerIdentityId === developerId)?.role;
  const canManage = currentRole === "owner" || currentRole === "administrator";
  const personal = team?.kind === "personal";
  const switcherId = useId();

  if (unavailable && team === null) {
    // Said once, plainly: the page used to sit on "Loading…" under two raw
    // "forbidden" errors, one for the team and one for its projects.
    return (
      <section aria-labelledby="team-title" className="grid max-w-xl gap-4">
        <Eyebrow>Team</Eyebrow>
        <h1 id="team-title" className="text-2xl">
          Team unavailable
        </h1>
        <Alert role="alert">
          <AlertDescription className="block">
            You are not a member of this team. If you were removed, ask one of its owners to invite
            you again.
          </AlertDescription>
        </Alert>
        <a href="/" className="text-sm text-primary underline underline-offset-4">
          Back to Home
        </a>
      </section>
    );
  }
  return (
    <section aria-labelledby="team-title" className="grid gap-6">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div className="grid gap-1">
          <Eyebrow>{personal ? "Your account" : "Team"}</Eyebrow>
          <h1 id="team-title" className="text-2xl">
            {personal ? "Personal projects" : (team?.name ?? "Loading…")}
          </h1>
        </div>
        {team?.kind === "team" ? (
          <div className="grid gap-1.5">
            <Label htmlFor={switcherId} className="text-xs text-muted-foreground">
              Switch team
            </Label>
            <NativeSelect
              id={switcherId}
              size="sm"
              wrapperClassName="w-auto min-w-56"
              value={teamId}
              onChange={(event) => onOpen(event.currentTarget.value)}
            >
              {teams
                .filter((item) => item.kind === "team")
                .map((item) => (
                  <option key={item.id} value={item.id}>
                    {item.name}
                  </option>
                ))}
            </NativeSelect>
          </div>
        ) : null}
      </div>
      <ApiFailureNotice failure={failure} />
      <div className="grid gap-4 lg:grid-cols-2">
        {team === null || personal ? null : (
          <>
            <MembersPanel
              teamId={teamId}
              members={members}
              currentRole={currentRole}
              canManage={canManage}
              onChanged={reload}
            />
            <InvitationPanel teamId={teamId} canManage={canManage} onChanged={reload} />
            {canManage ? (
              <RenameTeamPanel
                team={team}
                onRenamed={(renamed) => {
                  setTeam(renamed);
                  setTeams((current) =>
                    current.map((item) => (item.id === renamed.id ? renamed : item)),
                  );
                }}
              />
            ) : null}
          </>
        )}
        <ProjectsPanel
          teamId={teamId}
          scope={personal ? "personal" : "team"}
          onOpen={onOpenProject}
        />
        <a href="/usage-and-plan" className="text-sm text-primary underline underline-offset-4">
          Usage and plan
        </a>
      </div>
    </section>
  );
}

export function InvitationAcceptScreen({
  invitationId,
  onAccepted,
}: {
  readonly invitationId: string;
  readonly onAccepted: (teamId: string) => void;
}) {
  const client = useManagementClient();
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [pending, setPending] = useState(false);
  // An invitation link carries its token in the fragment, which never reaches
  // a server; it is read once and dropped from the address bar and history.
  const [linkToken] = useState(() => {
    const token = new URLSearchParams(window.location.hash.slice(1)).get("token") ?? "";
    if (token !== "") {
      window.history.replaceState(null, "", `${window.location.pathname}${window.location.search}`);
    }
    return token;
  });
  const tokenId = useId();
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    setPending(true);
    setFailure(null);
    const token = String(new FormData(event.currentTarget).get("token") ?? "").trim();
    try {
      const membership = await client.acceptInvitation(invitationId, token);
      onAccepted(membership.teamId);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
      setPending(false);
    }
  };
  return (
    <Card className="w-full max-w-lg" aria-labelledby="accept-invitation-title">
      <CardHeader>
        <Eyebrow>Team invitation</Eyebrow>
        <CardTitle as="h1" id="accept-invitation-title" className="text-2xl">
          Accept invitation
        </CardTitle>
        <CardDescription>
          Invitation <code className="font-mono text-xs text-foreground">{invitationId}</code>
        </CardDescription>
      </CardHeader>
      <CardContent className="grid gap-4">
        <ApiFailureNotice failure={failure} />
        <form className="grid gap-4" onSubmit={(event) => void submit(event)}>
          <Field label="Invitation token" htmlFor={tokenId}>
            <Input
              id={tokenId}
              name="token"
              type="password"
              required
              minLength={16}
              autoComplete="off"
              defaultValue={linkToken}
            />
          </Field>
          <Button type="submit" disabled={pending} className="justify-self-start">
            {pending ? "Accepting…" : "Accept invitation"}
          </Button>
        </form>
      </CardContent>
    </Card>
  );
}

function MembersPanel({
  teamId,
  members,
  currentRole,
  canManage,
  onChanged,
}: {
  readonly teamId: string;
  readonly members: TeamMembership[] | null;
  readonly currentRole: TeamRole | undefined;
  readonly canManage: boolean;
  readonly onChanged: () => Promise<void>;
}) {
  const client = useManagementClient();
  const { state } = useDeveloperAuth();
  // Members are listed by identity id alone, so the signed-in developer's own
  // row is marked: without it a lead could not tell they were changing their
  // own role.
  const selfId = state.status === "authenticated" ? state.session.profile.id : null;
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  // Managers change members; only an owner may change or remove an owner.
  const editable = (member: TeamMembership) =>
    canManage && (currentRole === "owner" || member.role !== "owner");
  const updateRole = async (developerIdentityId: string, role: TeamRole) => {
    const previousRole = members?.find(
      (member) => member.developerIdentityId === developerIdentityId,
    )?.role;
    if (
      previousRole !== role &&
      !confirmDestructiveAction({
        action: "Change",
        target: `${developerIdentityId}'s role from ${previousRole ?? "unknown"} to ${role}`,
        consequence: "Their team and project permissions will change immediately.",
      })
    ) {
      return;
    }
    try {
      await client.updateMember(teamId, developerIdentityId, role);
      await onChanged();
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  };
  const remove = async (developerIdentityId: string) => {
    if (
      !confirmDestructiveAction({
        action: "Remove",
        target: `team member ${developerIdentityId}`,
        consequence: "The member will lose team and project access.",
      })
    ) {
      return;
    }
    try {
      await client.removeMember(teamId, developerIdentityId);
      await onChanged();
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  };
  return (
    <Card aria-labelledby="members-title">
      <CardHeader>
        <CardTitle id="members-title">Members</CardTitle>
        <CardDescription>
          Your role: {currentRole?.replaceAll("_", " ") ?? "unknown"}
        </CardDescription>
      </CardHeader>
      <CardContent className="grid gap-4">
        <ApiFailureNotice failure={failure} />
        {members === null ? (
          <p className="m-0 text-sm text-muted-foreground">Loading members…</p>
        ) : (
          <Table>
            <TableHeader>
              <TableRow className="hover:bg-transparent">
                <TableHead scope="col">Developer identity</TableHead>
                <TableHead scope="col">Role</TableHead>
                <TableHead scope="col">Action</TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {members.map((member) => (
                <TableRow key={member.developerIdentityId}>
                  <TableCell>
                    <code className="font-mono text-xs">{member.developerIdentityId}</code>
                    {member.developerIdentityId === selfId ? (
                      <span className="ml-2 text-xs text-muted-foreground">(you)</span>
                    ) : null}
                  </TableCell>
                  <TableCell>
                    <NativeSelect
                      aria-label={`Role for ${member.developerIdentityId}${member.developerIdentityId === selfId ? " (you)" : ""}`}
                      size="sm"
                      wrapperClassName="w-auto min-w-36"
                      value={member.role}
                      disabled={!editable(member)}
                      onChange={(event) =>
                        void updateRole(
                          member.developerIdentityId,
                          event.currentTarget.value as TeamRole,
                        )
                      }
                    >
                      {/* The member's own role is always listed: an owner seen by a
                          non-owner otherwise showed as the first option offered. */}
                      {ROLES.filter(
                        (role) =>
                          currentRole === "owner" || role !== "owner" || role === member.role,
                      ).map((role) => (
                        <option key={role} value={role}>
                          {role}
                        </option>
                      ))}
                    </NativeSelect>
                  </TableCell>
                  <TableCell>
                    <Button
                      variant="ghost"
                      size="sm"
                      className="text-destructive hover:text-destructive"
                      disabled={!editable(member)}
                      aria-label={`Remove ${member.developerIdentityId}`}
                      onClick={() => void remove(member.developerIdentityId)}
                    >
                      Remove
                    </Button>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        )}
      </CardContent>
    </Card>
  );
}

function InvitationPanel({
  teamId,
  canManage,
  onChanged,
}: {
  readonly teamId: string;
  readonly canManage: boolean;
  readonly onChanged: () => Promise<void>;
}) {
  const client = useManagementClient();
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [issue, setIssue] = useState<InvitationIssue | null>(null);
  const id = useId();
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const form = event.currentTarget;
    setFailure(null);
    const data = new FormData(form);
    const email = String(data.get("email") ?? "").trim();
    const role = String(data.get("role") ?? "viewer") as TeamRole;
    try {
      const invitation = await client.createInvitation(teamId, {
        email,
        role,
        expiresAt: new Date(Date.now() + 7 * 24 * 60 * 60 * 1_000).toISOString(),
      });
      setIssue(invitation);
      form.reset();
      await onChanged();
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  };
  return (
    <Card aria-labelledby="invite-title">
      <CardHeader>
        <CardTitle id="invite-title">Invite a member</CardTitle>
      </CardHeader>
      <CardContent className="grid gap-4">
        <ApiFailureNotice failure={failure} />
        <form className="grid max-w-md gap-4" onSubmit={(event) => void submit(event)}>
          <Field label="Email" htmlFor={`${id}-email`}>
            <Input
              id={`${id}-email`}
              name="email"
              type="email"
              required
              autoComplete="email"
              disabled={!canManage}
            />
          </Field>
          <Field label="Role" htmlFor={`${id}-role`}>
            <NativeSelect id={`${id}-role`} name="role" defaultValue="viewer" disabled={!canManage}>
              {ROLES.filter((role) => role !== "owner").map((role) => (
                <option key={role} value={role}>
                  {role}
                </option>
              ))}
            </NativeSelect>
          </Field>
          <Button type="submit" disabled={!canManage} className="justify-self-start">
            Create invitation
          </Button>
        </form>
        {issue === null ? null : (
          // The invitee needs the invitation's own page as well as its token;
          // a bare token gave them nowhere to use it. The link carries both.
          <OneTimeSecretValue
            label="invitation link"
            value={`${window.location.origin}/invitations/${issue.invitation.id}#token=${encodeURIComponent(issue.token)}`}
            onDismiss={() => setIssue(null)}
          />
        )}
      </CardContent>
    </Card>
  );
}

// Renaming is offered to owners and administrators of a joined team; the
// personal space has no name of its own to change.
function RenameTeamPanel({
  team,
  onRenamed,
}: {
  readonly team: Team;
  readonly onRenamed: (team: Team) => void;
}) {
  const client = useManagementClient();
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const nameId = useId();
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const name = String(new FormData(event.currentTarget).get("name") ?? "").trim();
    setStatus(null);
    if (name === "") {
      setFailure({ message: "Enter a team name.", requestId: null });
      return;
    }
    if (name === team.name) {
      setFailure({ message: "That is already the team's name.", requestId: null });
      return;
    }
    setPending(true);
    setFailure(null);
    try {
      const renamed = await client.updateTeam(team.id, name);
      onRenamed(renamed);
      setStatus(`Renamed to ${renamed.name}. The change is audited.`);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    } finally {
      setPending(false);
    }
  };
  return (
    <Card aria-labelledby="rename-team-title">
      <CardHeader>
        <CardTitle id="rename-team-title">Team name</CardTitle>
        <CardDescription>
          The name appears everywhere the team is listed. Renaming is audited.
        </CardDescription>
      </CardHeader>
      <CardContent className="grid gap-4">
        <ApiFailureNotice failure={failure} />
        <form className="flex flex-wrap items-end gap-3" onSubmit={(event) => void submit(event)}>
          <Field label="New name" htmlFor={nameId} className="min-w-56 flex-1">
            <Input id={nameId} name="name" defaultValue={team.name} required maxLength={200} />
          </Field>
          <Button type="submit" disabled={pending}>
            {pending ? "Renaming…" : "Rename team"}
          </Button>
        </form>
        {status === null ? null : (
          <Alert variant="positive" role="status">
            <AlertDescription className="block">{status}</AlertDescription>
          </Alert>
        )}
      </CardContent>
    </Card>
  );
}

/**
 * Starts a team from the console: a lead setting up a project for several
 * people needs one before anyone can be invited, and the API's createTeam had
 * no place in the console.
 */
export function CreateTeamForm({ onCreated }: { readonly onCreated: (team: Team) => void }) {
  const client = useManagementClient();
  const [open, setOpen] = useState(false);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const [pending, setPending] = useState(false);
  const nameId = useId();
  if (!open) {
    return (
      <Button
        variant="ghost"
        size="sm"
        className="justify-self-start"
        onClick={() => setOpen(true)}
      >
        New team
      </Button>
    );
  }
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const name = String(new FormData(event.currentTarget).get("name") ?? "").trim();
    if (name === "") {
      setFailure({ message: "Enter a team name.", requestId: null });
      return;
    }
    setPending(true);
    setFailure(null);
    try {
      onCreated(await client.createTeam(name));
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
      setPending(false);
    }
  };
  return (
    <form
      className="grid gap-2 px-3"
      aria-label="New team"
      onSubmit={(event) => void submit(event)}
    >
      <ApiFailureNotice failure={failure} />
      <Field label="Team name" htmlFor={nameId}>
        <Input id={nameId} name="name" required maxLength={200} autoFocus />
      </Field>
      <div className="flex gap-2">
        <Button type="submit" size="sm" disabled={pending}>
          {pending ? "Creating…" : "Create team"}
        </Button>
        <Button type="button" size="sm" variant="ghost" onClick={() => setOpen(false)}>
          Cancel
        </Button>
      </div>
    </form>
  );
}
