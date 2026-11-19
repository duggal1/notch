# DESIGN_RULES.md

Read before writing any UI. No exceptions.

---

## 1. Your default output is AI slop

Whatever you design on autopilot is generic, ugly AI slop. Assume this about your first instinct, every time. The first layout, the first palette, the first component that comes to mind is the average of every SaaS template you have ever seen. That average is the enemy.

Treat your default as a draft to destroy, not a result to polish. If something shows up in your head instantly and effortlessly, it is probably slop. Throw it out and start from the actual problem.

Test: swap the logo. If the screen could belong to any other product, it's slop. Redesign it.

---

## 2. Hyper-sharp, step by step

Design is sharp or it's nothing. Go step by step and make every layer as precise as it can possibly be: spacing, alignment, type, color, states, edges, timing.

- Every visual decision has a reason you can state. If you can't explain why a value is what it is, you guessed. Guessing is how slop gets in.
- Nothing is approximately aligned. Nothing is almost consistent. Off by one pixel is wrong.
- Numbers don't jitter when they change. Layouts don't shift when state changes. Things snap to the pixel grid of the real display.
- Remove everything that carries no information and offers no action. Sharpness is subtraction.
- Real content, real states, real units. No lorem ipsum, no fake stats, no placeholder brand names.

Sharp is not minimal for its own sake and not decorated for its own sake. It is every element earning its place.

---

## 3. Creative, with ultra-smooth motion by default

Motion is part of the design, not a polish pass at the end. Every interface ships with extremely smooth, considered motion from the start.

- Motion explains a change of state. It shows where something came from, where it went, what just happened.
- Smooth means continuous: values roll, surfaces fill, elements travel. Nothing teleports, nothing pops.
- Fast and quiet. No bouncy springs, no looping decoration, no shimmer for the sake of shimmer.
- Always honor reduced motion, and make sure the interface is still complete and clear without it.

Be creative with how things move, but the creativity must serve comprehension. If you remove the animation and the screen loses information, the motion was doing real work. If nothing is lost, it was decoration. Cut it.

---

## 4. Non-generic. Think outside the box.

Do not reach for the standard pattern just because it is standard. For every component, ask what the most obvious version looks like, then deliberately don't build that.

Ask in this order:

1. What is this component actually for?
2. What does the user need to know or do in the next two seconds?
3. What is the obvious way to show that, and why is it forgettable?
4. What is a version only this product would have?

Reaching for the stock pattern is fine only when it is genuinely the best answer for the user, and you can say why. "It's what everyone does" is not a reason.

---

## 5. Think of novel concepts

Go beyond restyling. Look for a new idea in how the interface represents the thing it represents.

- Let the interface show the real thing, not a picture of it. If something is progressing, let the surface itself progress. If something has a rate, show the rate as a live quantity.
- Find the one idea per screen that the user would describe to a friend. Build the screen around it.
- Borrow from outside software: instruments, print, hardware, physical materials, data displays. Translate the principle, never copy the look.
- One novel idea, executed with total precision, beats five gimmicks.

Novelty is not weirdness. A novel concept makes the product clearer or faster to use. If it only makes the screen stranger, it failed.

---

## 6. Structure, skeleton, and system

This is the UX layer. Before any visual decision, design the structure.

- Decide what the screen is for, what the single primary thing is, and what is secondary. Everything else is support or gets removed.
- Information hierarchy first. The user should know where to look within a second, with no color or decoration helping.
- Group by meaning. Space inside a group is always smaller than space between groups. Grouping should be obvious from spacing alone.
- Design the whole state machine, not just the happy path: empty, loading, in progress, success, error, disabled. A screen that only exists in one state is unfinished.
- Show system status without being asked: what's running, what finished, what failed.
- B2B first: dense where people scan, spacious only around the one thing that matters. Keyboard-first, short verb-led copy, no marketing voice inside the product.
- Build it as a system. Same tokens, same control heights, same radii, same type scale everywhere. Decide them once, before building, and use only those.

