import { type FormEvent, type ReactNode, useEffect, useRef, useState } from "react";

import { useDeveloperAuth } from "./auth.js";

type Navigate = (path: string, replace?: boolean) => void;

export function HostedSignInView({ navigate }: { readonly navigate: Navigate }) {
  const { selfService, state } = useDeveloperAuth();
  const [pending, setPending] = useState(false);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    if (state.status === "authenticated") {
      navigate(state.session.audience === "mako-developer-waitlist" ? "/wait-list" : "/", true);
    }
  }, [navigate, state]);

  if (selfService === null) return null;
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    setPending(true);
    setFailed(false);
    try {
      const session = await selfService.signInWithPassword(
        requiredText(data, "email"),
        requiredText(data, "password"),
      );
      navigate(session.audience === "mako-developer-waitlist" ? "/wait-list" : "/", true);
    } catch {
      setFailed(true);
      setPending(false);
    }
  };
  return (
    <PublicAuthShell title="Sign in to your developer account">
      <p>
        Developer accounts administer Mako projects. They are separate from users of applications
        built on Mako.
      </p>
      {failed ? <p role="alert">The email or password was not accepted.</p> : null}
      <form onSubmit={(event) => void submit(event)}>
        <label>
          Developer email
          <input name="email" type="email" autoComplete="username" maxLength={320} required />
        </label>
        <label>
          Password
          <input
            name="password"
            type="password"
            autoComplete="current-password"
            maxLength={1024}
            required
          />
        </label>
        <button type="submit" disabled={pending}>
          {pending ? "Signing in…" : "Sign in"}
        </button>
      </form>
      <nav className="auth-links" aria-label="Developer account help">
        <a href="/create-account">Create an account</a>
        <a href="/forgot-password">Forgot password?</a>
      </nav>
      <p className="notice">
        New developer accounts must verify their email and be approved before product access.
      </p>
    </PublicAuthShell>
  );
}

export function CreateAccountView({ navigate }: { readonly navigate: Navigate }) {
  const { selfService } = useDeveloperAuth();
  const [pending, setPending] = useState(false);
  const [failed, setFailed] = useState(false);
  if (selfService === null) return <UnavailableAuthView />;
  const submit = async (event: FormEvent<HTMLFormElement>) => {
    event.preventDefault();
    const data = new FormData(event.currentTarget);
    setPending(true);
    setFailed(false);
    try {
      await selfService.register({
        email: requiredText(data, "email"),
        displayName: requiredText(data, "displayName"),
        password: requiredText(data, "password"),
      });
      navigate("/check-email", true);
    } catch {
      setFailed(true);
      setPending(false);
    }
  };
  return (
    <PublicAuthShell title="Create a Mako developer account">
      <p>
        This account is for building and administering Mako projects, not for signing into an app
        that uses Mako.
      </p>
      {failed ? (
        <p role="alert">Registration is unavailable or the submitted details are invalid.</p>
      ) : null}
      <form onSubmit={(event) => void submit(event)}>
        <label>
          Display name
          <input name="displayName" autoComplete="name" minLength={1} maxLength={200} required />
        </label>
        <label>
          Developer email
          <input name="email" type="email" autoComplete="email" maxLength={320} required />
        </label>
        <label>
          Password
          <input
            name="password"
            type="password"
            autoComplete="new-password"
            minLength={12}
            maxLength={1024}
            aria-describedby="new-password-help"
            required
          />
        </label>
        <p id="new-password-help">Use at least 12 characters.</p>
        <button type="submit" disabled={pending}>
          {pending ? "Submitting…" : "Create developer account"}
        </button>
      </form>
      <p>
        Already registered? <a href="/sign-in">Sign in</a>
      </p>
    </PublicAuthShell>
  );
}

