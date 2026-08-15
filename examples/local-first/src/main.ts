import type { ReferenceBackend } from "./backend.js";
import { type LiveBackendOptions, LiveMakoBackend } from "./live-backend.js";
import { FakeMakoBackend } from "./mock-backend.js";
import { createReferenceApplication, type ReferenceTodo } from "./reference-app.js";
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

function selectBackend(): ReferenceBackend {
  const live = window.__MAKO_EXAMPLE__;
  return live === undefined ? new FakeMakoBackend() : new LiveMakoBackend(live);
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
const application = await createReferenceApplication(selectBackend());
let todoSequence = 0;

function renderTodos(todos: readonly ReferenceTodo[]): void {
  todoList.replaceChildren(
    ...todos.map((todo) => {
      const item = document.createElement("li");
      item.dataset.testid = `todo-${todo.id}`;
      item.textContent = todo.title;
      return item;
    }),
  );
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
  todoSequence += 1;
  const value = title.value.trim();
  if (value.length === 0) {
    return;
  }
  void application
    .addTodo({
      id: `todo-${todoSequence}`,
      ownerId: "user-example",
      title: value,
      updatedAt: Date.now(),
    })
    .then(() => {
      title.value = "";
      renderDiagnostics();
    });
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
