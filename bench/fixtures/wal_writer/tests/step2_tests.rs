use wal_writer::{WalWriter, checksum, recover};

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir()
        .join(format!("wal-bench-rec-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn recovers_all_records_in_order() {
    let dir = tmpdir("all");
    let path = dir.join("wal.log");
    {
        let mut w = WalWriter::open(&path).unwrap();
        w.append(b"alpha").unwrap();
        w.append(b"beta").unwrap();
        w.append(b"gamma").unwrap();
    }
    assert_eq!(
        recover(&path).unwrap(),
        vec![b"alpha".to_vec(), b"beta".to_vec(), b"gamma".to_vec()]
    );
}

#[test]
fn torn_tail_is_discarded_and_truncated() {
    let dir = tmpdir("torn");
    let path = dir.join("wal.log");
    {
        let mut w = WalWriter::open(&path).unwrap();
        w.append(b"good1").unwrap();
        w.append(b"good2").unwrap();
    }
    // simulate a torn write: half of a third record
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new().append(true).open(&path).unwrap();
    let payload = b"torn";
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&checksum(payload).to_le_bytes());
    bytes.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    f.write_all(&bytes[..5]).unwrap(); // cut mid-header/payload

    assert_eq!(recover(&path).unwrap(), vec![b"good1".to_vec(), b"good2".to_vec()]);
    // truncated in place: appending continues cleanly
    let mut w = WalWriter::open(&path).unwrap();
    assert_eq!(w.append(b"next").unwrap(), 3);
    assert_eq!(
        recover(&path).unwrap(),
        vec![b"good1".to_vec(), b"good2".to_vec(), b"next".to_vec()]
    );
}

#[test]
fn corrupted_checksum_is_discarded() {
    let dir = tmpdir("corrupt");
    let path = dir.join("wal.log");
    {
        let mut w = WalWriter::open(&path).unwrap();
        w.append(b"ok-record").unwrap();
    }
    // corrupt the last byte of the file (part of payload) so the final
    // record's checksum no longer matches
    let data = std::fs::read(&path).unwrap();
    let mut corrupted = data.clone();
    let last = corrupted.len() - 1;
    corrupted[last] ^= 0xff;
    std::fs::write(&path, corrupted).unwrap();

    assert!(recover(&path).unwrap().is_empty(), "corrupted final record dropped");
}
