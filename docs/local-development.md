# Local album download with developer HTTPS

This is an explicit local development workflow, not a complete V1 deployment.
It uses the fixed `_anonymous` identity. It needs no Entra registration.
Production remains the default and requires Entra. All listeners stay on loopback.

## Build and create a fresh Windows setup

Use the pinned tools. Run these commands from the repository in PowerShell 7:

```powershell
.\dev.ps1 build
.\scripts\local-setup.ps1 -Directory C:\DomusDev
.\target\debug\domus.exe api-preview C:\DomusDev\service.local.json
```

Choose a new directory whose parent already exists. The script refuses an existing
directory. It creates an empty library, private staging, a local SQLite database,
default output settings and a 90-day ECDSA developer certificate for `localhost`
and `127.0.0.1`. Its directories inherit access for the current Windows user only.
It does not install software, change certificate trust or create provider credentials.
If setup fails, it retains the partial setup for inspection.

Open `https://localhost:8443`. Use the same hostname and port as `origin`.
The application uses the real guarded API, not `frontend/local-test`.
It serves the built browser files from `frontend/dist-app`.
Rebuild and restart after frontend changes. The browser has a Component library.

The generated certificate is self-signed. Use a certificate already trusted by
your browser, or review and trust the generated public certificate through your
normal development certificate process. Keep the private key private.
Do not disable certificate validation. Trust is an operator action, not part of setup.
For a verified command-line check without changing system trust:

```powershell
curl.exe --cacert C:\DomusDev\certificate.pem https://localhost:8443/api/session
```

This must return `object_id: "_anonymous"`. Browser commands still require the exact
Origin and `x-domus-csrf: 1`. Cross-origin, remote-peer and forwarded identity
requests cannot activate anonymous access.

## Enable one explicitly selected live source

Live provider use needs separate account and permitted-content approval.
Automated checks do not use your account. Do not put credentials or state in Git.

Create an owner-only JSON credential file outside the library and staging:

```json
{
  "username": "your-account-name",
  "password": "your-account-password"
}
```

The generated setup directory is private. A file created there inherits its
restricted access. On Unix, set the credential file mode to `0600`.
Never put these values into the service JSON, browser, URL or logs.

Add `local_acquisition` to `service.local.json` and restart the API:

```json
{
  "staging_root": "C:\\DomusDev\\staging",
  "credentials_file": "C:\\DomusDev\\soulseek.secret.json",
  "server": "PUBLIC_SERVER_IPV4:PORT",
  "acknowledge_plaintext_login": true
}
```

Replace the server placeholder with the actual public Soulseek IPv4 address and
port. It is not a default or a fallback address. The acknowledgement means you
accept the protocol's plaintext login. The API rejects this configuration in
production. It checks provider selection, native root binding and input-file
separation before enabling it. Browser arguments cannot choose credentials,
database paths, staging paths or native destinations.

Only one local source search, folder read or download runs at a time. Source
search can take about 45 seconds plus connection/login time. No shares, listener,
upload, port mapping, acquisition retry or scheduler starts as a side effect.

## Browser flow

1. Search for an artist or album. Open an exact MusicBrainz release.
2. Select MP3, the configured destination and current output settings.
   Prepare and review the frozen output plan. Approve that plan.
3. Read a selected peer folder or import its bounded listing JSON. Inspect the
   complete track mapping. Separately acknowledge the exact edition and permitted
   download, then approve the digest-bound source review.
4. Upload a permitted cover. Permission to use the image does not approve the
   preview. Inspect the normalized JPEG and approve that exact candidate/hash.
5. Select **Start Download** after all approvals. Acknowledge new-only publication.
   It receives, validates, writes fresh metadata, verifies the result and copies
   it into the approved new album folder. Follow actual state in the queue.

Default MP3 output:

```text
library\
  F\
    FIXTURE ARTIST\
      FIXTURE ALBUM\
        folder.jpg
        Fixture Artist - Fixture Album - 01 - First Light.mp3
```

The Rust template rule owns names. Folders are uppercase. Tags retain valid
catalog text, including text replaced only in filenames. Track numbers run
continuously across discs; original disc/track information stays in metadata.
MP3 output is fresh ID3v2.3, with exact artist/album/title, track/disc totals,
release date, MusicBrainz IDs and the approved front-cover JPEG. Encoded audio
must remain unchanged. Every audio frame must pass the actual 320 kbps checks
and complete decoding. Advertised peer bitrate is not sufficient.

The worker retains its fenced claim and private staging OS lock through new-only
publication. It never adopts an existing album folder or overwrites partial,
different or receipted-but-missing files. A verified primary can remain
`backup_pending`. New-only publication exposes partially copied final folders;
it is not an atomic replacement or power-loss acceptance.

## Failure and recovery

No command is automatically retried. A stopped worker keeps all received files,
transfer intent, normalization evidence and failed destination copies.
The page shows the local operation error and durable job/transfer/publication state.
The last-operation message is process-local; receipts and questions are durable.

Use existing `job-control`, artwork answer/resume and publication commands for
operator-controlled recovery. The browser identifies unresolved questions but
does not invent an answer or resume. An uncertain submission must not be reset.
All fully received, sealed inputs can finish locally without another peer
connection. Supply `source_candidate: null` only for an existing source; an idle
new job needs its exact approved source candidate.

Library browsing remains read-only. A completed copy is not automatically a
fresh index observation. Run `domus scan <database> <location-id>` when the worker
has finished, then refresh the browser's library view.

## Repeatable evidence without live credentials

```powershell
.\dev.ps1 test anonymous_https_approves_exact_album
.\dev.ps1 test complete_mp3_and_flac_albums
.\dev.ps1 check
.\dev.ps1 build
.\dev.ps1 smoke
```

The HTTPS test uses a fresh temporary database, generated catalog, generated
MP3 files and a generated cover. It uses the real TLS listener and command
guards, distinct approvals, sealed input receipts, the local worker and
new-only publication. It independently asserts the full folder/file set,
all required MP3 tags, exact cover bytes, removal of old metadata and unchanged
encoded audio for three tracks across two discs. It does not contact a real
Soulseek account. Separate bound-channel tests receive generated MP3 and FLAC
through local protocol sockets, normalize and publish them.

Passing generated tests does not prove that any arbitrary peer has the exact
edition or permitted content. No new live test was authorized or performed here.
The local browser shell is an initial connected implementation. Narrow-screen
polish, full artwork-recovery UI, scheduling, atomic replacement, service
packages and public acceptance remain outside this delivery.

## Manual setup on Linux or with an existing developer certificate

Use fresh, separate local directories. Keep private staging at mode `0700` and
credentials/private keys at mode `0600`. Build with `cargo task build`.
Run `domus init`, `location-add`, `scan`, `staging-init`, and save the JSON from
`settings-default` with `settings-save <database> 0 <settings-json>`.
Use absolute paths in the configuration:

```json
{
  "environment": "development",
  "database": "/absolute/local/state/domus.db",
  "listen": "127.0.0.1:8443",
  "origin": "https://localhost:8443",
  "certificate_file": "/absolute/local/certificate.pem",
  "private_key_file": "/absolute/local/private-key.pem",
  "browser_assets": "/absolute/checkout/frontend/dist-app"
}
```

The PEM certificate must be valid now, support server authentication, match
`localhost`, and match its private key. Configure the optional acquisition block
only after account approval. Run `domus api-preview <absolute-configuration-file>`.
