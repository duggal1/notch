# taste.md

Taste tells you what to **decide**. It does not tell you what to build.

> **Slop describes. Taste demonstrates.** For every block, replace the claim with evidence and the default with a decision.

This file sits next to `DESIGN.md` (and `DESIGN_RULES.md` if the repo has it).

- `DESIGN.md` = the **skin**: tokens, type, color, radii, shadows, bans. It wins any conflict on those.
- `taste.md` = everything **above the skin**: what to say, show, include, order, space, animate, and **delete**.
- Notch uses the motion in `DESIGN.md` for changing numbers, keyboard focus, and the notch silhouette. The general recipes below illustrate other products.

A model that follows DESIGN.md and ignores this file produces UI that is on-brand, consistent, and forgettable. DESIGN.md can make slop prettier. Only judgment makes it not slop.

**Look before you read.** Open the reference images in `taste/`:
- `margin-without.*` and `margin-with.*`: the same product UI built twice (§4)
- `rook-*.*`: a landing page by a human designer (§5)

The images explain themselves. Everything below is the explanation in words. If the images aren't available, this text is enough. Don't stall.

---

## 0. REFERENCE EXAMPLES. READ THIS FIRST.

**Margin** (a made-up AI code-review product UI, built twice) and **Rook** (a landing page for an always-on work bot, by a human designer) are **reference examples**.

- They calibrate your judgment. They are **not templates**.
- Never reuse their copy, layout, components, palette, or numbers in another product.
- Copying their surface onto a product that doesn't share their problem is a **second failure: cargo-cult taste**. Rook's pixel tiles are right for Rook. Margin's segmented control is right for Margin. Derive yours from your own product.
- What transfers is the reasoning in every **Why** and **Rule** line.
- The "AI default" page described in §5 is a typical output, described in words, not pictured.

---

## 1. Why AI fails

**The main problem is not the UI. It's that nothing above the UI was decided.**

Design has four layers. A model works the first one and guesses the rest:

| Layer | Question it answers | Fixed by DESIGN.md? |
|---|---|---|
| **Skin** | Colors, type, radii, shadows, icons | **Yes** |
| **Skeleton** | What's on the screen, in what arrangement, with how much space, and what was cut | No |
| **Story / UX** | What must the viewer believe or do, in what order, and what are they afraid of | No |
| **Concept** | What the visual language is *derived from* | No |

The Margin "without taste" render has decent skin: neutral palette, soft shadows, clean type. It still fails, because the skeleton is the template, the story is a feature list, and there is no concept. Better tokens will not fix that.

### The failure patterns (tell → fix)

1. **"Clean" means average.** Asking for *clean, modern, minimal, production-ready, Linear-like* returns the most common clean UI. Those words are not direction. Never use them as a brief, and never describe your own output with them. **Fix:** replace each adjective with a concrete decision (what is big, what is empty, what is cut).
2. **It fills components; a human uses the room.** The model asks "which components go in this section?" A designer asks "what is the one thing here, how big is it, and how much empty space does it get?" Space becomes leftover. **Fix:** §8.
3. **It adds and never cuts.** Every block gets one more "safe" element: a badge, an icon tile, a subtitle, a second button, a helper paragraph, "View more →". Taste is mostly subtraction: what to keep, what not to. **Fix:** delete at least one element per block before you finish.
4. **It describes instead of showing.** Claims ("understands your codebase", "works while you sleep") instead of artifacts (a real diff line, a real night of jobs). **Fix:** §3, principle 1.
5. **It has a style, not a concept.** A style moves onto any product unchanged. A concept can't: Rook's grid, pixel tiles, mono labels, and clock times all come from what Rook *is*. **Fix:** §3, principle 2.
6. **Everything is mid-sized.** Mid headline, mid body, one uniform gap, equal-weight cards. Confidence is contrast: giant beside tiny, dense beside empty. **Fix:** §8.
7. **It never thinks about the viewer's doubt.** Humans design the objection. Margin shows the double-billing bug. Rook shows `APPROVE` / `NOT NOW` on the €8,400 payment and a "0 done without asking" counter. **Fix:** name the fear, then show the mechanism that answers it.
8. **It writes category copy.** "AI-powered", "seamless", "Get started", "Lightning-fast". Copy is design. **Fix:** §6.
9. **It builds a page as an inventory.** Hero, logos, three cards, testimonial, pricing, CTA: every section the same card on the same grid. A page is an *argument*: a sequence of beliefs, one per viewport. **Fix:** §5.
10. **It ships static and hedged.** No states, no motion that explains anything, no hover or focus, and it hedges by offering options. **Fix:** §10 to §12, and decide.
11. **It doesn't look at its own output.** Render it, look at it for ten seconds, and the slop is obvious. **Fix:** the gate in §13 requires a screenshot review and a stranger test.

**The gap looks small and isn't.** Both Margin renders share the same canvas, cards, and palette, so a glance says "same screen". The difference is a stack of small decisions: a 34px button vs 40, "20%" vs "20.1%", a segmented control vs two toggles, a diff line vs a checklist. No single one is the reason. The sum is. Skipping one "minor" item slides the output back to slop. Run the whole list.

**Taste is not novelty.** Don't overcorrect into strange layouts to escape the average. Margin's "with" render keeps the familiar frame and changes what's inside. Rook is bold, but every bold choice is derived from the product, not from a wish to look different. Keep the frame or break it, but only for a reason. If DESIGN_RULES.md asks for a novel concept per screen, put the novelty in the idea and the data first.

---

## 2. The order of decisions

Never start at the skin. Decide top-down, and write each answer down before you build.

1. **What is it, physically?** One sentence of behavior, not category. Not "an AI productivity platform": "a bot with its own computer in the cloud that works through a list while you're offline."
2. **What must the viewer believe, and what are they afraid of?** One belief, one doubt.
3. **What real artifact proves it?** A diff line, a distribution, a night of jobs with real names and numbers. If you have no real data, invent plausible data in the product's own vocabulary.
4. **What is the order of the argument?** A page is a sequence of beliefs. A screen has one idea.
5. **Which one thing gets the room?** The biggest, emptiest area. Everything else is smaller, quieter, or cut.
6. **What is the visual language derived from?** (Rook: grid = its workspace, pixels = the tools it touches, mono = machine voice, clock = night work.)
7. **Only now, the skin.** Tokens from DESIGN.md.
8. **Delete pass,** then the gate (§13).

If you start at step 7, you will produce the average of step 7.

---

## 3. Principles that transfer

These carry over to any product. §4 and §5 show each one at work.

