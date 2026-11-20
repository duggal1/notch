# cw agent-to-agent protocol (this project)

This project coordinates agents through the `cw` CLI, not through terminal chatter.
Before any code work:

1. Read `.comms/protocol.yaml` fully. It is binding.
2. Register exactly once: `.comms/bin/cw register <role> <team> "<optional human name>"`.
   Never choose or guess an id; state the issued id in your first message and
   prefix every `cw` call with `CW_ID=<id>`.
3. Run `cw roster`, then `cw inbox`. Check the inbox before edits, builds,
   commits, and before declaring done.
4. If the id you forgot comes back, run `cw roster`, find your role+team and
   reuse that id. Never register twice.

`[cw] N UNREAD (U urgent)` in your context means: finish the current atomic
command, then run `CW_ID=<id> cw inbox`. Answer QUESTION/REQUEST/HANDOFF with
`cw reply` (the answer, not an acknowledgement) or close it with `cw done`.
Claim a path with `cw claim` before editing it and release it after committing.
When idle or blocked, run `cw wait 120` instead of polling; if blocked on a
claim, `cw wait --claim <path> 120` wakes when it is released or expires.
A finished agent releases every claim it holds; `cw claims` shows holders and TTL.

This mailbox is shared by every terminal in this repo and by sub-agents spawned
from any of them. Your cw id is not an OpenCode session, task, or agent id.
Do not call `cw status` to mirror OpenCode's own agent lifecycle — that state
belongs to OpenCode. Mail, claims, and meetings are the coordination layer.
