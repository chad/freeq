//! Visual content the bot projects onto its tile, on top of the
//! ghostly face: scene cards (title + bullets), file slices, status
//! chips. All overlays render as small inline SVG documents that the
//! face renderer composites over the particle field.

use std::sync::Arc;

use freeq_eliza::whiteboard::{Step, TextSize};
use resvg::tiny_skia::{Pixmap, PixmapPaint, Transform};
use resvg::usvg;

/// Active overlay, swapped atomically by the MCP tools.
#[derive(Clone, Debug, Default)]
pub enum TileOverlay {
    #[default]
    None,
    /// Status chip — small text + glyph in a corner.
    Status { label: String },
    /// Scene card — title + bullet list, fills the lower half.
    Card { title: String, bullets: Vec<String> },
    /// Quote card — large pulled-out text, fills the lower half.
    Quote { text: String, source: Option<String> },
    /// File slice — monospace code rendered as the tile.
    File { path: String, lines: Vec<String>, line_start: u32 },
    /// Live whiteboard — pre-laid-out nodes + arrows from the
    /// orchestrator's accumulated SVO triples.
    Graph { steps: Vec<Step> },
}

/// Shared, thread-safe overlay slot. The MCP tools write to it; the
/// face render thread reads on each frame.
pub type OverlayCell = Arc<std::sync::Mutex<TileOverlay>>;

pub fn new_overlay_cell() -> OverlayCell {
    Arc::new(std::sync::Mutex::new(TileOverlay::None))
}

/// Rasterize the overlay onto an existing pixmap (the particle face
/// frame). No-op when the overlay is `None`.
pub fn composite_overlay(
    overlay: &TileOverlay,
    pixmap: &mut Pixmap,
    opt: &usvg::Options,
    scratch: &mut Pixmap,
) {
    let svg = match overlay_svg(overlay, pixmap.width(), pixmap.height()) {
        Some(s) => s,
        None => return,
    };
    let Ok(tree) = usvg::Tree::from_str(&svg, opt) else {
        tracing::debug!("overlay SVG parse failed");
        return;
    };
    scratch.data_mut().fill(0);
    resvg::render(&tree, Transform::identity(), &mut scratch.as_mut());
    pixmap.draw_pixmap(
        0,
        0,
        scratch.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
}

fn overlay_svg(overlay: &TileOverlay, w: u32, h: u32) -> Option<String> {
    match overlay {
        TileOverlay::None => None,
        TileOverlay::Status { label } => Some(status_svg(label, w, h)),
        TileOverlay::Card { title, bullets } => Some(card_svg(title, bullets, w, h)),
        TileOverlay::Quote { text, source } => Some(quote_svg(text, source.as_deref(), w, h)),
        TileOverlay::File { path, lines, line_start } => {
            Some(file_svg(path, lines, *line_start, w, h))
        }
        TileOverlay::Graph { steps } => {
            if steps.is_empty() {
                None
            } else {
                Some(graph_svg(steps, w, h))
            }
        }
    }
}

fn status_svg(label: &str, w: u32, _h: u32) -> String {
    let label = escape(label);
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="36" viewBox="0 0 {w} 36">
  <rect x="12" y="8" width="180" height="22" rx="11" fill="#000c" />
  <circle cx="28" cy="19" r="4" fill="#7cf3a0" />
  <text x="42" y="24" font-family="-apple-system, Helvetica, sans-serif" font-size="13" fill="#e6f3ff">{label}</text>
</svg>"##
    )
}

fn card_svg(title: &str, bullets: &[String], w: u32, h: u32) -> String {
    let title = escape(title);
    let card_top = h / 2;
    let card_h = h - card_top - 16;
    let mut body = String::new();
    let mut y = (card_top + 56) as i32;
    for (i, b) in bullets.iter().take(6).enumerate() {
        let bullet = escape(b);
        body.push_str(&format!(
            r##"<circle cx="32" cy="{cy}" r="3.5" fill="#7cb5ff" />
<text x="48" y="{ty}" font-family="-apple-system, Helvetica, sans-serif" font-size="16" fill="#e6f3ff">{bullet}</text>"##,
            cy = y - 5,
            ty = y,
        ));
        y += 28;
        let _ = i;
    }
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">
  <rect x="16" y="{ct}" width="{cw}" height="{ch}" rx="14" fill="#000d" stroke="#7cb5ff44" stroke-width="1.5" />
  <text x="32" y="{tt}" font-family="-apple-system, Helvetica, sans-serif" font-size="18" font-weight="600" fill="#7cb5ff">{title}</text>
  {body}
</svg>"##,
        ct = card_top,
        cw = w - 32,
        ch = card_h,
        tt = card_top + 32,
    )
}

fn quote_svg(text: &str, source: Option<&str>, w: u32, h: u32) -> String {
    let text = escape(text);
    let card_top = h / 2;
    let card_h = h - card_top - 16;
    let source_block = match source {
        Some(s) => format!(
            r##"<text x="{tx}" y="{sy}" font-family="-apple-system, Helvetica, sans-serif" font-size="13" fill="#7cb5ff" text-anchor="end">— {}</text>"##,
            escape(s),
            tx = w - 32,
            sy = card_top + card_h - 18,
        ),
        None => String::new(),
    };
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">
  <rect x="16" y="{ct}" width="{cw}" height="{ch}" rx="14" fill="#000d" stroke="#7cb5ff44" stroke-width="1.5" />
  <text x="36" y="{ty}" font-family="Georgia, serif" font-size="19" font-style="italic" fill="#e6f3ff">{text}</text>
  {source_block}
</svg>"##,
        ct = card_top,
        cw = w - 32,
        ch = card_h,
        ty = card_top + 56,
    )
}

