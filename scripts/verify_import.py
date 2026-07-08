#!/usr/bin/env python3
"""Deep verification: rdb.sqlite (imported) vs guilds_db.json (source).
Checks CONTENT invariants, not just the report's counters."""
import json, math, sqlite3, os, sys, collections

ROOT = "/Users/jesus/Repos/trophy-bot"
problems = []

def check(cond, msg):
    if not cond:
        problems.append(msg)

with open(f"{ROOT}/guilds_db.json") as f:
    src = json.load(f)
valid = {int(gid): g for gid, g in src.items() if isinstance(g, dict)}

def rha(v):
    return int(math.floor(v + 0.5)) if v >= 0 else int(math.ceil(v - 0.5))

def norm(s):
    n = "".join(c for c in s.lower() if c.isalnum())
    return n if n else s.strip().lower()

db = sqlite3.connect(f"file:{ROOT}/rdb.sqlite?mode=ro", uri=True)
db.row_factory = sqlite3.Row

# --- 1. Guild set equality ---
db_guilds = {r[0] for r in db.execute("SELECT id FROM guilds")}
check(db_guilds == set(valid), f"guild sets differ: only-db={len(db_guilds-set(valid))} only-src={len(set(valid)-db_guilds)}")

# is_safe mapping
for r in db.execute("SELECT id, is_safe FROM guilds"):
    expected = 1 if valid[r[0]].get("imsafe") else 0
    if r[1] != expected:
        problems.append(f"guild {r[0]} is_safe={r[1]} expected {expected}")
        break

# --- 2. Trophies: per-guild counts + full field comparison ---
report = json.load(open(f"{ROOT}/import-report.json"))
renames = {(int(r["guild_id"]), r["legacy_id"]): r["new_name"] for r in report.get("renamed_trophies", [])}

db_trophies = {}
for r in db.execute("SELECT guild_id, legacy_id, name, normalized_name, value, description, emoji, details, signed, creator_user_id, dedication_user_id, dedication_text, image, created_at FROM trophies"):
    db_trophies[(r[0], r[1])] = r

src_trophy_count = 0
mismatch_fields = collections.Counter()
for gid, g in valid.items():
    for tid, t in g.get("trophies", {}).items():
        if tid == "current" or not isinstance(t, dict):
            continue
        src_trophy_count += 1
        row = db_trophies.get((gid, tid))
        if row is None:
            problems.append(f"trophy ({gid},{tid}) missing from DB")
            continue
        exp_name = renames.get((gid, tid), t["name"])
        if row["name"] != exp_name:
            mismatch_fields["name"] += 1
        if len(row["name"]) > 32:
            problems.append(f"trophy ({gid},{tid}) name >32 chars: {row['name']!r}")
        if row["normalized_name"] != norm(row["name"]):
            mismatch_fields["normalized_name"] += 1
        if row["value"] != rha(t.get("value", 10)):
            mismatch_fields["value"] += 1
        if t.get("description") is not None and row["description"] != t["description"]:
            mismatch_fields["description"] += 1
        if t.get("details") is not None and row["details"] != t["details"]:
            mismatch_fields["details"] += 1
        if t.get("emoji") is not None and row["emoji"] != t["emoji"]:
            mismatch_fields["emoji"] += 1
        exp_signed = 1 if t.get("signed") else 0
        if row["signed"] != exp_signed:
            mismatch_fields["signed"] += 1
        exp_creator = int(t["creator"]) if t.get("creator") is not None else None
        if row["creator_user_id"] != exp_creator:
            mismatch_fields["creator"] += 1
        ded = t.get("dedication") or {}
        exp_du = int(ded["user"]) if isinstance(ded, dict) and ded.get("user") else None
        exp_dt = ded.get("name") if isinstance(ded, dict) else None
        if row["dedication_user_id"] != exp_du:
            mismatch_fields["dedication_user"] += 1
        if (row["dedication_text"] or None) != (exp_dt or None):
            mismatch_fields["dedication_text"] += 1
        # image: local kept only if file exists; URLs -> filename or NULL
        img = t.get("image")
        if img is None:
            if row["image"] is not None:
                mismatch_fields["image_null"] += 1
        elif img.startswith("https://"):
            pass  # downloaded-or-NULL, checked via report
        else:
            exists = os.path.exists(f"{ROOT}/images/{img}")
            if exists and row["image"] != img:
                mismatch_fields["image_kept"] += 1
            if not exists and row["image"] is not None:
                mismatch_fields["image_missing_not_null"] += 1

for f, c in mismatch_fields.items():
    problems.append(f"trophy field mismatches: {f} x{c}")

db_count = db.execute("SELECT COUNT(*) FROM trophies").fetchone()[0]
check(db_count == src_trophy_count == 10853, f"trophy counts db={db_count} src={src_trophy_count}")

