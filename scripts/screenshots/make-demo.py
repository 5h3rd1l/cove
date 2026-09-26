#!/usr/bin/env python3
"""Builds a fake home folder full of INVENTED coding-agent chats plus a Cove state
file, for README screenshots. Nothing here comes from a real machine.
  make-demo.py  ->  /tmp/cove-demo-home  and  tools/demo-state.json"""
import json, os, shutil, time, uuid
from datetime import datetime, timedelta, timezone

D = os.path.dirname(os.path.abspath(__file__))
HOME = "/tmp/cove-demo-home"
shutil.rmtree(HOME, ignore_errors=True)
now = datetime.now(timezone.utc)

def code(lang, body): return f"```{lang}\n{body}\n```"

# (agent, workspace, dir, minutes_ago, [(user, assistant), ...])
CHATS = [
 ("claude","storefront","code/storefront", 3, [
   ("Add a dark mode toggle to the settings page. It should remember the choice and respect the system setting on first visit.",
    "I'll add a small theme store and wire it to the settings page.\n\n**Plan**\n- read `prefers-color-scheme` on first load\n- persist the choice in `localStorage`\n- toggle a `data-theme` attribute on `<html>`\n\n" + code("ts","export type Theme = 'light' | 'dark' | 'system';\n\nexport function applyTheme(theme: Theme) {\n  const dark = theme === 'dark' ||\n    (theme === 'system' && matchMedia('(prefers-color-scheme: dark)').matches);\n  document.documentElement.dataset.theme = dark ? 'dark' : 'light';\n  localStorage.setItem('theme', theme);\n}") + "\n\nThe toggle itself is a three-way control: light, dark, system."),
   ("Nice. Can the toggle live in the header too?",
    "Yes. I moved it into a `ThemeMenu` component and rendered it in both places, so the two stay in sync through the same store."),
 ]),
 ("codex","payments-api","code/payments-api", 41, [
   ("Retry failed webhooks with exponential backoff and stop after 6 attempts.",
    "Added a retry queue with delays of 1s, 4s, 16s, 1m, 4m and 15m. After the sixth failure the event is parked in `webhook_dead_letters` and an alert fires.")]),
 ("claude","storefront","code/storefront", 95, [
   ("Fix the cart total rounding when a percentage discount is applied to several items.",
    "The bug was rounding each line before summing. Rounding once at the end, in integer cents, fixes it. I added regression tests for 3 items at 15% off.")]),
 ("claude","infra","code/infra", 170, [
   ("Cut the CI pipeline from 14 minutes to under 6. Where is the time going?",
    "Most of it is dependency installs and a serial test suite. Caching the package store and sharding tests four ways gets a typical run to about 5m 40s.")]),
 ("codex","payments-api","code/payments-api", 240, [
   ("Add idempotency keys to POST /charges so a client retry cannot double charge.",
    "Charges now accept an `Idempotency-Key` header. The first response is stored for 24 hours and replayed for the same key and payload.")]),
 ("claude","side","code/photo-sorter", 330, [
   ("Build a tiny CLI that renames photos by the date they were taken.",
    "Here is a first version that reads EXIF `DateTimeOriginal`, falls back to the file date, and never overwrites an existing name.")]),
 ("claude",None,"", 420, [
   ("Regex to match ISO 8601 dates with an optional time and timezone.",
    "For dates with optional time and offset: `^\\d{4}-\\d{2}-\\d{2}(T\\d{2}:\\d{2}(:\\d{2})?(Z|[+-]\\d{2}:\\d{2})?)?$`")]),
 ("codex","infra","code/infra", 520, [
   ("Write a Terraform module for the staging VPC with two private subnets and a NAT gateway.",
    "I split it into `network` and `nat` modules and exposed subnet ids as outputs. The plan shows 11 resources to add.")]),
 ("claude","storefront","code/storefront", 610, [
   ("Migrate the product grid to server components without breaking infinite scroll.",
    "The grid becomes a server component that streams the first page; a small client island handles the scroll observer and appends pages.")]),
 ("claude","payments-api","code/payments-api", 800, [
   ("Why is this query doing a sequential scan on payments?",
    "The filter casts `created_at` to a date, which defeats the index. Comparing against a timestamp range lets Postgres use `idx_payments_created_at`.")]),
 ("claude",None,"", 950, [
   ("How do I undo a force push?",
    "If you still have the old commit id in `git reflog`, reset the branch to it and force-push again. Otherwise ask a teammate for their copy of the branch.")]),
 ("codex","infra","code/infra", 1100, [
   ("Rotate the database credentials without downtime.",
    "Create a second user, deploy the new secret alongside the old one, switch traffic, then drop the old user after one deploy cycle.")]),
 ("claude","storefront","code/storefront", 1300, [
   ("Write Playwright tests for the checkout flow, including a declined card.",
    "Added tests for guest checkout, saved address reuse and a declined card that shows the inline error and keeps the cart intact.")]),
 ("claude",None,"", 1500, [
   ("Compare SQLite WAL mode with the rollback journal for a desktop app.",
    "WAL gives concurrent readers with a single writer and faster commits, at the cost of extra files next to the database. For a desktop app WAL is usually the better default.")]),
 ("codex","side","code/reading-list", 1700, [
   ("Plan a small reading-list app: data model, screens and what to build first.",
    "Start with three tables (books, lists, notes), one list screen and quick add. Sync and sharing can wait.")]),
 ("claude","payments-api","code/payments-api", 2000, [
   ("Document the refund flow for the support team.",
    "Wrote a one-page guide: when a refund is automatic, when it needs approval, expected timings, and how to check status in the dashboard.")]),
 ("claude","infra","code/infra", 2300, [
   ("Set up alerts for p95 latency and error rate, but keep them quiet at night.",
    "Two alerts: p95 above 800 ms for 10 minutes, error rate above 2% for 5 minutes. Night hours route to a low-priority channel.")]),
 ("claude",None,"", 2600, [
   ("Explain this stack trace: TypeError: Cannot read properties of undefined (reading 'map')",
    "Something you expect to be an array is `undefined` at render time. Default it to `[]` or guard the call until the data has loaded.")]),
 ("codex","side","code/photo-sorter", 3000, [
   ("Add a --dry-run flag that prints what would be renamed.",
    "Added `--dry-run`. It prints old and new names in two columns and exits without touching any file.")]),
 ("claude",None,"", 3400, [
   ("Convert this bash script to fish.",
    "Here is the fish version. The main changes are `set` instead of `export`, and `for x in ...; end` loops.")]),
 ("claude",None,"", 4000, [
   ("Explain how tokenizers handle emoji.",
    "Most tokenizers split an emoji into several byte-level pieces, and combined emoji (skin tones, flags) take more tokens than they look.")]),
 # deleted chat
 ("claude","storefront","code/storefront", 5200, [
   ("Old prototype: scrape the docs site and build a search index.",
    "Here is a rough scraper. You may prefer the official export instead.")]),
 # chats of a deleted workspace
 ("claude","hackathon","code/hackathon", 6100, [
   ("Design the demo flow for the hackathon in under three minutes.",
    "Open on the problem, show the live demo, end on the numbers. Three slides at most.")]),
 ("codex","hackathon","code/hackathon", 6300, [
   ("Draft the pitch and a one-line description.",
    "One line: chats from every coding agent, organised by project. Pitch outline attached.")]),
]

