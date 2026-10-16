//! Attachments: files dropped or pasted into a note are copied into a `data`
//! folder next to it and linked with a relative path.

use std::io;
use std::path::{Component, Path, PathBuf};

use crate::workspace::{self, DATA_DIR};

/// Image types WIC decodes, which render inline when on a line of their own.
pub fn is_image(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "tif" | "tiff" | "ico" | "webp"
        )
    })
}

/// `dir/name`, or `dir/name (2).ext`, ... when that name is taken.
fn free_name(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let p = Path::new(name);
    let stem = p.file_stem().map_or(name.into(), |s| s.to_string_lossy());
    let ext = p
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .unwrap()
}

/// Copies `src` into the `data` folder next to `note`. A file there with the same
/// name and contents is reused; a different one keeps its name and the copy gets
/// a numbered one.
pub fn copy_in(src: &Path, note: &Path) -> io::Result<PathBuf> {
    let dir = note.parent().unwrap_or(Path::new("")).join(DATA_DIR);
    std::fs::create_dir_all(&dir)?;
    let name = src
        .file_name()
        .ok_or(io::ErrorKind::InvalidInput)?
        .to_string_lossy()
        .into_owned();
    let same = dir.join(&name);
    if same.is_file()
        && std::fs::metadata(&same)?.len() == std::fs::metadata(src)?.len()
        && std::fs::read(&same)? == std::fs::read(src)?
    {
        return Ok(same);
    }
    let target = free_name(&dir, &name);
    std::fs::copy(src, &target)?;
    Ok(target)
}

/// A free path for a new file named `name` in the `data` folder next to `note`,
/// creating the folder.
pub fn new_file(name: &str, note: &Path) -> io::Result<PathBuf> {
    let dir = note.parent().unwrap_or(Path::new("")).join(DATA_DIR);
    std::fs::create_dir_all(&dir)?;
    Ok(free_name(&dir, name))
}

/// The path from folder `from` to `to`, with `/` separators and `..` where needed.
/// Comparison ignores case, as on Windows.
pub fn relative_path(from: &Path, to: &Path) -> String {
    let parts = |p: &Path| -> Vec<String> {
        p.components()
            .filter(|c| !matches!(c, Component::CurDir))
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect()
    };
    let (a, b) = (parts(from), parts(to));
    let common = a
        .iter()
        .zip(&b)
        .take_while(|(x, y)| x.to_lowercase() == y.to_lowercase())
        .count();
    if common == 0 {
        return to.to_string_lossy().replace('\\', "/");
    }
    let mut out: Vec<&str> = vec![".."; a.len() - common];
    out.extend(b[common..].iter().map(String::as_str));
    out.join("/")
}

/// Percent-encodes what would end or confuse a link destination.
fn encode(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for c in path.chars() {
        match c {
            ' ' => out.push_str("%20"),
            '%' => out.push_str("%25"),
            '(' => out.push_str("%28"),
            ')' => out.push_str("%29"),
            '<' => out.push_str("%3C"),
            '>' => out.push_str("%3E"),
            c => out.push(c),
        }
    }
    out
}

/// Decodes `%XX` escapes (UTF-8), leaving anything malformed as it is.
pub fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |c: u8| (c as char).to_digit(16);
        if b[i] == b'%'
            && i + 2 < b.len()
            && let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2]))
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// A Markdown link from a note in `dir` to `file`, labelled with the file name:
/// an image (`![...]`) for picture files.
pub fn link(dir: &Path, file: &Path) -> String {
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let label = name.replace('[', r"\[").replace(']', r"\]");
    let bang = if is_image(file) { "!" } else { "" };
    format!("{bang}[{label}]({})", encode(&relative_path(dir, file)))
}

/// Links for `files` attached to `note` (in the folder `root`): files already in
/// the folder are linked where they are, others are copied in first. Returns the
/// Markdown, and how many files could not be attached with the first error.
pub fn attach(files: &[PathBuf], note: &Path, root: &Path) -> (Vec<String>, Option<String>) {
    let dir = note.parent().unwrap_or(Path::new(""));
    let mut links = Vec::new();
    let mut failed = (0, None);
    for f in files {
        let target = if f.is_dir() {
            Err(io::Error::other("folders can't be attached"))
        } else if workspace::contains(root, f) {
            Ok(f.clone())
        } else {
            copy_in(f, note)
        };
        match target {
            Ok(t) => links.push(link(dir, &t)),
            Err(e) => {
                failed.0 += 1;
                failed.1.get_or_insert_with(|| {
                    format!(
                        "{}: {e}",
                        f.file_name().unwrap_or_default().to_string_lossy()
                    )
                });
            }
        }
    }
    let err = failed.1.map(|e| {
        if failed.0 > 1 {
            format!("{} files not attached ({e})", failed.0)
        } else {
            format!("Not attached: {e}")
        }
    });
    (links, err)
}