1. **Prove, don't claim.** Replace summaries with artifacts: a real diff, a real distribution, a real night of jobs.
2. **Derive the look from the product.** Grid, shapes, type roles, palette, and motion come from what the product *is*. If the style could move to another product unchanged, it's a skin.
3. **Design the doubt.** Name what the viewer is afraid of and show the mechanism that answers it (Margin: the double-billing bug. Rook: `APPROVE` / `NOT NOW`, "0 done without asking").
4. **Name the outcome in the user's stakes.** Verb-first, concrete nouns, no adjectives in feature names. The CTA is the product's own verb.
5. **Numbers carry a denominator, a window, and a shape.**
6. **Match the control to the real cardinality.** Helper text shows the consequence of the current value.
7. **Do it in place.** Empty states are forms, warnings carry their own action, reversible things get Undo. No toast or modal for those.
8. **Make abstract things spatial.** Deadline → timeline. Comparison → overlay. Progress → filled track. A night of work → a vertical clock.
9. **One idea per block, one idea per viewport, one primary action per view.** The secondary is clearly quieter. The palette is small and flat, with a role for every color.
10. **Decoration must mean something.** If a shape, color, or ornament can be deleted without losing information or identity, delete it.
11. **Fewer containers.** Hierarchy from size, weight, value, spacing, and surface step. Group related stats on one surface.
12. **Color means something.** Good, bad, time pressure, status. Never plus/minus.
13. **Be honest with numbers.** Usable precision, a fair comparison window, no invented precision.
14. **Icons are bare.** Never in a tile, circle, or badge.
15. **Space is decided, scale is committed.** One thing owns the room. Huge beside tiny (§8).
16. **Subtract.** The default is to add. Taste is deciding what to keep and what not to. Delete before you finish.
17. **Everything responds.** Hover, press, focus, disabled, success, all animated (§10, §11, §12).
18. **Decide.** No A/B option menus. If you can't choose, you haven't understood the product.

---

## 4. Reference A: Margin (product UI, six blocks)

### 4.1 Hero

**Without**
- Pill: `✦ New · Introducing Margin 2.0 →`
- H1 (3 lines, default Geist): "AI code review that understands your codebase"
- Sub: "Margin reviews every pull request with full codebase context, catching bugs and security issues before they ship."
- Two equal-weight buttons: `Get started →` (filled, 40px) and `Book a demo` (outlined), 10px apart
- Product card: icon tile + "PR #4127 · Fix retry logic · Reviewed", then a checklist: ✓ No security issues / ⚠ 1 potential race condition / ⓘ 2 style suggestions

**With**
- Chip: `New · Margin now reads your whole repo ›` (no sparkle)
- H1, one line: "Catch bugs before your reviewer does."
- Sub: "Margin reads every pull request against your whole repo, then leaves the comments a senior engineer would."
- `Try it on a PR` (34px, near-black) + a **text link** `Read a real review ›`, 8px apart
- Below: a real diff, `billing/retry.ts +3 −1`, line 39 `if (res.status === 409) {`, line 40 `+ return charge(invoice)`, with an inline comment from Margin: "A 409 means the first charge went through. Retrying here bills the customer twice."

**Why**
- The headline names an outcome in the user's own stakes (their reviewer, their embarrassment). The failure headline names a category.
- The proof is one specific, checkable, slightly scary bug. It demonstrates "understands your codebase" without ever saying it. The checklist only claims.
- The chip announces a capability, not a version number. The sparkle icon is an AI-slop signal; deleted.

**Rule:** Prove with an artifact, not a summary. Name the outcome. One primary action; the secondary is clearly quieter (here, a text link).

### 4.2 Metric card

**Without:** bolt icon in a 40px tile · `New` badge · "Lightning-fast cold starts" · "Reviews start in milliseconds, so feedback is ready before you've switched tabs." · `140ms p95 latency` · `↗ 62% faster than v3` · `View benchmarks →`

**With:** small icon + label `Cold start` · blue-tint pill `−62% since v3` · `140` with a smaller muted `ms` · "p95 across 2.1M requests this week" · a **histogram** with a blue p95 marker line and a dashed, ghosted **v3 distribution** behind it · axis `20 ms · 200 · 400 ms`

**Why:** "140 ms" alone is trivia. A number needs a **denominator** (2.1M requests), a **window** (this week), and a **shape** (the distribution). The chart is the proof, so the "View benchmarks →" link (a promise of proof elsewhere) is gone. The comparison is overlaid, not described.

**Rule:** Let the data be the visual. No adjectives in feature names ("Lightning-fast"). No boxed icon. No badge that carries no information.

### 4.3 Notifications

**Without:** title + restating subtitle ("Choose how you want to be notified.") · two rows, each a toggle plus a generic paragraph ("Receive emails about mentions, comments and activity.")

**With:** title only · each channel gets a small colored icon + label + a **3-state segmented control** `Mentions | All | Off` with a sliding white pill · a helper line that **states the consequence and updates live**:
- Email Off → "Only billing and security"
- Email Mentions → "About 4 a week, only when someone needs you"
- Push All → "Every comment, as it happens"
- Push Off → "Nothing reaches your phone"

**Why:** the setting really has three levels, and a toggle can't express that. Descriptions of what a setting *is* are useless; descriptions of what will *happen* are the reason someone reads at all. The subtitle restated the title, so it's deleted.

**Rule:** Match the control to the real cardinality (2 → switch, 3–4 → segmented, many → select). The helper line shows the consequence of the current value.

### 4.4 Empty state

**Without:** dashed border · icon in a circle · "No projects yet" · "Create your first project to get started." · big `+ New project` button

**With:** "Start your first project" · an inline input (`Name it`) with a `Create` button that's disabled until there's text · "or start from" · three tinted chips: `Launch` (blue calendar), `Bug bash` (red bug), `Weekly sync` (green alarm). Click one and it confirms in place: `✓ Weekly sync created`.

**Why:** an empty state is the **first action**, not an apology. The dashed box is a placeholder for content that doesn't exist; the form is the content. Templates remove the blank-page problem.

**Rule:** Let the user do the thing right here. Seed with 2–4 real starting points. Confirm inline.

### 4.5 Billing warning

**Without:** yellow alert box, triangle icon, "Payment method expiring soon", a 3-line paragraph ("…to avoid any interruption to your service."), outlined `Update payment method` button.

**With:** the *object* instead of a warning about the object: a mini card chip (`4242`) · "Visa ending 4242" · "Team plan, $48 a month" · amber pill `6 days left` · a **timeline** `Today ●—○ Expires Nov 23 ——○ Renews Dec 1` · `Update card` (primary) + `Remind me Friday` (text). Click remind: the pill becomes `Reminder Fri`, a marker appears on the timeline at Friday, and the button becomes `Undo`. No toast. No modal.

