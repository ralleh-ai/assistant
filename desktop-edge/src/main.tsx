import React from "react";
import ReactDOM from "react-dom/client";
// Self-hosted fonts (L9): bundled with the app instead of fetched
// from the Google Fonts CDN at runtime. This removes a third-party
// network request on every launch and lets the CSP drop the
// `fonts.googleapis.com` / `fonts.gstatic.com` origins entirely.
// Latin subsets only (the shell ships `lang="en"`); Fraunces uses
// the variable opsz+wght axes so the 500/650 weights the design
// asks for render exactly as before (family "Fraunces Variable").
import "@fontsource/manrope/latin-400.css";
import "@fontsource/manrope/latin-600.css";
import "@fontsource/ibm-plex-mono/latin-400.css";
import "@fontsource/ibm-plex-mono/latin-500.css";
import "@fontsource-variable/fraunces/opsz.css";
import App from "./App";
import { ErrorBoundary } from "./ErrorBoundary";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ErrorBoundary>
      <App />
    </ErrorBoundary>
  </React.StrictMode>,
);
