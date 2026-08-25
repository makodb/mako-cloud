import { type FormEvent, useCallback, useEffect, useState } from "react";

import type {
  InvitationIssue,
  Organization,
  OrganizationMembership,
  OrganizationRole,
} from "@mako-cloud/management-sdk";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useDeveloperAuth } from "./auth.js";
import { useManagementClient } from "./management.js";
import { BillingPanel } from "./billing.js";
import { ProjectsPanel } from "./projects.js";
import { confirmDestructiveAction, OneTimeSecretValue } from "./safety.js";

const ROLES: readonly OrganizationRole[] = ["owner", "administrator", "developer", "viewer"];

export function OrganizationsScreen({
  onOpen,
}: {
  readonly onOpen: (organizationId: string) => void;
}) {
  const client = useManagementClient();
  const [organizations, setOrganizations] = useState<Organization[] | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  useEffect(() => {
    let live = true;
    client.listOrganizations().then(
      (items) => live && setOrganizations(items),
      (error: unknown) => live && setFailure(toConsoleApiFailure(error)),
    );
    return () => {
      live = false;
    };
  }, [client]);

  return (
    <section aria-labelledby="organizations-title">
      <div className="section-heading">
        <div>
          <p className="eyebrow">Workspace</p>
          <h1 id="organizations-title">Organizations</h1>
        </div>
      </div>
      <ApiFailureNotice failure={failure} />
      {organizations === null ? (
        <p aria-live="polite">Loading organizations…</p>
      ) : organizations.length === 0 ? (
        <div className="panel empty-state">No organizations are available for this account.</div>
      ) : (
        <div className="card-grid">
          {organizations.map((organization) => (
            <button
              type="button"
              className="resource-card"
              key={organization.id}
              onClick={() => onOpen(organization.id)}
            >
              <strong>{organization.name}</strong>
              <span>{organization.state.replaceAll("_", " ")}</span>
            </button>
          ))}
        </div>
      )}
    </section>
  );
}

export function OrganizationScreen({
  organizationId,
  onOpen,
  onOpenProject,
}: {
  readonly organizationId: string;
  readonly onOpen: (organizationId: string) => void;
  readonly onOpenProject: (projectId: string) => void;
}) {
  const client = useManagementClient();
  const { state } = useDeveloperAuth();
  const [organization, setOrganization] = useState<Organization | null>(null);
  const [organizations, setOrganizations] = useState<Organization[]>([]);
  const [members, setMembers] = useState<OrganizationMembership[] | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const reload = useCallback(async () => {
    setFailure(null);
    try {
      const [selected, available, membershipItems] = await Promise.all([
        client.getOrganization(organizationId),
        client.listOrganizations(),
        client.listMembers(organizationId),
      ]);
      setOrganization(selected);
      setOrganizations(available);
      setMembers(membershipItems);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  }, [client, organizationId]);
  useEffect(() => {
    void reload();
  }, [reload]);

  const developerId = state.status === "authenticated" ? state.session.profile.id : "";
  const currentRole = members?.find((member) => member.developerIdentityId === developerId)?.role;
  const canManage = currentRole === "owner" || currentRole === "administrator";

  return (
    <section aria-labelledby="organization-title">
      <div className="section-heading">
        <div>
          <p className="eyebrow">Organization</p>
          <h1 id="organization-title">{organization?.name ?? "Loading…"}</h1>
        </div>
        <label>
          Switch organization
          <select value={organizationId} onChange={(event) => onOpen(event.currentTarget.value)}>
            {organizations.map((item) => (
              <option key={item.id} value={item.id}>
                {item.name}
              </option>
            ))}
          </select>
        </label>
      </div>
      <ApiFailureNotice failure={failure} />
      <div className="split-grid">
        <MembersPanel
          organizationId={organizationId}
          members={members}
          currentRole={currentRole}
          canManage={canManage}
          onChanged={reload}
        />
        <InvitationPanel organizationId={organizationId} canManage={canManage} onChanged={reload} />
        <ProjectsPanel organizationId={organizationId} onOpen={onOpenProject} />
        <BillingPanel organizationId={organizationId} />
      </div>
    </section>
  );
}

export function InvitationAcceptScreen({
  invitationId,
  onAccepted,
}: {
  readonly invitationId: string;
  readonly onAccepted: (organizationId: string) => void;
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
      onAccepted(membership.organizationId);
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
      setPending(false);
    }
  };
  return (
    <section className="panel" aria-labelledby="accept-invitation-title">
      <p className="eyebrow">Organization invitation</p>
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
  organizationId,
  members,
  currentRole,
  canManage,
  onChanged,
}: {
  readonly organizationId: string;
  readonly members: OrganizationMembership[] | null;
  readonly currentRole: OrganizationRole | undefined;
  readonly canManage: boolean;
  readonly onChanged: () => Promise<void>;
}) {
  const client = useManagementClient();
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  const updateRole = async (developerIdentityId: string, role: OrganizationRole) => {
    const previousRole = members?.find(
      (member) => member.developerIdentityId === developerIdentityId,
    )?.role;
    if (
      previousRole !== role &&
      !confirmDestructiveAction({
        action: "Change",
        target: `${developerIdentityId}'s role from ${previousRole ?? "unknown"} to ${role}`,
        consequence: "Their organization and project permissions will change immediately.",
      })
    ) {
      return;
    }
    try {
      await client.updateMember(organizationId, developerIdentityId, role);
      await onChanged();
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  };
  const remove = async (developerIdentityId: string) => {
    if (
      !confirmDestructiveAction({
        action: "Remove",
        target: `organization member ${developerIdentityId}`,
        consequence: "The member will lose organization and project access.",
      })
    ) {
      return;
    }
    try {
      await client.removeMember(organizationId, developerIdentityId);
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
                          event.currentTarget.value as OrganizationRole,
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
  organizationId,
  canManage,
  onChanged,
}: {
  readonly organizationId: string;
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
    const role = String(data.get("role") ?? "viewer") as OrganizationRole;
    try {
      const invitation = await client.createInvitation(organizationId, {
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
