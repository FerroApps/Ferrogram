//! Safe extraction of a release tarball (`.tar.gz`).
//!
//! Every entry is checked before anything is written: only regular files
//! and directories, only relative paths without `..`, all under one
//! top-level directory, and bounded total size and entry count. Symlinks,
//! hard links, devices and FIFOs reject the whole archive.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use super::UpdateError;

/// Bounds for one archive.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Total bytes of all files (uncompressed).
    pub max_total: u64,
    pub max_entries: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_total: 512 * 1024 * 1024,
            max_entries: 1000,
        }
    }
}

fn unsafe_archive(why: &str) -> UpdateError {
    UpdateError::UnsafeArchive(why.to_owned())
}

/// A relative, `..`-free path made of plain names, or `None`.
pub fn clean_path(path: &Path) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::Normal(name) => out.push(name),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

/// Extract the gzip-compressed tar from `reader` into `dest` (which must
/// exist and be empty). Returns the archive's single top-level directory.
pub fn extract_tar_gz(
    reader: impl Read,
    dest: &Path,
    limits: Limits,
) -> Result<PathBuf, UpdateError> {
    let gz = flate2::read::GzDecoder::new(reader);
    let mut archive = tar::Archive::new(gz);
    let entries = archive
        .entries()
        .map_err(|e| UpdateError::io("read archive", &e))?;
    let mut top: Option<PathBuf> = None;
    let mut total: u64 = 0;
    let mut count = 0usize;
    for entry in entries {
        let mut entry = entry.map_err(|_| unsafe_archive("corrupt tar"))?;
        count += 1;
        if count > limits.max_entries {
            return Err(unsafe_archive("too many entries"));
        }
        let kind = entry.header().entry_type();
        if kind.is_pax_global_extensions() {
            continue;
        }
        if !(kind.is_file() || kind.is_dir()) {
            return Err(unsafe_archive(
                if kind.is_symlink() || kind.is_hard_link() {
                    "links are not allowed"
                } else {
                    "special files are not allowed"
                },
            ));
        }
        let raw = entry
            .path()
            .map_err(|_| unsafe_archive("unreadable path"))?
            .into_owned();
        let rel = clean_path(&raw).ok_or_else(|| unsafe_archive("unsafe path"))?;
        let first = rel
            .components()
            .next()
            .map(|c| PathBuf::from(c.as_os_str()))
            .ok_or_else(|| unsafe_archive("unsafe path"))?;
        match &top {
            None => top = Some(first),
            Some(t) if *t == first => {}
            Some(_) => return Err(unsafe_archive("more than one top-level directory")),
        }
        let target = dest.join(&rel);
        if kind.is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| UpdateError::io("extract", &e))?;
            continue;
        }
        if rel.components().count() < 2 {
            return Err(unsafe_archive("file outside the top-level directory"));
        }
        let size = entry.size();
        total = total.saturating_add(size);
        if total > limits.max_total {
            return Err(UpdateError::TooLarge);
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| UpdateError::io("extract", &e))?;
        }
        // `create_new`: a duplicate entry can never overwrite (or follow) an
        // earlier one.
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|e| UpdateError::io("extract", &e))?;
        let copied = std::io::copy(&mut (&mut entry).take(size.saturating_add(1)), &mut file)
            .map_err(|_| unsafe_archive("corrupt tar"))?;
        if copied != size {
            return Err(unsafe_archive("truncated entry"));
        }
    }
    top.ok_or_else(|| unsafe_archive("empty archive"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Build a tar.gz in memory: `(path, kind, data)` with kind
    /// `'f'` file, `'d'` dir, `'l'` symlink, `'h'` hard link.
    pub(crate) fn tar_gz(entries: &[(&str, char, &[u8])]) -> Vec<u8> {
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut b = tar::Builder::new(gz);
        for (path, kind, data) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_mode(0o755);
            match kind {
                'd' => {
                    h.set_entry_type(tar::EntryType::Directory);
                    h.set_size(0);
                }
                'l' | 'h' => {
                    h.set_entry_type(if *kind == 'l' {
                        tar::EntryType::Symlink
                    } else {
                        tar::EntryType::Link
                    });
                    h.set_size(0);
                    h.set_link_name("/etc/passwd").unwrap();
                }
                _ => {
                    h.set_entry_type(tar::EntryType::Regular);
                    h.set_size(data.len() as u64);
                }
            }
            // Write the raw name so tests can produce `..` and absolute paths
            // (the builder's own setters refuse them).
            let name = &mut h.as_old_mut().name;
            name.fill(0);
            name[..path.len()].copy_from_slice(path.as_bytes());
            h.set_cksum();
            b.append(&h, *data).unwrap();
        }
        b.into_inner().unwrap().finish().unwrap()
    }

    fn extract(entries: &[(&str, char, &[u8])]) -> Result<PathBuf, UpdateError> {
        let dir = tempfile::tempdir().unwrap();
        let data = tar_gz(entries);
        extract_tar_gz(&data[..], dir.path(), Limits::default())
            .map(|top| PathBuf::from(top.file_name().unwrap()))
    }

    #[test]
    fn extracts_a_release_layout() {
        let dir = tempfile::tempdir().unwrap();
        let data = tar_gz(&[
            ("ferrogram-v1/", 'd', b""),
            ("ferrogram-v1/ferrogram", 'f', b"ELF"),
            ("ferrogram-v1/packaging/x.desktop", 'f', b"[Desktop Entry]"),
        ]);
        let top = extract_tar_gz(&data[..], dir.path(), Limits::default()).unwrap();
        assert_eq!(top, PathBuf::from("ferrogram-v1"));
        assert_eq!(
            std::fs::read(dir.path().join("ferrogram-v1/ferrogram")).unwrap(),
            b"ELF"
        );
        assert!(
            dir.path()
                .join("ferrogram-v1/packaging/x.desktop")
                .is_file()
        );
    }

    #[test]
    fn rejects_traversal_absolute_paths_and_links() {
        for (bad, why) in [
            (vec![("top/../../evil", 'f', &b"x"[..])], "unsafe path"),
            (vec![("/etc/evil", 'f', &b"x"[..])], "unsafe path"),
            (vec![("top/link", 'l', &b""[..])], "links are not allowed"),
            (vec![("top/hard", 'h', &b""[..])], "links are not allowed"),
            (
                vec![("a/x", 'f', &b"x"[..]), ("b/y", 'f', &b"y"[..])],
                "more than one top-level directory",
            ),
            (
                vec![("loose", 'f', &b"x"[..])],
                "file outside the top-level directory",
            ),
            (
                vec![("top/x", 'f', &b"1"[..]), ("top/x", 'f', &b"2"[..])],
                "extract",
            ),
        ] {
            match extract(&bad) {
                Err(UpdateError::UnsafeArchive(w)) => assert_eq!(w, why),
                Err(UpdateError::Io(w)) => assert!(w.starts_with(why), "{w}"),
                other => panic!("{bad:?} -> {other:?}"),
            }
        }
    }

    #[test]
    fn rejects_oversized_and_garbage_archives() {
        let dir = tempfile::tempdir().unwrap();
        let data = tar_gz(&[("top/big", 'f', &[0u8; 4096])]);
        let small = Limits {
            max_total: 1024,
            max_entries: 10,
        };
        assert_eq!(
            extract_tar_gz(&data[..], dir.path(), small),
            Err(UpdateError::TooLarge)
        );
        let many: Vec<(String, char, &[u8])> = (0..5)
            .map(|i| (format!("top/f{i}"), 'f', &b"x"[..]))
            .collect();
        let many: Vec<(&str, char, &[u8])> =
            many.iter().map(|(p, k, d)| (p.as_str(), *k, *d)).collect();
        let dir2 = tempfile::tempdir().unwrap();
        let few = Limits {
            max_total: 1 << 20,
            max_entries: 3,
        };
        assert!(matches!(
            extract_tar_gz(&tar_gz(&many)[..], dir2.path(), few),
            Err(UpdateError::UnsafeArchive(_))
        ));
        let dir3 = tempfile::tempdir().unwrap();
        assert!(extract_tar_gz(&b"not gzip at all"[..], dir3.path(), Limits::default()).is_err());
    }

    #[test]
    fn clean_paths() {
        assert_eq!(clean_path(Path::new("./a/b")), Some(PathBuf::from("a/b")));
        assert_eq!(clean_path(Path::new("a/../b")), None);
        assert_eq!(clean_path(Path::new("/a")), None);
        assert_eq!(clean_path(Path::new(".")), None);
    }
}
