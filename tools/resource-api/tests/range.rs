use glue5_resource_api::range::RangeReader;
use std::io::{Cursor, Read, Seek, SeekFrom};

#[test]
fn reads_and_seeks_stay_inside_window() {
    let mut range = RangeReader::new(Cursor::new(b"beforeDATAafter"), 6, 4).unwrap();
    let mut data = Vec::new();
    range.read_to_end(&mut data).unwrap();
    assert_eq!(data, b"DATA");
    assert!(range.seek(SeekFrom::End(1)).is_err());
    assert!(range.seek(SeekFrom::Start(u64::MAX)).is_err());
    range.seek(SeekFrom::End(-2)).unwrap();
    let mut tail = [0; 2];
    range.read_exact(&mut tail).unwrap();
    assert_eq!(&tail, b"TA");
    assert!(range.seek(SeekFrom::Current(-5)).is_err());
}

#[test]
fn overflowing_window_is_rejected() {
    assert!(RangeReader::new(Cursor::new(Vec::<u8>::new()), u64::MAX, 1).is_err());
}
