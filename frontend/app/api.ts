export type { LocalStatus } from "../src/generated/LocalStatus";
export type { SourceSearchReport as SearchReport } from "../src/generated/SourceSearchReport";

export class ApiError extends Error {
  constructor(
    public readonly status: number,
    message: string,
    public readonly retryAfterSeconds: number | null = null,
    public readonly providerState: CatalogProviderState | null = null,
  ) {
    super(message);
  }
}

type RequestOptions = {
  method?: "POST" | "PUT";
  body?: unknown;
  file?: File;
  signal?: AbortSignal;
};

type ProviderSnapshot = { pending: boolean; status?: CatalogStatus; error?: unknown };
const idleProvider: ProviderSnapshot = { pending: false };
let providerSnapshot = idleProvider;
const providerListeners = new Set<() => void>();
let pendingMetadata = 0;
let statusTimer: ReturnType<typeof setTimeout> | undefined;
let statusController: AbortController | undefined;
let statusReads = 0;

function publishProvider(snapshot: ProviderSnapshot) {
  providerSnapshot = snapshot;
  for (const listener of providerListeners) listener();
}

function stopProviderObservation() {
  clearTimeout(statusTimer);
  statusTimer = undefined;
  statusController?.abort();
  statusController = undefined;
}

function observeProvider() {
  if (
    !pendingMetadata ||
    !providerListeners.size ||
    statusTimer ||
    statusController ||
    statusReads >= 30
  )
    return;
  statusTimer = setTimeout(async () => {
    statusTimer = undefined;
    const controller = new AbortController();
    statusController = controller;
    statusReads++;
    try {
      const status = await performRequest<CatalogStatus>("/api/discovery/status", {
        signal: controller.signal,
      });
      if (!controller.signal.aborted) publishProvider({ pending: true, status });
    } catch (error) {
      if (!controller.signal.aborted) publishProvider({ pending: true, error });
      return;
    } finally {
      if (statusController === controller) statusController = undefined;
    }
    if (!controller.signal.aborted) observeProvider();
  }, 1000);
}

/** One passive observer for all pending metadata requests, never a retry loop. */
export function subscribeProvider(listener: () => void) {
  providerListeners.add(listener);
  observeProvider();
  return () => {
    providerListeners.delete(listener);
    if (!providerListeners.size) stopProviderObservation();
  };
}
export const getProviderSnapshot = () => providerSnapshot;
export const getIdleProviderSnapshot = () => idleProvider;

/** Commands never retry. Status reads cannot invoke acquisition or approval. */
export async function request<T>(path: string, options: RequestOptions = {}): Promise<T> {
  const metadata =
    (options.method === undefined &&
      options.body === undefined &&
      options.file === undefined &&
      /^\/api\/discovery\/(artists|releases|release-groups)([/?]|$)/.test(path)) ||
    /\/artwork\/caa\/(search|download)$/.test(path);
  if (metadata) {
    if (pendingMetadata++ === 0) {
      statusReads = 0;
      publishProvider({ pending: true });
    }
    observeProvider();
  }
  try {
    return await performRequest<T>(path, options);
  } finally {
    if (metadata && --pendingMetadata === 0) {
      stopProviderObservation();
      publishProvider(idleProvider);
    }
  }
}

/** The browser supplies Origin on same-origin POSTs. */
async function performRequest<T>(path: string, options: RequestOptions): Promise<T> {
  if (!path.startsWith("/api/") || path.includes("\\") || path.includes("#")) {
    throw new Error("Only same-origin API paths are allowed.");
  }
  const headers = new Headers({ Accept: "application/json" });
  const hasBody = options.body !== undefined || options.file !== undefined;
  const command = options.method !== undefined || hasBody;
  if (command) {
    headers.set("x-domus-csrf", "1");
  }
  if (hasBody) {
    headers.set("Content-Type", options.file ? "application/octet-stream" : "application/json");
  }
  const response = await fetch(path, {
    method: command ? (options.method ?? "POST") : "GET",
    credentials: "same-origin",
    redirect: "error",
    headers,
    ...(options.signal ? { signal: options.signal } : {}),
    ...(hasBody ? { body: options.file ?? JSON.stringify(options.body) } : {}),
  });
  if (!response.ok) {
    let message = `Request failed (${response.status}).`;
    let providerState: CatalogProviderState | null = null;
    try {
      const error: unknown = await response.json();
      if (
        typeof error === "object" &&
        error !== null &&
        "error" in error &&
        typeof error.error === "string"
      ) {
        message = error.error;
        if (
          "provider_state" in error &&
          (error.provider_state === "active" ||
            error.provider_state === "backoff" ||
            error.provider_state === "paused" ||
            error.provider_state === "saturated")
        ) {
          providerState = error.provider_state;
        }
      }
    } catch {
      // A proxy or listener can return a non-JSON rejection.
    }
    const retryAfter = response.headers.get("Retry-After");
    const seconds = retryAfter !== null && /^\d+$/.test(retryAfter) ? Number(retryAfter) : NaN;
    throw new ApiError(
      response.status,
      message,
      Number.isInteger(seconds) && seconds >= 0 && seconds <= 4294967295 ? seconds : null,
      providerState,
    );
  }
  if (response.status === 204 || response.headers.get("content-length") === "0") {
    return undefined as T;
  }
  const text = await response.text();
  return (text ? JSON.parse(text) : undefined) as T;
}

export const query = (values: Record<string, string | number | boolean>) =>
  new URLSearchParams(
    Object.entries(values).map(([key, value]) => [key, String(value)]),
  ).toString();

export function errorText(error: unknown): string {
  return error instanceof Error ? error.message : "The request failed.";
}
import type { CatalogStatus } from "../src/generated/CatalogStatus";
import type { CatalogProviderState } from "../src/generated/CatalogProviderState";
