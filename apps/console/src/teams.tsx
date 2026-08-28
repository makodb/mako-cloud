import { type FormEvent, useCallback, useEffect, useState } from "react";

import type { InvitationIssue, Team, TeamMembership, TeamRole } from "@mako-cloud/management-sdk";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useDeveloperAuth } from "./auth.js";
import { useManagementClient } from "./management.js";
import { BillingPanel } from "./billing.js";
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
    <>
      <section aria-labelledby="personal-space-title">
        <div className="section-heading">
          <div>
            <p className="eyebrow">Personal space</p>
            <h1 id="personal-space-title">Your projects</h1>
          </div>
        </div>
        <ApiFailureNotice failure={failure} />
        {teams === null ? (
          <p aria-live="polite">Loading your projects…</p>
        ) : personalSpace === undefined ? (
          <FirstProjectPanel onCreated={reload} />
        ) : (
          <ProjectsPanel teamId={personalSpace.id} scope="personal" onOpen={onOpenProject} />
        )}
      </section>
      <section aria-labelledby="teams-title">
        <div className="section-heading">
          <div>
            <p className="eyebrow">Workspace</p>
            <h2 id="teams-title">Teams</h2>
          </div>
        </div>
        {teams === null ? (
          <p aria-live="polite">Loading teams…</p>
        ) : joinedTeams.length === 0 ? (
          <div className="panel empty-state">No teams are available for this account.</div>
        ) : (
          <div className="card-grid">
            {joinedTeams.map((team) => (
              <button
                type="button"
                className="resource-card"
                key={team.id}
                onClick={() => onOpen(team.id)}
              >
                <strong>{team.name}</strong>
                <span>{team.state.replaceAll("_", " ")}</span>
              </button>
            ))}
          </div>
        )}
      </section>
    </>
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
  const reload = useCallback(async () => {
    setFailure(null);
    try {
      const [selected, available] = await Promise.all([client.getTeam(teamId), client.listTeams()]);
      // A personal space has exactly one member and refuses membership changes,
      // so there is nothing to manage and nothing to fetch.
      const membershipItems = selected.kind === "personal" ? [] : await client.listMembers(teamId);
      setTeam(selected);
      setTeams(available);
      setMembers(membershipItems);
    } catch (error) {
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

  return (
    <section aria-labelledby="team-title">
      <div className="section-heading">
        <div>
          <p className="eyebrow">{personal ? "Personal space" : "Team"}</p>
          <h1 id="team-title">{team?.name ?? "Loading…"}</h1>
        </div>
        <label>
          Switch team
          <select value={teamId} onChange={(event) => onOpen(event.currentTarget.value)}>
            {teams.map((item) => (
              <option key={item.id} value={item.id}>
                {teamLabel(item)}
              </option>
            ))}
          </select>
        </label>
      </div>
      <ApiFailureNotice failure={failure} />
      <div className="split-grid">
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
          </>
        )}
        <ProjectsPanel
          teamId={teamId}
          scope={personal ? "personal" : "team"}
          onOpen={onOpenProject}
        />
        <BillingPanel teamId={teamId} />
      </div>
    </section>
  );
}

/** How a team reads wherever teams are listed for navigation. */
function teamLabel(team: Team): string {
  return team.kind === "personal" ? "Your projects" : team.name;
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
    <section className="panel" aria-labelledby="accept-invitation-title">
      <p className="eyebrow">Team invitation</p>
      <h1 id="accept-invitation-title">Accept invitation</h1>
      <p>
        Invitation <code>{invitationId}</code>
      </p>
      <ApiFailureNotice failure={failure} />
      <form onSubmit={(event) => void submit(event)}>
        <label>
          Invitation token
          <input name="token" type="password" required minLength={16} autoComplete="off" />
        </label>
        <button type="submit" disabled={pending}>
          {pending ? "Accepting…" : "Accept invitation"}
        </button>
      </form>
    </section>
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
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
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
    <section className="panel" aria-labelledby="members-title">
      <h2 id="members-title">Members</h2>
      <p>Your role: {currentRole?.replaceAll("_", " ") ?? "unknown"}</p>
      <ApiFailureNotice failure={failure} />
      {members === null ? (
        <p>Loading members…</p>
      ) : (
        <div className="table-scroll">
          <table>
            <thead>
              <tr>
                <th scope="col">Developer identity</th>
                <th scope="col">Role</th>
                <th scope="col">Action</th>
              </tr>
            </thead>
            <tbody>
              {members.map((member) => (
                <tr key={member.developerIdentityId}>
                  <td>
                    <code>{member.developerIdentityId}</code>
                  </td>
                  <td>
                    <select
                      aria-label={`Role for ${member.developerIdentityId}`}
                      value={member.role}
                      disabled={!canManage}
                      onChange={(event) =>
                        void updateRole(
                          member.developerIdentityId,
                          event.currentTarget.value as TeamRole,
                        )
                      }
                    >
                      {ROLES.filter((role) => currentRole === "owner" || role !== "owner").map(
                        (role) => (
                          <option key={role} value={role}>
                            {role}
                          </option>
                        ),
                      )}
                    </select>
                  </td>
                  <td>
                    <button
                      type="button"
                      className="danger-link"
                      disabled={!canManage}
                      onClick={() => void remove(member.developerIdentityId)}
                    >
                      Remove
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
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
    <section className="panel" aria-labelledby="invite-title">
      <h2 id="invite-title">Invite a member</h2>
      <ApiFailureNotice failure={failure} />
      <form onSubmit={(event) => void submit(event)}>
        <label>
          Email
          <input name="email" type="email" required autoComplete="email" disabled={!canManage} />
        </label>
        <label>
          Role
          <select name="role" defaultValue="viewer" disabled={!canManage}>
            {ROLES.filter((role) => role !== "owner").map((role) => (
              <option key={role} value={role}>
                {role}
              </option>
            ))}
          </select>
        </label>
        <button type="submit" disabled={!canManage}>
          Create invitation
        </button>
      </form>
      {issue === null ? null : (
        <OneTimeSecretValue
          label="invitation token"
          value={issue.token}
          onDismiss={() => setIssue(null)}
        />
      )}
    </section>
  );
}
