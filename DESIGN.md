# Notch design

Square data, soft controls. Tree follows a supervisor spine into a transcript. Mux gives every session the same footprint. Comms puts messages on a time axis. Alarms give failures a readable reason and an immediate action.

## 1. Shared system

- Surfaces: #000000, #0A0310, #12061A, #1B0A26. Ramp: #2A0A33, #701A75, #A21CAF, #D946EF, #F5D0FE. White text with .78 and .56 secondary values, black on bright accent fills.
- Instrument Sans Variable. Sizes 88, 40, 22, 15 and 12. Body weight 450, controls and titles 500. Mono for identifiers, paths and terminal output. Sentence case and tabular numbers.
- Pills for controls, continuous corners for panels, square data cells. No decorative borders, outlines, dividers or outer shadows. Buttons allow one white keyboard focus ring. Field focus changes fill and caret.
- Pixel icons use hand-authored 12×12 patterns at integer scale. The logo uses the existing SVG mask. Dither belongs to data instruments, empty artwork and the popover, with 16px of clear space around text.
- No zero-only idle metrics or empty charts. Disable destructive controls with no running work.

## 2. Dashboard

- One horizontal navigation row with four labeled pixel tabs, a quiet active fill, and a visible Fleet entry. A separate view header gives the title, grouped live readouts and two-step Kill switch room below native window controls.
- Tree uses a supervisor spine, a stage with explicit header, scrolling transcript and bottom composer rows, and an inspector when a target exists. When the tree is empty, show the single orchestrator node in a compact section alongside the first-assignment workspace. The composer and its templates share one left edge. Decorative empty artwork stays small.
- Mux uses equal-height 224px session cards, a three-line tail, an iteration meter and no duplicate goal. Empty offers a route to Tree.
- Comms uses timestamps beside a continuous vertical axis and route filters. The idle axis has one waiting node.
- Alarms use an inset accent rail. Idle has a quiet dither field and one line of copy.

## 3. Popover

Rest and peek hug the notch. Every mode has an opaque #000 shell, shaped with rounded lower corners. No blur, glass, glow or moving clip path. Active rest and peek show the live working-agent count beside the logo and status. Open preserves its compact idle composer and shows the live count, rate, load, latency, supervisors and sessions when work exists.

## 4. Motion and verification

Animate opacity and small translations. Controls keep their geometry when pressed. The shell resizes directly with its native frame. Skip ripples and moving masks. Reduced motion removes animated offsets. Review three headless capture passes at 1280×800, 1024×720 and 900×700 across all dashboard demos and the popover modes. Record native hit-testing, focus and vibrancy separately.
