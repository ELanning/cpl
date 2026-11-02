//! The embedded standard library and its digest (`spec/toolchain.md` 3.6).

use cpl_diag::json::Digest;

include!(concat!(env!("OUT_DIR"), "/std_files.rs"));

/// `std_digest` (Section 3.6): blake3 over the concatenation of
/// `name LF content LF` for every file, in the order given, which for the
/// standard library is sorted file-name order. `name` is the file name as
/// in [`STD_FILES`] (`"prelude.cpl"`). The digest of the embedded library is
/// `std_digest(STD_FILES)`; [`crate::Project::std_digest`] digests the files
/// a project was loaded with.
pub fn std_digest(files: &[(&str, &str)]) -> Digest {
    let mut hasher = blake3::Hasher::new();
    for (name, content) in files {
        hasher.update(name.as_bytes());
        hasher.update(b"\n");
        hasher.update(content.as_bytes());
        hasher.update(b"\n");
    }
    Digest(hasher.finalize().to_hex().to_string())
}

/// blake3-256 of some bytes, as 64 lowercase hex characters.
pub(crate) fn digest_bytes(bytes: &[u8]) -> Digest {
    Digest(blake3::hash(bytes).to_hex().to_string())
}
