import {
  Alert,
  AlertDescription,
  Button,
  Card,
  CardContent,
  CardHeader,
  Eyebrow,
  Field,
  Input,
  useTheme,
} from "@mako-cloud/ui";
import { type FormEvent, type ReactNode, useEffect, useRef, useState } from "react";

import { useDeveloperAuth } from "./auth.js";

type Navigate = (path: string, replace?: boolean) => void;

/**
 * The developer's theme preference, under the same key the authenticated
 * shell keeps it, so the sign-in card is drawn the way the console was left.
 * (Declared here rather than imported: the shell imports these views.)
 */
const THEME_KEY = "mako.console.theme";

/** A link inside a sentence, or standing alone under a form. */
const LINK = "text-sm text-primary underline-offset-4 hover:underline";

export function HostedSignInView({ navigate }: { readonly navigate: Navigate }) {
  const { selfService, state } = useDeveloperAuth();
  const [pending, setPending] = useState(false);
  const [failed, setFailed] = useState(false);

  // Shown in place of a page that needs a session -- an invitation link, say --
  // signing in returns there; from the sign-in page itself it goes home.
  const destination =
    window.location.pathname === "/login"
      ? "/"
      : `${window.location.pathname}${window.location.search}${window.location.hash}`;
  useEffect(() => {
    if (state.status === "authenticated") {
      navigate(
        state.session.audience === "mako-developer-waitlist" ? "/wait-list" : destination,
        true,
      );
    }
  }, [destination, navigate, state]);

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
      navigate(session.audience === "mako-developer-waitlist" ? "/wait-list" : destination, true);
    } catch {
      setFailed(true);
      setPending(false);
    }
  };
  return (
    <PublicAuthShell title="Sign in to your developer account">
      <p className="m-0 text-sm text-muted-foreground">
        Developer accounts administer Mako projects. They are separate from users of applications
        built on Mako.
      </p>
      {failed ? (
        <Alert variant="destructive" role="alert">
          <AlertDescription>The email or password was not accepted.</AlertDescription>
        </Alert>
      ) : null}
      <form className="grid gap-4" onSubmit={(event) => void submit(event)}>
        <Field label="Developer email" htmlFor="sign-in-email">
          <Input
            id="sign-in-email"
            name="email"
            type="email"
            autoComplete="username"
            maxLength={320}
            required
          />
        </Field>
        <Field label="Password" htmlFor="sign-in-password">
          <Input
            id="sign-in-password"
            name="password"
            type="password"
            autoComplete="current-password"
            maxLength={1024}
            required
          />
        </Field>
        <Button type="submit" className="w-full" disabled={pending}>
          {pending ? "Signing in…" : "Sign in"}
        </Button>
      </form>
      <nav
        className="flex flex-wrap items-center justify-between gap-3"
        aria-label="Developer account help"
      >
        <a className={LINK} href="/create-account">
          Create an account
        </a>
        <a className={LINK} href="/forgot-password">
          Forgot password?
        </a>
      </nav>
      <Alert role="note">
        <AlertDescription>
          New developer accounts must verify their email and be approved before product access.
        </AlertDescription>
      </Alert>
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
      <p className="m-0 text-sm text-muted-foreground">
        This account is for building and administering Mako projects, not for signing into an app
        that uses Mako.
      </p>
      {failed ? (
        <Alert variant="destructive" role="alert">
          <AlertDescription>
            Registration is unavailable or the submitted details are invalid.
          </AlertDescription>
        </Alert>
      ) : null}
      <form className="grid gap-4" onSubmit={(event) => void submit(event)}>
        <Field label="Display name" htmlFor="create-account-display-name">
          <Input
            id="create-account-display-name"
            name="displayName"
            autoComplete="name"
            minLength={1}
            maxLength={200}
            required
          />
        </Field>
        <Field label="Developer email" htmlFor="create-account-email">
          <Input
            id="create-account-email"
            name="email"
            type="email"
            autoComplete="email"
            maxLength={320}
            required
          />
        </Field>
        <Field label="Password" htmlFor="create-account-password">
          <Input
            id="create-account-password"
            name="password"
            type="password"
            autoComplete="new-password"
            minLength={12}
            maxLength={1024}
            aria-describedby="new-password-help"
            required
          />
          <p id="new-password-help" className="m-0 text-sm text-muted-foreground">
            Use at least 12 characters.
          </p>
        </Field>
        <Button type="submit" className="w-full" disabled={pending}>
          {pending ? "Submitting…" : "Create developer account"}
        </Button>
      </form>
      <p className="m-0 text-sm text-muted-foreground">
        Already registered?{" "}
        <a className={LINK} href="/sign-in">
          Sign in
        </a>
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
      <p className="m-0 text-sm text-muted-foreground">
        If the submitted address is eligible, we sent a single-use verification link. After
        verification, the developer account joins the review wait list.
      </p>
      {selfService === null ? null : (
        <form
          className="grid gap-4"
          onSubmit={(event) => {
            event.preventDefault();
            setSent(false);
            void selfService.resendVerification(email).then(
              () => setSent(true),
              () => setSent(true),
            );
          }}
        >
          <Field label="Developer email" htmlFor="resend-verification-email">
            <Input
              id="resend-verification-email"
              type="email"
              value={email}
              maxLength={320}
              required
              onChange={(event) => setEmail(event.currentTarget.value)}
            />
          </Field>
          <Button type="submit" variant="secondary" className="justify-self-start">
            Resend verification email
          </Button>
          {sent ? (
            <p role="status" className="m-0 text-sm text-muted-foreground">
              If the address is eligible, another message was queued.
            </p>
          ) : null}
        </form>
      )}
      <a className={LINK} href="/sign-in">
        Return to sign in
      </a>
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
        <p role="status" className="m-0 text-sm text-muted-foreground">
          Checking the single-use verification link…
        </p>
      ) : result === "verified" ? (
        <>
          <p role="status" className="m-0 text-sm">
            Your email is verified and your developer account is now waiting for review.
          </p>
          <Button asChild className="justify-self-start">
            <a href="/sign-in">Sign in to view status</a>
          </Button>
        </>
      ) : (
        <>
          <Alert variant="destructive" role="alert">
            <AlertDescription>
              This verification link is invalid, expired, or already used.
            </AlertDescription>
          </Alert>
          <a className={LINK} href="/check-email">
            Request another verification email
          </a>
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
      <p className="m-0 text-sm text-muted-foreground">
        We will send a single-use reset link if the developer account is eligible.
      </p>
      <form
        className="grid gap-4"
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
        <Field label="Developer email" htmlFor="recovery-email">
          <Input
            id="recovery-email"
            name="email"
            type="email"
            autoComplete="email"
            maxLength={320}
            required
          />
        </Field>
        <Button type="submit" className="w-full" disabled={pending}>
          {pending ? "Submitting…" : "Send recovery email"}
        </Button>
      </form>
      <a className={LINK} href="/sign-in">
        Return to sign in
      </a>
    </PublicAuthShell>
  );
}

