#!/usr/bin/env python3
"""Build the corpus from the local message cache: every cached HTML body as corpus/<id>.html,
an index (sender, subject) and the list of messages whose body is not cached yet.

Reads mail.db read-only. The corpus is your private mail: it lives under target/ (or
$RENDER_CORPUS), never in git.

usage: corpus.py [path/to/mail.db]
"""
import json, os, sqlite3, sys
from common import CORPUS


def default_db():
    if os.environ.get("XDG_DATA_HOME"):
        return os.path.join(os.environ["XDG_DATA_HOME"], "cosmic-mail", "mail.db")
    if sys.platform == "darwin":
        return os.path.expanduser("~/Library/Application Support/cosmic-mail/mail.db")
    if os.name == "nt":
        return os.path.join(os.environ.get("APPDATA", ""), "cosmic-mail", "mail.db")
    return os.path.expanduser("~/.local/share/cosmic-mail/mail.db")


db_path = sys.argv[1] if len(sys.argv) > 1 else default_db()
db = sqlite3.connect("file:" + db_path + "?mode=ro", uri=True)
os.makedirs(CORPUS, exist_ok=True)
index, exported = {}, 0
for id_, subject, from_name, from_addr, date, html in db.execute(
    "select id, subject, from_name, from_addr, date, body_html from messages"
):
    index[id_] = {"subject": subject, "from_name": from_name, "from_addr": from_addr, "date": date}
    if html and html.strip():
        with open(os.path.join(CORPUS, f"{id_}.html"), "w") as f:
            f.write(html)
        exported += 1
with open(os.path.join(CORPUS, "index.json"), "w") as f:
    json.dump(index, f, indent=1)
uncached = db.execute(
    "select m.id, m.uid, f.account_id, f.name from messages m join folders f on f.id = m.folder_id "
    "where m.body_cached = 0 order by f.account_id, f.name, m.uid"
).fetchall()
with open(os.path.join(CORPUS, "uncached.tsv"), "w") as f:
    for row in uncached:
        f.write("\t".join(str(x) for x in row) + "\n")
print(f"{exported} cached HTML bodies -> {CORPUS}")
if uncached:
    print(f"{len(uncached)} messages have no cached body. To fetch them (read-only, BODY.PEEK):")
    print(f"  cargo run --example mail_corpus_fetch -- {CORPUS}/uncached.tsv {CORPUS}")
