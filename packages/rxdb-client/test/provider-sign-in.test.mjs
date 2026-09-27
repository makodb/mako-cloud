import assert from "node:assert/strict";
import test from "node:test";

import {
  MakoAuthClient,
  MakoAuthError,
  MemoryAuthSessionPersistence,
  normalizeMakoRxdbConfig,
} from "../dist/node/index.js";

const user = {
  id: "usr_abcdefgh",
  email: "user@example.test",
  status: "active",
  authorizationEpoch: 1,
};

test("classifies location fragments with or without the leading hash", () => {
  assert.deepEqual(MakoAuthClient.signInFragment("#code=one-time%20code"), {
    kind: "provider_code",
    value: "one-time code",
  });
  assert.deepEqual(MakoAuthClient.signInFragment("code=abc&state=x"), {
    kind: "provider_code",
    value: "abc",
  });
  assert.deepEqual(MakoAuthClient.signInFragment("#magic_link_token=tok.en"), {
    kind: "magic_link",
    value: "tok.en",
  });
  assert.deepEqual(MakoAuthClient.signInFragment("#error=email_not_verified&code=ignored"), {
    kind: "error",
    value: "email_not_verified",
  });
  assert.deepEqual(MakoAuthClient.signInFragment("#verification_token=abc123"), {
    kind: "none",
    value: null,
  });
  assert.equal(MakoAuthClient.verificationFragment("#verification_token=abc123"), "abc123");
  assert.equal(MakoAuthClient.verificationFragment("verification_token=abc123"), "abc123");
  assert.equal(MakoAuthClient.verificationFragment("#magic_link_token=x"), null);
  assert.deepEqual(MakoAuthClient.signInFragment(""), { kind: "none", value: null });
  assert.deepEqual(MakoAuthClient.signInFragment("#"), { kind: "none", value: null });
  assert.deepEqual(MakoAuthClient.signInFragment("#/route?x=1"), { kind: "none", value: null });
});

test("starts a provider sign-in with the public key only and hands back the authorization URL", async () => {
  const requests = [];
  const fetch = async (input, init) => {
    requests.push({ url: String(input), init });
    return Response.json({
      authorizationUrl: "https://accounts.example.test/authorize?state=signed",
      provider: "google",
    });
  };
  const auth = new MakoAuthClient(config(), { fetch });
  const start = await auth.startProviderSignIn("google", "https://app.example.test/auth/callback");
  assert.equal(start.authorizationUrl, "https://accounts.example.test/authorize?state=signed");
  assert.equal(start.provider, "google");
  assert.equal(
    requests[0].url,
    "https://api.example.test/v1/projects/prj_abcdefgh/environments/env_abcdefgh/auth/providers/google/start",
  );
  assert.equal(requests[0].init.method, "POST");
  assert.equal(requests[0].init.headers["X-Mako-Key"], "mako_pk.public.example");
  assert.equal(requests[0].init.headers.Authorization, undefined);
  assert.deepEqual(JSON.parse(requests[0].init.body), {
    redirectUrl: "https://app.example.test/auth/callback",
  });

  await assert.rejects(
    () => auth.startProviderSignIn("Bad Name", "https://app.example.test/cb"),
    (error) => error instanceof MakoAuthError && error.message === "provider name is invalid",
  );
  await assert.rejects(
    () => auth.startProviderSignIn("google", "https://app.example.test/cb#fragment"),
    MakoAuthError,
  );
  assert.equal(requests.length, 1);
});

test("completes a provider sign-in from the fragment and the session behaves like a password one", async () => {
  const requests = [];
  const fetch = async (input, init) => {
    requests.push({ url: String(input), init });
    if (String(input).endsWith("/auth/providers/exchange")) {
      return Response.json({
        accessToken: "provider-access",
        refreshToken: "provider-refresh",
        expiresIn: 1,
        user,
      });
    }
    if (String(input).endsWith("/auth/token")) {
      return Response.json({
        accessToken: "refreshed-access",
        refreshToken: "refreshed-refresh",
        expiresIn: 600,
        user,
      });
    }
    return new Response(null, { status: 204 });
  };
  const persistence = new MemoryAuthSessionPersistence();
  const auth = new MakoAuthClient(config(), { fetch, persistence, now: () => 1_000 });
  const session = await auth.completeProviderSignIn("#code=one-time-code");
  assert.equal(session.accessToken, "provider-access");
  assert.equal(session.refreshToken, undefined);
  assert.equal(session.user.id, user.id);
  assert.equal(auth.authenticationRequired, false);
  assert.equal(
    requests[0].url,
    "https://api.example.test/v1/projects/prj_abcdefgh/environments/env_abcdefgh/auth/providers/exchange",
  );
  assert.equal(requests[0].init.headers["X-Mako-Key"], "mako_pk.public.example");
  assert.equal(requests[0].init.headers.Authorization, undefined);
  assert.deepEqual(JSON.parse(requests[0].init.body), { code: "one-time-code" });
  assert.equal((await persistence.load()).refreshToken, "provider-refresh");

  // The one-second session is refreshed exactly like a password session.
  assert.equal(await auth.validAccessToken(), "refreshed-access");
  assert.deepEqual(JSON.parse(requests[1].init.body), { refreshToken: "provider-refresh" });

  const restored = new MakoAuthClient(config(), { fetch, persistence });
  assert.equal((await restored.restoreSession()).accessToken, "refreshed-access");
  await restored.signOut();
  assert.equal(requests[2].init.headers.Authorization, "Bearer refreshed-access");
  assert.equal(await persistence.load(), null);
});

