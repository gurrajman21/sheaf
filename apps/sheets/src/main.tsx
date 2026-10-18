import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { isTauri } from "./api";
import "./styles.css";

// the desktop app draws its own title bar under the macOS window buttons
document.documentElement.dataset.shell = isTauri ? (navigator.userAgent.includes("Mac") ? "desktop-mac" : "desktop") : "browser";

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
