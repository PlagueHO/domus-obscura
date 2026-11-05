import { createContext, useContext, useEffect, useRef, useState, type ReactNode } from "react";
import type { CatalogReleaseSummary } from "../src/generated/CatalogReleaseSummary";
import type { CatalogRelease } from "../src/generated/CatalogRelease";
import type { CatalogResult } from "../src/generated/CatalogResult";
import type { CatalogBatchPlanRequest } from "../src/generated/CatalogBatchPlanRequest";
import type { CreatedJob } from "../src/generated/CreatedJob";
import type { StoredPlan } from "../src/generated/StoredPlan";
import type { SettingsVersion } from "../src/generated/SettingsVersion";
import type { AudioProfile } from "../src/generated/AudioProfile";
import type { Location } from "../src/generated/Location";
import type { JobSummary } from "../src/generated/JobSummary";
import { request } from "./api";
import { useCommand } from "./hooks";
import { Alert, Button, Check } from "./components/ui";
import { Icon } from "./components/Media";
import { audioProfileLabel, AudioProfileSelect } from "./audio-profiles";

export const selectionLimit = 25;
const steps = ["Choose", "Prepare", "Approve each", "Start"];

export type SelectedAlbum = {
  release: CatalogReleaseSummary;
  profile?: AudioProfile;
  job?: CreatedJob;
  plan?: StoredPlan;
  creationUnknown?: boolean;
  submitted?: boolean;
};

export function toggleAlbum(items: SelectedAlbum[], release: CatalogReleaseSummary) {
  if (items.some((item) => item.release.id === release.id))
    return items.filter((item) => item.release.id !== release.id);
  if (items.length >= selectionLimit) throw new Error("Select at most 25 exact album editions.");
  return [...items, { release }];
}

type Selection = {
  items: SelectedAlbum[];
  busy: boolean;
  locked: boolean;
  setBusy: (busy: boolean) => void;
  setLocked: (locked: boolean) => void;
  toggle: (release: CatalogReleaseSummary) => void;
  update: (id: string, change: Partial<SelectedAlbum>) => void;
};

const SelectionContext = createContext<Selection | null>(null);

export function AlbumSelectionProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<SelectedAlbum[]>([]);
  const [busy, setBusy] = useState(false);
  const [locked, setLocked] = useState(false);
  const value: Selection = {
    items,
    busy,
    locked,
    setBusy,
    setLocked,
    toggle: (release) => setItems((current) => toggleAlbum(current, release)),
    update: (id, change) =>
      setItems((current) =>
        current.map((item) => (item.release.id === id ? { ...item, ...change } : item)),
      ),
  };
  return <SelectionContext value={value}>{children}</SelectionContext>;
}

export function useAlbumSelection() {
  return useContext(SelectionContext);
}

export function SelectAlbum({ release }: { release: CatalogReleaseSummary }) {
  const selection = useAlbumSelection();
  if (!selection) return null;
  const selected = selection.items.some((item) => item.release.id === release.id);
  return (
    <Button
      variant="secondary"
      aria-pressed={selected}
      aria-label={`${selected ? "Remove" : "Select"} ${release.title}`}
      disabled={
        selection.busy ||
        selection.locked ||
        (!selected && selection.items.length >= selectionLimit)
      }
      onClick={() => selection.toggle(release)}
    >
      {selected ? "Selected" : "Select album"}
    </Button>
  );
}

export function selectionRequest(
  items: SelectedAlbum[],
  settings: SettingsVersion,
  locations: number[],
  coverage: boolean,
): CatalogBatchPlanRequest {
  if (!items.length || items.length > selectionLimit || !locations.length)
    throw new Error("Select 1–25 editions and at least one destination.");
  return {
    albums: items.map((item) => {
      const profile = item.profile ?? settings.policy.preferred;
      if (!item.job || item.creationUnknown || !settings.policy.accepted.includes(profile))
        throw new Error("Review the saved job and accepted audio profile for every album.");
      return {
        release_id: item.release.id,
        request: {
          job_id: item.job.job_id,
          expected_version: item.job.plan_version,
          settings_version: settings.version,
          format:
            settings.audio_profiles.find((entry) => entry.profile === profile)?.format ?? null,
          audio_profile: profile,
          locations,
          accepts_incomplete_coverage: coverage,
          bucket: null,
        },
      };
    }),
  };
}

