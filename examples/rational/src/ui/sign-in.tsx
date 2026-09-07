import { Button, Card, CardContent, CardHeader, Field, Input, Separator } from "@mako-cloud/ui";
import { type FormEvent, useState } from "react";

import type { SignInProviderSetting } from "../config.js";
import type { AppState, RationalApp } from "../data/rational.js";

/**
 * Every sign-in method the environment offers, on one screen: email and
 * password, the providers it has registered, and a magic link. A provider it
 * knows but has switched off is shown disabled rather than failing when it is
 * pressed, which is what the requirement asks for.
 */
export function SignInScreen({ app, state }: { app: RationalApp; state: AppState }) {
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState<"sign_in" | "sign_up" | "magic_link" | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  const settings = app.config.signIn;
  const error = state.authError;

  const submit = async (event: FormEvent, action: "sign_in" | "sign_up") => {
    event.preventDefault();
    setBusy(action);
    setMessage(null);
    try {
      if (action === "sign_up") {
        await app.signUp(email, password);
        setMessage("Account created. Signing you in…");
      }
      await app.signIn(email, password);
    } catch {
      // The app state carries the error message.
    } finally {
      setBusy(null);
    }
  };

  const sendMagicLink = async (event: FormEvent) => {
    event.preventDefault();
    setBusy("magic_link");
    setMessage(null);
    try {
      await app.requestMagicLink(email);
    } finally {
      setBusy(null);
    }
  };

  return (
    <main className="flex min-h-screen items-center justify-center bg-background p-6">
      <Card className="w-full max-w-md" aria-labelledby="sign-in-title">
        <CardHeader className="gap-2">
          <p className="text-xs font-semibold tracking-[0.18em] text-primary uppercase">Rational</p>
          <h1 id="sign-in-title" className="text-2xl">
            Sign in
          </h1>
          <p className="text-sm text-muted-foreground">
            Your money, kept on your device — and shared with the people you choose.
          </p>
        </CardHeader>
        <CardContent className="grid gap-6">
          <form className="grid gap-4" onSubmit={(event) => void submit(event, "sign_in")}>
            <Field label="Email" htmlFor="email">
              <Input
                id="email"
                name="email"
                type="email"
                autoComplete="username"
                required
                value={email}
                onChange={(event) => setEmail(event.target.value)}
              />
            </Field>
            <Field label="Password" htmlFor="password">
              <Input
                id="password"
                name="password"
                type="password"
                autoComplete="current-password"
                required
                minLength={8}
                value={password}
                onChange={(event) => setPassword(event.target.value)}
              />
            </Field>
            {error === null ? null : (
              <p className="text-sm text-destructive" role="alert">
                {error}
              </p>
            )}
            {message === null ? null : <p className="text-sm text-muted-foreground">{message}</p>}
            <div className="flex flex-wrap gap-2">
              <Button type="submit" disabled={busy !== null}>
                {busy === "sign_in" ? "Signing in…" : "Sign in"}
              </Button>
              <Button
                variant="secondary"
                disabled={busy !== null}
                onClick={(event) => void submit(event, "sign_up")}
              >
                {busy === "sign_up" ? "Creating…" : "Create account"}
              </Button>
            </div>
          </form>

          {settings.providers.length === 0 ? null : (
            <section className="grid gap-3" aria-labelledby="providers-title">
              <DividerHeading id="providers-title">Or continue with</DividerHeading>
              <ul className="m-0 grid list-none gap-2 p-0">
                {settings.providers.map((provider) => (
                  <li key={provider.name}>
                    <ProviderButton app={app} provider={provider} disabled={busy !== null} />
                  </li>
                ))}
              </ul>
            </section>
          )}

          {!settings.magicLinks ? null : (
            <section className="grid gap-3" aria-labelledby="magic-link-title">
              <DividerHeading id="magic-link-title">Or with a sign-in link</DividerHeading>
              {state.magicLinkSentTo === null ? (
                <form
                  className="grid gap-3"
                  onSubmit={(event) => void sendMagicLink(event)}
                  aria-label="Magic link"
                >
                  <p className="text-sm text-muted-foreground">
                    We send a single-use link to the address above; opening it signs you in on this
                    device.
                  </p>
                  <Button
                    type="submit"
                    variant="secondary"
                    className="justify-self-start"
                    disabled={busy !== null || email.trim() === ""}
                    data-testid="send-magic-link"
                  >
                    {busy === "magic_link" ? "Sending…" : "Email me a link"}
                  </Button>
                </form>
              ) : (
                <p
                  className="text-sm text-muted-foreground"
                  role="status"
                  data-testid="magic-link-sent"
                >
                  Check your email: if {state.magicLinkSentTo} has an account, a sign-in link is on
                  its way.
                </p>
              )}
            </section>
          )}
        </CardContent>
      </Card>
    </main>
  );
}

/** A section heading drawn as a rule with the words in the middle. */
function DividerHeading({ id, children }: { id: string; children: string }) {
  return (
    <h2 id={id} className="flex items-center gap-3 text-xs font-medium text-muted-foreground">
      <Separator className="flex-1" />
      <span className="whitespace-nowrap">{children}</span>
      <Separator className="flex-1" />
    </h2>
  );
}

function ProviderButton({
  app,
  provider,
  disabled,
}: {
  app: RationalApp;
  provider: SignInProviderSetting;
  disabled: boolean;
}) {
  const label = provider.label ?? provider.name;
  if (!provider.enabled) {
    return (
      <span className="grid gap-1" data-testid={`provider-${provider.name}`}>
        <Button variant="outline" className="w-full" disabled>
          Continue with {label}
        </Button>
        <small className="text-xs text-muted-foreground">not enabled for this environment</small>
      </span>
    );
  }
  return (
    <span className="block" data-testid={`provider-${provider.name}`}>
      <Button
        variant="outline"
        className="w-full"
        disabled={disabled}
        onClick={() => void app.signInWithProvider(provider.name)}
      >
        Continue with {label}
      </Button>
    </span>
  );
}
