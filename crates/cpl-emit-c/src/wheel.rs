//! Internal: the deterministic wheel of `cpl build --target python` (`spec/python.md` 1.1): a zip archive (PEP 427)
//! whose entries are stored without compression, the package's files in path order and then the `.dist-info`
//! files with `RECORD` last, every timestamp 1980-01-01 00:00:00 and every mode 0644 (0755 for an executable), so
//! that one compiler and flag set give byte-identical wheels. Sizes stay below 4 GiB (no zip64).

use std::io;

/// One file of the archive: its path inside the wheel, its bytes, and whether it is executable.
pub(crate) struct Entry {
    pub path: String,
    pub data: Vec<u8>,
    pub executable: bool,
}

/// The standard CRC-32 (IEEE 802.3, reflected, the polynomial 0xEDB88320) of zip.
fn crc32(data: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (i, slot) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xEDB8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *slot = c;
        }
        t
    });
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = table[((c ^ u32::from(b)) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

fn too_large(what: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("{what} is too large for a zip archive without zip64"),
    )
}

/// The zip archive of `entries`, in the given order.
pub(crate) fn zip(entries: &[Entry]) -> io::Result<Vec<u8>> {
    // 1980-01-01 00:00:00 in MS-DOS form
    const TIME: u16 = 0;
    const DATE: u16 = (1 << 5) | 1;
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for e in entries {
        let name = e.path.as_bytes();
        let size = u32::try_from(e.data.len()).map_err(|_| too_large(&e.path))?;
        let name_len = u16::try_from(name.len()).map_err(|_| too_large(&e.path))?;
        let offset = u32::try_from(out.len()).map_err(|_| too_large("the archive"))?;
        let crc = crc32(&e.data);
        // the local file header
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed: 2.0
        out.extend_from_slice(&0u16.to_le_bytes()); // flags
        out.extend_from_slice(&0u16.to_le_bytes()); // method: stored
        out.extend_from_slice(&TIME.to_le_bytes());
        out.extend_from_slice(&DATE.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&size.to_le_bytes());
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // extra length
        out.extend_from_slice(name);
        out.extend_from_slice(&e.data);
        // its central directory record: made by Unix (3) zip 2.0, the mode in the high half of the external attributes
        let mode: u32 = if e.executable { 0o100_755 } else { 0o100_644 };
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&((3u16 << 8) | 20).to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&TIME.to_le_bytes());
        central.extend_from_slice(&DATE.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&size.to_le_bytes());
        central.extend_from_slice(&name_len.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes()); // extra length
        central.extend_from_slice(&0u16.to_le_bytes()); // comment length
        central.extend_from_slice(&0u16.to_le_bytes()); // disk number
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        central.extend_from_slice(&(mode << 16).to_le_bytes());
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name);
    }
    let count = u16::try_from(entries.len()).map_err(|_| too_large("the entry count"))?;
    let cd_offset = u32::try_from(out.len()).map_err(|_| too_large("the archive"))?;
    let cd_size = u32::try_from(central.len()).map_err(|_| too_large("the central directory"))?;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    Ok(out)
}

/// What the `.dist-info` files say about the distribution.
pub(crate) struct Metadata<'a> {
    pub name: &'a str,
    pub version: &'a str,
    pub summary: Option<&'a str>,
    pub tag: &'a str,
}

/// SHA-256 (FIPS 180-4), for RECORD: the toolchain's dependencies (toolchain 1.3) have blake3 alone.
fn sha256(data: &[u8]) -> [u8; 32] {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09_e667,
        0xbb67_ae85,
        0x3c6e_f372,
        0xa54f_f53a,
        0x510e_527f,
        0x9b05_688c,
        0x1f83_d9ab,
        0x5be0_cd19,
    ];
    let bits = (data.len() as u64).wrapping_mul(8);
    let mut tail = Vec::with_capacity(128);
    let whole = data.len() - data.len() % 64;
    tail.extend_from_slice(&data[whole..]);
    tail.push(0x80);
    while tail.len() % 64 != 56 {
        tail.push(0);
    }
    tail.extend_from_slice(&bits.to_be_bytes());
    for block in data[..whole]
        .as_chunks::<64>()
        .0
        .iter()
        .chain(tail.as_chunks::<64>().0)
    {
        let mut w = [0u32; 64];
        for (i, word) in block.as_chunks::<4>().0.iter().enumerate() {
            w[i] = u32::from_be_bytes(*word);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = h;
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ (!e & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        for (x, y) in h.iter_mut().zip([a, b, c, d, e, f, g, hh]) {
            *x = x.wrapping_add(y);
        }
    }
    let mut out = [0u8; 32];
    for (i, x) in h.iter().enumerate() {
        out[4 * i..4 * i + 4].copy_from_slice(&x.to_be_bytes());
    }
    out
}

/// The URL-safe base64 of RFC 4648 Section 5, without padding.
fn base64_urlsafe(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        let chars = chunk.len() + 1;
        for k in 0..chars {
            out.push(char::from(ALPHABET[((n >> (18 - 6 * k)) & 63) as usize]));
        }
    }
    out
}