export function preparedSelectionMatches(
  items: SelectedAlbum[],
  plans: StoredPlan[],
  expected?: CatalogBatchPlanRequest,
) {
  return (
    items.length > 0 &&
    new Set(items.map((item) => item.release.id)).size === items.length &&
    new Set(items.map((item) => item.job?.job_id)).size === items.length &&
    new Set(plans.map((plan) => plan.job_id)).size === plans.length &&
    plans.length === items.length &&
    items.every((item) =>
      plans.some(
        (plan) =>
          plan.job_id === item.job?.job_id &&
          plan.version === item.job.plan_version + 1 &&
          plan.plan.edition.release_id === item.release.id &&
          (!expected ||
            expected.albums.some(
              ({ release_id, request: selection }) =>
                release_id === item.release.id &&
                plan.settings_version === selection.settings_version &&
                plan.plan.audio?.profile === selection.audio_profile &&
                plan.plan.accepts_incomplete_coverage === selection.accepts_incomplete_coverage &&
                plan.plan.destinations.length === selection.locations.length &&
                plan.plan.destinations.every((destination) =>
                  selection.locations.includes(destination.location_id),
                ),
            )),
      ),
    )
  );
}

export function AlbumSelection({
  settings,
  locations,
  blocked,
  onPrepared,
}: {
  settings: SettingsVersion | null | undefined;
  locations: Location[];
  blocked: boolean;
  onPrepared: (job: number, version: number) => void;
}) {
  const selection = useAlbumSelection();
  const command = useCommand();
  const [destinations, setDestinations] = useState<number[]>([]);
  const [coverage, setCoverage] = useState(false);
  const [open, setOpen] = useState(false);
  const [batchMismatch, setBatchMismatch] = useState(false);
  const uncertain = useRef<CatalogBatchPlanRequest | null>(null);
  useEffect(() => {
    const close = () => setOpen(false);
    window.addEventListener("hashchange", close);
    return () => window.removeEventListener("hashchange", close);
  }, []);
  if (!selection || !selection.items.length) return null;
  const { items, update, setBusy, setLocked } = selection;
  const prepared = items.every((item) => item.plan);
  const step = items.every((item) => item.submitted) ? 3 : prepared ? 2 : 1;
  async function prepare() {
    if (!settings || !selection || blocked) throw new Error("Reload system settings first.");
    setBusy(true);
    try {
      const pending = items.filter((item) => !item.plan).map((item) => ({ ...item }));
      if (pending.some((item) => item.creationUnknown))
        throw new Error(
          "A job creation response was lost. Inspect the queue before removing that selection. No job creation is retried.",
        );
      if (uncertain.current) {
        setBatchMismatch(false);
        const plans: StoredPlan[] = [];
        for (const item of pending) {
          if (!item.job) throw new Error("The batch has an unknown job. Inspect the queue.");
          const rows = await request<JobSummary[]>(
            `/api/jobs?after=${item.job.job_id - 1}&limit=1`,
          );
          const job = rows.find((row) => row.id === item.job?.job_id);
          if (!job) {
            setBatchMismatch(true);
            throw new Error("A selected job is missing. Inspect the queue.");
          }
          if (job.plan_version !== item.job.plan_version) {
            plans.push(await request<StoredPlan>(`/api/jobs/${job.id}/plans/${job.plan_version}`));
          }
        }
        if (plans.length) {
          if (!preparedSelectionMatches(pending, plans, uncertain.current)) {
            setBatchMismatch(true);
            throw new Error(
              "Saved plans changed. Inspect each selected request before continuing.",
            );
          }
          for (const item of pending)
            update(item.release.id, {
              plan: plans.find((plan) => plan.job_id === item.job?.job_id)!,
            });
          uncertain.current = null;
          setLocked(false);
          return;
        }
        uncertain.current = null;
        setLocked(false);
        throw new Error(
          "The saved jobs have no new plans. Review the selection, then explicitly prepare it again.",
        );
      }
      for (const item of pending) {
        const release = await request<CatalogResult<CatalogRelease>>(
          `/api/discovery/releases/${encodeURIComponent(item.release.id)}`,
        );
        if (release.data.summary.id !== item.release.id)
          throw new Error("The provider returned a different exact release.");
      }
      for (const item of pending) {
        if (!item.job) {
          try {
            item.job = await request<CreatedJob>("/api/jobs", { body: {} });
            if (
              !Number.isSafeInteger(item.job.job_id) ||
              item.job.job_id <= 0 ||
              !Number.isSafeInteger(item.job.plan_version) ||
              item.job.plan_version <= 0
            )
              throw new Error(
                "The new job response has no valid identity. Inspect the saved queue.",
              );
            update(item.release.id, { job: item.job });
          } catch (error) {
            update(item.release.id, { creationUnknown: true });
            throw error;
          }
        }
      }
      const body = selectionRequest(pending, settings, destinations, coverage);
      uncertain.current = body;
      setLocked(true);
      const plans = await request<StoredPlan[]>("/api/discovery/plans", { body });
      if (!preparedSelectionMatches(pending, plans, body))
        throw new Error(
          "The batch response does not match the selected editions. Inspect saved jobs.",
        );
      for (const item of pending)
        update(item.release.id, { plan: plans.find((plan) => plan.job_id === item.job?.job_id)! });
      uncertain.current = null;
      setLocked(false);
      const first = plans[0]!;
      onPrepared(first.job_id, first.version);
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="album-selection" aria-labelledby="selected-albums-title">
      <div className="tray-spacer" aria-hidden="true" />
      <div className="tray" data-open={open}>
        <div className="tray-bar">
          <h2 id="selected-albums-title">
            Selected albums · {items.length} / {selectionLimit}
          </h2>
          <ol className="step-rail" aria-label="Request steps">
            {steps.map((label, index) => (
              <li
                key={label}
                data-state={index < step ? "done" : index === step ? "current" : "next"}
                aria-current={index === step ? "step" : undefined}
              >
                <span aria-hidden="true">{index < step ? "✓" : index + 1}</span>
                {label}
              </li>
            ))}
          </ol>
          <Button
            variant={open ? "secondary" : "default"}
            aria-expanded={open}
            aria-controls="selection-drawer"
            onClick={() => setOpen(!open)}
          >
            {open ? "Hide selection" : "Open selection"}
          </Button>
        </div>
        <div className="tray-drawer" id="selection-drawer" hidden={!open}>
          <p className="muted">Each album keeps its own approvals.</p>
          <ul className="selected-album-list">
            {items.map((item) => (
              <li key={item.release.id}>
                <div>
                  <a href={`#/album/${encodeURIComponent(item.release.id)}`}>
                    {item.release.title}
                  </a>
                  <small>
                    {[
                      item.release.artist,
                      item.release.date,
                      item.release.country,
                      item.release.disambiguation,
                    ]
                      .filter(Boolean)
                      .join(" · ")}
                  </small>
                  {item.job && (
                    <a
                      href={`#/request/${item.job.job_id}/${item.plan?.version ?? item.job.plan_version}`}
                    >
                      Request {item.job.job_id}
                      {item.submitted ? " · Start submitted" : ""}
                    </a>
                  )}
                  {item.creationUnknown && (
                    <p className="warning">
                      Job creation result unknown. Inspect the queue. This item cannot create
                      another job.
                    </p>
                  )}
                </div>
                {settings && !item.plan && (
                  <label>
                    Audio profile for {item.release.title}
                    <AudioProfileSelect
                      value={item.profile ?? settings.policy.preferred}
                      accepted={settings.policy.accepted}
                      descriptors={settings.audio_profiles}
                      disabled={selection.busy || selection.locked}
                      onChange={(profile) => update(item.release.id, { profile })}
                    />
                  </label>
                )}
                {item.plan && (
                  <a
                    className="button button-secondary"
                    href={`#/request/${item.plan.job_id}/${item.plan.version}`}
                  >
                    Review {item.release.title}
                  </a>
                )}
                {item.plan && (
                  <small>
                    Frozen audio profile ·{" "}
                    {item.plan.plan.audio
                      ? audioProfileLabel(
                          item.plan.plan.audio.profile,
                          settings?.audio_profiles ?? [],
                        )
                      : "Legacy MP3 · 320 kbps CBR"}
                  </small>
                )}
                <Button
                  variant="secondary"
                  className="icon-button"
                  aria-label={`Remove ${item.release.title}`}
                  title={`Remove ${item.release.title}`}
                  disabled={selection.busy || selection.locked}
                  onClick={() => selection.toggle(item.release)}
                >
                  <Icon name="close" />
                </Button>
              </li>
            ))}
          </ul>
          {!prepared && (
            <>
              <fieldset
                className="destination-tiles"
                disabled={selection.busy || selection.locked || blocked}
              >
                <legend>Save to</legend>
                {locations.map((location) => (
                  <label
                    key={location.id}
                    className="destination-tile"
                    title={location.root}
                    data-checked={destinations.includes(location.id)}
                  >
                    <input
                      type="checkbox"
                      checked={destinations.includes(location.id)}
                      onChange={(event) =>
                        setDestinations((current) =>
                          event.target.checked
                            ? [...current, location.id]
                            : current.filter((id) => id !== location.id),
                        )
                      }
                    />
                    <strong>{location.kind}</strong>
                    <small data-coverage={location.coverage}>
                      {location.coverage === "complete"
                        ? "Index complete"
                        : `Index ${location.coverage}`}
                      {location.last_error ? ` · ${location.last_error}` : ""}
                    </small>
                  </label>
                ))}
              </fieldset>
              {locations.some((location) => location.coverage !== "complete") && (
                <Check
                  checked={coverage}
                  onChange={setCoverage}
                  disabled={selection.busy || selection.locked || blocked}
                >
                  I accept incomplete index coverage. Unknown coverage does not prove absence.
                </Check>
              )}
              <p className="muted">Saves plans only. Nothing is approved or downloaded yet.</p>
              <div className="tray-actions">
                <Button
                  disabled={
                    selection.busy ||
                    blocked ||
                    !settings ||
                    !destinations.length ||
                    items.some((item) => Boolean(item.creationUnknown))
                  }
                  onClick={() => void command.run(prepare)}
                >
                  {command.busy
                    ? "Preparing selected albums…"
                    : uncertain.current
                      ? "Check saved batch result"
                      : "Prepare selected albums"}
                </Button>
                <a href="#/queue">Inspect the saved queue</a>
              </div>
              {batchMismatch && (
                <div>
                  <p>
                    After inspecting the saved requests, remove this batch from the selection to
                    unlock the tray. Saved jobs stay unchanged. No jobs are created or retried.
                  </p>
                  <Button
                    variant="secondary"
                    disabled={selection.busy}
                    onClick={() =>
                      void command.run(async () => {
                        const batch = uncertain.current;
                        if (!batch) return;
                        for (const item of items)
                          if (
                            batch.albums.some((album) => album.request.job_id === item.job?.job_id)
                          )
                            selection.toggle(item.release);
                        uncertain.current = null;
                        setBatchMismatch(false);
                        setLocked(false);
                      })
                    }
                  >
                    Remove inspected batch from selection
                  </Button>
                </div>
              )}
            </>
          )}
          <Alert error={command.error} />
          {prepared && <a href="#/queue">Inspect the saved queue</a>}
        </div>
      </div>
    </section>
  );
}
