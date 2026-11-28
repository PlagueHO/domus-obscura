---
description: "Review a page or component against docs/ui-guide.md and fix what is wrong"
agent: "agent"
---

Review ${input:target:File or page, for example frontend/app/Library.tsx} against #file:../../docs/ui-guide.md.

For each problem give the line, the rule, why it matters and the fix. Check: raw hex, per-component sizes or gaps,
missing states, icon-only controls without labels, hand-made controls, dashed outlines, all-caps text, jargon.
Then fix, run `pnpm --dir frontend run check` and `pnpm format:check`, and summarize.