fn file_svg(path: &str, lines: &[String], line_start: u32, w: u32, h: u32) -> String {
    let path = escape(path);
    let inner_h = h - 32;
    let mut body = String::new();
    let max_lines = ((inner_h - 36) / 16) as usize;
    let visible = lines.iter().take(max_lines);
    let mut y = 52;
    for (i, line) in visible.enumerate() {
        // Truncate at ~80 visible chars to keep lines from overflowing.
        let trimmed = if line.chars().count() > 78 {
            line.chars().take(75).collect::<String>() + "…"
        } else {
            line.to_string()
        };
        let n = line_start + i as u32;
        body.push_str(&format!(
            r##"<text x="20" y="{y}" font-family="ui-monospace, Menlo, monospace" font-size="11" fill="#566876">{n:>4}</text>
<text x="60" y="{y}" font-family="ui-monospace, Menlo, monospace" font-size="11" fill="#d6e3f3">{}</text>"##,
            escape(&trimmed),
            y = y,
        ));
        y += 16;
    }
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">
  <rect x="0" y="0" width="{w}" height="{h}" fill="#0a0e15ee" />
  <text x="20" y="22" font-family="ui-monospace, Menlo, monospace" font-size="12" fill="#7cb5ff">{path}</text>
  <line x1="0" y1="32" x2="{w}" y2="32" stroke="#7cb5ff33" stroke-width="1" />
  {body}
</svg>"##,
    )
}

fn graph_svg(steps: &[Step], w: u32, h: u32) -> String {
    // Steps are positioned for a 640×360 canvas (see
    // freeq_eliza::diagram::to_steps). Our tile is also 640×360, so we
    // pass through. If the dimensions differed we'd scale here.
    let _ = (w, h);
    let mut body = String::new();
    // Defs once for the arrowhead.
    body.push_str(
        r##"<defs><marker id="ah" viewBox="0 0 10 10" refX="8" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0,0 L10,5 L0,10 Z" fill="#7cb5ff"/></marker></defs>"##,
    );
    // Translucent dark canvas behind the graph so it reads over the face.
    body.push_str(&format!(
        r##"<rect x="0" y="0" width="{w}" height="{h}" fill="#000a" />"##
    ));
    // Title chip.
    body.push_str(
        r##"<rect x="12" y="8" width="120" height="22" rx="11" fill="#000c" />
<text x="28" y="24" font-family="-apple-system, Helvetica, sans-serif" font-size="13" fill="#7cb5ff">whiteboard</text>"##,
    );
    // Draw arrows first (under boxes) so labels read on top.
    for s in steps {
        if let Step::Arrow { x1, y1, x2, y2, label } = s {
            body.push_str(&format!(
                r##"<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="#7cb5ff" stroke-width="1.6" marker-end="url(#ah)" opacity="0.85" />"##,
            ));
            if let Some(lab) = label {
                let mx = (x1 + x2) / 2.0;
                let my = (y1 + y2) / 2.0 - 6.0;
                body.push_str(&format!(
                    r##"<rect x="{rx:.1}" y="{ry:.1}" width="{rw:.1}" height="14" rx="3" fill="#000c" />
<text x="{mx:.1}" y="{my:.1}" text-anchor="middle" font-family="-apple-system, Helvetica, sans-serif" font-size="11" fill="#d6e3f3">{lab}</text>"##,
                    rx = mx - (lab.chars().count() as f32 * 3.2) - 4.0,
                    ry = my - 11.0,
                    rw = (lab.chars().count() as f32 * 6.4) + 8.0,
                    lab = escape(lab),
                ));
            }
        }
    }
    // Then boxes.
    for s in steps {
        if let Step::Box { x, y, w: bw, h: bh, label } = s {
            body.push_str(&format!(
                r##"<rect x="{x}" y="{y}" width="{bw}" height="{bh}" rx="8" fill="#0a1422ee" stroke="#7cb5ff" stroke-width="1.4" />
<text x="{cx:.1}" y="{cy:.1}" text-anchor="middle" font-family="-apple-system, Helvetica, sans-serif" font-size="14" fill="#e6f3ff">{label}</text>"##,
                cx = x + bw / 2.0,
                cy = y + bh / 2.0 + 5.0,
                label = escape(label),
            ));
        }
    }
    // Free-floating text steps (titles, captions).
    for s in steps {
        if let Step::Text { x, y, content, size } = s {
            let px = match size {
                TextSize::Small => 12,
                TextSize::Med => 16,
                TextSize::Large => 22,
            };
            body.push_str(&format!(
                r##"<text x="{x}" y="{y}" text-anchor="middle" font-family="-apple-system, Helvetica, sans-serif" font-size="{px}" fill="#e6f3ff">{c}</text>"##,
                c = escape(content),
            ));
        }
    }
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">{body}</svg>"##
    )
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}
