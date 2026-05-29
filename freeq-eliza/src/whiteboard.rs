//! Whiteboard step types — Eliza's "explain it with a diagram" mode.
//!
//! The model decides per-answer whether a diagram would help, and if so
//! returns a sequence of [`Step`]s describing what to draw. The video
//! tile reveals one step at a time so the diagram unfolds while she
//! speaks. Steps are intentionally minimal: box, arrow, text — enough
//! for flow diagrams and concept maps, not enough to over-design.

use serde::Deserialize;

/// One drawing primitive on the whiteboard. The renderer reveals each
/// step in order, ~900 ms apart, with a brief fade-in.
#[derive(Deserialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Step {
    /// A labeled rectangle. `(x,y)` is the top-left in the 640×360
    /// canvas; safe content area is roughly `x∈[60,580]`, `y∈[80,300]`.
    Box {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        label: String,
    },
    /// A line with an arrowhead at `(x2,y2)`, optionally labeled at the
    /// midpoint.
    Arrow {
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
        #[serde(default)]
        label: Option<String>,
    },
    /// Standalone text positioned at `(x,y)` (text baseline).
    Text {
        x: f32,
        y: f32,
        content: String,
        #[serde(default)]
        size: TextSize,
    },
}

/// Three coarse type sizes — large for titles, med for normal labels,
/// small for captions. Keeps the model from inventing pixel values.
#[derive(Deserialize, Debug, Clone, Copy, Default)]
#[serde(rename_all = "lowercase")]
pub enum TextSize {
    Small,
    #[default]
    Med,
    Large,
}

impl TextSize {
    pub fn px(self) -> u32 {
        match self {
            TextSize::Small => 12,
            TextSize::Med => 16,
            TextSize::Large => 26,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_box_arrow_text_steps() {
        let json = r#"[
          {"type":"text","x":320,"y":48,"content":"How TCP works","size":"large"},
          {"type":"box","x":80,"y":120,"w":120,"h":50,"label":"Client"},
          {"type":"arrow","x1":210,"y1":145,"x2":420,"y2":145,"label":"SYN"},
          {"type":"box","x":430,"y":120,"w":120,"h":50,"label":"Server"}
        ]"#;
        let steps: Vec<Step> = serde_json::from_str(json).unwrap();
        assert_eq!(steps.len(), 4);
        match &steps[0] {
            Step::Text { size, .. } => assert!(matches!(size, TextSize::Large)),
            _ => panic!("expected Text"),
        }
        match &steps[2] {
            Step::Arrow { label, .. } => assert_eq!(label.as_deref(), Some("SYN")),
            _ => panic!("expected Arrow"),
        }
    }

    #[test]
    fn arrow_label_optional_and_text_size_default_med() {
        let json = r#"[
          {"type":"arrow","x1":0,"y1":0,"x2":10,"y2":10},
          {"type":"text","x":0,"y":0,"content":"plain"}
        ]"#;
        let steps: Vec<Step> = serde_json::from_str(json).unwrap();
        match &steps[0] {
            Step::Arrow { label, .. } => assert!(label.is_none()),
            _ => panic!(),
        }
        match &steps[1] {
            Step::Text { size, .. } => assert!(matches!(size, TextSize::Med)),
            _ => panic!(),
        }
    }
}