test("surfaces a provider refusal from the fragment without calling the service", async () => {
  let calls = 0;
  const fetch = async () => {
    calls += 1;
    return Response.json({});
  };
  const auth = new MakoAuthClient(config(), { fetch });
  await assert.rejects(
    () => auth.completeProviderSignIn("#error=provider_refused"),
    (error) =>
      error instanceof MakoAuthError &&
      error.reason === "provider_refused" &&
      error.message.includes("provider_refused"),
  );
  await assert.rejects(
    () => auth.completeProviderSignIn("#magic_link_token=not-a-code"),
    (error) => error instanceof MakoAuthError && error.reason === undefined,
  );
  await assert.rejects(() => auth.completeProviderSignIn(""), MakoAuthError);
  assert.equal(calls, 0);
  assert.equal(auth.currentSession(), null);
});

test("maps a refused exchange onto the API error", async () => {
  const fetch = async () =>
    Response.json(
      {
        apiVersion: "v1",
        error: {
          code: "unauthenticated",
          message: "code expired",
          requestId: "req_exchange",
          retry: { kind: "never" },
        },
      },
      { status: 401 },
    );
  const auth = new MakoAuthClient(config(), { fetch });
  await assert.rejects(
    () => auth.completeProviderSignIn("code=stale"),
    (error) =>
      error instanceof MakoAuthError &&
      error.status === 401 &&
      error.apiError.error.requestId === "req_exchange",
  );
  assert.equal(auth.currentSession(), null);
});

test("requests a magic link and treats only 202 as accepted", async () => {
  const requests = [];
  const responses = [
    Response.json({ accepted: true }, { status: 202 }),
    Response.json({ accepted: true }, { status: 200 }),
    Response.json(
      {
        apiVersion: "v1",
        error: {
          code: "invalid_request",
          message: "redirect is not registered",
          requestId: "req_redirect",
          retry: { kind: "never" },
        },
      },
      { status: 400 },
    ),
  ];
  const fetch = async (input, init) => {
    requests.push({ url: String(input), init });
    return responses.shift();
  };
  const auth = new MakoAuthClient(config(), { fetch });
  assert.equal(
    await auth.requestMagicLink("person@example.test", "https://app.example.test/auth/magic"),
    undefined,
  );
  assert.equal(
    requests[0].url,
    "https://api.example.test/v1/projects/prj_abcdefgh/environments/env_abcdefgh/auth/magic-link",
  );
  assert.equal(requests[0].init.headers["X-Mako-Key"], "mako_pk.public.example");
  assert.deepEqual(JSON.parse(requests[0].init.body), {
    email: "person@example.test",
    redirectUrl: "https://app.example.test/auth/magic",
  });
  await assert.rejects(
    () => auth.requestMagicLink("person@example.test", "https://app.example.test/auth/magic"),
    (error) => error instanceof MakoAuthError && error.status === 200,
  );
  await assert.rejects(
    () => auth.requestMagicLink("person@example.test", "https://app.example.test/auth/magic"),
    (error) => error instanceof MakoAuthError && error.apiError.error.requestId === "req_redirect",
  );
  await assert.rejects(() => auth.requestMagicLink("person@example.test", "not a url"), MakoAuthError);
  assert.equal(requests.length, 3);
});

