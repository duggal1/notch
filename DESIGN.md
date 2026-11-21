# Notch design

Square data, soft controls. Tree follows a supervisor spine into a transcript. Mux gives every session the same footprint. Comms puts messages on a time axis. Alarms give failures a readable reason and an immediate action.

## 1. Shared system

- Surfaces: #000000, #0F0618, #1A0B28, #271239. Ramp: #2A0A33, #701A75, #A21CAF, #D946EF, #F5D0FE. White text with .78 and .56 secondary values, black on bright accent fills.
- Instrument Sans Variable. Sizes 88, 40, 22, 15 and 12. Body weight 450, controls and titles 500. Mono for identifiers, paths and terminal output. Sentence case and tabular numbers.
- Pills for controls, continuous corners for panels, square data cells. No decorative borders, outlines, dividers or outer shadows. Buttons allow one white keyboard focus ring. Field focus changes fill and caret.
- Pixel icons use hand-authored 12×12 patterns at integer scale. The logo uses the existing SVG mask. Dither belongs to data instruments, empty artwork and the popover, with 16px of clear space around text.
- No zero-only idle metrics or empty charts. Disable destructive controls with no running work.

## 2. Dashboard

- One navigation rail with four labeled pixel tabs and a shared moving selection pill. One optional live metrics capsule and a two-step Kill switch. The active tab names the page.
- Tree uses a supervisor spine, a stage with explicit header, scrolling transcript and bottom composer rows, and an inspector when a target exists. Idle centers the logo field, a wrapping template row and the composer on one width.
- Mux uses equal-height 224px session cards, a three-line tail, an iteration meter and no duplicate goal. Empty offers a route to Tree.
- Comms uses timestamps beside a continuous vertical axis and route filters. The idle axis has one waiting node.
- Alarms use an inset accent rail. Idle has a quiet dither field and one line of copy.

## 3. Popover

Close the rest and peek silhouette around the notch, logo and status. The open shell and cards use translucent glass, 24px backdrop blur, saturation and a blurred inner light. Idle gives the sphere and first-task composer one centered column. Active shows rate, load, latency, supervisors and sessions, with fades at scroll edges and accessible footer actions.

## 4. Motion and verification

Animate transform and opacity, with clip-path reserved for the shell. Use a delegated pointer ripple, a single rail indicator, an 8px view entry and 160ms value cross-fades. Reduced motion removes animated and pressed offsets. Review three headless capture passes at 1280×800, 1024×720 and 900×700 across all dashboard demos and the popover modes. Record native hit-testing, focus and vibrancy separately.
