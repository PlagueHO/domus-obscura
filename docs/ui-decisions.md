# UI decisions

- Body 13px, meta 12px, spacing 4/8/12/16.
- Sidebar icons 20px in 32px targets (modern apps use 16 to 20px).
- Headings: h1 clamp 25 to 40px, card titles 15px, h2 20px.
- `HeaderCard` is the one card with a compact header. Header text is vertically centered with inline-flex badges and buttons.
- Settings use rows, not long forms with expanders.
- Errors name the folder problem in plain words and the next step.
- Page headings (`.page-heading`) use 4px between title, subtitle and link. Hints under a heading are `muted`.
- Pages open with `.page-heading`, not a hero banner. Find music keeps its hero as the one landing exception.
- Setup wizard steps are bordered cards. Row titles (`h3`) are 13px, weight 600.
- Empty and loading messages use `EmptyState` and `LoadingState`. Do not add loose `Empty` text.
- Titled cards on Home use `HeaderCard`, so header heights match.
- Badges take a tone (neutral, info, success, warning, danger). `StatusBadge` holds the one status-to-tone map.
- Library folders use two subcards. Primary library shows the current folder and opens the browse and check form in the same subcard. Backup libraries uses a compact table with one row per folder, aligned status badges, and an Add backup action. An empty list shows a single message.
