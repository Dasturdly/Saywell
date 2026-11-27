"""Seed / read the Saywell test profile database (E2E helper). Usage:
   e2e_db.py seed <db> <terms comma-separated> <find=>replace>
   e2e_db.py history <db>"""
import sqlite3, sys
mode, db = sys.argv[1], sys.argv[2]
c = sqlite3.connect(db)
if mode == "seed":
    c.executescript("""
      CREATE TABLE IF NOT EXISTS history (id INTEGER PRIMARY KEY, ts INTEGER NOT NULL, app TEXT NOT NULL DEFAULT '',
        raw TEXT NOT NULL, text TEXT NOT NULL, audio_ms INTEGER NOT NULL DEFAULT 0, latency_ms INTEGER NOT NULL DEFAULT 0);
      CREATE TABLE IF NOT EXISTS dict_terms (id INTEGER PRIMARY KEY, term TEXT NOT NULL UNIQUE COLLATE NOCASE);
      CREATE TABLE IF NOT EXISTS dict_rules (id INTEGER PRIMARY KEY, find TEXT NOT NULL UNIQUE COLLATE NOCASE, replace TEXT NOT NULL);""")
    for t in filter(None, sys.argv[3].split(",")):
        c.execute("INSERT OR IGNORE INTO dict_terms (term) VALUES (?)", (t,))
    if len(sys.argv) > 4 and "=>" in sys.argv[4]:
        f, r = sys.argv[4].split("=>", 1)
        c.execute("INSERT OR REPLACE INTO dict_rules (find, replace) VALUES (?, ?)", (f, r))
    c.commit()
else:
    for row in c.execute("SELECT app, text, audio_ms FROM history ORDER BY id"):
        print("history  :", row)
