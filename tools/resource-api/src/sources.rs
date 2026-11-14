//! Unified access to original game installations. Source IDs are stable and
//! remain part of every resource address. Valid Legacy resources take priority;
//! WebAssembly supplies fallback resources with an explicit attempt history.
use crate::{Resources, Result};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceKind {
    WebAssembly,
    Legacy,
}

pub struct Source {
    pub kind: SourceKind,
    pub resources: Resources,
}

#[derive(Default)]
pub struct Sources {
    entries: BTreeMap<String, Source>,
}

/// Candidates refer to the same logical resource, not necessarily the same
/// physical path. Mount discovery must supply DLC/update precedence within
/// each kind; the resolver preserves that order.
pub struct Candidate<'a> {
    pub source: &'a str,
    pub uri: &'a str,
}

#[derive(Debug)]
pub struct Rejected {
    pub source: String,
    pub uri: String,
    pub reason: String,
}

pub struct Resolved<T> {
    pub source: String,
    pub uri: String,
    pub kind: SourceKind,
    pub value: T,
    pub rejected: Vec<Rejected>,
}

impl Sources {
    /// Register the actual installation, not a directory of pre-exported UI
    /// resources. Existing IDs cannot be replaced while requests refer to them.
    pub fn register(&mut self, id: &str, source: Source) -> Result<()> {
        if id.is_empty() || id.len() > 64
            || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        {
            return Err("invalid source ID".into());
        }
        if self.entries.contains_key(id) {
            return Err("source ID already registered".into());
        }
        self.entries.insert(id.to_owned(), source);
        Ok(())
    }

    pub fn get(&self, id: &str) -> Result<&Source> {
        self.entries.get(id).ok_or_else(|| "unknown source ID".into())
    }

    /// Prefer Legacy, falling back only after read/decode/compatibility failure.
    /// The caller must decode and validate the resource for its intended use;
    /// a successful byte read alone is not proof of engine compatibility.
    /// No output is activated or written by this method.
    pub fn resolve<T>(
        &self,
        candidates: &[Candidate<'_>],
        mut decode: impl FnMut(SourceKind, &str, &[u8]) -> Result<T>,
    ) -> std::result::Result<Resolved<T>, Vec<Rejected>> {
        let mut rejected = Vec::new();
        for kind in [SourceKind::Legacy, SourceKind::WebAssembly] {
            for candidate in candidates {
                let Ok(source) = self.get(candidate.source) else { continue };
                if source.kind != kind { continue; }
                let result = source.resources.read(candidate.uri)
                    .and_then(|bytes| decode(kind, candidate.uri, &bytes));
                match result {
                    Ok(value) => return Ok(Resolved {
                        source: candidate.source.to_owned(),
                        uri: candidate.uri.to_owned(),
                        kind,
                        value,
                        rejected,
                    }),
                    Err(reason) => rejected.push(Rejected {
                        source: candidate.source.to_owned(),
                        uri: candidate.uri.to_owned(),
                        reason,
                    }),
                }
            }
        }
        for candidate in candidates {
            if let Err(reason) = self.get(candidate.source) {
                rejected.push(Rejected {
                    source: candidate.source.to_owned(),
                    uri: candidate.uri.to_owned(),
                    reason,
                });
            }
        }
        Err(rejected)
    }

    /// Reading never falls back to another installation after an error. Such
    /// fallback would hide unsupported encryption or silently change versions.
    pub fn read(&self, source: &str, uri: &str) -> Result<Vec<u8>> {
        self.get(source)?.resources.read(uri)
    }

    pub fn list(&self, source: &str, path: &str, offset: usize, limit: usize)
        -> Result<serde_json::Value>
    {
        let mut result = self.get(source)?.resources.list(path, offset, limit)?;
        result["source"] = serde_json::json!(source);
        Ok(result)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, SourceKind)> {
        self.entries.iter().map(|(id, source)| (id.as_str(), source.kind))
    }
}
