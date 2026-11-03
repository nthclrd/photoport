import App from "./App.svelte";
import "./app.css";

async function boot() {
  if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
    await import("./dev-mock");
  }
  new App({ target: document.getElementById("app")! });
}

void boot();
