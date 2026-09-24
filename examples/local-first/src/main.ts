import { type LiveBackendOptions, LiveMakoBackend } from "./live-backend.js";
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

/** The environment's password policy: Mako's default asks for 12 characters. */
const MINIMUM_PASSWORD_LENGTH = 12;

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
  const panel = requiredElement("sign-in");
  const signInForm = panel.querySelector<HTMLFormElement>("form");
  const failure = requiredElement("sign-in-error");
  if (signInForm === null) {
    throw new Error("sign-in form is missing");
  }
  panel.hidden = false;
  status.textContent = "signed out";
  for (;;) {
    const attempt = await new Promise<{ email: string; password: string; createAccount: boolean }>(
      (resolve) =>
        signInForm.addEventListener(
          "submit",
          (event) => {
            event.preventDefault();
            const data = new FormData(signInForm);
            const submitter = (event as SubmitEvent).submitter as HTMLButtonElement | null;
            resolve({
              email: String(data.get("email") ?? "").trim(),
              password: String(data.get("password") ?? ""),
              createAccount: submitter?.value === "create",
            });
          },
          { once: true },
        ),
    );
    failure.textContent = "";
    if (attempt.createAccount && Array.from(attempt.password).length < MINIMUM_PASSWORD_LENGTH) {
      failure.textContent = `Use a password of at least ${MINIMUM_PASSWORD_LENGTH} characters.`;
      continue;
    }
    status.textContent = attempt.createAccount ? "creating account" : "signing in";
    try {
      const application = await createReferenceApplication(
        new LiveMakoBackend({ ...live, ...attempt }),
      );
      panel.hidden = true;
      return { application, email: attempt.email };
    } catch (error) {
      failure.textContent = `${attempt.createAccount ? "Could not create the account" : "Could not sign in"}: ${
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
 * with another user's todo that they cannot, and be refused. */
function nextTodoId(): string {
  return `todo-${crypto.randomUUID()}`;
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
  status.textContent = value.activity;
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
