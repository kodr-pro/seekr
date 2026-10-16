use wal_writer::WalWriter;

fn tmpdir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir()
        .join(format!("wal-bench-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn checksum_matches_fnv1a_reference() {
    assert_eq!(wal_writer::checksum(b""), 0x811c9dc5);
    assert_eq!(wal_writer::checksum(b"a"), 0xe40c292c);
    assert_eq!(wal_writer::checksum(b"foobar"), 0xbf9cf968);
}

#[test]
fn append_assigns_sequential_sequence_numbers() {
    let dir = tmpdir("seq");
    let mut w = WalWriter::open(dir.join("wal.log")).unwrap();
    assert_eq!(w.append(b"one").unwrap(), 1);
    assert_eq!(w.append(b"two").unwrap(), 2);
    assert_eq!(w.append(b"three").unwrap(), 3);
}

#[test]
fn append_persists_across_reopen() {
    let dir = tmpdir("persist");
    {
        let mut w = WalWriter::open(dir.join("wal.log")).unwrap();
        w.append(b"first").unwrap();
        w.append(b"second").unwrap();
    }
    let mut w = WalWriter::open(dir.join("wal.log")).unwrap();
    assert_eq!(w.append(b"third").unwrap(), 3, "sequence continues");
}

#[test]
fn empty_payload_is_a_valid_record() {
    let dir = tmpdir("empty");
    let mut w = WalWriter::open(dir.join("wal.log")).unwrap();
    assert_eq!(w.append(b"").unwrap(), 1);
}
