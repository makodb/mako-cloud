import type { ExplorerGrant, MakoManagementClient } from "@mako-cloud/management-sdk";

/** One collection's in-memory access. The service still checks the developer's role. */
export function createExplorerAccess(
  client: Pick<MakoManagementClient, "issueExplorerGrant" | "revokeExplorerGrant">,
  projectId: string,
  environmentId: string,
  collectionId: string,
) {
  let closed = false;
  let current: ExplorerGrant | null = null;
  let pending: Promise<ExplorerGrant> | null = null;

  const revoke = (grant: ExplorerGrant) => {
    void client.revokeExplorerGrant(projectId, environmentId, grant.grantId).catch(() => {});
  };
  const invalidate = () => {
    if (current !== null) revoke(current);
    current = null;
  };

  return {
    async getGrant(): Promise<ExplorerGrant> {
      if (closed) throw new Error("The selected collection changed.");
      if (current !== null && current.expiresAtUnixSeconds > Date.now() / 1000 + 5) {
        return current;
      }
      if (pending !== null) return pending;
      invalidate();
      pending = client
        .issueExplorerGrant(projectId, environmentId, {
          tenant: { projectId, environmentId },
          collectionId,
          mode: "administrative",
          operations: ["get", "browse", "query", "plan", "history", "simulate", "mutate"],
          applicationUserId: null,
          reason: "Browse and manage documents in the cloud console",
          durationSeconds: 300,
        })
        .then((grant) => {
          if (closed) {
            revoke(grant);
            throw new Error("The selected collection changed.");
          }
          if (grant.expiresAtUnixSeconds <= Date.now() / 1000 + 5) {
            revoke(grant);
            throw new Error("Document access could not be renewed. Try again.");
          }
          current = grant;
          return grant;
        })
        .finally(() => {
          pending = null;
        });
      return pending;
    },
    invalidate,
    close() {
      closed = true;
      invalidate();
    },
  };
}

export type ExplorerAccess = ReturnType<typeof createExplorerAccess>;
