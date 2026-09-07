import {
  Alert,
  AlertDescription,
  Button,
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
  Field,
  Input,
} from "@mako-cloud/ui";
import {
  createContext,
  type FormEvent,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
} from "react";

export interface OperatorProfile {
  readonly id: string;
  readonly developerIdentityId: string;
  readonly email: string;
  readonly displayName: string;
  readonly developerStatus: string | null;
}

export interface OperatorSession {
  readonly expiresAt: string;
  readonly passwordVerifiedAt: string;
  readonly permissions: readonly string[];
  readonly profile: OperatorProfile;
}

export interface OperatorAuthAdapter {
  loadSession(): Promise<OperatorSession | null>;
  signIn(email: string, password: string): Promise<OperatorSession>;
  verifyPassword(password: string): Promise<OperatorSession>;
  signOut(): Promise<void>;
  subscribe(listener: (session: OperatorSession | null) => void): () => void;
}

export type OperatorAuthState =
  | { readonly status: "unconfigured" }
  | { readonly status: "loading" }
  | { readonly status: "anonymous" }
  | { readonly status: "authenticated"; readonly session: OperatorSession }
  | { readonly status: "error"; readonly message: string };

interface OperatorAuthContextValue {
  readonly state: OperatorAuthState;
  readonly signIn: (email: string, password: string) => Promise<void>;
  readonly signOut: () => Promise<void>;
  readonly clearSession: () => void;
  readonly requestStepUp: () => Promise<void>;
}

interface PendingStepUp {
  readonly resolve: () => void;
  readonly reject: (error: Error) => void;
}

const OperatorAuthContext = createContext<OperatorAuthContextValue | null>(null);

export function OperatorAuthProvider({
  adapter,
  children,
}: {
  readonly adapter?: OperatorAuthAdapter | undefined;
  readonly children: ReactNode;
}) {
  const [state, setState] = useState<OperatorAuthState>(
    adapter === undefined ? { status: "unconfigured" } : { status: "loading" },
  );
  const [stepUp, setStepUp] = useState<PendingStepUp | null>(null);

  useEffect(() => {
    if (adapter === undefined) {
      setState({ status: "unconfigured" });
      return;
    }
    let live = true;
    const applySession = (session: OperatorSession | null) => {
      if (live) {
        setState(
          session !== null && isOperatorSessionActive(session)
            ? { status: "authenticated", session }
            : { status: "anonymous" },
        );
      }
    };
    const unsubscribe = adapter.subscribe(applySession);
    adapter.loadSession().then(applySession, (error: unknown) => {
      if (live) setState({ status: "error", message: safeAuthenticationMessage(error) });
    });
    return () => {
      live = false;
      unsubscribe();
    };
  }, [adapter]);

  const signIn = useCallback(
    async (email: string, password: string) => {
      if (adapter === undefined) throw new Error("Operator authentication is not configured.");
      const session = await adapter.signIn(email, password);
      setState({ status: "authenticated", session });
    },
    [adapter],
  );
  const clearSession = useCallback(() => {
    setState(adapter === undefined ? { status: "unconfigured" } : { status: "anonymous" });
  }, [adapter]);
  const signOut = useCallback(async () => {
    if (adapter !== undefined) await adapter.signOut();
    clearSession();
  }, [adapter, clearSession]);
  const requestStepUp = useCallback(
    () =>
      new Promise<void>((resolve, reject) => {
        setStepUp((current) => {
          if (current !== null) {
            reject(new Error("Password verification is already pending."));
            return current;
          }
          return { resolve, reject };
        });
      }),
    [],
  );
  const value = useMemo(
    () => ({ state, signIn, signOut, clearSession, requestStepUp }),
    [state, signIn, signOut, clearSession, requestStepUp],
  );

  return (
    <OperatorAuthContext.Provider value={value}>
      {children}
      {stepUp === null || adapter === undefined ? null : (
        <OperatorStepUpDialog
          onCancel={() => {
            stepUp.reject(new Error("Password verification was cancelled."));
            setStepUp(null);
          }}
          onVerify={async (password) => {
            const session = await adapter.verifyPassword(password);
            setState({ status: "authenticated", session });
            stepUp.resolve();
            setStepUp(null);
          }}
        />
      )}
    </OperatorAuthContext.Provider>
  );
}

