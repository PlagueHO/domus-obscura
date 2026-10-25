import { Component, type ErrorInfo, type ReactNode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import "./styles.css";

class AppBoundary extends Component<{ children: ReactNode }, { failed: boolean }> {
  state = { failed: false };
  static getDerivedStateFromError() {
    return { failed: true };
  }
  componentDidCatch(_error: Error, _info: ErrorInfo) {
    // Do not log provider details or private collection data to the browser.
  }
  render() {
    return this.state.failed ? (
      <main>
        <h1>The Music page could not open.</h1>
        <p>Reload the page to read the current server state. Commands are not retried.</p>
        <a href="/">Reload Domus Obscura</a>
      </main>
    ) : (
      this.props.children
    );
  }
}

const root = document.getElementById("root");
if (!root) throw new Error("The app root is missing.");
createRoot(root).render(
  <AppBoundary>
    <App />
  </AppBoundary>,
);