A cluttered design always fails. If a screen needs a tour to be understood, the structure is wrong. Fix the structure, don't add a tooltip.

---

## 7. Taste and judgment

Combine all of the above into one clean, hyper-sharp result that has its own taste.

Taste and judgment in design are rare and extremely valuable. You are expected to have them. That means:

- You decide. Don't ship options, don't hedge, don't ask the user to pick between three versions of the same thing.
- You can tell good from generic by looking, and you say which one yours is.
- You cut your own work when it's average, even if it technically works.
- You hold one point of view per product: one accent, one family of shapes, one rhythm, one voice. Consistency is what makes taste visible.
- Restraint is the signal. The more confident the design, the less it needs to shout.

---

## Reference: good vs generic (same job, a download card)

Both of these are a card that gets a user to download a file. The difference between them is the whole point of this document.

### Generic (slop)

An icon sitting in a tinted rounded square. A pill badge saying "Latest release" with a green dot. A title, a one-line description, a divider, three metadata columns (version, size, format) separated by vertical lines, a big accent button with an arrow, a "compatible with" line with a seal icon underneath. Border, shadow, and background all stacked on the same card. Tiny uppercase bold labels.

Why it's slop:

- It's a brochure for a link. It describes a download instead of being one.
- It has exactly one state. Nothing happens when you press the button. No progress, no speed, no verifying, no installed, no failure.
- Every part is a template part: icon tile, badge, divider, metadata row, call-to-action. Swap the name and it fits any app on earth.
- The metadata (version, size, format) is the same information three times in the same visual weight, so nothing leads.
- It decorates instead of informing: the badge and the seal tell the user nothing they can act on.
- Hierarchy is faked with uppercase labels and heavy weights instead of structure.

### Good (sharp)

A calm card, borderless, separated from the page by a background step. A single header row: small leading mark, title, a quiet detail line, one trailing control. When the download starts, the card itself becomes the progress: the surface fills across from the left, and the leading edge dissolves into a pixel dither instead of a hard line. Underneath, a large live throughput number rolls as it changes, sized so it never shifts the layout, with the time remaining below it. Next to it, a pixel-grid strip draws speed history, scrolling smoothly and in step with the display's real pixels. The detail line rolls its byte count in place. Then the card moves through verifying, installed, and failed, each with its own clean state.

Why it's sharp:

- The card is the thing it represents. Progress is the surface, not a bar stuck inside a box.
- Every state exists and is designed: idle, downloading with known progress, downloading with unknown progress, verifying, installed, failed.
- Real data in real units: bytes done of total, speed, time left, with honest fallbacks like "Estimating" when the number would be a lie.
- Numbers use fixed-width digits and reserved space, so nothing jitters while they update.
- One accent, used for the live data. No badges, no icon tiles, no dividers, no stacked border and shadow.
- Motion explains state: digits roll, the fill advances, the strip scrolls. With reduced motion on, it still reads completely.
- Everything snaps to the physical pixel grid. The dither, the strip cells, and the fill edge all align.
- The idea (the surface is the progress, the pixel dither is the edge, the strip is the rate) is something only this product has. Swap the logo and it no longer fits anyone else.

Use this gap as the bar. If your output looks like the first card, you haven't started. Redesign it.

---

## Process (mandatory)

1. Before writing UI, write down the structure: purpose, primary thing, states, hierarchy.
2. Write down the tokens you'll use: spacing, radii, type sizes, colors, control heights. Use only those.
3. Name the one novel idea for this screen. If you can't, go back to rule 4 and 5.
4. Build it, with motion included from the start.
5. **Screenshot it. Review it against every rule above. Fix what's weak.**
6. **Screenshot it again. Review again as if someone else made it. Fix what's still weak.**
7. Delete anything that doesn't carry information or affordance. Then report done.

Two screenshots, two passes, always. The first catches mistakes. The second catches what you got used to.