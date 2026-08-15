import { createContext, type ReactNode, useContext, useMemo } from "react";

import { createOperatorClient, type MakoOperatorClient } from "@mako-cloud/management-sdk";

import { isOperatorSessionActive, useOperatorAuth } from "./operator-auth.js";

const OperatorManagementContext = createContext<MakoOperatorClient | null>(null);

export function OperatorManagementProvider({
  endpoint,
  children,
}: {
  readonly endpoint: string;
  readonly children: ReactNode;
}) {
  const { state, clearSession, requestStepUp } = useOperatorAuth();
  const client = useMemo(() => {
    if (state.status !== "authenticated" || !isOperatorSessionActive(state.session)) {
      return null;
    }
    return createOperatorClient({
      endpoint,
      fetch: async (request) => {
        const original = request instanceof Request ? request : new Request(request);
        const retry = original.clone();
        let response = await globalThis.fetch(original);
        if (response.status !== 401) return response;
        let code: unknown;
        try {
          const value: unknown = await response.clone().json();
          code = (value as { error?: { code?: unknown } }).error?.code;
        } catch {
          code = undefined;
        }
        if (code === "operator_step_up_required") {
          await requestStepUp();
          response = await globalThis.fetch(retry);
          if (response.status === 401) clearSession();
          return response;
        }
        clearSession();
        return response;
      },
    });
  }, [endpoint, state, clearSession, requestStepUp]);

  return (
    <OperatorManagementContext.Provider value={client}>
      {children}
    </OperatorManagementContext.Provider>
  );
}

export function useOperatorClient(): MakoOperatorClient {
  const client = useContext(OperatorManagementContext);
  if (client === null) {
    throw new OperatorSessionExpiredError();
  }
  return client;
}

export class OperatorSessionExpiredError extends Error {
  override readonly name = "OperatorSessionExpiredError";

  constructor() {
    super("Your operator session expired. Sign in again to continue.");
  }
}
