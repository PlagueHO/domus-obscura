import { useState } from "react";
import type { Location } from "../src/generated/Location";
import type { ShareFolderInspection } from "../src/generated/ShareFolderInspection";
import type { ShareRoot } from "../src/generated/ShareRoot";
import { query, request } from "./api";
import { useCommand, useResource } from "./hooks";
import { Alert, Button, Check } from "./components/ui";

/** Binding is an explicit metadata command, separate from saving sharing policy. */
export function SharingFolders({
  roots,
  disabled,
  onChange,
}: {
  roots: ShareRoot[];
  disabled: boolean;
  onChange: (roots: ShareRoot[]) => void;
}) {
  const [locationId, setLocationId] = useState("");
  const [relativePath, setRelativePath] = useState("");
  const [inspection, setInspection] = useState<ShareFolderInspection>();
  const [after, setAfter] = useState(0);
  const [locationAfter, setLocationAfter] = useState(0);
  const command = useCommand();
  const locations = useResource(
    (signal) =>
      request<Location[]>(`/api/locations?${query({ after: locationAfter, limit: 100 })}`, {
        signal,
      }),
    `sharing-locations:${locationAfter}`,
  );
  const bindings = useResource(
    (signal) =>
      request<ShareFolderInspection[]>(`/api/sharing/bindings?${query({ after, limit: 50 })}`, {
        signal,
      }),
    `sharing-bindings:${after}`,
  );
  const availableLocations = locations.error || locations.loading ? [] : (locations.data ?? []);
  const availableBindings = bindings.error || bindings.loading ? [] : (bindings.data ?? []);
  const selectedLocation = availableLocations.find((value) => String(value.id) === locationId);
  const freshInspection =
    inspection?.location_id === selectedLocation?.id && inspection?.relative_path === relativePath
      ? inspection
      : undefined;
  const busy = disabled || command.busy;
  return (
    <div className="configuration-sharing-folders">
      <p>
        Select only registered library folders. The public alias hides the native path. Binding a
        folder does not enable sharing or change media files.
      </p>
      <Alert error={locations.error || bindings.error || command.error} />
      {roots.map((root, index) => {
        const binding = availableBindings.find((value) => value.binding_id === root.binding_id);
        return (
          <div className="configuration-secret" key={root.binding_id}>
            <label>
              Public alias for binding {root.binding_id}
              <input
                name={`production_worker.sharing.policy.roots.${index}.alias`}
                value={root.alias}
                maxLength={128}
                required
                spellCheck={false}
                onChange={(event) =>
                  onChange(
                    roots.map((value, i) =>
                      i === index ? { ...value, alias: event.target.value } : value,
                    ),
                  )
                }
              />
              <small>
                {binding?.bound
                  ? `Bound folder: ${binding.relative_path || "library root"}`
                  : "Binding not checked on this page. Refresh and inspect before restarting the worker."}
              </small>
            </label>
            <Button
              variant="ghost"
              disabled={busy}
              onClick={() =>
                onChange(roots.filter((value) => value.binding_id !== root.binding_id))
              }
            >
              Remove share folder
            </Button>
          </div>
        );
      })}
      <label>
        Registered library location
        <select
          value={locationId}
          disabled={busy || locations.loading || Boolean(locations.error)}
          onChange={(event) => {
            setLocationId(event.target.value);
            setInspection(undefined);
          }}
        >
          <option value="">Select a library location</option>
          {availableLocations.map((location) => (
            <option key={location.id} value={location.id}>
              {location.kind} · {location.root}
            </option>
          ))}
        </select>
        <small>Up to 100 registered locations per page. This is not a native path input.</small>
      </label>
      <div className="actions">
        <Button
          variant="ghost"
          disabled={busy || locations.loading || locationAfter === 0}
          onClick={() => {
            setLocationAfter(0);
            setLocationId("");
            setInspection(undefined);
          }}
        >
          First library locations
        </Button>
        <Button
          variant="ghost"
          disabled={busy || availableLocations.length !== 100}
          onClick={() => {
            setLocationAfter(availableLocations.at(-1)?.id ?? locationAfter);
            setLocationId("");
            setInspection(undefined);
          }}
        >
          Next library locations
        </Button>
      </div>
      <label>
        Library-relative subfolder
        <input
          value={relativePath}
          spellCheck={false}
          onChange={(event) => {
            setRelativePath(event.target.value);
            setInspection(undefined);
          }}
        />
        <small>Leave blank for the whole library root. Use forward slashes between folders.</small>
      </label>
      <div className="actions">
        <Button
          variant="secondary"
          disabled={busy || !selectedLocation}
          onClick={() =>
            void command.run(async () => {
              setInspection(undefined);
              const result = await request<ShareFolderInspection>(
                `/api/sharing/folders?${query({ location_id: selectedLocation!.id, relative_path: relativePath })}`,
              );
              setInspection(result);
            })
          }
        >
          Inspect folder
        </Button>
        <Button
          variant="secondary"
          disabled={busy || !freshInspection || freshInspection.bound}
          onClick={() =>
            void command.run(async () => {
              if (!freshInspection) return;
              setInspection(undefined);
              const result = await request<ShareFolderInspection>("/api/sharing/folders/bind", {
                body: {
                  location_id: freshInspection.location_id,
                  relative_path: freshInspection.relative_path,
                  expected_identity: freshInspection.native_identity,
                },
              });
              setInspection(result);
              bindings.refresh();
            })
          }
        >
          Bind inspected folder
        </Button>
        <Button
          variant="ghost"
          disabled={busy || locations.loading || bindings.loading}
          onClick={() => {
            locations.refresh();
            bindings.refresh();
            setInspection(undefined);
          }}
        >
          Refresh folders
        </Button>
      </div>
      {freshInspection && (
        <p role="status">
          {freshInspection.bound
            ? `Folder bound as ${freshInspection.binding_id}. Select it below or add it now.`
            : "Folder inspected. Bind this exact folder before selecting it."}
        </p>
      )}
      {freshInspection?.bound &&
        freshInspection.binding_id !== null &&
        !roots.some((root) => root.binding_id === freshInspection.binding_id) && (
          <Button
            variant="secondary"
            disabled={busy}
            onClick={() =>
              onChange([...roots, { binding_id: freshInspection.binding_id!, alias: "" }])
            }
          >
            Add inspected share folder
          </Button>
        )}
      {bindings.loading && <p role="status">Checking bound folders…</p>}
      {!bindings.loading && !bindings.error && availableBindings.length === 0 && (
        <p>No bound folders on this page. Inspect and bind a registered folder first.</p>
      )}
      {availableBindings.map((binding) => (
        <Check
          key={binding.binding_id}
          disabled={busy || !binding.bound || binding.binding_id === null}
          checked={roots.some((root) => root.binding_id === binding.binding_id)}
          onChange={(selected) => {
            if (binding.binding_id === null) return;
            onChange(
              selected
                ? [...roots, { binding_id: binding.binding_id, alias: "" }]
                : roots.filter((root) => root.binding_id !== binding.binding_id),
            );
          }}
        >
          Binding {binding.binding_id} · location {binding.location_id} ·{" "}
          {binding.relative_path || "library root"}
          {!binding.bound && " · unavailable"}
        </Check>
      ))}
      <div className="actions">
        <Button variant="ghost" disabled={busy || after === 0} onClick={() => setAfter(0)}>
          First bound folders
        </Button>
        <Button
          variant="ghost"
          disabled={busy || availableBindings.length !== 50}
          onClick={() => setAfter(availableBindings.at(-1)?.binding_id ?? after)}
        >
          Next bound folders
        </Button>
      </div>
    </div>
  );
}
