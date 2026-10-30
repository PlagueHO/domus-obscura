import { useState } from "react";
import type { CatalogProviderState } from "../../src/generated/CatalogProviderState";
import { ApiError } from "../../app/api";
import { Alert, Button, ProviderWaitNotice } from "../../app/components/ui";

/** Presentation fixtures only: no API, provider, approval or worker calls. */
export function ProviderTest() {
  const [state, setState] = useState<CatalogProviderState>("active");
  return (
    <main className="library-test">
      <header className="page-heading">
        <div>
          <h1>Provider waiting states</h1>
          <p>Generated presentation fixtures. No live providers or library are connected.</p>
          <a href="/">Return to the local test bench</a>
        </div>
      </header>
      <div className="actions">
        {(["active", "backoff", "paused", "saturated", "available"] as const).map((value) => (
          <Button
            key={value}
            variant={value === state ? "default" : "secondary"}
            onClick={() => setState(value)}
          >
            {value === "active"
              ? "Active request"
              : value === "backoff"
                ? "Timed backoff"
                : value === "paused"
                  ? "Manual pause"
                  : value === "saturated"
                    ? "Capacity full"
                    : "Budget available"}
          </Button>
        ))}
      </div>
      <ProviderWaitNotice
        status={{
          state,
          retry_after_seconds: state === "paused" ? 4294967295 : state === "backoff" ? 120 : 0,
          known_retry_after_seconds: state === "backoff" ? 120 : state === "available" ? 0 : null,
          failures: 0,
        }}
      />
      {state !== "available" && (
        <Alert
          error={
            new ApiError(
              503,
              "This metadata request could not finish.",
              state === "backoff" ? 120 : null,
              state,
            )
          }
        />
      )}
      <p>
        The real application offers manual metadata retry through the original action. A timer never
        starts another metadata request.
      </p>
    </main>
  );
}
