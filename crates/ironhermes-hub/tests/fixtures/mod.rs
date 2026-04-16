//! Test fixtures for ironhermes-hub integration tests.
//!
//! Provides helpers to build in-memory tarballs and fixture JSON files
//! so no binary blobs need to be committed to the repo.

use std::io::Write;

use flate2::{write::GzEncoder, Compression};
use tar::Builder;

/// Builds a gzipped tarball in memory containing a sample skill under `anthropics/skills`.
///
/// Structure:
/// ```
/// anthropics-skills-abc123/
///   tenor-gif/
///     SKILL.md     — valid frontmatter with `name: tenor-gif`
///     handler.py   — `# stub`
/// ```
///
/// The GitHub tarball convention wraps everything in `{owner}-{repo}-{sha}/`.
pub fn sample_skill_tarball() -> Vec<u8> {
    let buf = Vec::new();
    let enc = GzEncoder::new(buf, Compression::default());
    let mut ar = Builder::new(enc);

    // SKILL.md content with valid frontmatter
    let skill_md = b"---\nname: tenor-gif\ndescription: Tenor GIF search skill\nversion: 1.0.0\n---\n\n# Tenor GIF\n\nSearches Tenor for animated GIFs.\n";
    add_file(&mut ar, "anthropics-skills-abc123/tenor-gif/SKILL.md", skill_md);

    // handler.py content
    let handler_py = b"# stub\n";
    add_file(&mut ar, "anthropics-skills-abc123/tenor-gif/handler.py", handler_py);

    let enc = ar.into_inner().expect("tar finish");
    enc.finish().expect("gz finish")
}

/// Builds a tarball with a path-traversal entry to test rejection.
///
/// The `tar` crate's `set_path` rejects `..` components, so we write a raw
/// POSIX ustar header directly with the malicious path embedded in the name field.
pub fn traversal_tarball() -> Vec<u8> {
    // Build the raw uncompressed tar bytes with a hand-crafted header
    let mut raw = Vec::<u8>::new();

    // POSIX ustar header is 512 bytes:
    //  [0..100]   name
    //  [100..108] mode (octal string)
    //  [108..116] uid
    //  [116..124] gid
    //  [124..136] size (octal)
    //  [136..148] mtime (octal)
    //  [148..156] checksum (filled later)
    //  [156]      typeflag ('0' = regular file)
    //  [157..257] linkname
    //  [257..265] magic ("ustar")
    //  etc.
    let mut header = [0u8; 512];
    // Malicious path: starts with the expected keep_prefix but then traverses up.
    // After stripping "anthropics-skills-abc123/tenor-gif/" the rel becomes "../../etc/passwd".
    let evil_path = b"anthropics-skills-abc123/tenor-gif/../../etc/passwd";
    let len = evil_path.len().min(99);
    header[..len].copy_from_slice(&evil_path[..len]);
    // mode = 0000644\0
    header[100..108].copy_from_slice(b"0000644\0");
    // uid/gid = 0000000\0
    header[108..116].copy_from_slice(b"0000000\0");
    header[116..124].copy_from_slice(b"0000000\0");
    // size = 0000000\0 (16 bytes for "root:x:0:0:root\n")
    header[124..136].copy_from_slice(b"00000000020\0");
    // mtime
    header[136..148].copy_from_slice(b"00000000000\0");
    // typeflag = '0' (regular file)
    header[156] = b'0';
    // magic = "ustar  \0"
    header[257..265].copy_from_slice(b"ustar  \0");

    // Compute checksum: sum of all bytes with checksum field as spaces
    header[148..156].copy_from_slice(b"        ");
    let cksum: u32 = header.iter().map(|&b| b as u32).sum();
    // Write octal checksum padded to 7 digits + NUL
    let cksum_str = format!("{:07o}\0", cksum);
    header[148..156].copy_from_slice(cksum_str.as_bytes());

    raw.extend_from_slice(&header);

    // File content: "root:x:0:0:root\n" = 16 bytes, padded to 512 boundary
    let content = b"root:x:0:0:root\n";
    raw.extend_from_slice(content);
    // Pad to 512-byte block
    let padding = 512 - (content.len() % 512);
    if padding < 512 {
        raw.extend(std::iter::repeat(0u8).take(padding));
    }

    // End-of-archive: two 512-byte zero blocks
    raw.extend(std::iter::repeat(0u8).take(1024));

    // Compress
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(&raw).expect("gz write");
    enc.finish().expect("gz finish")
}

/// Builds a tarball whose single entry claims to be very large (size field set high).
/// Used to test the MAX_EXTRACTED_BYTES cap.
pub fn oversized_tarball() -> Vec<u8> {
    let buf = Vec::new();
    let enc = GzEncoder::new(buf, Compression::default());
    let mut ar = Builder::new(enc);

    // We add many entries of 1MB each, total > 50MB
    let one_mb = vec![0u8; 1024 * 1024];
    for i in 0..60 {
        let name = format!("anthropics-skills-abc123/tenor-gif/file_{i}.bin");
        add_file(&mut ar, &name, &one_mb);
    }

    let enc = ar.into_inner().expect("tar finish");
    enc.finish().expect("gz finish")
}

/// Builds a tarball for the WellKnownSkillSource: no GitHub-style prefix wrapper.
///
/// Structure:
/// ```
/// foo-skill/
///   SKILL.md
///   handler.py
/// ```
pub fn well_known_skill_tarball() -> Vec<u8> {
    let buf = Vec::new();
    let enc = GzEncoder::new(buf, Compression::default());
    let mut ar = Builder::new(enc);

    let skill_md = b"---\nname: foo-skill\ndescription: Example well-known skill\nversion: 1.0\n---\n\n# Foo Skill\n";
    add_file(&mut ar, "foo-skill/SKILL.md", skill_md);
    add_file(&mut ar, "foo-skill/handler.py", b"# stub\n");

    let enc = ar.into_inner().expect("tar finish");
    enc.finish().expect("gz finish")
}

/// Add a file entry to the tar builder.
fn add_file(ar: &mut Builder<GzEncoder<Vec<u8>>>, path: &str, data: &[u8]) {
    let mut header = tar::Header::new_gnu();
    header.set_path(path).expect("set path");
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    ar.append(&header, data).expect("append");
}

/// Returns the bytes of the well-known index JSON fixture.
pub fn well_known_index_json() -> &'static str {
    r#"[
  {
    "name": "foo-skill",
    "description": "Example well-known skill",
    "version": "1.0",
    "identifier": "well-known:example.com/foo-skill",
    "tarball_url": "https://PLACEHOLDER/foo-skill.tar.gz"
  }
]"#
}