test("redeems a magic link token into a persisted session", async () => {
  const requests = [];
  const fetch = async (input, init) => {
    requests.push({ url: String(input), init });
    return Response.json({
      accessToken: "magic-access",
      refreshToken: "magic-refresh",
      expiresIn: 300,
      user,
    });
  };
  const persistence = new MemoryAuthSessionPersistence();
  const auth = new MakoAuthClient(config(), { fetch, persistence, now: () => 0 });
  const fragment = MakoAuthClient.signInFragment("#magic_link_token=mlt.token");
  assert.equal(fragment.kind, "magic_link");
  const session = await auth.redeemMagicLink(fragment.value);
  assert.equal(session.accessToken, "magic-access");
  assert.equal(session.expiresAtUnixMilliseconds, 300_000);
  assert.equal(
    requests[0].url,
    "https://api.example.test/v1/projects/prj_abcdefgh/environments/env_abcdefgh/auth/magic-link/redeem",
  );
  assert.deepEqual(JSON.parse(requests[0].init.body), { token: "mlt.token" });
  assert.equal(requests[0].init.headers.Authorization, undefined);
  assert.equal((await persistence.load()).refreshToken, "magic-refresh");
  assert.equal(await auth.validAccessToken(), "magic-access");
  await assert.rejects(() => auth.redeemMagicLink(""), MakoAuthError);
});

test("asks for a password link and redeems it with the new password", async () => {
  const requests = [];
  const fetch = async (input, init) => {
    requests.push({ url: String(input), init });
    return String(input).endsWith("/auth/password-recovery")
      ? Response.json({ accepted: true }, { status: 202 })
      : Response.json({ accessToken: "reset-access", refreshToken: "reset-refresh", expiresIn: 300, user });
  };
  const persistence = new MemoryAuthSessionPersistence();
  const auth = new MakoAuthClient(config(), { fetch, persistence, now: () => 0 });
  await auth.requestPasswordRecovery("person@example.test", "https://app.example.test/reset");
  assert.equal(
    requests[0].url,
    "https://api.example.test/v1/projects/prj_abcdefgh/environments/env_abcdefgh/auth/password-recovery",
  );
  assert.deepEqual(JSON.parse(requests[0].init.body), {
    email: "person@example.test",
    redirectUrl: "https://app.example.test/reset",
  });
  const token = MakoAuthClient.passwordLinkFragment("#password_reset_token=prt-token");
  assert.equal(token, "prt-token");
  assert.equal(MakoAuthClient.passwordLinkFragment("#magic_link_token=x"), null);
  assert.equal(MakoAuthClient.signInFragment("#password_reset_token=prt-token").kind, "none");
  const session = await auth.redeemPasswordLink(token, "a new long password");
  assert.equal(session.accessToken, "reset-access");
  assert.equal(
    requests[1].url,
    "https://api.example.test/v1/projects/prj_abcdefgh/environments/env_abcdefgh/auth/password-recovery/redeem",
  );
  assert.deepEqual(JSON.parse(requests[1].init.body), { token: "prt-token", password: "a new long password" });
  assert.equal((await persistence.load()).refreshToken, "reset-refresh");
  await assert.rejects(() => auth.redeemPasswordLink("", "a new long password"), MakoAuthError);
  await assert.rejects(() => auth.requestPasswordRecovery("person@example.test", "not a url"), MakoAuthError);
});

test("signs up with a verification redirect and redeems the mailed token", async () => {
  const requests = [];
  const fetch = async (input, init) => {
    requests.push({ url: String(input), init });
    return String(input).endsWith("/auth/signup")
      ? Response.json({ accepted: true, verificationRequired: true }, { status: 202 })
      : Response.json({ verified: true });
  };
  const auth = new MakoAuthClient(config(), { fetch, now: () => 0 });
  const accepted = await auth.signUp("new@example.test", "a long password", {
    redirectUrl: "https://app.example.test/",
  });
  assert.equal(accepted.verificationRequired, true);
  assert.deepEqual(JSON.parse(requests[0].init.body), {
    email: "new@example.test",
    password: "a long password",
    redirectUrl: "https://app.example.test/",
  });
  await auth.signUp("plain@example.test", "a long password");
  assert.deepEqual(Object.keys(JSON.parse(requests[1].init.body)), ["email", "password"]);
  assert.deepEqual(await auth.verifyEmail("evc-token"), { verified: true });
  assert.equal(
    requests[2].url,
    "https://api.example.test/v1/projects/prj_abcdefgh/environments/env_abcdefgh/auth/verify-email",
  );
  assert.deepEqual(JSON.parse(requests[2].init.body), { token: "evc-token" });
  await assert.rejects(() => auth.verifyEmail(""), MakoAuthError);
  await assert.rejects(
    () => auth.signUp("x@example.test", "a long password", { redirectUrl: "not a url" }),
    MakoAuthError,
  );
});

function config() {
  return normalizeMakoRxdbConfig({
    endpoint: "https://api.example.test",
    projectId: "prj_abcdefgh",
    environmentId: "env_abcdefgh",
    collectionId: "todos",
    schemaVersion: 1,
    publicProjectKey: "mako_pk.public.example",
    rxdbVersion: "17.4.0",
    runtime: "browser",
  });
}
