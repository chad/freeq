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
    "protocol": ("site", "PROTOCOL.md"),
    "features": ("site", "Features.md"),
    "limitations": ("site", "KNOWN-LIMITATIONS.md"),
    "architecture": ("site", "architecture-decisions.md"),
    "s2s": ("site", "s2s-audit.md"),
    "future": ("site", "FutureDirection.md"),
    "web-infra": ("site", "proposal-web-infra.md"),
    "whats-new": ("site", "WHATS-NEW.md"),
    "demo": ("site", "DEMO.md"),
    "encryption": ("site", "ENCRYPTION.md"),
    "bot-quickstart": ("site", "BOT-QUICKSTART.md"),
    "policy-system": ("site", "POLICY.md"),
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
        # Return helpful 404 with debug info
        import json
        info = {
            "slug": slug,
            "source": source,
            "filename": filename,
            "filepath": str(filepath),
            "exists": filepath.exists(),
            "site_docs_dir": str(SITE_DOCS_DIR),
            "site_docs_exists": SITE_DOCS_DIR.exists(),
            "site_docs_files": sorted(f.name for f in SITE_DOCS_DIR.iterdir()) if SITE_DOCS_DIR.exists() else [],
        }
        return f"<pre>Doc not found:\n{json.dumps(info, indent=2)}</pre>", 404
    doc = render_md(filepath)
    return render_template("doc_page.html", doc=doc)


@app.route("/debug/docs")
def debug_docs():
    import json
    result = {
        "site_docs_dir": str(SITE_DOCS_DIR),
        "site_docs_exists": SITE_DOCS_DIR.exists(),
        "site_docs_files": sorted(f.name for f in SITE_DOCS_DIR.iterdir()) if SITE_DOCS_DIR.exists() else [],
        "repo_docs_dir": str(REPO_DOCS_DIR),
        "repo_docs_exists": REPO_DOCS_DIR.exists(),
        "app_file": str(Path(__file__)),
        "cwd": str(Path.cwd()),
    }
    return json.dumps(result, indent=2), 200, {"Content-Type": "application/json"}


@app.route("/favicon.ico")
def favicon():
    return "", 204


if __name__ == "__main__":
    app.run(debug=True, port=8000)
