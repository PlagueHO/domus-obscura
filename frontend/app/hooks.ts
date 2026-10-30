import { useCallback, useEffect, useRef, useState } from "react";

/** Retained metadata is not fresh provider evidence after a failed refresh. */
export function useMetadataResource<T>(load: (signal: AbortSignal) => Promise<T>, key: string) {
  const resource = useResource(load, key);
  return { ...resource, data: resource.loading || resource.error ? undefined : resource.data };
}

export function useResource<T>(
  load: (signal: AbortSignal) => Promise<T>,
  key: string,
  interval = 0,
) {
  const loader = useRef(load);
  loader.current = load;
  const [data, setData] = useState<T>();
  const [error, setError] = useState<unknown>();
  const [loading, setLoading] = useState(true);
  const [revision, setRevision] = useState(0);
  const previousKey = useRef(key);
  const refresh = useCallback(() => setRevision((value) => value + 1), []);
  useEffect(() => {
    const controller = new AbortController();
    let timer: ReturnType<typeof setTimeout> | undefined;
    if (previousKey.current !== key) {
      setData(undefined);
      setError(undefined);
      previousKey.current = key;
    }
    setLoading(true);
    const run = async () => {
      try {
        const result = await loader.current(controller.signal);
        if (!controller.signal.aborted) {
          setData(result);
          setError(undefined);
        }
      } catch (error) {
        if (!controller.signal.aborted) setError(error);
      } finally {
        if (!controller.signal.aborted) {
          setLoading(false);
          if (interval) timer = setTimeout(() => void run(), interval);
        }
      }
    };
    void run();
    return () => {
      controller.abort();
      clearTimeout(timer);
    };
  }, [key, revision, interval]);
  const changed = previousKey.current !== key;
  return {
    data: changed ? undefined : data,
    error: changed ? undefined : error,
    loading: changed || loading,
    refresh,
  };
}

/** A ref closes the gap before React paints disabled command buttons. */
export function useCommand() {
  const locked = useRef(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<unknown>();
  async function run(action: () => Promise<void>) {
    if (locked.current) return;
    locked.current = true;
    setBusy(true);
    setError(undefined);
    try {
      await action();
    } catch (error) {
      setError(error);
    } finally {
      locked.current = false;
      setBusy(false);
    }
  }
  return { busy, error, run };
}