**Why:** urgency is a *time* problem, so make time visible. The deadline becomes a position, not a sentence. Amber is spent only on the countdown, not on a whole container. A snooze is reversible, so it happens in place with an undo.

**Rule:** Make abstract things spatial (deadline → timeline). Show the object, not a banner about it. Reversible actions change state in place with Undo.

### 4.6 Stats row

**Without:** three separate equal cards, 16px apart · generic monochrome icons top-right · `1,297 +20.1% from last month` · `305 +12.4%` · `4m 17s −8.3%` in **red**.

**With:** one caption above, "November so far, against the same days in October", then **one surface, three columns, hairline dividers** · labels `Reviewed`, `Bugs caught before merge`, `Time to first comment` · big tight numerals · green pills `↑ 20%`, `↑ 12%`, `↓ 8%` · a **sparkline** per stat with a blue end-dot.

**Why:**
- The failure red-flags `−8.3%` on review time. **Lower is better there**, so color by sign is a lie. Color by meaning.
- "from last month" compares a partial month against a full one. The caption states a fair window.
- `+20.1%` is false precision. `20%` is what a person can use.
- Related stats share a surface. Three cards = three unrelated things.
- The sparkline gives context the number can't.

**Rule:** Group related things. Round to usable precision. State the comparison basis. Color encodes good/bad, never plus/minus.

---

## 5. Reference B: Rook (landing page by a human designer)

**Product:** an always-on bot with its own computer in the cloud. You hand it a job, it works while you're offline, and it stops and asks before anything new. The page's whole job: make you believe *it does the work and you keep control*.

**The AI default for this brief (described, not pictured):** a centered hero with an "✦ Introducing Rook" pill; "Your AI assistant that works 24/7" over a gradient mesh; two rounded buttons ("Get started" / "Book a demo"); a logo wall; three feature cards with icon tiles (Fast, Secure, Seamless integrations); a stats band; testimonials; three pricing cards; a final CTA. Every section is the same card on the same grid. It could sell any SaaS.

### 5.1 Hero: "Hand it the job. Keep the final say."

- **Concept on screen one:** a hairline grid with numbered coordinates (001 to 010) at the intersections. Flat pixel mosaics sit on the cells, each labeled with a tool the bot works in (Inbox, Drive, CRM, Slack, Stripe). The mosaics are the integrations, the brand pattern, and the mascot's world at once.
- **Decoration that means something:** the Stripe tile is hatched, dashed, and carries a lock. The payments tile is *locked*. The ornament already tells the trust story.
- **Headline, two-tone serif:** "Hand it the job." in gray (the setup), "Keep the final say." in black (the punch). The whole deal in eight words.
- **Eyebrow:** mono caps with a small orange pixel bar, `ALWAYS-ON BOT · V1.0`.
- **CTAs:** sharp rectangles, mono caps, arrow prefix: `→ GIVE IT A JOB` (filled) and `SEE HOW IT WORKS` (outline, clearly quieter). The label is the product's verb, not "Get started".
- **Space:** the headline owns the lower-left. A two-line description and the CTAs sit far right. The rest is grid and air.

**Rule:** Put the concept on screen one. Make decoration carry meaning. Say the whole deal in the headline.

### 5.2 "I'm Rook. Give me a job and the tools it needs. I'll take it from there."

- First person, because the product is a bot you hand work to. "I'm Rook." in black serif, the rest in gray sans, and the black pixel mascot stands in for one of the o's in the wordmark. The last words darken as they're read.
- Two large black panels of pixel fields that fill in from sparse to dense. White caption cards overlap them at **opposite corners** (bottom-left, top-right): a diagonal, not two identical cards.
- Each card is one promise. **"I Knock Before Anything New"**: "Payments, client replies, anything I haven't done before. I stop and ask first. One tap from you and I carry on." **"I Keep Going After You Log Off"**. Each ends in a mono one-line guarantee: `NOTHING NEW WITHOUT YOUR OK`, `ON MY OWN COMPUTER IN THE CLOUD`.
- No icons, no feature grid.

**Rule:** Two promises beat six features. Give each one a card, a sentence, a guarantee. Alternate the composition.

### 5.3 "Tell it the Job": the interaction is the demo

- Centered: eyebrow `FROM ASK TO DONE`, one serif headline, one CTA (`→ GIVE IT YOUR FIRST JOB`), then a **working prompt composer** typing a real request ("When an inv…") with the mascot perched on its corner, suggestion chips under it, and a `01 / Ask · 02 / Plan · 03 / Build` tracker with a progress underline.
- One soft pink atmosphere at the bottom of this section only, with faint hairline circuitry in the corners.

**Rule:** Show the primary action working, in place. A gradient is allowed once, as atmosphere tied to a moment, never as the identity.

### 5.4 "The Night Shift": proof over time

- Headline: "Close the laptop at seven." (gray) / "Read the summary at nine." (black).
- A **vertical timeline** with clock times (19:12, 20:40, 23:30, 02:14, 06:20) and a chip that travels with the playhead: `NOW 19:12 · DONE 1/5`. Job cards alternate left and right of the center line. Upcoming cards are `Queued` skeletons.
- Every card is a **real artifact with real names and numbers**:
  - Gmail, "Reply to support emails": *Re: Where is my order #4471? Hi Maya, it shipped this morning. Tracking is below.* "+22 more replies in the same tone", then ✓ 23 Replies Sent.
  - CRM, "Update the CRM after today's calls": ACME Corp, Stage `Discovery → Proposal`, Next Step `Send Pricing, Fri`, ✓ 14 Records Updated.
  - Sheets, "Find leads for Mondat": Northwind Pay / New CFO, Lumen Bank / Series B, Harbor Ops / Hiring Finance. Each lead carries its *trigger event*.
  - **Stripe, "Pay vendor €8,400"**: "The invoice matches the contract", `APPROVE` / `NOT NOW`, and a yellow `⚠ Waiting for you`. The one job that stops.
  - Docs, "Prepare Monday's report": Weekly Report · 8 Pages, "Revenue by region, up 6% on last week", a real bar chart, ✓ Ready in Drive.
- **Payoff:** `4 jobs done · 1 waiting for you · 0 done without asking`, then `→ REVIEW 1 REQUEST`.
- The status vocabulary is tiny and consistent: Queued, Now, Done, Waiting for you.

