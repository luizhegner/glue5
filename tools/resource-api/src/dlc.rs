use crate::{Resources, Result, io, safe_name};
use serde::Serialize;
use std::{fs, io::ErrorKind};

#[derive(Debug, Serialize)]
pub struct DlcPack {
    pub name: String,
    pub archive: String,
    pub stored_bytes: u64,
}

impl Resources {
    /// Inventory physical DLC packs without reading RPF contents. An entry
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
                let metadata = fs::metadata(path).map_err(io)?;
                if metadata.is_file() {
                    packs.push(DlcPack { name: name.to_owned(), archive, stored_bytes: metadata.len() });
                }
            }
        }
        packs.sort_by(|left, right| left.archive.cmp(&right.archive));
        Ok(packs)
    }
}