export function CheckRecoveryEmailView() {
  return (
    <PublicAuthShell title="Check your email">
      <p className="m-0 text-sm text-muted-foreground">
        If the developer account is eligible, a single-use password reset link has been queued.
      </p>
      <a className={LINK} href="/sign-in">
        Return to sign in
      </a>
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
        <Alert variant="destructive" role="alert">
          <AlertDescription>
            This password reset link is invalid, expired, or already used.
          </AlertDescription>
        </Alert>
      ) : result === "updated" ? (
        <>
          <p role="status" className="m-0 text-sm">
            Your password was updated. Existing sessions were revoked.
          </p>
          <Button asChild className="justify-self-start">
            <a href="/sign-in">Sign in with the new password</a>
          </Button>
        </>
      ) : (
        <form
          className="grid gap-4"
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
          <Field label="New password" htmlFor="reset-password">
            <Input
              id="reset-password"
              name="password"
              type="password"
              autoComplete="new-password"
              minLength={12}
              maxLength={1024}
              required
            />
          </Field>
          <p className="m-0 text-sm text-muted-foreground">
            Use at least 12 characters. Completing recovery signs out every existing session.
          </p>
          <Button type="submit" className="w-full" disabled={pending}>
            {pending ? "Updating…" : "Update password"}
          </Button>
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
        <p className="m-0 text-sm text-muted-foreground">The wait-list review is complete.</p>
        <Button asChild className="justify-self-start">
          <a href="/">Continue to the developer console</a>
        </Button>
      </PublicAuthShell>
    );
  }
  return (
    <PublicAuthShell title="Your account is waiting for review">
      <p className="m-0 text-sm text-muted-foreground">
        Your email is verified. A platform operator must approve this developer account before it
        can access teams, projects, data, or functions.
      </p>
      {status === "loading" ? (
        <p role="status" className="m-0 text-sm text-muted-foreground">
          Checking current wait-list status…
        </p>
      ) : status === "failed" ? (
        <Alert variant="destructive" role="alert">
          <AlertDescription>
            The current wait-list status could not be loaded. Sign in again.
          </AlertDescription>
        </Alert>
      ) : (
        <Alert role="status">
          <AlertDescription className="text-foreground">Status: pending review</AlertDescription>
        </Alert>
      )}
      <p className="m-0 text-sm text-muted-foreground">
        Signed in as {state.status === "authenticated" ? state.session.profile.email : "developer"}.
        Queue position, reviewer notes, and approval estimates are not published.
      </p>
      <div className="flex flex-wrap gap-2">
        <Button variant="outline" onClick={() => void signOut()}>
          Sign out
        </Button>
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

/** One card in the middle of an otherwise empty page: every public auth view. */
function PublicAuthShell({
  title,
  children,
}: {
  readonly title: string;
  readonly children: ReactNode;
}) {
  useTheme(THEME_KEY);
  return (
    <main
      className="flex min-h-screen items-center justify-center bg-background p-6 text-foreground"
      id="main-content"
      tabIndex={-1}
    >
      <Card className="w-full max-w-md" aria-labelledby="public-auth-title">
        <CardHeader className="gap-2">
          <Eyebrow className="tracking-[0.18em] text-primary">Mako Cloud</Eyebrow>
          <h1 id="public-auth-title" className="text-2xl">
            {title}
          </h1>
        </CardHeader>
        <CardContent className="grid gap-4">{children}</CardContent>
      </Card>
    </main>
  );
}

function UnavailableAuthView() {
  return (
    <PublicAuthShell title="Developer authentication unavailable">
      <Alert variant="destructive" role="alert">
        <AlertDescription>
          This console host does not have self-service developer authentication.
        </AlertDescription>
      </Alert>
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