# --- 3. Normalized-name uniqueness per guild ---
dup = db.execute("SELECT guild_id, normalized_name, COUNT(*) c FROM trophies GROUP BY 1,2 HAVING c>1 LIMIT 5").fetchall()
check(not dup, f"normalized_name duplicates: {[tuple(d) for d in dup]}")

# --- 4. Awards: per (guild,user) counts + score equality (FULL) ---
db_awards = collections.Counter()
for r in db.execute("SELECT guild_id, user_id, COUNT(*) FROM user_trophies GROUP BY 1,2"):
    db_awards[(r[0], r[1])] = r[2]

legacy_ids_by_uuid = {}  # trophy value lookup via join instead
db_scores = {(r[0], r[1]): r[2] for r in db.execute(
    "SELECT ut.guild_id, ut.user_id, COALESCE(SUM(t.value),0) FROM user_trophies ut JOIN trophies t ON t.id=ut.trophy_id GROUP BY 1,2")}

src_awards = 0
score_diff = 0
for gid, g in valid.items():
    tvals = {tid: rha(t.get("value", 10)) for tid, t in g.get("trophies", {}).items() if tid != "current" and isinstance(t, dict)}
    for uid, u in (g.get("users") or {}).items():
        arr = u.get("trophies") or []
        src_awards += len(arr)
        got = db_awards.get((gid, int(uid)), 0)
        if got != len(arr):
            problems.append(f"award count ({gid},{uid}): db={got} src={len(arr)}")
        exp_score = sum(tvals.get(t, 0) for t in arr)
        if arr and db_scores.get((gid, int(uid)), 0) != exp_score:
            score_diff += 1
check(score_diff == 0, f"score mismatches vs rounded source: {score_diff}")
db_award_total = db.execute("SELECT COUNT(*) FROM user_trophies").fetchone()[0]
check(db_award_total == src_awards == 60554, f"award totals db={db_award_total} src={src_awards}")

# awarded_by all NULL, FK integrity
nb = db.execute("SELECT COUNT(*) FROM user_trophies WHERE awarded_by IS NOT NULL").fetchone()[0]
check(nb == 0, f"{nb} imported awards have awarded_by set")
orph = db.execute("SELECT COUNT(*) FROM user_trophies ut LEFT JOIN trophies t ON t.id=ut.trophy_id WHERE t.id IS NULL").fetchone()[0]
check(orph == 0, f"{orph} awards reference missing trophies")

# --- 5. Rewards: dedup(lowest) equality ---
db_rw = collections.defaultdict(dict)
for r in db.execute("SELECT guild_id, role_id, requirement FROM role_rewards"):
    db_rw[r[0]][r[1]] = r[2]
for gid, g in valid.items():
    best = {}
    for rw in g.get("rewards") or []:
        role, req = int(rw["role"]), rw["requirement"]
        if role not in best or req < best[role]:
            best[role] = req
    if db_rw.get(gid, {}) != best:
        problems.append(f"rewards differ for guild {gid}: db={db_rw.get(gid)} expected={best}")

# --- 6. Panels ---
db_panels = {r[0]: (r[1], r[2]) for r in db.execute("SELECT guild_id, channel_id, message_id FROM leaderboard_panels")}
src_panels = {gid: (int(g["panel"]["channel"]), int(g["panel"]["message"])) for gid, g in valid.items() if g.get("panel")}
check(db_panels == src_panels, f"panels differ: {len(set(db_panels.items()) ^ set(src_panels.items()))} entries")

# --- 7. Settings ---
db_settings = {r[0]: dict(zip(["dedication_display","stack_roles","hide_unused_trophies","hide_quit_users","leaderboard_format"], r[1:]))
               for r in db.execute("SELECT guild_id, dedication_display, stack_roles, hide_unused_trophies, hide_quit_users, leaderboard_format FROM guild_settings")}
for gid, g in valid.items():
    s = g.get("settings") or {}
    if s:
        row = db_settings.get(gid)
        if row is None:
            problems.append(f"guild {gid} settings row missing")
            continue
        for k, v in s.items():
            if k in row and row[k] != v:
                problems.append(f"guild {gid} setting {k}: db={row[k]} src={v}")
    else:
        if gid in db_settings and any(v is not None for v in db_settings[gid].values()):
            problems.append(f"guild {gid} has settings row but empty source settings")

# --- 8. bot_stats ---
bot = json.load(open(f"{ROOT}/bot_db.json"))
stats = {r[0]: r[1] for r in db.execute("SELECT name, total FROM bot_stats")}
for k, v in bot["commands"].items():
    if stats.get(k) != v:
        problems.append(f"bot_stats {k}: db={stats.get(k)} src={v}")
        break

print(f"checked: {src_trophy_count} trophies, {src_awards} awards, {len(valid)} guilds, {len(src_panels)} panels")
if problems:
    print(f"PROBLEMS ({len(problems)}):")
    for p in problems[:25]:
        print(" -", p)
    sys.exit(1)
print("DEEP VERIFICATION: NO PROBLEMS DETECTED")
