// a2a-cw — injects the cw unread banner into every OpenCode model request.
// Installed by `a2a ptc` at .opencode/plugins/a2a-cw/.
// No runtime dependency: OpenCode's V2 plugin API reads the default export's
// `id` and `setup()` directly (docs: /docs/build/plugins/server).
import { readdirSync, readFileSync } from "node:fs"
import { join } from "node:path"

function read(path) {
  try {
    return readFileSync(path, "utf8")
  } catch {
    return ""
  }
}

function list(path) {
  try {
    return readdirSync(path)
  } catch {
    return []
  }
}

function field(text, key) {
  for (const line of text.split("\n")) {
    if (!line.startsWith(key + ":")) continue
    let v = line.slice(key.length + 1).trim()
    if (v.startsWith('"') && v.endsWith('"')) {
      v = v.slice(1, -1).replace(/\\"/g, '"').replace(/\\\\/g, "\\")
    }
    return v
  }
  return ""
}

export function banner(root, sessionID) {
  if (!sessionID) return ""
  const sid = String(sessionID).replace(/[^A-Za-z0-9_-]/g, "")
  if (!sid) return ""

  // cw writes this mapping on every invocation that has CW_ID set.
  const id = read(join(root, "sessions", sid)).trim()
  if (!/^\d{4}$/.test(id)) return ""

  const lines = []
  const inbox = join(root, "inbox", id, "new")
  const mail = list(inbox).filter((f) => f.endsWith(".yaml"))
  const urgent = mail.filter((f) => field(read(join(inbox, f)), "priority") === "urgent").length
  if (mail.length) {
    lines.push(`[cw] ${mail.length} UNREAD (${urgent} urgent) -> run: CW_ID=${id} cw inbox`)
  }

  const meetings = join(root, "meetings")
  for (const md of list(meetings)) {
    if (!md.startsWith("mt-")) continue
    const dir = join(meetings, md)
    if (read(join(dir, "status")).trim() !== "open") continue
    const meta = read(join(dir, "meeting.yaml"))
    const section = meta.split(/^invitees:\s*$/m)[1] || ""
    const invited = [...section.matchAll(/^\s*-\s*"?(\d{4})"?\s*$/gm)].map((m) => m[1])
    if (!invited.includes(id)) continue
    const turns = list(join(dir, "turns")).filter((f) => f.endsWith(".yaml")).length
    const seen = parseInt(read(join(root, "inbox", id, "meetcursor", md)).trim(), 10) || 0
    if (turns > seen) {
      lines.push(`[cw] meeting ${md}: ${turns - seen} new turns -> cw meeting read ${md}`)
    }
  }

  return lines.join("\n")
}

export default {
  id: "a2a-cw",
  setup(ctx) {
    const root = process.env.CW_ROOT || join(ctx.location?.directory || process.cwd(), ".comms")
    ctx.session.hook("context", (event) => {
      const injected = banner(root, event.sessionID)
      if (injected) event.system.push({ type: "text", text: injected })
    })
  },
}
