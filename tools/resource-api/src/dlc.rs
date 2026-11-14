use crate::{Resources, Result, io, safe_name};
use serde::Serialize;
use std::{fs, io::{ErrorKind, Read}};

#[derive(Debug, Serialize)]
pub struct DlcPack {
    pub name: String,
    pub archive: String,
    pub stored_bytes: u64,
    pub header: &'static str,
}

impl Resources {
    /// Report the manifest's declared order, retaining missing and ambiguous
    /// packs for the caller. setup2.xml and engine mount rules are separate.
    pub fn dlc_manifest(&self, uri: &str) -> Result<serde_json::Value> {
        let bytes = self.read(uri)?;
        let names = manifest_names(&bytes)?;
        let packs = self.discover_dlc_packs()?;
        let entries: Vec<_> = names.into_iter().map(|(mount, name)| {
            let candidates: Vec<_> = packs.iter().filter(|pack| pack.name.eq_ignore_ascii_case(name) && match mount {
                "dlcpacks" => true,
                "platform" => pack.archive.starts_with("update/x64/dlcpacks/"),
                _ => false,
            }).collect();
            serde_json::json!({"name":name,"mount":mount,"candidates":candidates})
        }).collect();
        Ok(serde_json::json!({"manifest":uri,"declared_order":entries}))
    }

    /// Inventory physical DLC packs, reading only the fixed-size RPF header. An entry
    /// does not imply the pack is enabled or establish its mount precedence.
    pub fn discover_dlc_packs(&self) -> Result<Vec<DlcPack>> {
        let mut packs = Vec::new();
        for base in ["update/x64/dlcpacks", "x64/dlcpacks"] {
            let directory = match self.root.join(base).canonicalize() {
                Ok(directory) if directory.starts_with(&self.root) => directory,
                Ok(_) => return Err("DLC directory escapes the approved source".into()),
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => return Err(io(error)),
            };
            for (index, entry) in fs::read_dir(directory).map_err(io)?.enumerate() {
                if index >= 4096 {
                    return Err("DLC directory exceeds inventory limit".into());
                }
                let entry = entry.map_err(io)?;
                if !entry.file_type().map_err(io)?.is_dir() { continue; }
                let name = entry.file_name();
                let Some(name) = name.to_str().filter(|name| safe_name(name)) else { continue };
                let archive = format!("{base}/{name}/dlc.rpf");
                let path = match self.root.join(&archive).canonicalize() {
                    Ok(path) if path.starts_with(&self.root) => path,
                    Ok(_) => return Err("DLC archive escapes the approved source".into()),
                    Err(error) if error.kind() == ErrorKind::NotFound => continue,
                    Err(error) => return Err(io(error)),
                };
                let metadata = fs::metadata(&path).map_err(io)?;
                if metadata.is_file() {
                    let mut header = [0; 16];
                    let header = match fs::File::open(path).and_then(|mut file| file.read_exact(&mut header)) {
                        Ok(()) => classify_header(&header),
                        Err(error) if error.kind() == ErrorKind::UnexpectedEof => "truncated",
                        Err(_) => "unreadable",
                    };
                    packs.push(DlcPack { name: name.to_owned(), archive, stored_bytes: metadata.len(), header });
                }
            }
        }
        packs.sort_by(|left, right| left.archive.cmp(&right.archive));
        Ok(packs)
    }
}

fn classify_header(header: &[u8; 16]) -> &'static str {
    if &header[..4] != b"7FPR" { return "unknown-format"; }
    match u32::from_le_bytes(header[12..16].try_into().unwrap()) {
        0x4e45504f => "rpf7-open",
        0x0ffffff9 => "rpf7-aes",
        0x0fefffff => "rpf7-ng",
        _ => "rpf7-unknown-encryption",
    }
}