export function CheckEmailView() {
  const { selfService } = useDeveloperAuth();
  const [email, setEmail] = useState("");
  const [sent, setSent] = useState(false);
  return (
    <PublicAuthShell title="Check your email">
      <p>
        If the submitted address is eligible, we sent a single-use verification link. After
        verification, the developer account joins the review wait list.
      </p>
      {selfService === null ? null : (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            setSent(false);
            void selfService.resendVerification(email).then(
              () => setSent(true),
              () => setSent(true),
            );
          }}
        >
          <label>
            Developer email
            <input
              type="email"
              value={email}
              maxLength={320}
              required
              onChange={(event) => setEmail(event.currentTarget.value)}
            />
          </label>
          <button type="submit">Resend verification email</button>
          {sent ? (
            <p role="status">If the address is eligible, another message was queued.</p>
          ) : null}
        </form>
      )}
      <a href="/sign-in">Return to sign in</a>
    </PublicAuthShell>
  );
}

export function VerifyEmailView() {
  const { selfService } = useDeveloperAuth();
  const token = useRef(consumeFragmentToken());
  const [result, setResult] = useState<"working" | "verified" | "failed">("working");
  useEffect(() => {
    const value = token.current;
    if (selfService === null || value === null) {
      setResult("failed");
      return;
    }
    void selfService.verifyEmail(value).then(
      () => setResult("verified"),
      () => setResult("failed"),
    );
  }, [selfService]);
  return (
    <PublicAuthShell title="Verify developer email">
      {result === "working" ? (
        <p role="status">Checking the single-use verification link…</p>
      ) : result === "verified" ? (
        <>
          <p role="status">
            Your email is verified and your developer account is now waiting for review.
          </p>
          <a className="button-link" href="/sign-in">
            Sign in to view status
          </a>
        </>
      ) : (
        <>
          <p role="alert">This verification link is invalid, expired, or already used.</p>
          <a href="/check-email">Request another verification email</a>
        </>
      )}
    </PublicAuthShell>
  );
}

export function ForgotPasswordView({ navigate }: { readonly navigate: Navigate }) {
  const { selfService } = useDeveloperAuth();
  const [pending, setPending] = useState(false);
  if (selfService === null) return <UnavailableAuthView />;
  return (
    <PublicAuthShell title="Recover your developer account">
      <p>We will send a single-use reset link if the developer account is eligible.</p>
      <form
        onSubmit={(event) => {
          event.preventDefault();
          const data = new FormData(event.currentTarget);
          setPending(true);
          void selfService.requestPasswordRecovery(requiredText(data, "email")).then(
            () => navigate("/check-recovery-email", true),
            () => navigate("/check-recovery-email", true),
          );
        }}
      >
        <label>
          Developer email
          <input name="email" type="email" autoComplete="email" maxLength={320} required />
        </label>
        <button type="submit" disabled={pending}>
          {pending ? "Submitting…" : "Send recovery email"}
        </button>
      </form>
      <a href="/sign-in">Return to sign in</a>
    </PublicAuthShell>
  );
}

export function CheckRecoveryEmailView() {
  return (
    <PublicAuthShell title="Check your email">
      <p>If the developer account is eligible, a single-use password reset link has been queued.</p>
      <a href="/sign-in">Return to sign in</a>
    </PublicAuthShell>
  );
}

export function ResetPasswordView() {
  const { selfService } = useDeveloperAuth();
  const token = useRef(consumeFragmentToken());
  const [pending, setPending] = useState(false);
  const [result, setResult] = useState<"ready" | "updated" | "failed">(
    token.current === null ? "failed" : "ready",
  );
  if (selfService === null) return <UnavailableAuthView />;
  return (
    <PublicAuthShell title="Set a new developer password">
      {result === "failed" ? (
        <p role="alert">This password reset link is invalid, expired, or already used.</p>
      ) : result === "updated" ? (
        <>
          <p role="status">Your password was updated. Existing sessions were revoked.</p>
          <a className="button-link" href="/sign-in">
            Sign in with the new password
          </a>
        </>
      ) : (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            const data = new FormData(event.currentTarget);
            const value = token.current;
            if (value === null) return;
            setPending(true);
            void selfService.completePasswordRecovery(value, requiredText(data, "password")).then(
              () => {
                token.current = null;
                setResult("updated");
              },
              () => {
                token.current = null;
                setResult("failed");
              },
            );
          }}
        >
          <label>
            New password
            <input
              name="password"
              type="password"
              autoComplete="new-password"
              minLength={12}
              maxLength={1024}
              required
            />
          </label>
          <p>Use at least 12 characters. Completing recovery signs out every existing session.</p>
          <button type="submit" disabled={pending}>
            {pending ? "Updating…" : "Update password"}
          </button>
        </form>
      )}
    </PublicAuthShell>
  );
}