WORKSPACES = [("storefront","Storefront"),("payments-api","Payments API"),("infra","Infrastructure"),("side","Side projects"),("hackathon","Hackathon")]
ws_ids = {k: f"w{i+1}" for i,(k,_) in enumerate(WORKSPACES)}

assign, names, trash = {}, {}, {}
deleted_chat = None
for n,(agent, ws, folder, mins, turns) in enumerate(CHATS):
    sid = str(uuid.uuid5(uuid.NAMESPACE_URL, f"cove-demo-{n}"))
    directory = f"{HOME}/{folder}" if folder else HOME
    when = now - timedelta(minutes=mins)
    def ts(k): return (when + timedelta(seconds=30*k)).strftime("%Y-%m-%dT%H:%M:%S.000Z")
    if agent == "claude":
        p = f"{HOME}/.claude/projects/-demo-{ws or 'misc'}"
        os.makedirs(p, exist_ok=True); path = f"{p}/{sid}.jsonl"
        rows = []
        for k,(u,a) in enumerate(turns):
            rows.append({"type":"user","cwd":directory,"timestamp":ts(2*k),"message":{"role":"user","content":u}})
            rows.append({"type":"assistant","timestamp":ts(2*k+1),"message":{"role":"assistant","content":[{"type":"text","text":a}]}})
    else:
        p = f"{HOME}/.codex/sessions/{when:%Y/%m/%d}"
        os.makedirs(p, exist_ok=True); path = f"{p}/rollout-{when:%Y-%m-%dT%H-%M-%S}-{sid}.jsonl"
        rows = [{"type":"session_meta","payload":{"id":sid,"cwd":directory,"timestamp":ts(0)}}]
        for k,(u,a) in enumerate(turns):
            rows.append({"type":"event_msg","payload":{"type":"user_message","message":u}})
            rows.append({"type":"response_item","payload":{"role":"assistant","content":[{"text":a}]}})
    with open(path,"w") as f:
        for r in rows: f.write(json.dumps(r)+"\n")
    t = when.timestamp(); os.utime(path,(t,t))
    if ws in ws_ids: assign[sid] = ws_ids[ws]
    if ws == "hackathon": trash[sid] = ws_ids["hackathon"]
    if n == 2: names[sid] = "Cart rounding bug"
    if n == 4: names[sid] = "Idempotency keys"
    if n == 21: trash[sid] = ""          # the deleted prototype

state = {
 "workspaces": [{"id": ws_ids[k], "name": name, **({"deleted": True} if k=="hackathon" else {})} for k,name in WORKSPACES],
 "assign": assign, "names": names, "trash": trash, "scope": "*",
}
json.dump(state, open(f"{D}/demo-state.json","w"), indent=1)
print(f"{len(CHATS)} invented chats in {HOME}; state -> tools/demo-state.json")
