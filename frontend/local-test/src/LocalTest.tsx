import { useEffect, useState } from "react";
import { OutputPreview } from "../../src/OutputPreview";
import type { OutputPreview as Preview } from "../../src/generated/OutputPreview";
import type { Entry } from "../../src/generated/Entry";
import type { AudioProfile } from "../../src/generated/AudioProfile";
import type { StoredPlan } from "../../src/generated/StoredPlan";
import {
  AudioProfileSelect,
  audioProfileLabel,
  outputWithAudioPolicy,
} from "../../app/audio-profiles";
import { entries, request, sampleEdition, type AlbumReport, type Snapshot } from "./client";

export function LocalTest() {
  const [snapshot, setSnapshot] = useState<Snapshot | null>(null);
  const [albumReport, setAlbumReport] = useState<AlbumReport | null>(null);
  const [played, setPlayed] = useState<number[]>([]);
  const [artist, setArtist] = useState(sampleEdition.album_artist);
  const [album, setAlbum] = useState(sampleEdition.album);
  const [bucket, setBucket] = useState("");
  const [profile, setProfile] = useState<AudioProfile>();
  const [folderTemplate, setFolderTemplate] = useState("<LETTER>\\<ALBUM ARTIST>\\<ALBUM>");
  const [filenameTemplate, setFilenameTemplate] = useState(
    "<Track Artist> - <Album> - <Track:00> - <Title>.<ext>",
  );
  const [preview, setPreview] = useState<Preview[]>([]);
  const [folder, setFolder] = useState("");
  const [rows, setRows] = useState<Entry[]>([]);
  const [busy, setBusy] = useState(true);
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("Loading the isolated test database…");

  const settings = snapshot
    ? outputWithAudioPolicy(
        {
          ...snapshot.settings.settings,
          template: { folder: folderTemplate, filename: filenameTemplate },
        },
        {
          ...snapshot.settings.policy,
          preferred: profile ?? snapshot.settings.policy.preferred,
        },
        snapshot.settings.audio_profiles,
      )
    : null;
  const edition = { ...sampleEdition, album_artist: artist, album, bucket: bucket || null };

  async function refresh() {
    const current = await request<Snapshot>("/local/snapshot");
    setSnapshot(current);
    return current;
  }

  async function act(operation: () => Promise<void>) {
    setBusy(true);
    setError("");
    setNotice("");
    try {
      await operation();
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally {
      setBusy(false);
    }
  }

  useEffect(() => {
    void act(async () => {
      const current = await refresh();
      setProfile(current.settings.policy.preferred);
      setFolderTemplate(current.settings.settings.template.folder);
      setFilenameTemplate(current.settings.settings.template.filename);
      setRows(await entries(""));
      setAlbumReport(await request<AlbumReport | null>("/local/album-test"));
      setNotice("Ready. All albums and files on this page are generated test data.");
    });
  }, []);

  function changeFolder(next: string) {
    void act(async () => {
      const found = await entries(next);
      setRows(found);
      setFolder(next);
    });
  }

  return (
    <main>
      <header>
        <h1>Domus Obscura</h1>
        <p className="subtitle">Local test harness</p>
        <a href="/?library=1#/library?view=artists">Open generated Library browsing</a>
        <p>
          <a href="/?provider=1">Open generated provider waiting states</a>
          {" · "}
          <a href="/?artist-photos=1">Open generated artist photo states</a>
        </p>
        <p className="boundary">
          Sample data only. No sign-in, CAA, MusicBrainz or Soulseek requests. Your music library is
          not connected. Data resets when the server stops.
        </p>
        <p>
          Test a generated album download, filenames, tags and playback. You can also test planning,
          approval and read-only browsing. Live acquisition, publication and recovery are not
          available. This is not the final interface.
        </p>
      </header>
      <div role="status" aria-live="polite" className="status">
        {busy ? "Working…" : notice}
      </div>
      {error && (
        <div role="alert" className="error">
          {error} Change the input or reload the current state.
        </div>
      )}
      <section aria-labelledby="album-test-heading">
        <h2 id="album-test-heading">Download and check one sample album</h2>
        <p>
          This test downloads three generated tones over loopback HTTP. Each FLAC track is about
          three seconds long. Rust writes the real output names and tags, then checks every audio
          sample and the embedded cover. It uses fixed test settings, not the plan editor below.
        </p>
        <p className="muted">
          This is not a Soulseek download or a real catalog release. It does not test the worker
          transfer journal or production publication. No CAA or sign-in is needed.
        </p>
        <button
          disabled={busy}
          onClick={() =>
            void act(async () => {
              const result = await request<AlbumReport>("/local/album-test", "POST", {});
              setAlbumReport(result);
              setPlayed([]);
              setNotice("Sample album downloaded and checked. Play each track to check the sound.");
            })
          }
        >
          Run sample album test
        </button>
        <p className="muted">
          The output stays in its temporary folder while this server runs. Running the test again
          replaces the previous test output. Copy files you want to keep before stopping the server.
        </p>
        {albumReport && (
          <div key={albumReport.directory}>
            <h3>Output folder</h3>
            <code className="path">{albumReport.directory}</code>
            <p>
              The folder contains three FLAC files and folder.jpg. A verification.json report is
              saved at the test root, above the album folder.
            </p>
            <ol className="album-tracks">
              {albumReport.tracks.map((track) => (
                <li key={track.number}>
                  <h3>{track.title}</h3>
                  <code>{track.filename}</code>
                  <p>
                    {track.artist} — {track.album}. Output track {track.number}; original disc{" "}
                    {track.disc}, track {track.disc_track}. {track.duration_seconds.toFixed(2)} s.
                  </p>
                  <p>
                    Tags: {track.tags_verified ? "verified" : "not verified"}. Audio MD5:{" "}
                    {track.evidence.md5_verified ? "verified" : "not verified"}.
                  </p>
                  <audio
                    aria-label={`Play sample track ${track.number}: ${track.title}`}
                    controls
                    preload="none"
                    src={`/local/album-test/audio/${track.number}`}
                    onEnded={() => setPlayed((current) => [...new Set([...current, track.number])])}
                    onError={() =>
                      setError(
                        "The browser could not play this FLAC track. Open the output file in a FLAC player.",
                      )
                    }
                  />
                  <p className="muted">
                    {played.includes(track.number)
                      ? "Browser reached the end. Confirm that you heard the tone."
                      : "Listening check pending. Play the full track and confirm that you hear a tone."}
                  </p>
                </li>
              ))}
            </ol>
          </div>
        )}
      </section>
      <div className="workspace">
        <section aria-labelledby="plan-heading">
          <h2 id="plan-heading">Test an output plan</h2>
          <p>Edit sample metadata. Rust checks the names and renders each path.</p>
          <form
            onSubmit={(event) => {
              event.preventDefault();
              void act(async () => {
                setPreview(
                  await request<Preview[]>("/local/preview", "POST", { edition, settings }),
                );
                setNotice("Preview calculated by Rust. Nothing was saved.");
              });
            }}
          >
            <fieldset disabled={busy}>
              <legend>Album and output settings</legend>
              <label>
                Album artist
                <input
                  required
                  maxLength={512}
                  value={artist}
                  onChange={(event) => {
                    setArtist(event.target.value);
                    setPreview([]);
                  }}
                />
              </label>
              <label>
                Album
                <input
                  required
                  maxLength={512}
                  value={album}
                  onChange={(event) => {
                    setAlbum(event.target.value);
                    setPreview([]);
                  }}
                />
              </label>
              <div className="pair">
                <label>
                  Audio profile
                  <AudioProfileSelect
                    value={profile}
                    accepted={snapshot?.settings.policy.accepted ?? []}
                    descriptors={snapshot?.settings.audio_profiles ?? []}
                    disabled={busy}
                    onChange={(profile) => {
                      setProfile(profile);
                      setPreview([]);
                    }}
                  />
                </label>
                <label>
                  Letter bucket (optional)
                  <input
                    maxLength={16}
                    value={bucket}
                    onChange={(event) => {
                      setBucket(event.target.value);
                      setPreview([]);
                    }}
                  />
                </label>
              </div>
              <label>
                Folder template
                <input
                  required
                  maxLength={4096}
                  value={folderTemplate}
                  onChange={(event) => {
                    setFolderTemplate(event.target.value);
                    setPreview([]);
                  }}
                />
              </label>
              <label>
                Filename template
                <input
                  required
                  maxLength={4096}
                  value={filenameTemplate}
                  onChange={(event) => {
                    setFilenameTemplate(event.target.value);
                    setPreview([]);
                  }}
                />
              </label>
              <button type="submit">Preview output</button>
              <button
                type="button"
                onClick={() =>
                  void act(async () => {
                    if (!snapshot) throw new Error("Load the test state first.");
                    setSnapshot(
                      await request<Snapshot>("/local/settings", "PUT", {
                        expected_version: snapshot.settings.version,
                        settings,
                      }),
                    );
                    setNotice(
                      "Settings saved in the test database. Existing plans stay unchanged.",
                    );
                  })
                }
              >
                Save test settings
              </button>
            </fieldset>
          </form>
          <h3>Sample track mapping</h3>
          <ol>
            {sampleEdition.tracks.map((track, index) => (
              <li key={index}>
                {track.artist} — {track.title}{" "}
                <span className="muted">
                  (disc {track.disc}, track {track.disc_track})
                </span>
              </li>
            ))}
          </ol>
          {preview.length > 0 ? (
            <div className="previews">
              {preview.map((item, index) => (
                <OutputPreview key={index} preview={item} />
              ))}
            </div>
          ) : (
            <p className="muted">Select “Preview output” to inspect the generated filenames.</p>
          )}
          <button
            disabled={busy || !snapshot}
            onClick={() =>
              void act(async () => {
                if (!snapshot) throw new Error("Load the test state first.");
                // A plan always uses the saved settings version, not unsaved template edits.
                const result = await request<StoredPlan>("/local/plan", "POST", {
                  job_id: snapshot.job_id,
                  expected_version: snapshot.plan_version,
                  settings_version: snapshot.settings.version,
                  format: null,
                  audio_profile: profile ?? snapshot.settings.policy.preferred,
                  edition,
                  locations: [snapshot.location.id],
                  accepts_incomplete_coverage: false,
                });
                await refresh();
                setPreview(result.plan.outputs);
                setNotice(
                  `Plan version ${result.version} saved. It uses saved settings version ${result.settings_version}. No file operation started.`,
                );
              })
            }
          >
            Save plan using saved settings
          </button>
          <button
            disabled={busy || !snapshot?.plan || snapshot.plan.approved_at !== null}
            onClick={() =>
              void act(async () => {
                if (!snapshot?.plan) throw new Error("Save a plan first.");
                setSnapshot(
                  await request<Snapshot>("/local/approve", "POST", {
                    version: snapshot.plan.version,
                  }),
                );
                setNotice(
                  "Plan approved in the test database. No download or file operation started.",
                );
              })
            }
          >
            Approve saved plan
          </button>
          {snapshot?.plan && (
            <p className="saved">
              Saved plan v{snapshot.plan.version}:{" "}
              {snapshot.plan.approved_at === null ? "Not approved" : "Approved"} · Settings v
              {snapshot.plan.settings_version} ·{" "}
              {snapshot.plan.plan.audio
                ? audioProfileLabel(
                    snapshot.plan.plan.audio.profile,
                    snapshot.settings.audio_profiles,
                  )
                : snapshot.plan.plan.format}
            </p>
          )}
        </section>
        <section aria-labelledby="library-heading">
          <h2 id="library-heading">Browse the sample library</h2>
          <p>
            Read-only indexed observations. The sample contains a text file, not downloaded audio.
          </p>
          <p>
            Coverage: <strong>{snapshot?.location.coverage ?? "Loading"}</strong>
          </p>
          <code className="path">{folder || "(sample library root)"}</code>
          <div className="actions">
            <button
              disabled={busy || !folder}
              onClick={() => changeFolder(folder.split("/").slice(0, -1).join("/"))}
            >
              Parent folder
            </button>
            <button
              disabled={busy}
              onClick={() =>
                void act(async () => {
                  setSnapshot(await request<Snapshot>("/local/scan", "POST", {}));
                  setRows(await entries(folder));
                  setNotice(
                    "Sample library rescanned. Saved plan observations changed; approval now needs a new plan.",
                  );
                })
              }
            >
              Rescan sample library
            </button>
          </div>
          <div className="table-scroll">
            <table>
              <caption>Current folder</caption>
              <thead>
                <tr>
                  <th>Name</th>
                  <th>Type</th>
                  <th>Present</th>
                </tr>
              </thead>
              <tbody>
                {rows.map((entry) => (
                  <tr key={entry.id}>
                    <td>
                      {entry.kind === "directory" ? (
                        <button
                          className="folder"
                          disabled={busy}
                          onClick={() => changeFolder(entry.relative_path)}
                        >
                          {entry.relative_path.split("/").at(-1)}/
                        </button>
                      ) : (
                        entry.relative_path.split("/").at(-1)
                      )}
                    </td>
                    <td>{entry.kind}</td>
                    <td>{entry.present ? "Yes" : "No"}</td>
                  </tr>
                ))}
                {rows.length === 0 && (
                  <tr>
                    <td colSpan={3}>No indexed entries in this folder.</td>
                  </tr>
                )}
              </tbody>
            </table>
          </div>
          <h2>Test database state</h2>
          <button
            disabled={busy}
            onClick={() =>
              void act(async () => {
                await refresh();
                setNotice("Current database state loaded.");
              })
            }
          >
            Reload state
          </button>
          <p>
            Job {snapshot?.job_id ?? "—"} · Plan version {snapshot?.plan_version ?? "—"} · Saved
            settings v{snapshot?.settings.version ?? "—"}
          </p>
          <h3>Recorded events</h3>
          <ol className="events">
            {snapshot?.events.map((event) => (
              <li key={event.id}>
                <code>{event.event_type}</code>{" "}
                <span className="muted">plan {event.plan_version ?? "—"}</span>
              </li>
            ))}
          </ol>
          {snapshot?.events.length === 0 && <p>No events recorded yet.</p>}
          <h3>Suggested checks</h3>
          <ol>
            <li>
              Preview the sample. Check uppercase folders, continuous track numbers and filename
              replacements.
            </li>
            <li>Save settings, save a plan, then approve it.</li>
            <li>Rescan the sample library. Try the old approval from another tab; it must fail.</li>
            <li>
              Enter an invalid template or an artist that needs a letter bucket. Check the error
              message.
            </li>
          </ol>
        </section>
      </div>
    </main>
  );
}
