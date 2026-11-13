//! Derive bounded candidate material from the already compiled game module.
//! No literal game keys in source, logs or receipts; never start a game VM.
use anyhow::Result;
use std::collections::{BTreeSet,HashMap};
use wasmparser::{DataKind,KnownCustom,Name,Operator,Parser,Payload};

pub fn candidates(wasm:&[u8])->Result<Vec<[u8;32]>> {
    let mut names=HashMap::new();let mut bodies=Vec::new();let mut rodata=&[][..];
    let mut imported=0;
    for payload in Parser::new(0).parse_all(wasm) {
        match payload? {
            Payload::ImportSection(reader)=>for import in reader.into_imports() {
                if matches!(import?.ty,wasmparser::TypeRef::Func(_)) {imported+=1;}
            },
            Payload::DataSection(reader)=>if let Some(data)=reader.into_iter().next() {
                let data=data?;if matches!(data.kind,DataKind::Passive) {rodata=data.data;}
            },
            Payload::CodeSectionEntry(body)=>bodies.push(body),
            Payload::CustomSection(section)=>if let KnownCustom::Name(reader)=section.as_known() {
                for subsection in reader {if let Name::Function(map)=subsection? {
                    for item in map {let item=item?;names.insert(item.index,item.name.to_ascii_lowercase());}
                }}
            },
            _=>{},
        }
    }
    let mut addresses=BTreeSet::new();
    for (i,body) in bodies.iter().enumerate() {
        let Some(name)=names.get(&(imported+i as u32)) else {continue;};
        if !["aes","encryption","packfile"].iter().any(|needle|name.contains(needle)) {continue;}
        for op in body.get_operators_reader()? {
            let address=match op? {Operator::I64Const{value}=>value as u64,Operator::I32Const{value}=>value as u32 as u64,_=>continue};
            if address>=1024 && address-1024+32<=rodata.len() as u64 {addresses.insert(address);}
        }
    }
    let mut out=BTreeSet::new();
    for address in addresses {
        let at=(address-1024) as usize;
        for offset in [0,8,16,32] {
            if let Some(bytes)=rodata.get(at+offset..at+offset+32) {
                // Plain labels / zero-fill cannot be an AES key. Bound work
                // independently of a module's number of named functions.
                if bytes.iter().filter(|&&b|b!=0).count()>=24 && bytes.iter().any(|b|!b.is_ascii()) {
                    out.insert(<[u8;32]>::try_from(bytes).unwrap());
                    if out.len()>=512 {break;}
                }
            }
        }
        if out.len()>=512 {break;}
    }
    Ok(out.into_iter().collect())
}

pub fn generated(wasm:&[u8],build:&str,known_checksum:bool)->Result<String> {
    let keys=if known_checksum && build=="8b0b5899ed" {candidates(wasm)?}else{vec![]};
    // This generated file stays in OUT_DIR, alongside the module's own
    // generated read-only data. Never upload it as a diagnostic artifact.
    let mut output="pub fn resource_audio_keys() -> &'static [[u8;32]] { &[\n".to_owned();
    for key in keys {output.push_str(&format!("{key:?},\n"));}
    output.push_str("] }\n");Ok(output)
}