/// A file name for a pasted image, from the local date and time.
pub fn pasted_name(t: (i32, u32, u32, u32, u32, u32)) -> String {
    format!(
        "pasted-{:04}-{:02}-{:02}-{:02}{:02}{:02}.png",
        t.0, t.1, t.2, t.3, t.4, t.5
    )
}

/// A `.bmp` file's bytes for a clipboard DIB (a BITMAPINFO header, then pixels),
/// so the WIC BMP decoder can read it.
pub fn bmp_from_dib(dib: &[u8]) -> Option<Vec<u8>> {
    let u32_at = |i: usize| Some(u32::from_le_bytes(dib.get(i..i + 4)?.try_into().ok()?));
    let header = u32_at(0)? as usize;
    let bits = u16::from_le_bytes(dib.get(14..16)?.try_into().ok()?) as u32;
    let compression = u32_at(16)?;
    let used = u32_at(32)?;
    // BI_BITFIELDS (3) / BI_ALPHABITFIELDS (6) masks follow a plain 40-byte header.
    let masks = match (header, compression) {
        (40, 3) => 12,
        (40, 6) => 16,
        _ => 0,
    };
    let colors = if used > 0 {
        used
    } else if bits <= 8 {
        1 << bits
    } else {
        0
    };
    let offset = 14 + header + masks + colors as usize * 4;
    let size = 14 + dib.len();
    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(offset as u32).to_le_bytes());
    out.extend_from_slice(dib);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_paths() {
        let p = |s: &str| PathBuf::from(s);
        assert_eq!(
            relative_path(&p(r"C:\n"), &p(r"C:\n\data\a b.pdf")),
            "data/a b.pdf"
        );
        assert_eq!(
            relative_path(&p(r"C:\n\x"), &p(r"c:\N\y\z.png")),
            "../y/z.png"
        );
        assert_eq!(relative_path(&p(r"C:\n"), &p(r"D:\z.png")), "D:/z.png");
    }

    #[test]
    fn links() {
        let dir = Path::new(r"C:\notes");
        assert_eq!(
            link(dir, Path::new(r"C:\notes\data\My (1).pdf")),
            "[My (1).pdf](data/My%20%281%29.pdf)"
        );
        assert_eq!(
            link(dir, Path::new(r"C:\notes\data\a[b].png")),
            r"![a\[b\].png](data/a[b].png)"
        );
        assert_eq!(decode("data/My%20%281%29.pdf"), "data/My (1).pdf");
        assert_eq!(decode("100%"), "100%");
        assert_eq!(decode("caf%C3%A9 %zz"), "café %zz");
        assert_eq!(
            pasted_name((2026, 10, 6, 9, 5, 3)),
            "pasted-2026-10-06-090503.png"
        );
    }

    #[test]
    fn copies_into_data() {
        let root = std::env::temp_dir().join(format!("penmark-attach-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("notes")).unwrap();
        let outside = std::env::temp_dir().join(format!("penmark-src-{}.txt", std::process::id()));
        std::fs::write(&outside, "one").unwrap();
        std::fs::write(root.join("notes/inside.pdf"), "x").unwrap();
        let note = root.join("notes/n.md");
        let files = [outside.clone(), root.join("notes/inside.pdf")];
        let (links, err) = attach(&files, &note, &root);
        let name = outside.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(err, None);
        assert_eq!(links[0], format!("[{name}](data/{name})"));
        assert_eq!(links[1], "[inside.pdf](inside.pdf)");
        // Same contents: reused. Different contents: a numbered copy.
        assert_eq!(
            copy_in(&outside, &note).unwrap(),
            root.join("notes/data").join(&name)
        );
        std::fs::write(&outside, "two").unwrap();
        let second = copy_in(&outside, &note).unwrap();
        assert!(second.to_string_lossy().ends_with(" (2).txt"));
        let (_, err) = attach(&[root.join("notes")], &note, &root);
        assert!(err.is_some());
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_file(&outside);
    }

    #[test]
    fn dib_to_bmp() {
        // 2x1 24-bit DIB: 40-byte header, one padded row of 8 bytes.
        let mut dib = vec![0u8; 48];
        dib[0] = 40;
        dib[4] = 2;
        dib[8] = 1;
        dib[12] = 1;
        dib[14] = 24;
        let bmp = bmp_from_dib(&dib).unwrap();
        assert_eq!(&bmp[..2], b"BM");
        assert_eq!(u32::from_le_bytes(bmp[2..6].try_into().unwrap()), 62);
        assert_eq!(u32::from_le_bytes(bmp[10..14].try_into().unwrap()), 54);
        dib[16] = 3; // BI_BITFIELDS: three masks after the header
        dib[14] = 32;
        assert_eq!(
            u32::from_le_bytes(bmp_from_dib(&dib).unwrap()[10..14].try_into().unwrap()),
            66
        );
    }
}
