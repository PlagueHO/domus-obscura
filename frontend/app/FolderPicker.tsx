import { useEffect, useRef, useState } from "react";
import { hotkeysCoreFeature, selectionFeature, syncDataLoaderFeature } from "@headless-tree/core";
import { useTree } from "@headless-tree/react";
import type { HostFolderEntry } from "../src/generated/HostFolderEntry";
import type { HostFolderPage } from "../src/generated/HostFolderPage";
import { request } from "./api";
import { useCommand, useResource } from "./hooks";
import { Alert, Button } from "./components/ui";

const maximumEntries = 2048;

function FolderList({
  page,
  disabled,
  onOpen,
}: {
  page: HostFolderPage;
  disabled: boolean;
  onOpen: (folder: HostFolderEntry) => void;
}) {
  const [selected, setSelected] = useState<string[]>([]);
  const tree = useTree<HostFolderEntry>({
    rootItemId: "root",
    getItemName: (item) => item.getItemData().name,
    isItemFolder: () => false,
    state: { selectedItems: selected },
    setSelectedItems: (items) =>
      setSelected((current) => (typeof items === "function" ? items(current) : items).slice(-1)),
    onPrimaryAction: (item) => {
      if (!disabled && item.getItemData().identity) onOpen(item.getItemData());
    },
    dataLoader: {
      getItem: (id) => {
        if (id === "root")
          return {
            name: page.path,
            path: page.path,
            identity: page.identity,
            unavailable_reason: null,
          };
        const folder = page.entries.find((entry) => entry.path === id);
        if (!folder) throw new Error("The folder list changed. Reload this folder.");
        return folder;
      },
      getChildren: (id) => (id === "root" ? page.entries.map((entry) => entry.path) : []),
    },
    features: [syncDataLoaderFeature, selectionFeature, hotkeysCoreFeature],
  });
  return (
    <>
      <div
        className="folder-picker-list"
        {...tree.getContainerProps("Folders on the application host")}
      >
        {tree.getItems().map((item) => {
          const entry = item.getItemData();
          return (
            <button
              key={item.getId()}
              type="button"
              {...item.getProps()}
              className="folder-picker-item"
              aria-disabled={disabled || !entry.identity}
              data-selected={item.isSelected()}
            >
              <svg
                viewBox="0 0 24 24"
                width="22"
                height="22"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.6"
                aria-hidden="true"
              >
                <path d="M3 7V5h6l2 2h10v13H3Z" />
              </svg>
              <span>
                <strong>{entry.name}</strong>
                {entry.unavailable_reason && <small>{entry.unavailable_reason}</small>}
              </span>
            </button>
          );
        })}
      </div>
    </>
  );
}

