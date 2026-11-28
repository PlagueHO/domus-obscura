---
applyTo: "frontend/app/**/*.{tsx,css}"
---

# Frontend UI rules

Read `docs/ui-guide.md` before changing any screen.

- Compose from `components/ui.tsx`. Add a block there, plus a Component library example, before reusing new markup.
- Use the type and spacing variables in `styles.css`. Do not add raw hex, per-component font sizes or ad-hoc gaps.
- Fix the source rule. Never append an override block.
- Design empty, loading and error states with the happy path.
- Icon-only controls need `aria-label` and a tooltip. One `h1` per page.
- Solid outlines only. No all-caps text. Sentence case copy, ASD-STE100 style.
- Never edit `frontend/src/generated`.
- Finish with `pnpm --dir frontend run check`, `pnpm format:check`, and a visual pass at 375 and 1280px.