// Parse only the DLC list grammar. Reject entities and extensions instead of
// interpreting arbitrary XML or mistaking entries in comments for mounts.
fn manifest_names(bytes: &[u8]) -> Result<Vec<(&str, &str)>> {
    if bytes.len() > 1024 * 1024 { return Err("DLC manifest exceeds 1 MiB".into()); }
    let mut xml = std::str::from_utf8(bytes).map_err(|_| "DLC manifest must be UTF-8")?
        .trim_start_matches('\u{feff}').trim_start();
    if xml.starts_with("<?xml ") {
        xml = xml.split_once("?>").ok_or("unterminated XML declaration")?.1;
    }
    fn skip(mut rest: &str) -> Result<&str> {
        loop {
            rest = rest.trim_start();
            let Some(comment) = rest.strip_prefix("<!--") else { return Ok(rest) };
            rest = comment.split_once("-->").ok_or("unterminated XML comment")?.1;
        }
    }
    xml = skip(xml)?.strip_prefix("<SMandatoryPacksData>").ok_or("expected DLC manifest root")?;
    xml = skip(xml)?.strip_prefix("<Paths>").ok_or("expected DLC Paths")?;
    let mut names = Vec::new();
    while let Some(item) = skip(xml)?.strip_prefix("<Item>") {
        let (path, rest) = item.split_once("</Item>").ok_or("unterminated DLC Item")?;
        let path = path.trim();
        let (mount, path) = if let Some(path) = path.strip_prefix("platform:/dlcPacks/") {
            ("platform", path)
        } else {
            ("dlcpacks", path.strip_prefix("dlcpacks:/").or_else(|| path.strip_prefix("dlcpacks:\\")).ok_or("invalid DLC pack path")?)
        };
        let name = Some(path)
            .and_then(|path| path.strip_suffix(['/', '\\']))
            .filter(|name| safe_name(name) && !name.contains('&'))
            .ok_or("invalid DLC pack path")?;
        if names.len() >= 4096 { return Err("too many DLC entries".into()); }
        names.push((mount, name));
        xml = rest;
    }
    xml = skip(xml)?.strip_prefix("</Paths>").ok_or("expected end of DLC Paths")?;
    xml = skip(xml)?.strip_prefix("</SMandatoryPacksData>").ok_or("expected end of DLC manifest")?;
    if !skip(xml)?.is_empty() { return Err("unexpected trailing DLC manifest content".into()); }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::manifest_names;
    use crate::Resources;
    use std::{fs, time::{SystemTime, UNIX_EPOCH}};

    #[test]
    fn platform_declaration_matches_only_update_root() {
        let ticks = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let base = std::env::temp_dir().join(format!("glue5-dlc-reconcile-{}-{ticks}", std::process::id()));
        let root = base.join("game");
        let cache = base.join("cache");
        fs::create_dir_all(root.join("update/x64/dlcpacks/mpBeach")).unwrap();
        fs::create_dir_all(root.join("x64/dlcpacks/mpBeach")).unwrap();
        fs::create_dir_all(&cache).unwrap();
        fs::write(root.join("dlclist.xml"),
            "<SMandatoryPacksData><Paths><Item>platform:/dlcPacks/mpBeach/</Item><Item>dlcpacks:/mpBeach/</Item></Paths></SMandatoryPacksData>"
        ).unwrap();
        let mut header = [0u8; 16];
        header[..4].copy_from_slice(b"7FPR");
        header[12..16].copy_from_slice(&0x4e45504fu32.to_le_bytes());
        for archive in ["update/x64/dlcpacks/mpBeach/dlc.rpf", "x64/dlcpacks/mpBeach/dlc.rpf"] {
            fs::write(root.join(archive), header).unwrap();
        }
        let api = Resources::open(&root, &cache, None).unwrap();
        let report = api.dlc_manifest("dlclist.xml").unwrap();
        let entries = report["declared_order"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0]["mount"], "platform");
        let platform = entries[0]["candidates"].as_array().unwrap();
        assert_eq!(platform.len(), 1, "platform packs must not be misreported as absent");
        assert_eq!(platform[0]["archive"], "update/x64/dlcpacks/mpBeach/dlc.rpf");
        assert_eq!(entries[1]["candidates"].as_array().unwrap().len(), 2,
            "generic dlcpacks matching must remain unchanged");
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn preserves_order_and_ignores_commented_out_mounts() {
        let manifest = br"<?xml version='1.0'?><SMandatoryPacksData><Paths>
            <Item>dlcpacks:/second/</Item><!-- <Item>dlcpacks:/disabled/</Item> -->
            <Item>platform:/dlcPacks/mpBeach/</Item>
            <Item>dlcpacks:\first\</Item></Paths></SMandatoryPacksData>";
        assert_eq!(manifest_names(manifest).unwrap(), [("dlcpacks", "second"), ("platform", "mpBeach"), ("dlcpacks", "first")]);
    }
}
