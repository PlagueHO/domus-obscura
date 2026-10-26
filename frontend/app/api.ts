export type { LocalStatus } from "../src/generated/LocalStatus";
export type { SearchReport } from "../src/generated/SearchReport";

export class ApiError extends Error {
  constructor(
    public readonly status: number,
    message: string,
    public readonly retryAfterSeconds: number | null = null,
  ) {
    super(message);
  }
}

/** Commands never retry. The browser supplies Origin on same-origin POSTs. */
export async function request<T>(
  path: string,
  options: {
    method?: "POST" | "PUT";
    body?: unknown;
    file?: File;
    signal?: AbortSignal;
  } = {},
): Promise<T> {
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
    try {
      const error: unknown = await response.json();
      if (
        typeof error === "object" &&
        error !== null &&
        "error" in error &&
        typeof error.error === "string"
      ) {
        message = error.error;
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
