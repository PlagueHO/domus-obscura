import { useEffect, useState } from "react";
import { Library } from "../../app/Library";
import { LocalMatches } from "../../app/LocalMatches";
import { request, type Snapshot } from "./client";

export function LibraryTest() {
  const [snapshot, setSnapshot] = useState<Snapshot>();
  const [error, setError] = useState("");
  const [route, setRoute] = useState(window.location.hash.slice(1) || "/library?view=artists");
  useEffect(() => {
    void request<Snapshot>("/local/snapshot")
      .then(setSnapshot)
      .catch((error: unknown) => setError(error instanceof Error ? error.message : String(error)));
    const change = () => setRoute(window.location.hash.slice(1) || "/library?view=artists");
    window.addEventListener("hashchange", change);
    return () => window.removeEventListener("hashchange", change);
  }, []);
  return (
    <main className="library-test">
      <p>
        Generated Library fixtures only. Header fixtures are not playable music. No live library or
        providers are connected.
      </p>
      <a href="/">Return to the local test bench</a>
      {error && <p role="alert">{error}</p>}
      {snapshot && (
        <>
          <Library locations={[snapshot.location]} route={route} readOnly={false} />
          <LocalMatches
            locations={[snapshot.location]}
            queries={[{ artist: "Sample Artist", album: "Sample Album" }]}
          />
        </>
      )}
    </main>
  );
}
