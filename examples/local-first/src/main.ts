import {
  type LiveBackendOptions,
  LiveMakoBackend,
  requestPasswordRecovery,
  setPasswordFromLink,
  VerificationPendingError,
  verifyEmail,
} from "./live-backend.js";
import { FakeMakoBackend } from "./mock-backend.js";
import {
  createReferenceApplication,
  type ReferenceApplication,
  type ReferenceTodo,
} from "./reference-app.js";
import "./styles.css";

/**
 * The app runs against its in-browser fake by default so it is runnable with no
 * server. Supplying `window.__MAKO_EXAMPLE__` before load points it at a real
 * deployment instead, which is how the live end-to-end suite drives it.
 */
declare global {
  interface Window {
    __MAKO_EXAMPLE__?: LiveBackendOptions;
  }
}

const status = requiredElement("status");
const diagnostics = requiredElement("diagnostics");
const todoList = requiredElement("todos");
const form = document.querySelector<HTMLFormElement>("#todo-form");
const title = document.querySelector<HTMLInputElement>("#title");
if (form === null || title === null) {
  throw new Error("reference app form is missing");
}

/** The environment's password policy: Mako's default asks for 12 characters.
 * Declared before the sign-in below awaits, which reads it. */
const MINIMUM_PASSWORD_LENGTH = 12;
/** Said once the app is showing, about how the person got in: set when they
 * arrived through a password link, so the page confirms the password took. */
let arrivalMessage: string | null = null;

status.textContent = "starting";
const { application, email } = await startApplication();
requiredElement("app").hidden = false;
if (email !== null) {
  requiredElement("account-email").textContent = email;
  requiredElement("account").hidden = false;
  requiredElement("sign-out").addEventListener("click", () => {
    void application.signOut().finally(() => window.location.reload());
  });
}

interface SignInAttempt {
  email: string;
  password: string;
  createAccount: boolean;
}

/**
 * Starts the app as someone. The fake backend and a page configured with
 * credentials sign in by themselves; a live page without them asks, so each
 * person works as their own application user and the collection's policy
 * decides what they see.
 */
