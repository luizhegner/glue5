//! Bounded observations of existing engine compatibility fallbacks.
use std::sync::Mutex;

const GAP_LIMIT: usize = 128;
#[derive(Default)]
struct AssetGaps { entries: Vec<(&'static str, String, u32)>, omitted: u32 }
impl AssetGaps {
    fn add(&mut self, kind: &'static str, name: &str) {
        // Asset identifiers only; reject arbitrary text and oversized names.
        if name.is_empty() || name.len()>96 || !name.bytes().all(|b|
            b.is_ascii_alphanumeric() || matches!(b,b'_'|b'-'|b'.'|b'@'|b'/'|b'\\')) { return; }
        if let Some((_,_,count))=self.entries.iter_mut().find(|(k,n,_)|*k==kind && n.eq_ignore_ascii_case(name)) {
            *count=count.saturating_add(1);
        } else if self.entries.len()<GAP_LIMIT {
            self.entries.push((kind,name.to_owned(),1));
        } else { self.omitted=self.omitted.saturating_add(1); }
    }
}
static ASSET_GAPS: Mutex<AssetGaps> = Mutex::new(AssetGaps { entries: Vec::new(), omitted: 0 });

/// The engine never waits for a diagnostic lock; counts can be approximate.
pub(crate) fn record(kind:&'static str,name:&str) {
    if let Ok(mut gaps)=ASSET_GAPS.try_lock() {gaps.add(kind,name);}
}
pub(crate) fn snapshot()->Vec<String> {
    let Ok(gaps)=ASSET_GAPS.lock() else {return vec!["inventory unavailable".into()]};
    let mut lines:Vec<_>=gaps.entries.iter().map(|(kind,name,count)|format!("{kind}: {name} ({count} observations)")).collect();
    if gaps.omitted>0 {lines.push(format!("additional observations omitted: {}",gaps.omitted));}
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inventory_is_bounded_deduplicated_and_excludes_arbitrary_text() {
        let mut gaps=AssetGaps::default();
        gaps.add("animation","ANIM@DOOR");gaps.add("animation","anim@door");
        gaps.add("animation","private text with spaces\n");
        assert_eq!(gaps.entries.len(),1);assert_eq!(gaps.entries[0].2,2);
        for n in 0..GAP_LIMIT+5 {gaps.add("model",&format!("0x{n:08x}"));}
        assert_eq!(gaps.entries.len(),GAP_LIMIT);assert_eq!(gaps.omitted,6);
    }
}