**Why:** autonomy *and* control in one scroll. Four jobs run alone; the one that touches money stops. "0 done without asking" turns the trust promise into a number.

**Rule:** Show the product's behavior as a story in time, with real artifacts and one payoff metric.

### 5.5 Cross-cutting decisions

- **Three type voices with fixed roles:** a high-contrast serif for the promise (two-tone gray/black), a grotesk sans for explanation and data, mono caps for machine-voice labels (eyebrows, buttons, timestamps, tags). Roles never mix.
- **Corners follow a rule:** in the marketing frame (tiles, panels, buttons, tags) they're sharp rectangles. The product cards inside the demo are softly rounded, so they read as the real product. (As far as the screenshots show.)
- **The palette is flat and role-bound:** orange, amber, pink, periwinkle, black, white. Done = periwinkle, Now = coral, Waiting = yellow. No gradients on the tiles. It's derived from the concept, not picked from a trend.
- **Composition changes every viewport, the system doesn't:** asymmetric hero, diagonal cards, centered composer, alternating timeline.
- **Motion is the product's behavior:** the mascot walks to the active card, pixel fields fill in, headline words darken, the composer types, the playhead moves. Nothing decorative loops.
- **Specific data:** #4471, Maya, ACME, €8,400, 14 records, 8 pages, up 6%. Plausible, product-shaped numbers are a craft skill.
- **Voice is a decision from the concept.** Rook says "I" because it is a bot you hand work to. A product that shouldn't have a persona shouldn't borrow this. If DESIGN.md bans persona voice, it's banned.

### 5.6 What transfers

1. Derive the visual language from the product's nature.
2. Decoration carries meaning.
3. One idea per viewport; vary the composition inside one system.
4. The page is an argument in order: the deal → the rules → how you use it → proof over time → payoff.
5. Prove with a time-lapse of real artifacts.
6. Design the doubt: keep the human's control visible.
7. Commit to scale contrast: huge display type, tiny mono labels.
8. The CTA is the product's verb.

---

## 6. Copy bank: before → after (reference only)

| Without | With |
|---|---|
| AI code review that understands your codebase | Catch bugs before your reviewer does. |
| Introducing Margin 2.0 | Margin now reads your whole repo |
| Lightning-fast cold starts | Cold start |
| 62% faster than v3 | −62% since v3 |
| Choose how you want to be notified. | *(deleted)* |
| Receive emails about mentions, comments and activity. | About 4 a week, only when someone needs you |
| No projects yet. Create your first project to get started. | Start your first project |
| Your Visa ending in 4242 expires soon. Update your payment details to avoid any interruption to your service. | Visa ending 4242 · Team plan, $48 a month · 6 days left |
| +20.1% from last month | ↑ 20%, under "November so far, against the same days in October" |
| Update payment method | Update card |

**More, from Rook** (the left column is the typical default, illustrative, not pictured):

| Typical default | Rook |
|---|---|
| Your AI assistant that works 24/7 | Hand it the job. Keep the final say. |
| Get started / Book a demo | → Give it a job / See how it works |
| Powerful automations | I Knock Before Anything New |
| Enterprise-grade security | Nothing new without your OK |
| Runs in the cloud | On my own computer in the cloud |
| Save hours every week | Close the laptop at seven. Read the summary at nine. |
| Review pending approvals | → Review 1 request |
| 5 tasks completed | 4 jobs done · 1 waiting for you · 0 done without asking |

Copy rules: verb-first, concrete nouns, real numbers with denominators and windows, no filler ("to get started", "avoid any interruption"), no category words ("platform", "solution"), sentence case, no trailing periods on UI labels. Say what will **happen**, not what the thing **is**.

---

## 7. The never list (generalized from the failures)

**Copy**
- Headlines that name a category instead of an outcome
- Adjectives in feature names (Lightning-fast, Powerful, Seamless)
- "Introducing X", "Get started", "Learn more", "Effortlessly"
- Sub-headers that restate the header

**Structure**
- Icon in a tile, circle, or badge (icons are **bare**)
- Dashed-border empty states
- Alert banners where an in-place pattern exists
- Rows of equal-weight cards with no hierarchy
- Two equal-weight buttons, or a filled + outlined pair with generic labels ("Get started" / "Book a demo"). The secondary must be clearly quieter, and both labels must be the product's own verbs
- `New` badges and sparkle/wand/robot icons as decoration
- Links that promise proof elsewhere ("View benchmarks →"). Be the proof

**Numbers & color**
- Number with no denominator, window, or shape
- False precision (20.1%)
- Color by sign instead of meaning
- Rainbow accents. The palette is small, flat, and role-bound

**Motion**
- Bounce, overshoot, elastic, low-damping springs
- Anything that loops for decoration
- Spinners where a real state can be shown
- Layout jumps, snapping toggles, numbers that teleport
- A UI with no hover / press / focus states

**Process**
- Offering the user A/B design options. **Decide.**
- Copying a reference's surface instead of its reasoning

