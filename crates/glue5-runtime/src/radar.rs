//! Re-anchor the original minimap components in frontend.xml; no replacement
//! movie, extra renderer, game assets or WASM byte patches are involved.

const PARTS: [&str; 6] = ["minimap", "minimap_mask", "minimap_blur", "bigmap", "bigmap_mask", "bigmap_blur"];
const TOP: f64 = 0.04;

fn attribute(tag: &str, key: &str) -> Option<std::ops::Range<usize>> {
    let needle = format!("{key}=\"");
    let at = tag.find(&needle)?;
    if at > 0 && !tag.as_bytes()[at - 1].is_ascii_whitespace() { return None; }
    let start = at + needle.len();
    let end = start + tag[start..].find('"')?;
    if tag[end + 1..].contains(&needle) { return None; }
    Some(start..end)
}

fn number(tag: &str, key: &str) -> Option<f64> {
    let value: f64 = tag[attribute(tag, key)?].parse().ok()?;
    value.is_finite().then_some(value)
}

/// Change only the six in-game radar tags. Original position is the default;
/// the pause map, gallery, feed and other HUD components are untouched.
/// Refuse an unknown layout atomically, retaining the existing overlay.
pub(crate) fn top_left(text: String) -> Option<String> {
    let start = text.find("<minimap ")?;
    let end = start + text[start..].find("</minimap>")?;
    let mut tags: Vec<(usize, usize, usize)> = Vec::new();
    let mut cursor = start;
    while let Some(relative) = text[cursor..end].find("<data ") {
        let at = cursor + relative;
        let stop = at + text[at..end].find('>')? + 1;
        let tag = &text[at..stop];
        let name = &tag[attribute(tag, "name")?];
        if let Some(index) = PARTS.iter().position(|part| *part == name) {
            if tags.iter().any(|&(_, _, found)| found == index) { return None; }
            if &tag[attribute(tag, "alignX")?] != "L" || &tag[attribute(tag, "alignY")?] != "B" { return None; }
            tags.push((at, stop, index));
        }
        cursor = stop;
    }
    if tags.len() != PARTS.len() { return None; }
    let reference = |index| -> Option<(f64, f64)> {
        let &(at, stop, _) = tags.iter().find(|&&(_, _, found)| found == index)?;
        let tag = &text[at..stop];
        Some((number(tag, "posY")?, number(tag, "sizeY")?))
    };
    let normal = reference(0)?;
    let expanded = reference(3)?;
    let mut changes = Vec::new();
    for &(at, stop, index) in &tags {
        let tag = &text[at..stop];
        let y = attribute(tag, "posY")?;
        let old = &tag[y.clone()];
        if !old.starts_with("0.") || !old[2..].bytes().all(|b| b.is_ascii_digit()) || !(3..=12).contains(&old.len()) { return None; }
        let height = number(tag, "sizeY")?;
        let (ref_y, ref_height) = if index < 3 { normal } else { expanded };
        // CalculateHudPosition uses bottom - sizeY + posY for B, and
        // top + posY for T. Preserve the mask/blur's relative offsets, not
        // merely their old posY values (their rectangles have different heights).
        let new_y = TOP + number(tag, "posY")? - height + ref_height - ref_y;
        if !(0.0..=1.0).contains(&new_y) || !(0.0..1.0).contains(&height) { return None; }
        let new = format!("{new_y:.precision$}", precision = old.len() - 2);
        if new.len() != old.len() { return None; }
        changes.push((at + y.start..at + y.end, new));
        let align = attribute(tag, "alignY")?;
        changes.push((at + align.start..at + align.end, "T".into()));
    }
    let len = text.len();
    let mut output = text;
    changes.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    for (range, value) in changes { output.replace_range(range, &value); }
    (output.len() == len).then_some(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> String {
        let mut xml = "<frontend><minimap v=\"1.0\">\n".to_owned();
        for (name, y, h) in [
            ("minimap", "0.002", "0.188888"), ("minimap_mask", "0.032", "0.159"),
            ("minimap_blur", "0.022", "0.237"), ("bigmap", "0.022", "0.460416666"),
            ("bigmap_mask", "0.015", "0.395"), ("bigmap_blur", "0.022", "0.464"),
        ] {
            xml += &format!("<data name=\"{name}\" alignX=\"L\" alignY=\"B\" posX=\"0.0\" posY=\"{y}\" sizeX=\"0.2\" sizeY=\"{h}\" />\n");
        }
        xml + "<data name=\"pausemap\" alignX=\"I\" alignY=\"I\" posY=\"0.0\" /></minimap><GameStream>unchanged</GameStream></frontend>"
    }
    #[test]
    fn preserves_length_and_keeps_original_components_and_pause_map() {
        let before = fixture();
        let after = top_left(before.clone()).unwrap();
        assert_eq!(before.len(), after.len());
        assert_eq!(after.matches("alignY=\"T\"").count(), 6);
        for name in PARTS { assert!(after.contains(&format!("name=\"{name}\""))); }
        assert!(after.contains("<data name=\"pausemap\" alignX=\"I\" alignY=\"I\" posY=\"0.0\" />"));
        assert!(after.contains("<GameStream>unchanged</GameStream>"));
    }
    #[test]
    fn preserves_relative_component_offsets_within_rounding_precision() {
        let before = fixture();
        let after = top_left(before.clone()).unwrap();
        let tag = |text: &str, name: &str| -> String {
            let at = text.find(&format!("<data name=\"{name}\"")).unwrap();
            text[at..at + text[at..].find('>').unwrap() + 1].to_owned()
        };
        for group in [0..3, 3..6] {
            let anchor = tag(&before, PARTS[group.start]);
            let old_anchor = number(&anchor, "posY").unwrap() - number(&anchor, "sizeY").unwrap();
            for index in group {
                let old = tag(&before, PARTS[index]);
                let new = tag(&after, PARTS[index]);
                let old_offset = number(&old, "posY").unwrap() - number(&old, "sizeY").unwrap() - old_anchor;
                assert!((number(&new, "posY").unwrap() - TOP - old_offset).abs() <= 0.00051);
            }
        }
    }
    #[test]
    fn refuses_missing_duplicate_or_unexpected_layouts() {
        let xml = fixture();
        assert!(top_left(xml.replace("name=\"minimap_mask\"", "name=\"missing\"")).is_none());
        assert!(top_left(xml.replace("name=\"minimap_mask\"", "name=\"minimap\"")).is_none());
        assert!(top_left(xml.replace("alignY=\"B\"", "alignY=\"T\"")).is_none());
        assert!(top_left(xml.replace("0.188888", "NaN")).is_none());
    }
}
