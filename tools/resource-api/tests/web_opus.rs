use glue5_resource_api::web_opus::{BUILD, ogg};

fn pages(bytes: &[u8]) -> Vec<(u8,u64,Vec<u8>)> {
    let mut pages=Vec::new(); let mut at=0;
    while at<bytes.len() {
        assert_eq!(&bytes[at..at+4],b"OggS");
        let count=bytes[at+26] as usize;
        let laces=&bytes[at+27..at+27+count];
        let size=laces.iter().map(|&b|b as usize).sum::<usize>();
        let end=at+27+count+size;
        assert_eq!(u32::from_le_bytes(bytes[at+18..at+22].try_into().unwrap()),pages.len() as u32);
        // Independent bit-string polynomial long division, not a call to
        // the production CRC function. CRC field is zeroed for calculation.
        let mut polynomial=0u64;
        for (i,&byte) in bytes[at..end].iter().enumerate() {
            let byte=if (22..26).contains(&i) {0}else{byte};
            for bit in (0..8).rev() {
                polynomial=(polynomial<<1)^((((byte>>bit)&1) as u64)<<32);
                if polynomial & (1u64<<32)!=0 { polynomial^=0x104c11db7; }
            }
        }
        assert_eq!(u32::from_le_bytes(bytes[at+22..at+26].try_into().unwrap()),polynomial as u32);
        pages.push((bytes[at+5],u64::from_le_bytes(bytes[at+6..at+14].try_into().unwrap()),bytes[at+27+count..end].to_vec()));
        at=end;
    }
    pages
}
#[test]
fn repacking_preserves_packets_header_timing_crc_and_trim() {
    // Synthetic framing/TOC fixtures, NOT a claim to decode these payloads.
    let payload=[2,0x18,0x31,3,0x18,0x45,0x67];
    let out=ogg(&payload,5000,123,BUILD).unwrap();
    let pages=pages(&out);
    assert_eq!(pages.len(),4); assert_eq!(pages[0].0,2); assert_eq!(pages[3].0,4);
    assert_eq!(&pages[0].2[..8],b"OpusHead");
    assert_eq!(&pages[0].2[8..12],&[1,1,0,0]);
    assert_eq!(&pages[1].2[..8],b"OpusTags");
    assert_eq!(pages[2].2,[0x18,0x31]); assert_eq!(pages[3].2,[0x18,0x45,0x67]);
    assert_eq!(pages[2].1,2880); assert_eq!(pages[3].1,5000);
}
#[test]
fn a_255_byte_packet_gets_the_required_zero_terminating_lace() {
    let mut payload=vec![255,0x18]; payload.extend([7;254]);
    let out=ogg(&payload,2880,0,BUILD).unwrap();
    assert_eq!(pages(&out)[2].2.len(),255);
    let at=out.windows(4).enumerate().filter(|(_,b)|*b==b"OggS").nth(2).unwrap().0;
    assert_eq!(&out[at+26..at+29],&[2,255,0]);
}
#[test]
fn refuses_unknown_profiles_framing_durations_and_inconsistent_samples() {
    for bad in [vec![],vec![0],vec![3,0x18],vec![1,0],vec![1,0x1c],vec![1,0x1b],vec![2,0x1b,0]] {
        assert!(ogg(&bad,2880,0,BUILD).is_err(),"{bad:?}");
    }
    for samples in [0,2881,u32::MAX] { assert!(ogg(&[1,0x18],samples,0,BUILD).is_err()); }
    assert!(ogg(&[1,0x18,1,0x18],2880,0,BUILD).is_err());
    assert!(ogg(&[1,0x18],2880,0,"other-build").is_err());
    assert!(ogg(&[1,0x18].repeat(30_001),2880*30_001,0,BUILD).is_err());
}
