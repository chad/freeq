//! Live diagram from speech — accumulates a `Diagram` of nodes
//! (concepts mentioned) and edges (relationships asserted) extracted
//! from the running transcript. Drives the "I drew what we were
//! saying" demo: as the room talks, a graph quietly forms on the
//! whiteboard.
//!
//! Extraction is deliberately deterministic and small — a verb-frame
//! triple parser ("X verbs Y") that handles the highest-signal
//! shapes: simple SVO, multi-word noun phrases, and a handful of
//! common verb compounds ("depends on", "talks to"). An LLM enrichment
//! pass is a natural extension that wraps the deterministic core.

use std::collections::BTreeMap;

/// One node in the diagram — a concept or entity the room mentioned.
/// Identity is its canonical (lowercased, trimmed) label so repeated
/// mentions collapse to a single node regardless of casing.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Node {
    pub label: String,
}

/// One directed relationship — `from` ──`relation`──▶ `to`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Edge {
    pub from: String,
    pub relation: String,
    pub to: String,
}

/// Accumulating diagram. Nodes are kept in a BTreeMap keyed by
/// canonical label → display label; edges are deduplicated by their
/// full `(from, relation, to)` triple.
#[derive(Debug, Default, Clone)]
pub struct Diagram {
    nodes: BTreeMap<String, String>,
    edges: Vec<Edge>,
}

impl Diagram {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed `text` into the diagram. Extracts any triples it can find
    /// and merges them in. Returns the count of newly-added edges (0
    /// means nothing was extracted, or everything matched an existing
    /// edge).
    pub fn ingest(&mut self, text: &str) -> usize {
        let mut added = 0;
        for triple in extract_triples(text) {
            if self.insert_triple(&triple.0, &triple.1, &triple.2) {
                added += 1;
            }
        }
        added
    }

    /// True if a node with this canonical label exists.
    pub fn has_node(&self, label: &str) -> bool {
        self.nodes.contains_key(&canonical(label))
    }

    /// True if this exact directed triple exists.
    pub fn has_edge(&self, from: &str, relation: &str, to: &str) -> bool {
        let f = canonical(from);
        let r = canonical(relation);
        let t = canonical(to);
        self.edges
            .iter()
            .any(|e| e.from == f && e.relation == r && e.to == t)
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn edge_count(&self) -> usize {
        self.edges.len()
    }

    pub fn nodes(&self) -> impl Iterator<Item = &str> {
        self.nodes.values().map(String::as_str)
    }

    pub fn edges(&self) -> impl Iterator<Item = &Edge> {
        self.edges.iter()
    }

    /// Render the diagram as whiteboard steps the existing tile
    /// renderer already understands. Layout is a simple grid: nodes
    /// are placed in row-major order across the 640×360 canvas, edges
    /// are drawn from box-edge to box-edge with the relation as the
    /// arrow label.
    pub fn to_steps(&self) -> Vec<crate::whiteboard::Step> {
        use crate::whiteboard::Step;
        const CANVAS_W: f32 = 640.0;
        const SAFE_LEFT: f32 = 60.0;
        const SAFE_TOP: f32 = 80.0;
        const BOX_W: f32 = 140.0;
        const BOX_H: f32 = 50.0;
        const COL_GAP: f32 = 40.0;
        const ROW_GAP: f32 = 70.0;

        // Centre coords keyed by canonical label so the edge pass can
        // look up endpoints by `Edge::from` / `Edge::to`.
        let mut centres: BTreeMap<&str, (f32, f32)> = BTreeMap::new();
        let mut steps: Vec<Step> = Vec::new();

        let cols = ((CANVAS_W - 2.0 * SAFE_LEFT) / (BOX_W + COL_GAP)).max(1.0) as usize;
        let cols = cols.max(1);
        for (i, (canon, display)) in self.nodes.iter().enumerate() {
            let col = i % cols;
            let row = i / cols;
            let x = SAFE_LEFT + col as f32 * (BOX_W + COL_GAP);
            let y = SAFE_TOP + row as f32 * (BOX_H + ROW_GAP);
            steps.push(Step::Box {
                x,
                y,
                w: BOX_W,
                h: BOX_H,
                label: display.clone(),
            });
            centres.insert(canon.as_str(), (x + BOX_W / 2.0, y + BOX_H / 2.0));
        }

        for edge in &self.edges {
            let (Some(&(x1, y1)), Some(&(x2, y2))) =
                (centres.get(edge.from.as_str()), centres.get(edge.to.as_str()))
            else {
                continue;
            };
            steps.push(Step::Arrow {
                x1,
                y1,
                x2,
                y2,
                label: Some(edge.relation.clone()),
            });
        }
        steps
    }

    fn insert_triple(&mut self, from: &str, relation: &str, to: &str) -> bool {
        let from_c = canonical(from);
        let rel_c = canonical(relation);
        let to_c = canonical(to);
        if from_c.is_empty() || rel_c.is_empty() || to_c.is_empty() {
            return false;
        }
        self.nodes
            .entry(from_c.clone())
            .or_insert_with(|| display(from));
        self.nodes
            .entry(to_c.clone())
            .or_insert_with(|| display(to));
        if self
            .edges
            .iter()
            .any(|e| e.from == from_c && e.relation == rel_c && e.to == to_c)
        {
            return false;
        }
        self.edges.push(Edge {
            from: from_c,
            relation: rel_c,
            to: to_c,
        });
        true
    }
}

fn canonical(s: &str) -> String {
    s.trim().to_ascii_lowercase()
}

fn display(s: &str) -> String {
    s.trim().to_string()
}

/// Verbs (or verb compounds) we recognise as relation phrases. Order
/// matters — multi-word entries must appear before their shorter
/// prefixes so "depends on" wins over a hypothetical "depends".
const RELATIONS: &[&str] = &[
    " depends on ",
    " talks to ",
    " calls ",
    " uses ",
    " owns ",
    " deploys to ",
    " deploys ",
    " writes to ",
    " reads from ",
    " sends to ",
    " sends ",
    " connects to ",
    " connects ",
    " runs on ",
    " runs ",
    " ships ",
    " pulls ",
    " pushes to ",
    " pushes ",
    " hosts ",
    " serves ",
    " builds ",
];

/// Tokens we strip from the leading edge of a subject/object so noun
/// phrases like "the staging cluster" become "staging cluster".
const ARTICLES: &[&str] = &["the ", "a ", "an ", "our ", "their "];

fn split_sentences(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| c == '.' || c == '?' || c == '!' || c == ';' || c == '\n')
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Pull every recognisable `(subject, relation, object)` triple out
/// of `text`. Handles three shapes:
///   1. simple SVO: "X calls Y"
///   2. multi-clause via "and": "X calls Y and writes to Z"
///      (subject is carried across)
///   3. list object: "X depends on Y, Z, and W"
///      (one edge per item in the list)
fn extract_triples(text: &str) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for sentence in split_sentences(text) {
        extract_from_sentence(sentence, &mut out);
    }
    out
}

