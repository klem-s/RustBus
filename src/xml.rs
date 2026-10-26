//! Attribute helpers shared by the XML readers (settings, M.I.D.I. map, VBAN, presets).

use roxmltree::Node;

pub fn ai(n: Node, name: &str) -> Option<i64> {
    let v = n.attribute(name)?.trim();
    v.parse::<i64>().ok().or_else(|| v.parse::<f64>().ok().map(|f| f as i64))
}

pub fn af(n: Node, name: &str) -> Option<f32> {
    n.attribute(name)?.trim().parse::<f32>().ok()
}

pub fn ab(n: Node, name: &str) -> Option<bool> {
    ai(n, name).map(|v| v != 0)
}
