import { useEffect, useRef, useState } from "react";
import type { PrimaryLibraryChange } from "../src/generated/PrimaryLibraryChange";
import type { PrimaryLibraryControl } from "../src/generated/PrimaryLibraryControl";
import type { PrimaryLibraryPreview } from "../src/generated/PrimaryLibraryPreview";
import type { SelectPrimaryLibrary } from "../src/generated/SelectPrimaryLibrary";
import { request } from "./api";
import { useCommand, useResource } from "./hooks";
import { Alert, Button, Check, SettingsRow, SettingsSaveButton } from "./components/ui";
import { Icon } from "./components/Media";
import { FolderPicker } from "./FolderPicker";

export function useLibraryFolderControl() {
  return useResource(
    (signal) => request<PrimaryLibraryControl>("/api/library/primary/control", { signal }),
    "primary-library-control",
  );
}

type FolderChangeProps = {
  hasPrimary: boolean;
  disabled: boolean;
  onChanged: () => void;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
};

export function PrimaryLibrary(props: FolderChangeProps) {
  const control = useLibraryFolderControl();
  return <LibraryFolderChange {...props} control={control} />;
}

export function LibraryFolderChange({
  hasPrimary,
  disabled,
  onChanged,
  open: controlledOpen,
  onOpenChange,
  backup = false,
  control,
}: FolderChangeProps & { backup?: boolean; control: ReturnType<typeof useLibraryFolderControl> }) {
  const role = backup ? "backup library" : "primary library";
  const title = backup ? "Backup library" : "Primary library";
  const [path, setPath] = useState("");
  const [ownOpen, setOwnOpen] = useState(!hasPrimary);
  const external = controlledOpen !== undefined;
  const open = controlledOpen ?? ownOpen;
  const setOpen = onOpenChange ?? setOwnOpen;
  const [picking, setPicking] = useState(false);
  const [preview, setPreview] = useState<PrimaryLibraryPreview>();
  const [separate, setSeparate] = useState(false);
  const [change, setChange] = useState<PrimaryLibraryChange>();
  const [watchId, setWatchId] = useState<number>();
  const [watchError, setWatchError] = useState<unknown>();
  const command = useCommand();
  const changed = useRef(onChanged);
  changed.current = onChanged;
  const saved = change?.state === "approved" || change?.state === "activating";
  const pending = control.data?.pending_change_id;
  const otherPending = pending != null && Boolean(control.data?.pending_backup) !== backup;
  const pendingId = saved ? change.id : otherPending ? undefined : pending;
  const busy = disabled || command.busy || watchId !== undefined;
  const available = !control.loading && !control.error && control.data?.available;
  useEffect(() => {
    if (watchId === undefined) return;
    const abort = new AbortController();
    const deadline = Date.now() + 120_000;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const result = await request<PrimaryLibraryChange>(
          `/api/library/primary/changes/${watchId}`,
          { signal: AbortSignal.any([abort.signal, AbortSignal.timeout(5000)]) },
        );
        if (abort.signal.aborted) return;
        setChange(result);
        if (result.state === "active" || result.state === "failed") {
          setWatchId(undefined);
          setPreview(undefined);
          control.refresh();
          changed.current();
          return;
        }
      } catch (error) {
        if (abort.signal.aborted) return;
        if (Date.now() >= deadline) {
          setWatchError(error);
          setWatchId(undefined);
          return;
        }
      }
      if (Date.now() >= deadline) {
        setWatchError(
          new Error(
            "Activation is not confirmed. Check the saved change before restarting or submitting another location.",
          ),
        );
        setWatchId(undefined);
        return;
      }
      timer = setTimeout(() => void poll(), 1500);
    };
    void poll();
    return () => {
      abort.abort();
      clearTimeout(timer);
    };
  }, [watchId, control.refresh]);

  if (external && !open && pendingId == null && !change) return null;

  return (
    <section
      className="primary-library-change"
      aria-label={external ? (backup ? "Add backup library" : "Change primary library") : undefined}
      aria-labelledby={external ? undefined : "primary-library-title"}
    >
      {!external && (
        <div className="setting-row">
          <div>
            <h3 id="primary-library-title">
              {hasPrimary ? "Change primary library" : "Set primary library"}
            </h3>
            <p className="muted">Restarts the API and worker. No media is moved.</p>
          </div>
          {hasPrimary && pendingId == null && (
            <Button variant="secondary" disabled={busy} onClick={() => setOpen(!open)}>
              {open ? "Close" : "Change…"}
            </Button>
          )}
        </div>
      )}
      {(open || pendingId != null || change) && (
        <Alert error={control.error || command.error || watchError} />
      )}
      {open && control.loading && <p role="status">Checking folder controls…</p>}
      {open && !control.loading && control.data?.reason && (
        <p role="status">{control.data.reason}</p>
      )}
      {open && otherPending && (
        <p role="status">Finish or cancel the saved change in the other library section first.</p>
      )}
      {pendingId != null && !change && (
        <p role="status">
          {title} change {pendingId} has not finished.
        </p>
      )}
      {change && (
        <div role="status" className="primary-library-result">
          <strong>
            {change.state === "active"
              ? backup
                ? "Backup library added"
                : "Primary library changed"
              : change.state === "failed"
                ? `${title} change failed`
                : change.state === "activating"
                  ? `Applying ${role} change`
                  : `${title} change saved, not yet active`}
          </strong>
          <p className="library-folder-path">{change.path}</p>
          {change.last_error && <p className="warning">{change.last_error}</p>}
          {change.state === "active" && (
            <p className="muted">Run a library scan to update the index.</p>
          )}
        </div>
      )}
      {watchId !== undefined && <p role="status">Waiting for the API and worker to restart…</p>}
      {pendingId != null && watchId === undefined && (
        <div className="actions">
          <Button
            variant="secondary"
            disabled={busy}
            onClick={() =>
              void command.run(async () => {
                setWatchError(undefined);
                const saved = await request<PrimaryLibraryChange>(
                  `/api/library/primary/changes/${pendingId}`,
                );
                setChange(saved);
                if (saved.state === "activating") setWatchId(pendingId);
                control.refresh();
                changed.current();
              })
            }
          >
            Check saved change
          </Button>
          {change?.state === "approved" && (
            <>
              <Button
                disabled={busy || !available}
                onClick={() =>
                  void command.run(async () => {
                    setWatchError(undefined);
                    try {
                      await request<void>(`/api/library/primary/changes/${change.id}/activate`, {
                        body: {},
                      });
                    } finally {
                      setWatchId(change.id);
                    }
                  })
                }
              >
                Restart and apply {role}
              </Button>
              <Button
                variant="secondary"
                disabled={busy}
                onClick={() =>
                  void command.run(async () => {
                    const result = await request<PrimaryLibraryChange>(
                      `/api/library/primary/changes/${change.id}/cancel`,
                      { body: {} },
                    );
                    setChange(result);
                    control.refresh();
                  })
                }
              >
                Cancel saved change
              </Button>
            </>
          )}
        </div>
      )}
      {pendingId == null && !otherPending && open && (
        <form
          className="primary-library-form"
          onSubmit={(event) => {
            event.preventDefault();
            if (busy || !available) return;
            void command.run(async () => {
              setPreview(undefined);
              setSeparate(false);
              setChange(undefined);
              const result = await request<PrimaryLibraryPreview>("/api/library/primary/inspect", {
                body: backup ? { path, backup } : { path },
              });
              setPreview(result);
            });
          }}
        >
          <SettingsRow
            label={`${title} folder`}
            hint="Enter a path on the host or browse for a folder."
            wide
          >
            {(control) => (
              <div className="path-field">
                <input
                  {...control}
                  value={path}
                  required
                  maxLength={4096}
                  disabled={busy || !available}
                  placeholder="D:\Music or \\server\share\Music"
                  onChange={(event) => {
                    setPath(event.target.value);
                    setPreview(undefined);
                  }}
                />
                <Button
                  type="button"
                  variant="secondary"
                  width={external ? "small" : undefined}
                  icon={external ? <Icon name="folder" /> : undefined}
                  disabled={busy || !available}
                  onClick={() => setPicking(true)}
                >
                  Browse…
                </Button>
                <Button
                  type="submit"
                  disabled={busy || !available || !path}
                  width={external ? "small" : undefined}
                  icon={external ? <Icon name="search" /> : undefined}
                  busy={external && command.busy}
                >
                  {!external && command.busy ? "Checking…" : "Check"}
                </Button>
              </div>
            )}
          </SettingsRow>
          {preview && (
            <div className="primary-library-review" role="status">
              <span>
                <strong>Ready:</strong> <span className="library-folder-path">{preview.path}</span>
              </span>
              {backup && (
                <>
                  <dl>
                    {preview.storage_layout.map(({ role, path }) => (
                      <div key={`${role}:${path}`}>
                        <dt>{role}</dt>
                        <dd className="library-folder-path">{path}</dd>
                      </div>
                    ))}
                  </dl>
                  <Check checked={separate} onChange={setSeparate} disabled={busy || !available}>
                    I confirm that this folder is separate from all other folders shown above.
                  </Check>
                </>
              )}
              <SettingsSaveButton
                type="button"
                disabled={busy || !available || (backup && !separate)}
                busy={command.busy}
                onClick={() =>
                  void command.run(async () => {
                    const body: SelectPrimaryLibrary = {
                      path: preview.path,
                      expected_revision: preview.expected_revision,
                      previous_location_id: preview.previous_location_id,
                      review_token: preview.review_token,
                      confirm_separation: true,
                      ...(backup ? { backup } : {}),
                    };
                    try {
                      const result = await request<PrimaryLibraryChange>(
                        "/api/library/primary/changes",
                        { body },
                      );
                      setChange(result);
                      setPreview(undefined);
                    } finally {
                      control.refresh();
                    }
                  })
                }
              >
                Save {role} change
              </SettingsSaveButton>
              {external && (
                <span className="muted">Applying the change restarts the API and worker.</span>
              )}
            </div>
          )}
        </form>
      )}
      {picking && (
        <FolderPicker
          initialPath={path}
          onClose={() => setPicking(false)}
          onSelect={(selected) => {
            setPath(selected);
            setPreview(undefined);
            setPicking(false);
          }}
        />
      )}
    </section>
  );
}
