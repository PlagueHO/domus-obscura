import { useEffect, useId, useRef, useState } from "react";
import type { SoulseekAccountStatus } from "../src/generated/SoulseekAccountStatus";
import type { SoulseekAccountChange } from "../src/generated/SoulseekAccountChange";
import type { SaveSoulseekAccount } from "../src/generated/SaveSoulseekAccount";
import { ApiError, request } from "./api";
import { useCommand, useResource } from "./hooks";
import { Alert, Button, SettingsRow, SettingsSaveButton } from "./components/ui";

const endpoint = "/api/system/soulseek-account";

export function SoulseekAccount({
  disabled,
  onChanged,
}: {
  disabled: boolean;
  onChanged: () => void;
}) {
  const formId = useId();
  const status = useResource(
    (signal) => request<SoulseekAccountStatus>(endpoint, { signal }),
    "soulseek-account",
  );
  const [username, setUsername] = useState("");
  const [password, setPassword] = useState("");
  const [change, setChange] = useState<SoulseekAccountChange>();
  const [watchId, setWatchId] = useState<number>();
  const [watchError, setWatchError] = useState<unknown>();
  const [unknownSave, setUnknownSave] = useState(false);
  const command = useCommand();
  const changed = useRef(onChanged);
  changed.current = onChanged;
  const pendingId =
    change && ["preparing", "approved", "activating"].includes(change.state)
      ? change.id
      : status.data?.pending_change_id;
  const available = !disabled && !status.loading && !status.error && status.data?.available;
  const busy = command.busy || watchId !== undefined;
  const canSave = available && !busy && Boolean(username && password);
  const canCheckStatus = pendingId != null || unknownSave || Boolean(watchError || status.error);

  useEffect(() => {
    if (watchId === undefined) return;
    const abort = new AbortController();
    const deadline = Date.now() + 120_000;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const result = await request<SoulseekAccountChange>(`${endpoint}/changes/${watchId}`, {
          signal: AbortSignal.any([abort.signal, AbortSignal.timeout(5000)]),
        });
        if (abort.signal.aborted) return;
        setChange(result);
        if (result.state === "active" || result.state === "failed") {
          setWatchId(undefined);
          status.refresh();
          changed.current();
          return;
        }
      } catch (error) {
        if (abort.signal.aborted) return;
        if (error instanceof ApiError && [401, 403, 404].includes(error.status)) {
          setWatchError(error);
          setWatchId(undefined);
          return;
        }
        if (Date.now() >= deadline) {
          setWatchError(error);
          setWatchId(undefined);
          return;
        }
      }
      if (Date.now() >= deadline) {
        setWatchError(
          new Error(
            "Account activation is not confirmed. Check the saved change before restarting or saving another account.",
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
  }, [watchId, status.refresh]);

  return (
    <section className="soulseek-account" aria-label="Soulseek account credentials">
      <Alert error={status.error || watchError || (change?.state !== "active" && command.error)} />
      {status.loading && <p role="status">Checking account settings…</p>}
      {!status.loading && status.data?.reason && <p role="status">{status.data.reason}</p>}
      {!status.loading && !status.error && status.data && (
        <p role="status">
          {status.data.configured
            ? "An account is configured on this host. Connection not tested."
            : "No Soulseek account is configured."}
        </p>
      )}
      {unknownSave && (
        <p role="status">
          The save result is not confirmed. Check account status before submitting again.
        </p>
      )}
      {change && (
        <div role="status">
          <strong>
            {change.state === "active"
              ? "Account applied. No login was started."
              : change.state === "failed"
                ? "Account change failed or was cancelled"
                : change.state === "activating"
                  ? "Applying saved account"
                  : change.state === "preparing"
                    ? "Account save is incomplete"
                    : "Account saved, not yet active"}
          </strong>
          {change.last_error && <p className="warning">{change.last_error}</p>}
        </div>
      )}
      {watchId !== undefined && <p role="status">Waiting for the API and worker to restart…</p>}
      {canCheckStatus && (
        <div className="actions">
          <Button
            variant="secondary"
            disabled={busy}
            onClick={() =>
              void command.run(async () => {
                const current = await request<SoulseekAccountStatus>(endpoint);
                const id = current.pending_change_id ?? change?.id;
                if (id != null) {
                  const saved = await request<SoulseekAccountChange>(`${endpoint}/changes/${id}`);
                  setChange(saved);
                  if (saved.state === "activating") setWatchId(saved.id);
                }
                setUnknownSave(false);
                setWatchError(undefined);
                status.refresh();
                changed.current();
              })
            }
          >
            Check account status
          </Button>
          {pendingId != null && change?.state === "approved" && (
            <Button
              disabled={busy || !available}
              onClick={() =>
                void command.run(async () => {
                  setWatchError(undefined);
                  try {
                    await request<void>(`${endpoint}/changes/${change.id}/activate`, { body: {} });
                  } finally {
                    setWatchId(change.id);
                  }
                })
              }
            >
              Restart and apply account
            </Button>
          )}
          {pendingId != null && (change?.state === "approved" || change?.state === "preparing") && (
            <Button
              variant="secondary"
              disabled={busy || disabled || status.loading || Boolean(status.error)}
              onClick={() =>
                void command.run(async () => {
                  setChange(
                    await request<SoulseekAccountChange>(
                      `${endpoint}/changes/${pendingId}/cancel`,
                      {
                        body: {},
                      },
                    ),
                  );
                  status.refresh();
                })
              }
            >
              Cancel saved account
            </Button>
          )}
        </div>
      )}
      {pendingId != null && !change && (
        <p role="status">An account change is pending. Check account status to review it.</p>
      )}
      {pendingId == null && !unknownSave && (
        <form
          id={formId}
          autoComplete="off"
          onSubmit={(event) => {
            event.preventDefault();
            if (!canSave || !status.data) return;
            const body: SaveSoulseekAccount = {
              expected_revision: status.data.active_revision,
              username,
              password,
            };
            setPassword("");
            void command.run(async () => {
              setChange(undefined);
              try {
                setChange(await request<SoulseekAccountChange>(`${endpoint}/changes`, { body }));
              } catch (error) {
                setUnknownSave(true);
                throw error;
              } finally {
                body.password = "";
                status.refresh();
              }
            });
          }}
        >
          <SettingsRow label="Soulseek username">
            {(control) => (
              <input
                {...control}
                name="soulseek-username"
                value={username}
                required
                maxLength={30}
                autoComplete="username"
                autoCapitalize="none"
                spellCheck={false}
                disabled={busy || !available}
                onChange={(event) => setUsername(event.target.value)}
              />
            )}
          </SettingsRow>
          <SettingsRow label="Soulseek password">
            {(control) => (
              <input
                {...control}
                name="soulseek-password"
                type="password"
                value={password}
                required
                maxLength={128}
                autoComplete="new-password"
                disabled={busy || !available}
                onChange={(event) => setPassword(event.target.value)}
              />
            )}
          </SettingsRow>
          <p>Saving stores the account on this host. It does not sign in or enable downloads.</p>
          <SettingsSaveButton form={formId} type="submit" disabled={!canSave} busy={command.busy}>
            Save account
          </SettingsSaveButton>
        </form>
      )}
    </section>
  );
}