export function WaitListStatusView() {
  const { selfService, signOut, state } = useDeveloperAuth();
  const isWaitlisted =
    state.status === "authenticated" && state.session.audience === "mako-developer-waitlist";
  const [status, setStatus] = useState<"loading" | "waitlisted" | "failed">("loading");
  useEffect(() => {
    if (!isWaitlisted) return;
    if (selfService === null) {
      setStatus("failed");
      return;
    }
    void selfService.waitListStatus().then(
      () => setStatus("waitlisted"),
      () => setStatus("failed"),
    );
  }, [isWaitlisted, selfService]);
  if (state.status === "authenticated" && state.session.audience === "mako-management") {
    return (
      <PublicAuthShell title="Your developer account is active">
        <p>The wait-list review is complete.</p>
        <a className="button-link" href="/">
          Continue to the developer console
        </a>
      </PublicAuthShell>
    );
  }
  return (
    <PublicAuthShell title="Your account is waiting for review">
      <p>
        Your email is verified. A platform operator must approve this developer account before it
        can access organizations, projects, data, or functions.
      </p>
      {status === "loading" ? (
        <p role="status">Checking current wait-list status…</p>
      ) : status === "failed" ? (
        <p role="alert">The current wait-list status could not be loaded. Sign in again.</p>
      ) : (
        <p className="notice" role="status">
          Status: pending review
        </p>
      )}
      <p>
        Signed in as {state.status === "authenticated" ? state.session.profile.email : "developer"}.
        Queue position, reviewer notes, and approval estimates are not published.
      </p>
      <div className="button-row">
        <button type="button" className="secondary" onClick={() => void signOut()}>
          Sign out
        </button>
      </div>
    </PublicAuthShell>
  );
}

export function consumeFragmentToken(): string | null {
  if (typeof window === "undefined") return null;
  const fragment = window.location.hash;
  window.history.replaceState(null, "", `${window.location.pathname}${window.location.search}`);
  if (!fragment.startsWith("#")) return null;
  const parameters = new URLSearchParams(fragment.slice(1));
  const token = parameters.get("token");
  if (
    parameters.size !== 1 ||
    token === null ||
    token.length < 32 ||
    token.length > 4_096 ||
    token.trim() !== token ||
    token.includes("\n") ||
    token.includes("\r")
  ) {
    return null;
  }
  return token;
}

function PublicAuthShell({
  title,
  children,
}: {
  readonly title: string;
  readonly children: ReactNode;
}) {
  return (
    <main className="centered" id="main-content" tabIndex={-1}>
      <section className="panel sign-in" aria-labelledby="public-auth-title">
        <p className="eyebrow">Mako Cloud</p>
        <h1 id="public-auth-title">{title}</h1>
        {children}
      </section>
    </main>
  );
}

function UnavailableAuthView() {
  return (
    <PublicAuthShell title="Developer authentication unavailable">
      <p role="alert">This console host does not have self-service developer authentication.</p>
    </PublicAuthShell>
  );
}

function requiredText(data: FormData, name: string): string {
  const value = data.get(name);
  if (typeof value !== "string" || value.trim() === "") {
    throw new TypeError(`${name} is required`);
  }
  return value;
}