/// Find the earliest relation match in `lower`. Returns `(start, rel)`
/// or `None`. We pick earliest so "X uses Y depends on Z" doesn't
/// misorder; for ties (one relation prefix-matches another) the longer
/// wins.
fn first_relation(lower: &str) -> Option<(usize, &'static str)> {
    let mut best: Option<(usize, &'static str)> = None;
    for rel in RELATIONS {
        if let Some(pos) = lower.find(rel) {
            best = match best {
                Some((bp, br)) if bp < pos => Some((bp, br)),
                Some((bp, br)) if bp == pos && br.len() >= rel.len() => Some((bp, br)),
                _ => Some((pos, *rel)),
            };
        }
    }
    best
}

fn extract_from_sentence(sentence: &str, out: &mut Vec<(String, String, String)>) {
    let padded = format!(" {sentence} ").to_ascii_lowercase();
    let Some((pos, rel)) = first_relation(&padded) else {
        return;
    };
    let subject = strip_article(padded[..pos].trim()).to_string();
    let rest = padded[pos + rel.len()..].trim().to_string();
    let relation = rel.trim().to_string();
    if subject.is_empty() {
        return;
    }

    // The right-hand side may be a single object, a comma+and list of
    // objects ("Y, Z, and W"), or chained clauses ("Y and writes to
    // Z"). Split on ", " and " and " — each piece is either a fresh
    // clause (starts with a known relation phrase) or a bare object
    // that attaches to the current relation.
    for piece in split_object_phrase(&rest) {
        let piece = piece.trim();
        if piece.is_empty() {
            continue;
        }
        // Look for a relation at the *start* of the piece. If found,
        // this is an elided-subject clause: "(subject) writes to Z".
        let padded_piece = format!(" {piece} ");
        if let Some((p_pos, p_rel)) = first_relation(&padded_piece) {
            // A relation right at the leading edge of the piece (pos 0
            // in padded == 1, since we prepended a space) means the
            // piece IS a clause, not a bare object. Otherwise the
            // relation appears mid-piece — that doesn't happen with
            // our splits, but treat it as a bare object to be safe.
            if p_pos <= 1 {
                let p_relation = p_rel.trim().to_string();
                let p_object_raw = padded_piece[p_pos + p_rel.len()..].trim();
                let p_object = trim_trailing_punct(strip_article(p_object_raw));
                if !p_object.is_empty() {
                    out.push((subject.clone(), p_relation, p_object.to_string()));
                }
                continue;
            }
        }
        // Bare object — attach to the relation we found at the front
        // of the sentence.
        let obj = trim_trailing_punct(strip_article(piece));
        if !obj.is_empty() {
            out.push((subject.clone(), relation.clone(), obj.to_string()));
        }
    }
}

/// Split an object phrase into pieces. Recognises both comma-separated
/// lists and " and " conjunctions, including the Oxford-comma "Y, Z,
/// and W" shape where the final separator is both.
fn split_object_phrase(phrase: &str) -> Vec<String> {
    // Normalise ", and " to ", " so the comma-split alone catches the
    // Oxford comma case; then split any remaining " and " inside each
    // comma piece (handles plain "X and Y" without commas).
    let normalised = phrase.replace(", and ", ", ");
    let mut out = Vec::new();
    for chunk in normalised.split(", ") {
        for sub in chunk.split(" and ") {
            out.push(sub.to_string());
        }
    }
    out
}

fn strip_article(s: &str) -> &str {
    let lower = s;
    for a in ARTICLES {
        if let Some(rest) = lower.strip_prefix(a) {
            return rest.trim();
        }
    }
    s.trim()
}

fn trim_trailing_punct(s: &str) -> &str {
    s.trim_end_matches(|c: char| matches!(c, ',' | ':' | '"' | '\''))
        .trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_no_nodes_or_edges() {
        let mut d = Diagram::new();
        assert_eq!(d.ingest(""), 0);
        assert_eq!(d.node_count(), 0);
        assert_eq!(d.edge_count(), 0);
    }

    #[test]
    fn simple_svo_creates_two_nodes_one_edge() {
        let mut d = Diagram::new();
        assert_eq!(d.ingest("the api calls the database"), 1);
        assert!(d.has_node("api"));
        assert!(d.has_node("database"));
        assert!(d.has_edge("api", "calls", "database"));
        assert_eq!(d.edge_count(), 1);
    }

    #[test]
    fn multi_sentence_input_accumulates() {
        let mut d = Diagram::new();
        d.ingest("the api calls the database. the worker reads from the queue.");
        assert!(d.has_edge("api", "calls", "database"));
        assert!(d.has_edge("worker", "reads from", "queue"));
        assert_eq!(d.edge_count(), 2);
    }

    #[test]
    fn duplicate_edge_does_not_double_count() {
        let mut d = Diagram::new();
        d.ingest("the api calls the database");
        d.ingest("the api calls the database");
        assert_eq!(d.edge_count(), 1);
        assert_eq!(d.node_count(), 2);
    }

    #[test]
    fn shared_node_appears_once() {
        let mut d = Diagram::new();
        d.ingest("the api calls the database. the worker calls the database.");
        // database is shared between two edges
        assert_eq!(d.node_count(), 3);
        assert_eq!(d.edge_count(), 2);
    }

    #[test]
    fn multi_word_relation_depends_on() {
        let mut d = Diagram::new();
        d.ingest("the bot depends on the renderer");
        assert!(d.has_edge("bot", "depends on", "renderer"));
    }

    #[test]
    fn idle_chatter_yields_nothing() {
        let mut d = Diagram::new();
        // No recognised relation verb.
        d.ingest("hey how are you doing today");
        assert_eq!(d.edge_count(), 0);
    }

    #[test]
    fn and_clause_expands_to_two_edges_sharing_subject() {
        let mut d = Diagram::new();
        d.ingest("the api calls the database and writes to the queue");
        assert!(
            d.has_edge("api", "calls", "database"),
            "first clause missing"
        );
        assert!(
            d.has_edge("api", "writes to", "queue"),
            "second clause missing — subject should carry across the `and`"
        );
        assert_eq!(d.edge_count(), 2);
        assert_eq!(d.node_count(), 3);
    }

    #[test]
    fn list_object_expands_to_one_edge_per_item() {
        // "X depends on Y, Z, and W" — the three objects each become
        // their own edge from X.
        let mut d = Diagram::new();
        d.ingest("the bot depends on the renderer, the stt, and the tts");
        assert!(d.has_edge("bot", "depends on", "renderer"));
        assert!(d.has_edge("bot", "depends on", "stt"));
        assert!(d.has_edge("bot", "depends on", "tts"));
        assert_eq!(d.edge_count(), 3);
    }

    #[test]
    fn to_steps_emits_one_box_per_node_and_one_arrow_per_edge() {
        use crate::whiteboard::Step;
        let mut d = Diagram::new();
        d.ingest("the api calls the database");
        let steps = d.to_steps();
        let boxes = steps
            .iter()
            .filter(|s| matches!(s, Step::Box { .. }))
            .count();
        let arrows = steps
            .iter()
            .filter(|s| matches!(s, Step::Arrow { .. }))
            .count();
        assert_eq!(boxes, 2, "expected 2 boxes, got steps {steps:?}");
        assert_eq!(arrows, 1, "expected 1 arrow, got steps {steps:?}");
    }
}
