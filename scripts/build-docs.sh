#!/bin/bash
# Convert markdown docs to styled HTML pages.
# Requires: npx (for marked CLI) — or fallback to simple pre-formatted.
# Output: freeq-app/public/docs/*.html
set -e

DOCS_DIR="$(dirname "$0")/../docs"
OUT_DIR="$(dirname "$0")/../freeq-app/public/docs"
mkdir -p "$OUT_DIR"

# HTML template
render_doc() {
  local md_file="$1"
  local slug="$(basename "$md_file" .md | tr '[:upper:]' '[:lower:]')"
  local title="$(head -1 "$md_file" | sed 's/^[# ]*//')"
  local out_file="$OUT_DIR/$slug.html"
  
  # Try marked CLI, fall back to simple conversion
  if command -v npx &>/dev/null; then
    local body=$(npx marked --gfm < "$md_file" 2>/dev/null || cat "$md_file")
  else
    local body=$(cat "$md_file")
  fi

  cat > "$out_file" << HTMLEOF
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>${title} — freeq</title>
  <style>
    :root { --bg: #0c0c0f; --fg: #e4e4e7; --fg-dim: #71717a; --accent: #818cf8; --surface: #18181b; --border: #27272a; }
    * { box-sizing: border-box; margin: 0; padding: 0; }
    body { background: var(--bg); color: var(--fg); font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif; line-height: 1.7; }
    .container { max-width: 800px; margin: 0 auto; padding: 2rem 1.5rem 4rem; }
    .nav { display: flex; align-items: center; gap: 1rem; padding: 1rem 0 2rem; border-bottom: 1px solid var(--border); margin-bottom: 2rem; }
    .nav a { color: var(--accent); text-decoration: none; font-size: 14px; }
    .nav a:hover { text-decoration: underline; }
    .nav .brand { font-weight: 700; font-size: 18px; color: var(--accent); }
    h1 { font-size: 2rem; margin: 0 0 1rem; color: var(--fg); }
    h2 { font-size: 1.4rem; margin: 2rem 0 0.75rem; color: var(--fg); border-bottom: 1px solid var(--border); padding-bottom: 0.5rem; }
    h3 { font-size: 1.1rem; margin: 1.5rem 0 0.5rem; color: var(--fg); }
    p { margin: 0.75rem 0; }
    a { color: var(--accent); }
    code { background: var(--surface); padding: 0.15em 0.4em; border-radius: 4px; font-size: 0.9em; font-family: 'SF Mono', 'Fira Code', monospace; }
    pre { background: var(--surface); border: 1px solid var(--border); border-radius: 8px; padding: 1rem; overflow-x: auto; margin: 1rem 0; }
    pre code { background: none; padding: 0; font-size: 0.85em; }
    table { width: 100%; border-collapse: collapse; margin: 1rem 0; }
    th, td { text-align: left; padding: 0.5rem 0.75rem; border: 1px solid var(--border); }
    th { background: var(--surface); font-weight: 600; }
    ul, ol { padding-left: 1.5rem; margin: 0.5rem 0; }
    li { margin: 0.25rem 0; }
    blockquote { border-left: 3px solid var(--accent); padding-left: 1rem; color: var(--fg-dim); margin: 1rem 0; }
    hr { border: none; border-top: 1px solid var(--border); margin: 2rem 0; }
    strong { color: #fff; }
    img { max-width: 100%; }
  </style>
</head>
<body>
  <div class="container">
    <nav class="nav">
      <span class="brand">freeq</span>
      <a href="/">← Back to app</a>
      <a href="/docs/">All docs</a>
    </nav>
    ${body}
  </div>
</body>
</html>
HTMLEOF
  echo "  $slug.html"
}

# Build index page
build_index() {
  local out_file="$OUT_DIR/index.html"
  local items=""
  for f in "$DOCS_DIR"/*.md; do
    local slug="$(basename "$f" .md | tr '[:upper:]' '[:lower:]')"
    local title="$(head -1 "$f" | sed 's/^[# ]*//')"
    items="$items<li><a href=\"/docs/${slug}.html\">${title}</a></li>"
  done
  
  cat > "$out_file" << HTMLEOF
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>Documentation — freeq</title>
  <style>
    :root { --bg: #0c0c0f; --fg: #e4e4e7; --fg-dim: #71717a; --accent: #818cf8; --surface: #18181b; --border: #27272a; }
    * { box-sizing: border-box; margin: 0; padding: 0; }
    body { background: var(--bg); color: var(--fg); font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', system-ui, sans-serif; line-height: 1.7; }
    .container { max-width: 800px; margin: 0 auto; padding: 2rem 1.5rem 4rem; }
    .nav { display: flex; align-items: center; gap: 1rem; padding: 1rem 0 2rem; border-bottom: 1px solid var(--border); margin-bottom: 2rem; }
    .nav a { color: var(--accent); text-decoration: none; font-size: 14px; }
    .nav a:hover { text-decoration: underline; }
    .nav .brand { font-weight: 700; font-size: 18px; color: var(--accent); }
    h1 { font-size: 2rem; margin: 0 0 1rem; }
    ul { list-style: none; padding: 0; }
    li { padding: 0.75rem 0; border-bottom: 1px solid var(--border); }
    li a { color: var(--accent); text-decoration: none; font-size: 1.1rem; }
    li a:hover { text-decoration: underline; }
    p { color: var(--fg-dim); margin: 0.5rem 0 1.5rem; }
  </style>
</head>
<body>
  <div class="container">
    <nav class="nav">
      <span class="brand">freeq</span>
      <a href="/">← Back to app</a>
    </nav>
    <h1>Documentation</h1>
    <p>Guides, protocols, and architecture for freeq — IRC with AT Protocol identity.</p>
    <ul>${items}</ul>
  </div>
</body>
</html>
HTMLEOF
  echo "  index.html"
}

echo "Building docs..."
for f in "$DOCS_DIR"/*.md; do
  render_doc "$f"
done
build_index
echo "Done! Output in $OUT_DIR"
