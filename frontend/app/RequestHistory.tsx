import { useState } from "react";
import type { JobEvent } from "../src/generated/JobEvent";
import { request } from "./api";
import { useResource } from "./hooks";
import { Alert, Button, Empty, Pager } from "./components/ui";

export function RequestEvents({ events }: { events: JobEvent[] }) {
  return (
    <ol className="request-events">
      {events.map((event) => (
        <li key={event.id}>
          <strong>{event.event_type.replaceAll("_", " ")}</strong>
          <time dateTime={new Date(event.created_at * 1000).toISOString()}>
            {new Date(event.created_at * 1000).toLocaleString()}
          </time>
          <small>
            Event {event.id} · Plan {event.plan_version}
          </small>
        </li>
      ))}
    </ol>
  );
}

export function RequestHistory({
  base,
  revision,
  active,
}: {
  base: string;
  revision: number;
  active: boolean;
}) {
  const [pages, setPages] = useState([0]);
  const after = pages.at(-1) ?? 0;
  const events = useResource(
    (signal) => request<JobEvent[]>(`${base}/events?after=${after}&limit=50`, { signal }),
    `${base}:${after}:${revision}`,
    active ? 2000 : 0,
  );
  return (
    <details className="review-section edition-details">
      <summary>Saved request history</summary>
      <p className="muted">
        Events bound to this exact plan, oldest first. Events without a saved plan version are not
        assigned to this plan. This history is read-only. It does not resume or retry work.
      </p>
      <Alert error={events.error} />
      {events.error && events.data !== undefined ? (
        <p className="warning">
          The history read failed. Retained rows are not a current response.
        </p>
      ) : null}
      {events.loading && <p role="status">Reading saved events…</p>}
      {events.data && <RequestEvents events={events.data} />}
      {!events.loading && !events.error && events.data?.length === 0 && (
        <Empty>No events on this page are bound to the selected plan.</Empty>
      )}
      <div className="actions">
        <Button variant="secondary" disabled={events.loading} onClick={events.refresh}>
          Reload history
        </Button>
        <Pager
          busy={events.loading || Boolean(events.error)}
          previous={pages.length > 1 ? () => setPages(pages.slice(0, -1)) : undefined}
          next={
            events.data?.length === 50
              ? () => setPages([...pages, events.data?.at(-1)?.id ?? after])
              : undefined
          }
        />
      </div>
    </details>
  );
}
