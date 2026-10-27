import { useState } from "react";
import type { WorkerStatus } from "../src/generated/WorkerStatus";
import type { WorkerRequest } from "../src/generated/WorkerRequest";
import { request } from "./api";
import { useCommand, useResource } from "./hooks";
import { Alert, Button } from "./components/ui";

export function WorkerRequests({ status, job }: { status: WorkerStatus; job?: number }) {
  const [pages, setPages] = useState([0]);
  const after = pages.at(-1) ?? 0;
  const command = useCommand();
  const requests = useResource(
    (signal) =>
      request<WorkerRequest[]>(
        `/api/worker/requests?unfinished_only=true&after=${after}&limit=50${job === undefined ? "" : `&job_id=${job}`}`,
        { signal },
      ),
    `${after}:${job ?? "all"}`,
    2000,
  );
  return (
    <section className="review-section" aria-label="Production worker requests">
      <h2>Production worker</h2>
      <p>Unfinished requests</p>
      <p role="status">
        {status.stopping
          ? "Worker stopping; active work is draining."
          : status.online
            ? "Worker online."
            : "Worker offline; accepted requests stay queued."}{" "}
        {status.unfinished} of {status.capacity} queue places used. {status.active} operations
        active.
      </p>
      <details>
        <summary>Active worker configuration</summary>
        <p>{status.active_revision ?? "No worker has started."}</p>
        <p>Saving configuration or reloading the API does not restart the worker.</p>
      </details>
      <Alert error={requests.error || command.error} />
      {Boolean(requests.error) && (
        <p className="warning">
          Request data is stale because refresh failed. Commands are disabled.
        </p>
      )}
      {requests.data?.map((item) => (
        <div key={item.id}>
          <p>
            Request {item.id}: {item.operation.operation} · {item.state}
            {"location_id" in item.operation
              ? ` · location ${item.operation.location_id}`
              : ` · album ${item.operation.job_id}, plan ${item.operation.plan_version}`}
          </p>
          {item.last_error && <p className="warning">{item.last_error}</p>}
          {["waiting", "failed"].includes(item.state) && (
            <div className="actions">
              <Button
                variant="secondary"
                disabled={command.busy || requests.loading || Boolean(requests.error)}
                onClick={() =>
                  void command.run(async () => {
                    await request(`/api/worker/requests/${item.id}/retry`, { body: {} });
                    requests.refresh();
                  })
                }
              >
                Retry this saved request
              </Button>
              <Button
                variant="ghost"
                disabled={command.busy || requests.loading || Boolean(requests.error)}
                onClick={() =>
                  void command.run(async () => {
                    await request(`/api/worker/requests/${item.id}/dismiss`, { body: {} });
                    requests.refresh();
                  })
                }
              >
                Dismiss worker request
              </Button>
            </div>
          )}
        </div>
      ))}
      <p>
        Resolve saved decisions before retrying. Dismissal releases queue capacity; it does not
        cancel the album or delete files.
      </p>
      <div className="actions">
        <Button
          variant="secondary"
          disabled={requests.loading || pages.length === 1}
          onClick={() => setPages(pages.slice(0, -1))}
        >
          Previous worker requests
        </Button>
        <Button
          variant="secondary"
          disabled={requests.loading || requests.data?.length !== 50}
          onClick={() => setPages([...pages, requests.data?.at(-1)?.id ?? after])}
        >
          Next worker requests
        </Button>
      </div>
    </section>
  );
}