**Page-level**
- Template order: hero → logo wall → three feature cards → testimonial → pricing → CTA, with no argument behind it
- A centered hero with a pill, a generic two-button pair, and a gradient mesh
- Gradients as identity. A gradient may appear once, as atmosphere tied to a moment
- Three-card rows with icon tiles ("Fast", "Secure", "Seamless")
- Filler words: AI-powered, supercharge, seamless, 24/7, revolutionize, effortless
- Invented customer logos, testimonials, or metrics presented as real. (Mock data *inside a product demo* is fine, and encouraged, if it's specific and product-shaped.)
- Every section in the same layout
- A style that fits any product
- Starting at the skin (§2)

---

## 8. Space, scale, hierarchy (how to use the room)

Imagine an interior designer handed a small room: *make it beautiful and usable, and don't waste the space.* That is the job. The model's habit is to ask which components fit in the room. Decide the room first.

**The procedure**
1. List what the viewer must get in two seconds.
2. Give it the biggest, emptiest area.
3. Everything else is smaller, quieter, or tucked away.
4. If a region has nothing worth saying, leave it empty or delete the region. Never fill space to avoid emptiness.
5. Spend the same area to **show more**, not more chrome. In Margin's hero, the "with" version uses the card area for a real diff and a real comment. The "without" version spends it on a three-row checklist and a PR icon tile.

**The rules**
1. **Space has levels, not one value.** Use at least three: tight inside a group, medium between groups, huge between ideas. Keep ≥ 3:1 between neighboring levels. Starting ranges: 4 to 8 inside a group, 24 to 32 between groups, 96 to 160 between sections. One uniform gap (16 or 24 everywhere) is the tell.
2. **Proximity is meaning.** Things that belong together are close. Things that don't are far. Don't separate related things with a border when a gap does the job.
3. **Commit to scale.** Display type enormous, labels tiny and quiet (Rook: a giant serif beside tiny mono caps. Margin: big numerals beside muted captions). Mid-sized everything is the tell.
4. **Weight by value, not only size.** Gray = context, black = what matters (Rook's two-tone headlines, Margin's muted units beside big numbers).
5. **One idea per viewport.** A viewport is a slide. If it holds two ideas, split it.
6. **Empty space needs structure to read as intentional.** A hairline grid, rule lines, coordinates, mono labels. Emptiness with no structure looks unfinished; with structure it looks confident.
7. **Vary composition between sections, keep the system.** Left-heavy, centered, diagonal, alternating. The same layout repeated is a template.
8. **Density follows the job.** Marketing hero: spacious. Operational surface (settings, tables, dashboards): dense and tight. Don't give a settings panel hero spacing.
9. **Narrow measure.** Body copy in two or three short lines beside a huge headline, not a full-width paragraph.

The numbers are starting heuristics, not tokens. DESIGN.md's scale wins; keep the *ratios*.

---

## 9. Baseline look (when DESIGN.md is absent or silent)

This is a **fallback**, read from the Margin screenshots, for when DESIGN.md is absent or silent. It is not a style to apply everywhere: Rook (§5) sits at the opposite end (serif display, mono caps, sharp rectangles, flat four-color palette), and both are right because each is derived from its product. Choose the look from the concept; DESIGN.md pins the tokens. Values marked ≈ are approximate, read from pixels. Measurements in §14 are exact. DESIGN.md overrides anything here.

- **Canvas:** white page. Cards white, 1px hairline border (≈ neutral-200), large radius (≈ 24px), one very soft, large-blur, low-opacity shadow. No heavy drop shadows.
- **Ink:** near-black for headlines and numbers, neutral-500 for secondary, neutral-400 for meta. Muted + smaller for units (`ms`, `m`, `s`).
- **Type:** one deliberately chosen sans. **Not** the default Geist/Inter. Headlines weight 500–600, tracking ≈ −0.03 to −0.045em, leading ≈ 1.05–1.1. Numerals large, tight, **tabular**. Mono only for code. Sentence case.
- **Primary button:** near-black, white text, ≈ 12px radius, **34px tall**, faint inset top highlight + soft outer shadow. Secondary action = text link (underline or chevron), never an outline button.
- **Accent:** in product UI, one blue (≈ blue-600 family), used ≤ 4 times per screen: the chip, the key data marker (p95 line), sparkline end-dots, the delta pill (blue-50 bg, blue-700 text). A brand-led page may run a small flat palette (≤ 5 hues, role-bound, derived from the concept) when the palette *is* the identity, as Rook does. If DESIGN.md says one accent, it is one accent.
- **Semantic:** green = good, amber = time pressure, red = error / the bug icon. Nothing else is colored.
- **Icons:** bare, one family, 14–18px. Category color allowed (blue calendar, red bug, green alarm). Never boxed.
- **Padding scales with importance:** hero ≈ 32, standard ≈ 20, compact ≈ 16.
- **Density:** tighter than defaults. 34px CTAs, 8px between sibling actions, segmented control ≈ 166px for three options.
- **Dividers:** hairlines inside a surface instead of separate boxes.

---

## 10. Motion & micro-interactions (stack-matched engine, stack-independent feel)

**The feel is mandatory; the library is not.** Match the engine to the environment, then apply the same tokens, the same zero-bounce discipline, and the same reduced-motion rule everywhere.

- **In React:** everything that moves, fades, scales, slides, blurs, or draws is `motion/react`. Import from `"motion/react"`, never `framer-motion`, never GSAP, never hand-rolled CSS keyframes for interaction. Numbers use NumberFlow (§11).
- **In plain HTML / CSS / JavaScript, where React packages cannot be imported:** do not reach for a React package and do not stop. Use the Web Animations API and CSS transitions with the exact same curves and durations (§10.1), and write the blur and number-flow behavior yourself in JavaScript (§10.6). The ban on hand-rolled keyframes belongs to the React path only; in this environment it is the correct tool.

The goal is **hyper-smooth, quiet, fast, zero bounce**. If a user notices the animation instead of the feedback, it's too much.

### 10.1 Tokens (`lib/motion.ts`). Never inline curves.

```ts
export const EASE = [0.22, 1, 0.36, 1] as const;       // default for everything
export const EASE_EXPO = [0.16, 1, 0.3, 1] as const;   // large surfaces, reveals, draw-ins
export const CSS_EASE = "cubic-bezier(0.22, 1, 0.36, 1)"; // APIs that only take CSS strings (NumberFlow)

export const DUR = { micro: 0.16, state: 0.28, enter: 0.6, reveal: 0.75 } as const;

// Springs must be critically damped or over-damped (damping ratio >= 1). Never underdamped.
export const SPRING_SNAP = { type: "spring", stiffness: 500, damping: 42, mass: 0.7 } as const; // ratio ≈ 1.12
export const SPRING_SOFT = { type: "spring", stiffness: 260, damping: 34, mass: 0.9 } as const; // ratio ≈ 1.11
```

Root: `<MotionConfig reducedMotion="user" transition={{ ease: EASE, duration: DUR.state }}>`.

In plain HTML/CSS/JS the same tokens are CSS variables on `:root` — `--ease: cubic-bezier(0.22, 1, 0.36, 1)`, `--ease-expo: cubic-bezier(0.16, 1, 0.3, 1)`, `--dur-micro: 160ms`, `--dur-state: 280ms`, `--dur-enter: 600ms`, `--dur-reveal: 750ms`. Same values, different spelling.

### 10.2 What gets which treatment

| Moment | Spec |
|---|---|
| Hover on button | bg shifts one step (e.g. `#111` → `#1c1c1c`), 160ms `EASE`. No scale on hover |
| Press | `whileTap={{ scale: 0.97 }}` (large tiles/cards `0.985`), 160ms `EASE` |
| Link / chip chevron | `x: 3` on hover, 200ms `EASE`. It *nudges*, it doesn't travel |
| Segmented control | white pill slides with `layoutId` + `SPRING_SNAP`. Label color eases 200ms |
| Helper / consequence text | blur-crossfade: out `y:-4, blur(4px), opacity 0`, in `y:4 → 0`. 280ms `EASE`, `AnimatePresence mode="popLayout"` |
| Inline success (`✓ created`) | enters with `y:4 → 0`, opacity, blur(4px) → 0. Siblings reflow with `layout` |
| Button label swap (`Remind me Friday` → `Undo`) | `layout` on the button so its width animates; label blur-crossfades |
| Pill text (`6 days left` → `Reminder Fri`) | blur-crossfade; width follows via `layout` |
| Timeline marker | scale `0 → 1` + opacity, 300ms `EASE`; track fill animates `scaleX` from its origin |
| Disabled → enabled (`Create`) | color + opacity eased 200ms, never snapped |
| Input focus | 2px ring fades in 160ms. No layout shift |
| Mount / section reveal | `opacity 0, y 8, blur(6px)` → rest, `DUR.reveal` `EASE`, stagger 70ms |
| Sparkline | `pathLength 0 → 1`, 900ms `EASE_EXPO`, once in view; end-dot scales in after the path lands |
| Histogram | bars rise from the baseline (`scaleY`, 18ms stagger) → p95 line draws → ghost comparison fades in last |
| State morph between views | blur-crossfade (the house transition) |

### 10.3 Patterns

**Pressable button**
```tsx
<motion.button
  type="button"
  whileHover={{ backgroundColor: "#1c1c1c" }}
  whileTap={{ scale: 0.97 }}
  transition={{ duration: DUR.micro, ease: EASE }}
  className="h-[34px] cursor-pointer rounded-xl bg-[#111] px-4 text-sm font-medium text-white outline-none focus-visible:ring-2 focus-visible:ring-neutral-300"
>
  Try it on a PR
</motion.button>
```

**Link with chevron nudge**
```tsx
<motion.a initial="rest" animate="rest" whileHover="hover" className="inline-flex cursor-pointer items-center gap-1 underline underline-offset-4">
  Read a real review
  <motion.span variants={{ rest: { x: 0 }, hover: { x: 3 } }} transition={{ duration: 0.2, ease: EASE }}>
    <ChevronRight size={14} />
  </motion.span>
</motion.a>
```

**Segmented control with a sliding pill + live consequence line**
```tsx
export function Segmented<T extends string>({ value, onChange, options, label }: {
  value: T; onChange: (v: T) => void; options: { value: T; label: string }[]; label: string;
}) {
  const id = useId();
  return (
    <div role="radiogroup" aria-label={label} className="relative inline-flex rounded-xl bg-neutral-100 p-0.5">
      {options.map((o) => {
        const active = o.value === value;
        return (
          <button key={o.value} type="button" role="radio" aria-checked={active}
            onClick={() => onChange(o.value)}
            className="relative cursor-pointer rounded-[10px] px-3.5 py-1.5 text-sm font-medium outline-none focus-visible:ring-2 focus-visible:ring-neutral-300">
            {active && (
              <motion.span layoutId={`seg-${id}`} transition={SPRING_SNAP}
                className="absolute inset-0 rounded-[10px] bg-white shadow-[0_1px_2px_rgba(0,0,0,0.08),0_0_0_1px_rgba(0,0,0,0.04)]" />
            )}
            <motion.span className="relative" animate={{ color: active ? "#0a0a0a" : "#737373" }}
              transition={{ duration: 0.2, ease: EASE }}>
              {o.label}
            </motion.span>
          </button>
        );
      })}
    </div>
  );
}

// consequence line under it
<AnimatePresence mode="popLayout" initial={false}>
  <motion.p key={value}
    initial={{ opacity: 0, y: 4, filter: "blur(4px)" }}
    animate={{ opacity: 1, y: 0, filter: "blur(0px)" }}
    exit={{ opacity: 0, y: -4, filter: "blur(4px)" }}
    transition={{ duration: DUR.state, ease: EASE }}
    className="text-sm text-neutral-500">
    {CONSEQUENCE[value]}
  </motion.p>
</AnimatePresence>
```

**Sparkline draw-in** (SVG; the dot needs `transformBox` or it scales from the corner)
```tsx
<motion.path d={d} fill="none" stroke="currentColor" strokeWidth={1.5}
  initial={{ pathLength: 0 }} whileInView={{ pathLength: 1 }}
  viewport={{ once: true, margin: "-10%" }} transition={{ duration: 0.9, ease: EASE_EXPO }} />
<motion.circle cx={x} cy={y} r={3} fill="#2563eb"
  style={{ transformBox: "fill-box", transformOrigin: "center" }}
  initial={{ scale: 0, opacity: 0 }} whileInView={{ scale: 1, opacity: 1 }}
  viewport={{ once: true }} transition={{ delay: 0.8, duration: 0.3, ease: EASE }} />
```

### 10.4 Smoothness is also performance
- Animate only `transform`, `opacity`, and small-area `filter: blur`. For size changes use `layout`, not `width`/`height`.
- Blur ≤ 6px, and only on small elements (text, pills, numbers), never a whole card or page.
- Don't animate `box-shadow` blur radius. Fade a shadow layer's opacity instead.
- Text next to animating siblings: `layout="position"` so glyphs never stretch.
- Honor `prefers-reduced-motion`: swap movement for a plain opacity fade, or nothing.

### 10.5 When to add more, when to stop
**Add** motion when it (a) is a response to something the user did, (b) explains a state change (what moved, what appeared, what's now different), or (c) reveals data (draw-in).
**Delete** motion that is decorative, looping, or on a non-interactive thing. Non-interactive cards do not lift on hover. A motion that explains nothing is noise.
Every interactive element gets **hover, press, focus-visible, disabled**. No exceptions.

### 10.6 Plain HTML / CSS / JavaScript (no React packages)

If you are working in plain HTML, CSS, and JavaScript where React packages could not be imported, do not import them and do not stop here. Simply use JavaScript and CSS to create number flow, extremely ultra-smooth blur-in, blur-out, and extremely ultra-smooth easing animations. Everything above about *when* to animate, what to delete, and reduced motion applies unchanged.

```css
:root {
  --ease: cubic-bezier(0.22, 1, 0.36, 1);
  --ease-expo: cubic-bezier(0.16, 1, 0.3, 1);
  --dur-micro: 160ms;
  --dur-state: 280ms;
}
.num { font-variant-numeric: tabular-nums; }

/* interaction transitions — transform / opacity / small-area blur only */
button, a, [role="button"], [role="radio"], [role="switch"] {
  transition: background-color var(--dur-micro) var(--ease),
              transform var(--dur-micro) var(--ease),
              opacity var(--dur-state) var(--ease),
              filter var(--dur-state) var(--ease);
}
button:active { transform: scale(0.97); }

@media (prefers-reduced-motion: reduce) {
  *, *::before, *::after { animation-duration: 0.01ms !important; transition-duration: 0.01ms !important; }
}
```

```js
// The §10.1 easing curve, evaluated in JavaScript (no library).
const cubicBezier = (p1x, p1y, p2x, p2y) => {
  const cx = 3 * p1x, bx = 3 * (p2x - p1x) - cx, ax = 1 - cx - bx;
  const cy = 3 * p1y, by = 3 * (p2y - p1y) - cy, ay = 1 - cy - by;
  const sx = t => ((ax * t + bx) * t + cx) * t;
  const sy = t => ((ay * t + by) * t + cy) * t;
  const dx = t => (3 * ax * t + 2 * bx) * t + cx;
  return x => {
    let t = x;
    for (let i = 0; i < 5; i++) {
      const d = sx(t) - x, s = dx(t);
      if (Math.abs(d) < 1e-4 || Math.abs(s) < 1e-6) break;
      t -= d / s;
    }
    return sy(Math.max(0, Math.min(1, t)));
  };
};
const EASE = cubicBezier(0.22, 1, 0.36, 1);
const reduced = matchMedia('(prefers-reduced-motion: reduce)');

// Number flow + the fast blur-in / slow soft blur-out from §11.
function flowNumber(el, to, { duration = 700, unit = '' } = {}) {
  const from = Number(el.dataset.value ?? 0);
  el.dataset.value = String(to);
  const fmt = v => (Number.isInteger(to) ? Math.round(v) : v.toFixed(1)) + unit;
  if (reduced.matches) { el.textContent = fmt(to); return; }

  // one animation; offsets give 180ms in / 550ms out — never a stuck blur
  el.parentElement.animate([
    { filter: 'blur(0px)', offset: 0 },
    { filter: 'blur(4px)', offset: 180 / 730 },
    { filter: 'blur(0px)', offset: 1 },
  ], { duration: 730, easing: 'linear' });

  const t0 = performance.now();
  (function frame(now) {
    const t = Math.min(1, (now - t0) / duration);
    el.textContent = fmt(from + (to - from) * EASE(t));
    if (t < 1) requestAnimationFrame(frame);
  })(performance.now());
}
```

Use it as `<span class="num"><span id="kpi">140</span><span class="unit">ms</span></span>` — the unit stays outside the flowing number, exactly as in §11. For a segmented-control sliding pill, measure the active option's `offsetLeft` / `offsetWidth` and animate `transform: translateX(...)` with `var(--ease)`; that is the vanilla equivalent of `layoutId`. No bounce, no teleport, no layout jump.

---

## 11. Numbers: flow + blur, always

**Every number that changes flows, with an extremely ultra-smooth blur-in and blur-out.** In React, that means `NumberFlow` (`@number-flow/react`) and the wrapper below — one number engine, no `setInterval` counters, no `react-countup`, no hand-rolled rAF tween. In plain HTML/CSS/JavaScript where React packages could not be imported, use the `flowNumber` recipe in §10.6: the same curve, the same 180ms-in / 550ms-out blur, tabular numerals, units outside. The engine is chosen by the environment; the behavior is not optional.

### 11.1 The component (`components/num.tsx`)

```tsx
"use client";
import NumberFlow from "@number-flow/react";
import { animate, motion, useInView, useMotionValue, useTransform } from "motion/react";
import { useRef, type ComponentProps } from "react";
import { CSS_EASE, EASE } from "@/lib/motion";

type NumProps = Omit<ComponentProps<typeof NumberFlow>, "onAnimationsStart" | "onAnimationsFinish">;

export function Num({ className, ...props }: NumProps) {
  const blur = useMotionValue(0);
  // "none" at rest: no stuck blur, no extra compositing layer
  const filter = useTransform(blur, (v) => (v < 0.05 ? "none" : `blur(${v}px)`));

  return (
    <motion.span style={{ display: "inline-flex", filter }}>
      <NumberFlow
        transformTiming={{ duration: 700, easing: CSS_EASE }}
        spinTiming={{ duration: 700, easing: CSS_EASE }}
        opacityTiming={{ duration: 400, easing: CSS_EASE }}
        className={`tabular-nums ${className ?? ""}`}
        {...props}
        onAnimationsStart={() => animate(blur, 4, { duration: 0.18, ease: EASE })}   // blur in fast
        onAnimationsFinish={() => animate(blur, 0, { duration: 0.55, ease: EASE })}  // blur out slow and soft
      />
    </motion.span>
  );
}

// count up once when it first scrolls into view (hero / dashboard KPIs only)
export function CountOnView({ value, ...rest }: Omit<NumProps, "value"> & { value: number }) {
  const ref = useRef<HTMLSpanElement>(null);
  const seen = useInView(ref, { once: true, margin: "0px 0px -10% 0px" });
  return (
    <span ref={ref}>
      <Num value={seen ? value : 0} {...rest} />
    </span>
  );
}
```

Blur spec: peak ≤ 4px on value changes (≤ 6px on mount reveals). Rise ≈ 180ms, fall ≈ 550ms, both `EASE`. Blur is on the **wrapper**, not per digit. Nothing is blurred at rest.

Units stay **outside** the animated number: `<Num value={140} /> <span className="text-neutral-500">ms</span>`. Compound values are two numbers: `<Num value={4} suffix="m" /> <Num value={17} suffix="s" />`. Keep `tabular-nums` on so widths don't jitter. NumberFlow already respects reduced-motion.

### 11.2 When to use it

**Use NumberFlow for**
- Any KPI/stat that updates (live, polled, or on filter/period change)
- Values driven by input: sliders, steppers, calculators, totals, prices, quantity
- Scrub / hover readouts (hover a sparkline → the big number follows)
- Deltas and percentages next to a number (`↑ 20%` animates with its number)
- Countdowns, counts, progress %, optimistic updates after an action
- **Once, on first view**, for the hero/dashboard KPIs (`CountOnView`)

**Do NOT animate**
- Identifiers and labels: `PR #4127`, `4242`, line numbers, years, version names (`v3`), dates (`Nov 23`)
- Table cells on mount. In tables, animate only the cells that *change*
- Numbers in body copy
- Anything that matters before JS loads. SSR the final value for those and skip count-up
- The same number more than once per view load

**Count-up vs. change:** count-up (0 → value) is for a first impression of a *key* number, once. Everything else animates **change**, never replay.

In the vanilla path, the same list applies: `flowNumber` is the only number engine, and identifiers still never animate.

---

## 12. Cursor and details

Every clickable thing gets `cursor-pointer`, so the UI tells you what's alive. This section is plain CSS — it works identically in React and in vanilla HTML/CSS/JS. **Tailwind v4 resets `button` to `cursor: default`, so set it globally or it will be missing:**

```css
@layer base {
  button:not(:disabled),
  [role="button"]:not([aria-disabled="true"]),
  [role="tab"], [role="radio"], [role="switch"], [role="option"], [role="menuitem"],
  a[href], summary, select, label[for],
  input[type="checkbox"], input[type="radio"], input[type="range"] {
    cursor: pointer;
  }
  :disabled, [aria-disabled="true"] { cursor: not-allowed; }
}
```

Also put `cursor-pointer` on custom clickables (cards that navigate, chips, segmented options). Non-interactive things keep the default cursor, so the pointer stays meaningful.

### Details that read as craft

- **Focus:** a visible `focus-visible` ring on every interactive element (≈ 2px, soft). Never the browser-default blue, never removed.
- **Four states per control:** hover, press, selected, disabled. Disabled looks different *and* gets `not-allowed`.
- **One button system:** fixed heights, one radius rule, one label style, constant icon-to-label gap.
- **One icon family:** one stroke weight matched to the text weight, optically centered on the text, bare (no tiles).
- **`::selection`** styled to the palette.
- **Tabular numerals** for anything that changes (§11).
- **Keyboard:** tab order follows visual order, Esc closes layers.
- **Real states:** empty, loading, error, and success are designed, not defaulted.

---

## 13. Gate: definition of done

Run this before you say "done". Fix, then run it once more.

**Skeleton and story**
1. **Stranger test:** hide the logo. Could this be any other product? If yes, it's a skin.
2. **Argument test:** state the page's argument in one sentence, in order (belief 1 → 2 → 3). A feature list fails.
3. **Room test:** name the one thing that owns the biggest, emptiest area. Space has ≥ 3 levels, not one gap.
4. **Doubt test:** name the viewer's fear and point to where it's answered, visibly.
5. **Concept test:** the visual language is derived from the product, and decoration carries meaning.
6. **Subtraction test:** you deleted at least one element per block. If you can't find one, look again.
7. **Adjective test:** if you'd describe the result as clean, modern, minimal, sleek, or polished, you haven't designed it. Replace each word with a decision.

**Blocks**
8. Each block's one idea fits in ≤ 10 words, and it **proves** something with a real artifact.
9. Every number has a denominator, window, or shape. No false precision. Color encodes meaning, not sign.
10. Every control matches its cardinality. Helper text shows the consequence.
11. No boxed icons, dashed empty states, banners where in-place works, equal-weight card rows, or template section order.
12. One primary action per view. The secondary is clearly quieter. The palette is restrained and role-bound.

**Behavior**
13. Hover / press / focus-visible / disabled exist and animate on every interactive element. `cursor-pointer` is set (§12).
14. All motion uses the §10 tokens in the engine your environment supports — `motion/react` in React, the Web Animations API / CSS transitions in plain HTML/CSS/JS. Zero bounce. Reduced motion honored.
15. Every changing number flows with §11 (`<Num>` / NumberFlow in React, `flowNumber` in plain HTML/CSS/JS), with the ultra-smooth blur in/out. Identifiers aren't animated.

**Look**
16. Screenshot at desktop and mobile widths. Compare against the references for *reasoning*, not surface. Fix. Screenshot once more.

If you're unsure between two options, don't offer both. Pick one and defend it in one line.

### 13.1 Scoring rubric — the before-vs-after test

The 16 checks above are binary. To measure whether reading this file changed a model's output, you need a number. Run the **same prompt** through the model before and after, score both outputs **blind** (hide which is which), and total the weighted points.

| # | Dimension | Weight | 0 — default | 1 — partial | 2 — taste |
|---|---|---|---|---|---|
| 1 | Argument & skeleton | 20 | Inventory of sections (hero, logos, cards, pricing, CTA), no argument | Ordered sections, but still a feature list | One belief per viewport, stated in order; one primary action; one idea per block |
| 2 | Proof with artifacts | 20 | Claims and adjectives ("understands your codebase") | One real artifact, generic numbers | Specific checkable artifact; number has denominator, window, shape; the doubt is answered visibly |
| 3 | Concept & derived language | 15 | Style fits any product; decoration is decorative | Partly derived from the product | Grid, shapes, type roles, palette, and motion all trace to what the product *is*; decoration carries meaning |
| 4 | Space, scale, contrast | 15 | One uniform gap, mid-sized everything | Two spacing levels, some contrast | One thing owns the room; ≥ 3 spacing levels; huge beside tiny; composition varies per viewport |
| 5 | Copy | 10 | Category words, filler, "Get started" | Verb-first, but some claims | Outcome headline, product's own verb CTA, numbers with basis, no filler |
| 6 | States & motion | 10 | Static; no hover / focus / disabled | States exist but inconsistent, bouncy, or looping | Hover / press / focus-visible / disabled everywhere; §10–§11 feel in whichever engine the environment supports; zero bounce; reduced motion honored |
| 7 | Craft details | 10 | Boxed icons, dashed empty states, banners | Some in-place patterns | Bare icons, in-place empty states, Undo, one button/icon system, tabular numerals, `cursor-pointer` |

Score each dimension 0, 1, or 2; points for that dimension = `(score / 2) × weight`. Total out of 100. A gain under 10 is noise; 15 or more is a real effect. Report the **per-dimension deltas**, not just the total — the total can rise while the concept dimension stays at 0, which means the model learned a style, not taste.

If the environment cannot import React packages, score dimension 6 against §10.6 and §11. The engine is irrelevant; the feel is not.

---

## 14. Reference measurements (Margin; exact, from the inspector overlay)

Use these to calibrate **density and restraint**, not as universal tokens.

| What | Without taste | With taste |
|---|---|---|
| Headline face | Geist 30 (default) | A chosen face at 31, tight tracking |
| Primary CTA height | 40 | 34 |
| Gap CTA → sibling action | 10 | 8 |
| Secondary action | Outlined button | Text link |
| Icon container | 40px tile, 24 from card edge | None. Bare icon ≈ 18–20 from edge |
| Notification control | Toggle, 36 wide, ×2 | Segmented, 166 wide |
| Empty-state primary action | `New project`, 118 wide | `Create`, 62 wide, inline in the input |
| Billing container | 336-wide alert box | None. The object + timeline |
| Gap between stat blocks | 16 (three cards) | 0 (one surface, hairline dividers) |

Note the failure's 10px gap and 118/336 widths are *content-driven accidents*, not decisions. In the "with" version, measurements are consistent because they were chosen.

---

*Reference examples only. Never copy Margin or Rook. Copy the thinking.*
