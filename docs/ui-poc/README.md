# UI proof of concept

This folder holds the approved interface study for Domus Obscura V1. It is a
static HTML prototype with generated sample data. It contacts no provider,
library or account. It is not the production frontend.

## Run

```sh
node docs/ui-poc/server.mjs
```

The server binds to `127.0.0.1` on a free port and prints the URL. Open
`/structure` for the current design.

## Files

| File                    | Purpose                                                                        |
| ----------------------- | ------------------------------------------------------------------------------ |
| structure.html          | The current design: all pages, slide-outs, and the Component library page.     |
| server.mjs              | Loopback-only GET/HEAD server with an exact-path allowlist and a strict CSP.   |
| index.html              | Earlier navigation and shell studies (`/`, `/navigation`, `/shells`).          |
| reference.html          | Earlier cover-wall reference study.                                            |
| navigation.mjs          | Content for the navigation study.                                              |
| shell-studies.mjs       | Content for the shell studies.                                                 |
| BebasNeue-Regular.ttf   | Poster heading font. Licensed under the SIL Open Font License (see `OFL.txt`). |

## How to read structure.html

- The **Component library** button opens the visual language. Every screen is
  built from the shared `CMP` components, so one change updates every screen.
- Open a page with `go({t:'home'|'lib'|'album'|'artist'|'queue'|'search'|'review', id:'0'})`.
  Review ids are `albumIndex:editionIndex`, for example `3:0`.
- The CSP blocks inline `style` attributes. Use classes.

## Known limits

- The narrow-screen layout needs more work.
- The Agent-open layout is not fully checked.
- Only Chromium was checked.
- Failed-item Retry and the 7-day auto-hide of finished items are not built.