/** Selection observes the host. Library inspection and approval stay separate. */
export function FolderPicker({
  initialPath,
  onSelect,
  onClose,
}: {
  initialPath: string;
  onSelect: (path: string) => void;
  onClose: () => void;
}) {
  const dialog = useRef<HTMLDialogElement>(null);
  const [path, setPath] = useState(initialPath);
  const [page, setPage] = useState<HostFolderPage>();
  const [generation, setGeneration] = useState(0);
  const cursor = useRef<string | null>(null);
  const mounted = useRef(false);
  const command = useCommand();
  const roots = useResource(
    (signal) => request<string[]>("/api/system/folders/roots", { signal }),
    "host-folder-roots",
  );

  const release = async () => {
    const current = cursor.current;
    if (current) await request<void>("/api/system/folders/close", { body: { cursor: current } });
    if (cursor.current === current) cursor.current = null;
  };
  const browse = async (folderPath: string, identity: string | null = null) => {
    setPage(undefined);
    setPath(folderPath);
    await release();
    const result = await request<HostFolderPage>("/api/system/folders/browse", {
      body: { path: folderPath, expected_identity: identity, cursor: null },
    });
    cursor.current = result.next_cursor;
    if (!mounted.current) {
      await release();
      return;
    }
    setPath(result.path);
    setPage(result);
    setGeneration((value) => value + 1);
  };

  useEffect(() => {
    mounted.current = true;
    dialog.current?.showModal();
    return () => {
      mounted.current = false;
      dialog.current?.close();
      void release().catch((error: unknown) =>
        console.error("Could not close the host folder listing.", error),
      );
    };
  }, []);

  const close = () =>
    void command.run(async () => {
      await release();
      onClose();
    });

  return (
    <dialog
      ref={dialog}
      className="folder-picker"
      aria-labelledby="folder-picker-title"
      onCancel={(event) => {
        event.preventDefault();
        if (!command.busy) close();
      }}
    >
      <header className="design-card-header">
        <h2 id="folder-picker-title">Choose a folder</h2>
        <Button variant="secondary" disabled={command.busy} onClick={close}>
          Close
        </Button>
      </header>
      <p>Browse folders on the application host, not on this browser's computer.</p>
      <details open={!page} className="folder-picker-input">
        <summary>Local drives and network shares</summary>
        <label>
          Host drive
          <select
            value=""
            disabled={command.busy || roots.loading || Boolean(roots.error)}
            onChange={(event) => {
              const drive = event.target.value;
              if (drive) void command.run(() => browse(drive));
            }}
          >
            <option value="">Choose a local drive</option>
            {!roots.loading &&
              !roots.error &&
              roots.data?.map((drive) => (
                <option key={drive} value={drive}>
                  {drive}
                </option>
              ))}
          </select>
        </label>
        <Alert error={roots.error} />
        <form
          onSubmit={(event) => {
            event.preventDefault();
            void command.run(() => browse(path));
          }}
        >
          <label>
            Host folder path
            <input
              value={path}
              required
              maxLength={4096}
              disabled={command.busy}
              aria-describedby="folder-picker-path-help"
              onChange={(event) => {
                setPath(event.target.value);
                setPage(undefined);
              }}
            />
          </label>
          <Button type="submit" variant="secondary" disabled={command.busy || !path}>
            Open folder
          </Button>
        </form>
        <p id="folder-picker-path-help" className="muted">
          For Windows SMB, enter a share path such as <code>\\server\Music</code>. For Linux SMB or
          NFS, enter the mounted path, such as <code>/mnt/music</code>. The application account must
          have read access.
        </p>
      </details>
      <Alert error={command.error} />
      {command.busy && <p role="status">Checking host folders…</p>}
      {page && !command.error && (
        <>
          <div className="folder-picker-location">
            <strong className="library-folder-path">{page.path}</strong>
            <div className="actions">
              <Button
                variant="secondary"
                disabled={command.busy || !page.parent_path}
                onClick={() => {
                  const parent = page.parent_path;
                  if (parent) void command.run(() => browse(parent));
                }}
              >
                Parent folder
              </Button>
              <Button
                variant="secondary"
                disabled={command.busy}
                onClick={() => void command.run(() => browse(page.path))}
              >
                Reload folder
              </Button>
            </div>
          </div>
          {page.entries.length ? (
            <FolderList
              key={generation}
              page={page}
              disabled={command.busy}
              onOpen={(folder) => void command.run(() => browse(folder.path, folder.identity))}
            />
          ) : (
            <p>No selectable folders in this page.</p>
          )}
          {page.next_cursor && (
            <Button
              variant="secondary"
              disabled={command.busy || page.entries.length > maximumEntries - 128}
              onClick={() =>
                void command.run(async () => {
                  setPage(undefined);
                  const more = await request<HostFolderPage>("/api/system/folders/browse", {
                    body: {
                      path: page.path,
                      expected_identity: page.identity,
                      cursor: page.next_cursor,
                    },
                  });
                  cursor.current = more.next_cursor;
                  if (!mounted.current) {
                    await release();
                    return;
                  }
                  setPage({ ...more, entries: [...page.entries, ...more.entries] });
                  setGeneration((value) => value + 1);
                })
              }
            >
              Load more folders
            </Button>
          )}
          {page.next_cursor && page.entries.length > maximumEntries - 128 && (
            <p role="status">The list limit is reached. Enter a child folder path to continue.</p>
          )}
          <footer className="folder-picker-footer">
            <p>Use this folder as the path to check. No registration or media change occurs.</p>
            <Button
              disabled={command.busy}
              onClick={() =>
                void command.run(async () => {
                  setPage(undefined);
                  await release();
                  const checked = await request<HostFolderPage>("/api/system/folders/browse", {
                    body: { path: page.path, expected_identity: page.identity, cursor: null },
                  });
                  cursor.current = checked.next_cursor;
                  await release();
                  onSelect(checked.path);
                })
              }
            >
              Use this folder
            </Button>
          </footer>
        </>
      )}
    </dialog>
  );
}
