//! Re-anchor the original minimap components in frontend.xml; no replacement
//! movie, extra renderer, game assets or WASM byte patches are involved.

const PARTS: [&str; 6] = ["minimap", "minimap_mask", "minimap_blur", "bigmap", "bigmap_mask", "bigmap_blur"];
const TOP: f64 = 0.04;
const SCALE: f64 = 1.10;
// Keep the normal help box to the right of both radar sizes and below the
// touch camera/pause strip. Floating, world-anchored help is left alone.
const HELP_LEFT: f64 = 0.5;
const HELP_TOP: f64 = 0.2;

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

fn numeric_change(tag: &str, at: usize, key: &str, value: f64) -> Option<(std::ops::Range<usize>, String)> {
    let range = attribute(tag, key)?;
    let old = &tag[range.clone()];
    let decimal = old.find('.')?;
    if !value.is_finite() || !old[decimal + 1..].bytes().all(|b| b.is_ascii_digit()) { return None; }
    let new = format!("{value:.precision$}", precision = old.len() - decimal - 1);
    (new.len() == old.len()).then_some((at + range.start..at + range.end, new))
}

/// Change only the six in-game radar tags. Original position is the default;
/// the pause map, gallery and feed are untouched. The help box moves clear
/// of the radar; both radar sizes grow together by ten percent.
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
    let reference = |index| -> Option<(f64, f64, f64)> {
        let &(at, stop, _) = tags.iter().find(|&&(_, _, found)| found == index)?;
        let tag = &text[at..stop];
        Some((number(tag, "posX")?, number(tag, "posY")?, number(tag, "sizeY")?))
    };
    let normal = reference(0)?;
    let expanded = reference(3)?;
    let mut changes = Vec::new();
    for &(at, stop, index) in &tags {
        let tag = &text[at..stop];
        let height = number(tag, "sizeY")?;
        let width = number(tag, "sizeX")?;
        let (ref_x, ref_y, ref_height) = if index < 3 { normal } else { expanded };
        // CalculateHudPosition uses bottom - sizeY + posY for B, and
        // top + posY for T. Preserve the mask/blur's relative offsets, not
        // merely their old posY values (their rectangles have different heights).
        let new_y = TOP + (number(tag, "posY")? - height + ref_height - ref_y) * SCALE;
        let new_x = ref_x + (number(tag, "posX")? - ref_x) * SCALE;
        if !(0.0..1.0).contains(&new_y) || !(0.0..1.0).contains(&(height * SCALE))
            || !(-0.1..1.0).contains(&new_x)
            || !(0.0..1.0).contains(&(width * SCALE)) || new_y + height * SCALE > 1.0
            || new_x + width * SCALE >= HELP_LEFT { return None; }
        for (key, value) in [("posY", new_y), ("posX", new_x), ("sizeX", width * SCALE), ("sizeY", height * SCALE)] {
            changes.push(numeric_change(tag, at, key, value)?);
        }
        let align = attribute(tag, "alignY")?;
        changes.push((at + align.start..at + align.end, "T".into()));
    }
    let hud_start = text.find("<Hud>")?;
    let hud_end = hud_start + text[hud_start..].find("</Hud>")?;
    let mut help = Vec::new();
    let mut cursor = hud_start;
    while let Some(relative) = text[cursor..hud_end].find("<component ") {
        let at = cursor + relative;
        let stop = at + text[at..hud_end].find('>')? + 1;
        let tag = &text[at..stop];
        if &tag[attribute(tag, "name")?] == "HUD_HELP_TEXT" { help.push((at, stop)); }
        cursor = stop;
    }
    if help.len() != 1 { return None; }
    let (at, stop) = help[0];
    let tag = &text[at..stop];
    if &tag[attribute(tag, "alignX")?] != "L" || &tag[attribute(tag, "alignY")?] != "T"
        || HELP_LEFT + number(tag, "sizeX")? > 1.0 || HELP_TOP + number(tag, "sizeY")? > 1.0 { return None; }
    changes.push(numeric_change(tag, at, "posX", HELP_LEFT)?);
    changes.push(numeric_change(tag, at, "posY", HELP_TOP)?);
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
        let mut xml = "<frontend><Hud><component name=\"HUD_HELP_TEXT\" alignX=\"L\" alignY=\"T\" posX=\"0.0\" posY=\"0.0\" sizeX=\"0.203\" sizeY=\"0.389\" /></Hud><minimap v=\"1.0\">\n".to_owned();
        for (name, y, h) in [
            ("minimap", "0.002", "0.188888"), ("minimap_mask", "0.032", "0.159"),
            ("minimap_blur", "0.022", "0.237"), ("bigmap", "0.022", "0.460416666"),
            ("bigmap_mask", "0.015", "0.395"), ("bigmap_blur", "0.022", "0.464"),
        ] {
            xml += &format!("<data name=\"{name}\" alignX=\"L\" alignY=\"B\" posX=\"0.000\" posY=\"{y}\" sizeX=\"0.200\" sizeY=\"{h}\" />\n");
        }
        xml + "<data name=\"pausemap\" alignX=\"I\" alignY=\"I\" posY=\"0.0\" /></minimap><GameStream>unchanged</GameStream></frontend>"
    }
    #[test]
    fn preserves_length_and_keeps_original_components_and_pause_map() {
        let before = fixture();
        let after = top_left(before.clone()).unwrap();
        assert_eq!(before.len(), after.len());
        assert_eq!(after.matches("alignY=\"T\"").count(), 7);
        for name in PARTS { assert!(after.contains(&format!("name=\"{name}\""))); }
        assert!(after.contains("<data name=\"pausemap\" alignX=\"I\" alignY=\"I\" posY=\"0.0\" />"));
        assert!(after.contains("<GameStream>unchanged</GameStream>"));
        assert!(after.contains("posX=\"0.5\" posY=\"0.2\" sizeX=\"0.203\" sizeY=\"0.389\""));
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
                assert!((number(&new, "posY").unwrap() - TOP - old_offset * SCALE).abs() <= 0.00051);
                assert!((number(&new, "sizeX").unwrap() / number(&old, "sizeX").unwrap() - SCALE).abs() < 0.0001);
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
        assert!(top_left(xml.replace("HUD_HELP_TEXT", "HUD_OTHER")).is_none());
    }

    #[test]
    fn signed_offsets_and_larger_expanded_radar_stay_clear_of_help() {
        let before = fixture().replace("posX=\"0.000\"", "posX=\"-0.0045\"")
            .replace("sizeX=\"0.200\"", "sizeX=\"0.364\"");
        let after = top_left(before.clone()).unwrap();
        assert_eq!(after.len(), before.len());
        assert!(after.contains("sizeX=\"0.400\""));
        for tag in after.split('<').filter(|t| t.starts_with("data ") && t.contains("alignY=\"T\"")) {
            assert!(number(tag, "posX").unwrap() + number(tag, "sizeX").unwrap() < HELP_LEFT);
            assert!(number(tag, "posY").unwrap() + number(tag, "sizeY").unwrap() < 1.0);
        }
    }
}