async function startApplication(): Promise<{
  application: ReferenceApplication;
  email: string | null;
}> {
  const live = window.__MAKO_EXAMPLE__;
  if (live === undefined) {
    return { application: await createReferenceApplication(new FakeMakoBackend()), email: null };
  }
  if (live.email !== undefined && live.password !== undefined) {
    return {
      application: await createReferenceApplication(new LiveMakoBackend(live)),
      email: live.email,
    };
  }
  // A link pasted into a tab that already shows this page changes only the
  // fragment, which does not load the page again; load it, so the link is
  // acted on.
  window.addEventListener("hashchange", () => {
    const fragment = new URLSearchParams(window.location.hash.slice(1));
    if (fragment.has("password_reset_token") || fragment.has("verification_token")) {
      window.location.reload();
    }
  });
  // A session kept from an earlier visit signs the person straight back in;
  // with none, or one the service no longer accepts, the form is shown. A
  // password link is for whoever it was mailed to, so it signs out a session
  // this browser remembers instead of being ignored behind it.
  const remembered = new LiveMakoBackend({ ...live, rememberSession: true });
  if (new URLSearchParams(window.location.hash.slice(1)).has("password_reset_token")) {
    await remembered.sessionPersistence?.clear();
  } else {
    status.textContent = "resuming session";
    try {
      const application = await createReferenceApplication(remembered);
      return { application, email: remembered.signedInEmail };
    } catch {
      await remembered.sessionPersistence?.clear();
    }
  }
  const panel = requiredElement("sign-in");
  const signInForm = panel.querySelector<HTMLFormElement>("form");
  const failure = requiredElement("sign-in-error");
  if (signInForm === null) {
    throw new Error("sign-in form is missing");
  }
  // One listener for the life of the form. A listener added per attempt left
  // the form without one while an attempt ran or after one threw, and the
  // next click fell through to the browser's own submit: a reload, with the
  // email and password in the URL. A submit during an attempt is ignored.
  let waiting: ((attempt: SignInAttempt) => void) | null = null;
  signInForm.addEventListener("submit", (event) => {
    event.preventDefault();
    const resolve = waiting;
    if (resolve === null) {
      return;
    }
    waiting = null;
    const data = new FormData(signInForm);
    const submitter = (event as SubmitEvent).submitter as HTMLButtonElement | null;
    resolve({
      email: String(data.get("email") ?? "").trim(),
      password: String(data.get("password") ?? ""),
      createAccount: submitter?.value === "create",
    });
  });
  panel.hidden = false;
  status.textContent = "signed out";
  // Opened from a verification mail: confirm the address, then drop the
  // token from the address bar so a reload or a shared URL cannot replay it.
  const token = new URLSearchParams(window.location.hash.slice(1)).get("verification_token");
  if (token !== null && token !== "") {
    window.history.replaceState(null, "", `${window.location.pathname}${window.location.search}`);
    status.textContent = "confirming email";
    try {
      failure.textContent = (await verifyEmail(live, token))
        ? "Email confirmed. Sign in to continue."
        : "That confirmation link has expired or was already used.";
    } catch (error) {
      failure.textContent = `Could not confirm the email: ${
        error instanceof Error ? error.message : String(error)
      }`;
    }
    status.textContent = "signed out";
  }
  // Opened from a password link -- a recovery the person asked for, or an
  // invitation: the form asks only for the new password, and the person is
  // signed in with it. The token leaves the address bar at once.
  let resetToken = new URLSearchParams(window.location.hash.slice(1)).get("password_reset_token");
  const emailField = signInForm.querySelector<HTMLElement>("#email");
  const emailLabel = signInForm.querySelector<HTMLElement>('label[for="email"]');
  const createButton = signInForm.querySelector<HTMLButtonElement>('button[value="create"]');
  const signInButton = signInForm.querySelector<HTMLButtonElement>('button[value="sign-in"]');
  const choosingPassword = (on: boolean) => {
    for (const element of [emailField, emailLabel, createButton, forgot]) {
      if (element !== null) element.hidden = on;
    }
    emailField?.toggleAttribute("required", !on);
    if (signInButton !== null) signInButton.textContent = on ? "Set password" : "Sign in";
  };
  // Forgot the password: mail a link to choose a new one to the address typed.
  const forgot = document.createElement("button");
  forgot.type = "button";
  forgot.className = "secondary";
  forgot.textContent = "Forgot password?";
  signInForm.querySelector(".auth-actions")?.append(forgot);
  forgot.addEventListener("click", () => {
    const address = String(new FormData(signInForm).get("email") ?? "").trim();
    if (address === "") {
      failure.textContent = "Type your email address first.";
      return;
    }
    failure.textContent = "";
    requestPasswordRecovery(live, address).then(
      () => {
        failure.textContent = `If ${address} has an account, a link to choose a new password is on its way.`;
      },
      (error: unknown) => {
        failure.textContent = `Could not send the link: ${error instanceof Error ? error.message : String(error)}`;
      },
    );
  });
  if (resetToken !== null && resetToken !== "") {
    window.history.replaceState(null, "", `${window.location.pathname}${window.location.search}`);
    choosingPassword(true);
    failure.textContent = "Choose a password for your account.";
  } else {
    resetToken = null;
  }
  for (;;) {
    const attempt = await new Promise<SignInAttempt>((resolve) => {
      waiting = resolve;
    });
    failure.textContent = "";
    if (resetToken !== null) {
      if (Array.from(attempt.password).length < MINIMUM_PASSWORD_LENGTH) {
        failure.textContent = `Use a password of at least ${MINIMUM_PASSWORD_LENGTH} characters.`;
        continue;
      }
      status.textContent = "setting password";
      try {
        const email = await setPasswordFromLink(live, resetToken, attempt.password);
        resetToken = null;
        choosingPassword(false);
        if (email === null) {
          failure.textContent =
            "That link has expired or was already used. Sign in, or ask for a new one.";
          status.textContent = "signed out";
          continue;
        }
        attempt.email = email;
        attempt.createAccount = false;
        arrivalMessage = `Your new password is set, and you are signed in as ${email}.`;
      } catch (error) {
        failure.textContent = `Could not set the password: ${
          error instanceof Error ? error.message : String(error)
        }`;
        status.textContent = "signed out";
        continue;
      }
    }
    if (attempt.createAccount && Array.from(attempt.password).length < MINIMUM_PASSWORD_LENGTH) {
      failure.textContent = `Use a password of at least ${MINIMUM_PASSWORD_LENGTH} characters.`;
      continue;
    }
    status.textContent = attempt.createAccount ? "creating account" : "signing in";
    try {
      const application = await createReferenceApplication(
        new LiveMakoBackend({ ...live, ...attempt, rememberSession: true }),
      );
      panel.hidden = true;
      return { application, email: attempt.email };
    } catch (error) {
      arrivalMessage = null;
      failure.textContent =
        error instanceof VerificationPendingError
          ? error.message
          : `${attempt.createAccount ? "Could not create the account" : "Could not sign in"}: ${
              error instanceof Error ? error.message : String(error)
            }`;
      status.textContent = "signed out";
    }
  }
}

// A failed local write used to vanish: the promise was dropped and nothing
// on the page changed. Say what went wrong, where the todo was typed.
const notice = document.createElement("p");
notice.id = "notice";
notice.setAttribute("role", "alert");
form.insertAdjacentElement("afterend", notice);
// Its own line under the account, so a replication warning in the notice
// cannot replace it.
if (arrivalMessage !== null) {
  const arrival = document.createElement("p");
  arrival.id = "arrival";
  arrival.setAttribute("role", "status");
  arrival.textContent = arrivalMessage;
  requiredElement("account").insertAdjacentElement("afterend", arrival);
}

function report(action: string, error: unknown): void {
  notice.textContent = `${action} failed: ${error instanceof Error ? error.message : String(error)}`;
}

