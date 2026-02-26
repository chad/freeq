"""freeq.at — static site with markdown docs rendering."""

import os
from pathlib import Path

from flask import Flask, render_template, abort, send_from_directory
import markdown
from markdown.extensions.codehilite import CodeHiliteExtension
from markdown.extensions.fenced_code import FencedCodeExtension
from markdown.extensions.tables import TableExtension
from markdown.extensions.toc import TocExtension

app = Flask(__name__)

# Docs directory — site docs/ and repo docs/
SITE_DOCS_DIR = Path(__file__).parent / "docs"
REPO_DOCS_DIR = Path(__file__).parent.parent / "docs"

# Markdown renderer
MD_EXTENSIONS = [
    FencedCodeExtension(),
    CodeHiliteExtension(css_class="highlight", guess_lang=False),
    TableExtension(),
    TocExtension(permalink=True),
    "nl2br",
]


def render_md(filepath: Path) -> dict:
    """Render a markdown file, return {html, toc, title}."""
    text = filepath.read_text()
    md = markdown.Markdown(extensions=MD_EXTENSIONS)
    html = md.convert(text)
    toc = getattr(md, "toc", "")
    # Extract title from first H1
    title = "freeq"
    for line in text.splitlines():
        if line.startswith("# "):
            title = line[2:].strip()
            break
    md.reset()
    return {"html": html, "toc": toc, "title": title}


# ── Slug → file mapping ──────────────────────────────────────────

SLUG_MAP = {
    # Site docs (new content)
    "what-is-freeq": ("site", "what-is-freeq.md"),
    "getting-started": ("site", "getting-started.md"),
    "authentication": ("site", "authentication.md"),
    "web-client": ("site", "web-client.md"),
    "ios-app": ("site", "ios-app.md"),
    "bots": ("site", "bots.md"),
    "policy-framework": ("site", "policy-framework.md"),
    "verifiers": ("site", "verifiers.md"),
    "moderation": ("site", "moderation.md"),
    "federation": ("site", "federation.md"),
    "self-hosting": ("site", "self-hosting.md"),
    "api-reference": ("site", "api-reference.md"),
    # Repo docs (existing technical docs)
    "protocol": ("repo", "PROTOCOL.md"),
    "features": ("repo", "Features.md"),
    "limitations": ("repo", "KNOWN-LIMITATIONS.md"),
    "architecture": ("repo", "architecture-decisions.md"),
    "s2s": ("repo", "s2s-audit.md"),
    "future": ("repo", "FutureDirection.md"),
    "web-infra": ("repo", "proposal-web-infra.md"),
    "whats-new": ("repo", "WHATS-NEW.md"),
    "demo": ("site", "demo.md"),
    "encryption": ("repo", "ENCRYPTION.md"),
}


# ── Routes ────────────────────────────────────────────────────────


@app.route("/")
def index():
    return render_template("index.html")


@app.route("/connect/")
def connect():
    return render_template("connect.html")


@app.route("/sdk/")
def sdk():
    return render_template("sdk.html")


@app.route("/about/")
def about():
    return render_template("about.html")


@app.route("/docs/")
def docs_index():
    return render_template("docs_index.html")


@app.route("/docs/<path:slug>/")
def docs_page(slug):
    """Render a doc page from either site or repo docs."""
    entry = SLUG_MAP.get(slug)
    if not entry:
        abort(404)
    source, filename = entry
    if source == "site":
        filepath = SITE_DOCS_DIR / filename
    else:
        filepath = REPO_DOCS_DIR / filename
    if not filepath.exists():
        abort(404)
    doc = render_md(filepath)
    return render_template("doc_page.html", doc=doc)


@app.route("/favicon.ico")
def favicon():
    return "", 204


if __name__ == "__main__":
    app.run(debug=True, port=8000)
