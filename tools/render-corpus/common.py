"""Where the render-corpus tools keep their data."""
import os

TOOLS = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(TOOLS))
# The working directory: corpus/ (the mails) and out/ (renders). Never in git.
WORK = os.environ.get("RENDER_CORPUS") or os.path.join(REPO, "target", "render-corpus")
CORPUS = os.path.join(WORK, "corpus")
OUT = os.path.join(WORK, "out")
