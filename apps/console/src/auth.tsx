import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";

export interface DeveloperProfile {
  readonly id: string;
  readonly email: string;
  readonly displayName: string;
}

export interface DeveloperSession {
  readonly accessToken: string;
  readonly expiresAt: string;
  readonly audience: "mako-management" | "mako-developer-waitlist";
  readonly profile: DeveloperProfile;
}

export interface DeveloperSelfServiceAdapter extends DeveloperAuthAdapter {
  register(input: {
    readonly email: string;
    readonly displayName: string;
    readonly password: string;
  }): Promise<void>;
  verifyEmail(token: string): Promise<"waitlisted" | "password_updated">;
  resendVerification(email: string): Promise<void>;
  signInWithPassword(email: string, password: string): Promise<DeveloperSession>;
  requestPasswordRecovery(email: string): Promise<void>;
  completePasswordRecovery(
    token: string,
    password: string,
  ): Promise<"waitlisted" | "password_updated">;
  waitListStatus(): Promise<{
    readonly developerIdentityId: string;
    readonly status: "waitlisted";
  }>;
}

export interface DeveloperAuthAdapter {
  loadSession(): Promise<DeveloperSession | null>;
  beginSignIn(returnTo: string): Promise<void>;
  signOut(): Promise<void>;
  subscribe(listener: (session: DeveloperSession | null) => void): () => void;
}

export type DeveloperAuthState =
  | { readonly status: "loading" }
  | { readonly status: "anonymous" }
  | { readonly status: "authenticated"; readonly session: DeveloperSession }
  | { readonly status: "error"; readonly message: string };

interface DeveloperAuthContextValue {
  readonly state: DeveloperAuthState;
  readonly acceptsSessionToken: boolean;
  readonly selfService: DeveloperSelfServiceAdapter | null;
  readonly signIn: (sessionToken?: string) => Promise<void>;
  readonly signOut: () => Promise<void>;
  /** A current session, renewed with the refresh cookie when the access token has expired. */
  readonly renew: () => Promise<DeveloperSession | null>;
}

const DeveloperAuthContext = createContext<DeveloperAuthContextValue | null>(null);

export function DeveloperAuthProvider({
  adapter,
  children,
}: {
  readonly adapter: DeveloperAuthAdapter;
  readonly children: ReactNode;
}) {
  const [state, setState] = useState<DeveloperAuthState>({ status: "loading" });

  useEffect(() => {
    let live = true;
    const applySession = (session: DeveloperSession | null) => {
      if (live) {
        setState(
          session !== null && isSessionActive(session)
            ? { status: "authenticated", session }
            : { status: "anonymous" },
        );
      }
    };
    const unsubscribe = adapter.subscribe(applySession);
    adapter.loadSession().then(applySession, (error: unknown) => {
      if (live) {
        setState({ status: "error", message: safeAuthenticationMessage(error) });
      }
    });
    return () => {
      live = false;
      unsubscribe();
    };
  }, [adapter]);

  const acceptsSessionToken = "acceptSessionToken" in adapter;
  const selfService = isDeveloperSelfServiceAdapter(adapter) ? adapter : null;
  const signIn = useCallback(
    async (sessionToken?: string) => {
      if (
        sessionToken !== undefined &&
        "acceptSessionToken" in adapter &&
        typeof adapter.acceptSessionToken === "function"
      ) {
        await adapter.acceptSessionToken(sessionToken);
        return;
      }
      await adapter.beginSignIn(`${window.location.pathname}${window.location.search}`);
    },
    [adapter],
  );
  const signOut = useCallback(async () => {
    await adapter.signOut();
    setState({ status: "anonymous" });
  }, [adapter]);
  // The access token lasts fifteen minutes and was renewed only when a page
  // loaded, so a page left open that long answered every action with "session
  // expired" although the refresh cookie was still valid. Requests share one
  // renewal: each refresh rotates the cookie, and a second refresh with the old
  // one would read as a replay.
  const renewing = useRef<Promise<DeveloperSession | null> | null>(null);
  const renew = useCallback(() => {
    renewing.current ??= adapter
      .loadSession()
      .then(
        (session) => {
          const active = session !== null && isSessionActive(session) ? session : null;
          setState(
            active === null
              ? { status: "anonymous" }
              : { status: "authenticated", session: active },
          );
          return active;
        },
        () => {
          setState({ status: "anonymous" });
          return null;
        },
      )
      .finally(() => {
        renewing.current = null;
      });
    return renewing.current;
  }, [adapter]);
  const value = useMemo(
    () => ({ state, acceptsSessionToken, selfService, signIn, signOut, renew }),
    [state, acceptsSessionToken, selfService, signIn, signOut, renew],
  );

  return <DeveloperAuthContext.Provider value={value}>{children}</DeveloperAuthContext.Provider>;
}

export function useDeveloperAuth(): DeveloperAuthContextValue {
  const value = useContext(DeveloperAuthContext);
  if (value === null) {
    throw new Error("useDeveloperAuth must be used inside DeveloperAuthProvider");
  }
  return value;
}

export function isSessionActive(session: DeveloperSession, now = Date.now()): boolean {
  const expiresAt = Date.parse(session.expiresAt);
  return (
    session.accessToken.length >= 16 &&
    session.accessToken.length <= 16 * 1024 &&
    Number.isFinite(expiresAt) &&
    expiresAt > now &&
    (session.audience === "mako-management" || session.audience === "mako-developer-waitlist")
  );
}

export function isDeveloperSelfServiceAdapter(
  adapter: DeveloperAuthAdapter,
): adapter is DeveloperSelfServiceAdapter {
  return (
    "signInWithPassword" in adapter &&
    typeof adapter.signInWithPassword === "function" &&
    "register" in adapter &&
    typeof adapter.register === "function"
  );
}

/** Test/local adapter that intentionally keeps bearer credentials only in memory. */
export class MemoryDeveloperAuthAdapter implements DeveloperAuthAdapter {
  readonly #listeners = new Set<(session: DeveloperSession | null) => void>();
  #session: DeveloperSession | null;

  constructor(session: DeveloperSession | null = null) {
    this.#session = session;
  }

  async loadSession(): Promise<DeveloperSession | null> {
    return this.#session;
  }

  async beginSignIn(_returnTo: string): Promise<void> {
    // A host application supplies the actual developer identity-provider redirect.
  }

  async signOut(): Promise<void> {
    this.setSession(null);
  }

  subscribe(listener: (session: DeveloperSession | null) => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  setSession(session: DeveloperSession | null): void {
    this.#session = session;
    for (const listener of this.#listeners) {
      listener(session);
    }
  }
}

function safeAuthenticationMessage(error: unknown): string {
  return error instanceof Error && error.name === "DeveloperAuthenticationError"
    ? error.message
    : "Developer authentication is unavailable.";
}