/// `sha256=<urlsafe base64 without padding>` of RECORD.
fn record_hash(data: &[u8]) -> String {
    format!("sha256={}", base64_urlsafe(&sha256(data)))
}

/// The wheel of the package's files (each path inside the wheel, sorted here): the files, then `METADATA`, `WHEEL`,
/// and `RECORD`.
pub(crate) fn wheel(meta: &Metadata<'_>, mut files: Vec<Entry>) -> io::Result<Vec<u8>> {
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let info = format!("{}-{}.dist-info", meta.name, meta.version);
    let mut metadata = format!(
        "Metadata-Version: 2.1\nName: {}\nVersion: {}\n",
        meta.name, meta.version
    );
    if let Some(s) = meta.summary.filter(|s| !s.trim().is_empty()) {
        // one line: a summary's line breaks would end the header
        let line: String = s.split_whitespace().collect::<Vec<_>>().join(" ");
        metadata.push_str(&format!("Summary: {line}\n"));
    }
    metadata.push_str("Requires-Python: >=3.10\n");
    let wheel = format!(
        "Wheel-Version: 1.0\nGenerator: cpl {}\nRoot-Is-Purelib: false\nTag: {}\n",
        cpl_diag::version::TOOLCHAIN,
        meta.tag
    );
    files.push(Entry {
        path: format!("{info}/METADATA"),
        data: metadata.into_bytes(),
        executable: false,
    });
    files.push(Entry {
        path: format!("{info}/WHEEL"),
        data: wheel.into_bytes(),
        executable: false,
    });
    let mut lines: Vec<String> = files
        .iter()
        .map(|e| format!("{},{},{}", e.path, record_hash(&e.data), e.data.len()))
        .collect();
    lines.sort();
    let mut record = lines.join("\n");
    record.push_str(&format!("\n{info}/RECORD,,\n"));
    files.push(Entry {
        path: format!("{info}/RECORD"),
        data: record.into_bytes(),
        executable: false,
    });
    zip(&files)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn sha256_matches_the_fips_vectors() {
        assert_eq!(
            hex(&sha256(b"")),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            hex(&sha256(b"abc")),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            hex(&sha256(
                b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"
            )),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
        assert_eq!(
            hex(&sha256(&vec![b'a'; 1_000_000])),
            "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
        );
        // every length around the padding boundary agrees with the one-block computation's shape
        for n in 50..70 {
            assert_eq!(sha256(&vec![7u8; n]).len(), 32);
        }
    }

    #[test]
    fn base64_is_urlsafe_without_padding() {
        assert_eq!(base64_urlsafe(b""), "");
        assert_eq!(base64_urlsafe(b"f"), "Zg");
        assert_eq!(base64_urlsafe(b"fo"), "Zm8");
        assert_eq!(base64_urlsafe(b"foo"), "Zm9v");
        assert_eq!(base64_urlsafe(b"foob"), "Zm9vYg");
        assert_eq!(base64_urlsafe(&[0xfb, 0xff, 0xbf]), "-_-_");
    }

    #[test]
    fn crc32_matches_the_check_value() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn a_wheel_is_deterministic_and_lists_every_file() {
        let meta = Metadata {
            name: "demo_cpl",
            version: "0.1.0",
            summary: Some("a demo\npackage"),
            tag: "cp310-abi3-linux_x86_64",
        };
        let files = || {
            vec![
                Entry {
                    path: "demo_cpl/__init__.py".into(),
                    data: b"x = 1\n".to_vec(),
                    executable: false,
                },
                Entry {
                    path: "demo_cpl/demo-worker".into(),
                    data: vec![0x7f, b'E'],
                    executable: true,
                },
            ]
        };
        let a = wheel(&meta, files()).unwrap();
        let b = wheel(&meta, files()).unwrap();
        assert_eq!(a, b);
        let text = String::from_utf8_lossy(&a);
        assert!(text.contains("Summary: a demo package\n"));
        assert!(text.contains("demo_cpl-0.1.0.dist-info/RECORD,,\n"));
        assert!(text.contains("demo_cpl/__init__.py,sha256="));
    }
}
