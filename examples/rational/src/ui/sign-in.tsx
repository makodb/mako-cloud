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
    <main className="centered">
      <section className="panel sign-in" aria-labelledby="sign-in-title">
        <p className="eyebrow">Rational</p>
        <h1 id="sign-in-title">Sign in</h1>
        <p>Money for households, kept on your device and shared with the people you choose.</p>
        <form onSubmit={(event) => void submit(event, "sign_in")}>
          <label htmlFor="email">Email</label>
          <input
            id="email"
            name="email"
            type="email"
            autoComplete="username"
            required
            value={email}
            onChange={(event) => setEmail(event.target.value)}
          />
          <label htmlFor="password">Password</label>
          <input
            id="password"
            name="password"
            type="password"
            autoComplete="current-password"
            required
            minLength={8}
            value={password}
            onChange={(event) => setPassword(event.target.value)}
          />
          {error === null ? null : (
            <p className="error" role="alert">
              {error}
            </p>
          )}
          {message === null ? null : <p className="hint">{message}</p>}
          <div className="actions">
            <button type="submit" disabled={busy !== null}>
              {busy === "sign_in" ? "Signing in…" : "Sign in"}
            </button>
            <button
              type="button"
              className="secondary"
              disabled={busy !== null}
              onClick={(event) => void submit(event, "sign_up")}
            >
              {busy === "sign_up" ? "Creating…" : "Create account"}
            </button>
          </div>
        </form>

        {settings.providers.length === 0 ? null : (
          <section className="providers" aria-labelledby="providers-title">
            <h2 id="providers-title" className="divider">
              Or continue with
            </h2>
            <ul className="provider-list">
              {settings.providers.map((provider) => (
                <li key={provider.name}>
                  <ProviderButton app={app} provider={provider} disabled={busy !== null} />
                </li>
              ))}
            </ul>
          </section>
        )}

        {!settings.magicLinks ? null : (
          <section className="magic-link" aria-labelledby="magic-link-title">
            <h2 id="magic-link-title" className="divider">
              Or with a sign-in link
            </h2>
            {state.magicLinkSentTo === null ? (
              <form onSubmit={(event) => void sendMagicLink(event)} aria-label="Magic link">
                <p className="hint">
                  We send a single-use link to the address above; opening it signs you in on this
                  device.
                </p>
                <button
                  type="submit"
                  className="secondary"
                  disabled={busy !== null || email.trim() === ""}
                  data-testid="send-magic-link"
                >
                  {busy === "magic_link" ? "Sending…" : "Email me a link"}
                </button>
              </form>
            ) : (
              <p className="hint" role="status" data-testid="magic-link-sent">
                Check your email: if {state.magicLinkSentTo} has an account, a sign-in link is on
                its way.
              </p>
            )}
          </section>
        )}
      </section>
    </main>
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
      <span className="provider disabled" data-testid={`provider-${provider.name}`}>
        <button type="button" className="secondary" disabled>
          Continue with {label}
        </button>
        <small>not enabled for this environment</small>
      </span>
    );
  }
  return (
    <span className="provider" data-testid={`provider-${provider.name}`}>
      <button
        type="button"
        className="secondary"
        disabled={disabled}
        onClick={() => void app.signInWithProvider(provider.name)}
      >
        Continue with {label}
      </button>
    </span>
  );
}
