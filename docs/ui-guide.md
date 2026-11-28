# UI guide

Rules for every screen in `frontend/app`. Adapted from the M365 app-ui-starter playbook. The stack stays plain CSS and the owned primitives in `components/ui.tsx`.

## Build loop

1. Read PRODUCT.md and `docs/ui-poc` for scope. State the page job in one sentence.
2. Compose from the vocabulary below. Never hand-roll a new control.
3. Design the empty, loading and error states with the happy path.
4. Verify: `pnpm --dir frontend run check`, Prettier, then look at 375, 768, 1280 and 1920px, keyboard only, and read every string aloud.
5. Walk the checklist. Fix. Report what changed.

## Tokens (styles.css, one place)

| Role      | Rule                                                                              |
| --------- | --------------------------------------------------------------------------------- |
| Body text | `--fs-body` 13px. Never set a body size per component.                            |
| Meta text | `--fs-meta` 12px, muted.                                                          |
| Headings  | h1 page title, h2 card title (15px), h3 sub-section. One h1 per page.             |
| Spacing   | `--space-1..4` (4, 8, 12, 16). No other gaps or paddings.                         |
| Sidebar   | 20px icons in 32px targets. Tooltip and `aria-label` on each.                     |
| Colour    | Variables only. No new hex in components. Colour carries meaning, not decoration. |
| Outlines  | Solid, shaded. Never dashed or dotted. No all-caps text.                          |

## Vocabulary

`Button`, `Card`, `HeaderCard`, `Alert`, `Empty`, `EmptyState`, `LoadingState`, `Badge`, `StatusBadge`, `SettingsRow`, `StatTile`, `Check`, `Pager`, `Cover`, `FolderPicker`. Add a block to `ui.tsx` and the Component library page before using it twice.

- Page: heading, description, then cards.
- HeaderCard: title, optional actions, optional body. Header-only when no children.
- HeaderIconAction: a compact, outlined header control with a tooltip and accessible name. Use a link for navigation and a button for commands.
- Subcard: a compact header and optional action inside a parent card. Use CompactTable for dense rows with aligned columns and one shared outline.
- Button: use `small`, `medium` or `large` widths to align peer actions. The standard minimum height is 36px. The optional icon sits on the left. A busy button keeps its label and replaces the icon with a spinner. Reduced motion stops the spin.
- SettingsRow: keep the caption and subtitle on the left and the control on the right at all widths. Use `wide` only for controls that need more room, such as paths and templates. Wide rows put the control between the caption and subtitle. Spread the attributes from the row onto the form control to associate its label and description.
- Switch: use for on or off settings. Keep the label visible in the settings row. The switch changes the form draft; the form still controls when changes are saved.
- System Settings opens in a centered, fixed-width modal that uses 75% of the window height. The sidebar and content scroll separately. On narrow screens, the section picker stays above the scrollable content. Close the modal with Escape or the Close settings button.
- SettingsSaveButton: use the standard button with a Save icon and the same large width for each Save action. Align it to the right in the footer. The footer sits flush with the settings pane and reverses the card header gradient. Disable Save until there is a valid change. After a successful save, use the saved values as the new baseline. Keep check, cancel and restart actions with their controls.
- One primary button per region. Destructive only inside a confirmation.

## States and words

- Empty: reason plus next step. Error: plain words, what to do. Loading: shape of the final content.
- Sentence case. Verb on buttons. No jargon, no raw error text. ASD-STE100 style.
- A long explanation or expander means the design is wrong. Simplify the choice.

## Anti-patterns

- Override blocks appended to styles.css instead of fixing the source rule.
- Per-page font sizes or margins. Raw hex values. Dashed outlines.
- Icon-only button without label. Table or card without empty state.
- Retained data shown as current after a failed refresh.

## Checklist

- [ ] Uses the vocabulary and tokens only
- [ ] One h1, landmarks, visible focus, labels on inputs
- [ ] Empty, loading and error states designed
- [ ] No overflow at 375px, no clipped text
- [ ] Text contrast at least 4.5:1
- [ ] Copy is plain and sentence case
- [ ] Tests and Prettier pass
