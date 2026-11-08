import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import QuitDialog from "./QuitDialog";
import "./styles.css";

const root = createRoot(document.getElementById("root")!);

// PROTOTYPE: dev-only app shell variants, see src/prototype/app-shell.
if (import.meta.env.DEV && new URLSearchParams(window.location.search).get("prototype") === "app-shell") {
  void import("./prototype/app-shell/AppShell.prototype").then(({ default: Prototype }) =>
    root.render(
      <StrictMode>
        <Prototype />
      </StrictMode>,
    ),
  );
} else {
  root.render(
    <StrictMode>
      <App />
      <QuitDialog />
    </StrictMode>,
  );
}