function OperatorStepUpDialog({
  onVerify,
  onCancel,
}: {
  readonly onVerify: (password: string) => Promise<void>;
  readonly onCancel: () => void;
}) {
  const [password, setPassword] = useState("");
  const [pending, setPending] = useState(false);
  const [failure, setFailure] = useState<string | null>(null);
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const submitted = password;
    setPassword("");
    setPending(true);
    setFailure(null);
    try {
      await onVerify(submitted);
    } catch (error) {
      setFailure(safeAuthenticationMessage(error));
      setPending(false);
    }
  };
  // The layer is modal: Escape and a click on the backdrop cancel the
  // verification the same way the Cancel button does, except while a
  // verification is already on its way to the server.
  return (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open && !pending) onCancel();
      }}
    >
      <DialogContent showCloseButton={false} aria-labelledby="step-up-title">
        <DialogHeader>
          <DialogTitle id="step-up-title">Verify your operator password</DialogTitle>
          <DialogDescription>
            Privileged changes require a password verification from the last five minutes.
          </DialogDescription>
        </DialogHeader>
        {failure === null ? null : (
          <Alert variant="destructive">
            <AlertDescription className="block">{failure}</AlertDescription>
          </Alert>
        )}
        <form className="grid gap-4" onSubmit={(event) => void submit(event)}>
          <Field label="Password" htmlFor="operator-step-up-password">
            <Input
              id="operator-step-up-password"
              type="password"
              autoComplete="current-password"
              value={password}
              required
              maxLength={1024}
              onChange={(event) => setPassword(event.currentTarget.value)}
            />
          </Field>
          <DialogFooter>
            <Button variant="secondary" disabled={pending} onClick={onCancel}>
              Cancel
            </Button>
            <Button type="submit" disabled={pending || password === ""}>
              {pending ? "Verifying…" : "Verify and continue"}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}

export function useOperatorAuth(): OperatorAuthContextValue {
  const value = useContext(OperatorAuthContext);
  if (value === null) throw new Error("useOperatorAuth must be used inside OperatorAuthProvider");
  return value;
}

export function isOperatorSessionActive(session: OperatorSession, now = Date.now()): boolean {
  const expiresAt = Date.parse(session.expiresAt);
  const passwordVerifiedAt = Date.parse(session.passwordVerifiedAt);
  return (
    Number.isFinite(expiresAt) &&
    Number.isFinite(passwordVerifiedAt) &&
    expiresAt > now &&
    passwordVerifiedAt <= expiresAt &&
    /^opr_[A-Za-z0-9_-]{8,64}$/u.test(session.profile.id) &&
    /^dev_[A-Za-z0-9_-]{8,64}$/u.test(session.profile.developerIdentityId) &&
    session.permissions.length > 0
  );
}

/** Test-only in-memory adapter; production bootstrap uses the hosted cookie adapter. */
export class MemoryOperatorAuthAdapter implements OperatorAuthAdapter {
  readonly #listeners = new Set<(session: OperatorSession | null) => void>();
  #session: OperatorSession | null;

  constructor(session: OperatorSession | null = null) {
    this.#session = session;
  }

  async loadSession(): Promise<OperatorSession | null> {
    return this.#session;
  }

  async signIn(_email: string, _password: string): Promise<OperatorSession> {
    if (this.#session === null) throw new Error("No test operator session is configured.");
    return this.#session;
  }

  async verifyPassword(_password: string): Promise<OperatorSession> {
    if (this.#session === null) throw new Error("No test operator session is configured.");
    return this.#session;
  }

  async signOut(): Promise<void> {
    this.#session = null;
    this.#emit();
  }

  subscribe(listener: (session: OperatorSession | null) => void): () => void {
    this.#listeners.add(listener);
    return () => this.#listeners.delete(listener);
  }

  setSession(session: OperatorSession | null) {
    this.#session = session;
    this.#emit();
  }

  #emit() {
    for (const listener of this.#listeners) listener(this.#session);
  }
}

function safeAuthenticationMessage(error: unknown): string {
  if (error instanceof Error && error.message.length <= 512) return error.message;
  return "Operator authentication is unavailable.";
}