let currentTodos: readonly ReferenceTodo[] = [];
// The todo being edited, and the text typed so far. The list re-renders on
// every change, local or pulled, so an edit in progress has to survive that.
let editing: { readonly id: string; draft: string } | null = null;

/** A new todo's id, unique across every user of the collection. Ids are
 * primary keys shared by all users, while each user reads only their own todos
 * under an owner policy: an id derived from what one user can see would clash
 * with another user's todo that they cannot, and be refused. The list is
 * sorted by id, so the id starts with the time, in fixed-width base 36: a
 * random id alone showed the list in a shuffled order. */
function nextTodoId(): string {
  return `todo-${Date.now().toString(36).padStart(9, "0")}-${crypto.randomUUID()}`;
}

function renderTodos(todos: readonly ReferenceTodo[]): void {
  currentTodos = todos;
  if (editing !== null && !todos.some((todo) => todo.id === editing?.id)) {
    editing = null;
  }
  todoList.replaceChildren(...todos.map((todo) => renderTodo(todo)));
}

function renderTodo(todo: ReferenceTodo): HTMLLIElement {
  const item = document.createElement("li");
  item.className = "todo";
  if (editing?.id === todo.id) {
    const edit = editing;
    const input = document.createElement("input");
    input.type = "text";
    input.value = edit.draft;
    input.setAttribute("aria-label", `Edit ${todo.title}`);
    input.addEventListener("input", () => {
      edit.draft = input.value;
    });
    input.addEventListener("keydown", (event) => {
      if (event.key === "Enter") {
        event.preventDefault();
        void saveEdit(todo);
      } else if (event.key === "Escape") {
        editing = null;
        renderTodos(currentTodos);
      }
    });
    const save = button("Save", () => void saveEdit(todo));
    const cancel = button("Cancel", () => {
      editing = null;
      renderTodos(currentTodos);
    });
    item.append(input, save, cancel);
    queueMicrotask(() => input.focus());
    return item;
  }
  const label = document.createElement("span");
  label.className = "todo-title";
  label.dataset.testid = `todo-${todo.id}`;
  label.textContent = todo.title;
  const edit = button("Edit", () => {
    editing = { id: todo.id, draft: todo.title };
    renderTodos(currentTodos);
  });
  edit.setAttribute("aria-label", `Edit ${todo.title}`);
  const remove = button("Delete", () => {
    application
      .deleteTodo(todo.id)
      .then(() => {
        notice.textContent = "";
      })
      .catch((error: unknown) => report("Delete", error));
  });
  remove.setAttribute("aria-label", `Delete ${todo.title}`);
  item.append(label, edit, remove);
  return item;
}

async function saveEdit(todo: ReferenceTodo): Promise<void> {
  const draft = editing?.draft.trim() ?? "";
  editing = null;
  if (draft.length === 0 || draft === todo.title) {
    renderTodos(currentTodos);
    return;
  }
  try {
    await application.updateTodo(todo.id, draft, Date.now());
    notice.textContent = "";
  } catch (error) {
    report("Edit", error);
    renderTodos(currentTodos);
  }
}

function button(text: string, onClick: () => void): HTMLButtonElement {
  const element = document.createElement("button");
  element.type = "button";
  element.textContent = text;
  element.addEventListener("click", onClick);
  return element;
}

function renderDiagnostics(): void {
  const value = application.diagnostics();
  diagnostics.textContent = JSON.stringify(value);
  if (value.recovery === null) {
    status.textContent = value.activity;
    return;
  }
  // Replication is paused until the app is updated; local writes still land
  // in the browser and are sent once a version with the new schema runs.
  status.textContent = "update required";
  notice.textContent = value.recovery.startsWith("schema_migration_required")
    ? "This app is out of date: the server now uses a newer data format. Your changes are kept on this device; reload to update and sync them."
    : "Sync must restart from the beginning. Reload the page to resync.";
}

const todoSubscription = application.observeTodos(renderTodos);
const refreshTimer = window.setInterval(renderDiagnostics, 25);
renderDiagnostics();

form.addEventListener("submit", (event) => {
  event.preventDefault();
  const value = title.value.trim();
  if (value.length === 0) {
    return;
  }
  application
    .addTodo({
      id: nextTodoId(),
      ownerId: application.userId,
      title: value,
      updatedAt: Date.now(),
    })
    .then(() => {
      title.value = "";
      notice.textContent = "";
      renderDiagnostics();
    })
    .catch((error: unknown) => report("Add", error));
});

requiredElement("offline").addEventListener("click", () => void application.setOnline(false));
requiredElement("online").addEventListener("click", () => void application.setOnline(true));

const browserWindow = window as Window & {
  makoExample?: typeof application;
};
browserWindow.makoExample = application;
window.addEventListener("beforeunload", () => {
  window.clearInterval(refreshTimer);
  todoSubscription.unsubscribe();
  void application.close();
});

function requiredElement(id: string): HTMLElement {
  const element = document.querySelector<HTMLElement>(`#${id}`);
  if (element === null) {
    throw new Error(`missing #${id}`);
  }
  return element;
}
