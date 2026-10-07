//! `trellis run --image <path>` (ADR 0041): which runs may carry an image,
//! how the file is read, and what the trail keeps of it.
//!
//! Hosted tier 1 only. Everything here happens before the snapshot, so a
//! refused image is a run that did not start and costs nothing. The bytes go
//! to the provider and nowhere else: the trail records a name, a size and a
//! SHA-256, and the run's output the same.

use anyhow::{Result, anyhow, bail};
use std::io::Read as _;
use std::path::{Component, Path, PathBuf};
use trellis_model::{Image, ImageError, MAX_IMAGE_BYTES};

/// Said before anything else about an image a model cannot take.
pub(crate) const NO_IMAGE_INPUT: &str = "this model does not take image input";

/// Refuses images for any driver that is not hosted tier 1, naming why.
/// `what` is how the driver is named in the message.
pub(crate) fn refuse_unless_hosted(paths: &[PathBuf], hosted: bool, what: &str) -> Result<()> {
    if paths.is_empty() || hosted {
        return Ok(());
    }
    bail!(
        "{NO_IMAGE_INPUT}: {what}. Images are sent only to a hosted tier-1 model \
         (`anthropic:`, `openai-chat:` or a `[[providers]]` entry): no local runtime in \
         the tier matrix takes image input, and a scripted model is sent no prompt \
         (ADR 0041). Nothing was snapshotted: the run did not start."
    )
}

/// The images a run was given, and what its trail records of them.
#[derive(Debug, Default)]
pub(crate) struct Loaded {
    pub(crate) images: Vec<Image>,
    pub(crate) inputs: Vec<trellis_trail::Input>,
}

/// Reads, checks and encodes every `--image`, then counts the base64
/// against `budget`, the byte budget the hosted run elides to (ADR 0024 §5).
pub(crate) fn load(root: &Path, paths: &[PathBuf], budget: usize) -> Result<Loaded> {
    let mut out = Loaded::default();
    for p in paths {
        let (name, bytes) = read_one(root, p)?;
        let image = Image::from_bytes(&bytes).map_err(|e| refused(&name, &e))?;
        out.inputs
            .push(trellis_trail::Input::hashed("image", &name, &bytes));
        out.images.push(image);
    }
    fits(&out, budget, "")?;
    Ok(out)
}

/// Counts the base64 of `loaded` against `budget`. `whose` follows the
/// budget in the refusal, naming where it came from: `run` checks again
/// once `preflight` has identified colibri, whose default is smaller than
/// the hosted one (ADR 0047 decision 6, #527).
pub(crate) fn fits(loaded: &Loaded, budget: usize, whose: &str) -> Result<()> {
    let total: usize = loaded.images.iter().map(Image::base64_len).sum();
    if total > budget {
        bail!(
            "--image: the images are {total} bytes as base64, the form they are sent in, \
             and this run's context budget is {budget} bytes{whose} ([model] context_bytes, or the model's \
             [[providers]] input_bytes when that is smaller). Attach a smaller image, \
             or raise context_bytes in your user config. \
             Nothing was snapshotted: the run did not start."
        );
    }
    Ok(())
}

/// One line per image for the run's output: what the trail records.
pub(crate) fn lines(loaded: &Loaded) -> Vec<String> {
    loaded
        .inputs
        .iter()
        .zip(&loaded.images)
        .map(|(i, img)| {
            format!(
                "image:    {} ({} bytes, {}, sha256 {})",
                i.name,
                i.bytes,
                img.media_type(),
                i.sha256.as_deref().unwrap_or("")
            )
        })
        .collect()
}

fn refused(name: &str, e: &ImageError) -> anyhow::Error {
    anyhow!("--image {name} was refused: {e}. Nothing was snapshotted: the run did not start.")
}

/// The file's name as the trail records it, and at most
/// `MAX_IMAGE_BYTES + 1` of its bytes.
///
/// A path inside the repository is repository input and is read through
/// `resolve_repo_input`, so a committed link into `.git` or out of the
/// repository is refused as it is for every other prompt input. A path
/// outside it is the user's own file, named by the user on the command
/// line, and is read as given. Which one it is is decided on the path as
/// written, made absolute and with `..` applied, never on where a link
/// leads: deciding on the resolved path would let a link inside the
/// repository pass as the user's own file.
fn read_one(root: &Path, p: &Path) -> Result<(String, Vec<u8>)> {
    let abs = normal(&std::path::absolute(p)?);
    let roots = [
        normal(&std::path::absolute(root)?),
        std::fs::canonicalize(root).map(|r| normal(&r))?,
    ];
    let inside = roots
        .iter()
        .find_map(|r| abs.strip_prefix(r).ok().map(Path::to_path_buf));
    let (name, real) = match inside {
        Some(rel) => {
            let name = rel.to_string_lossy().replace('\\', "/");
            let real = trellis_core::paths::resolve_repo_input(root, &abs)
                .map_err(|e| anyhow!("--image {name}: {e}"))?
                .ok_or_else(|| anyhow!("--image {name}: no such file"))?;
            (name, real)
        }
        None => {
            let file = abs
                .file_name()
                .map_or_else(String::new, |f| f.to_string_lossy().into_owned());
            (format!("<outside>/{file}"), abs)
        }
    };
    let meta = std::fs::metadata(&real)
        .map_err(|e| anyhow!("--image {}: cannot read it ({e})", p.display()))?;
    if !meta.is_file() {
        bail!("--image {}: not a file", p.display());
    }
    // Checked on the metadata first, so a large file is refused without
    // being read, and bounded on the read, so one that grows in between is
    // refused too rather than read whole.
    if meta.len() > MAX_IMAGE_BYTES as u64 {
        return Err(refused(&name, &ImageError::TooLarge { bytes: meta.len() }));
    }
    let mut bytes = Vec::new();
    std::fs::File::open(&real)
        .and_then(|f| f.take(MAX_IMAGE_BYTES as u64 + 1).read_to_end(&mut bytes))
        .map_err(|e| anyhow!("--image {}: cannot read it ({e})", p.display()))?;
    Ok((name, bytes))
}

/// `p` with `.` dropped and `..` applied to the components before it, and
/// without the `\\?\` prefix Windows' canonical form carries, so a path as
/// typed and the canonical root compare component by component.
fn normal(p: &Path) -> PathBuf {
    let plain = p.to_string_lossy();
    let plain = plain.strip_prefix(r"\\?\").unwrap_or(&plain);
    let mut out = PathBuf::new();
    for c in Path::new(plain).components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_inside_the_repository_is_named_relative_and_one_outside_by_file_name() {
        // The risk: an absolute path, the user's home layout, in a trail
        // that is committed and pushed.
        let d = std::env::temp_dir().join(format!("trellis-image-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let root = d.join("repo");
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("docs/a.gif"), b"GIF89a-in").unwrap();
        std::fs::write(d.join("b.gif"), b"GIF89a-out").unwrap();
        let (name, _) = read_one(&root, &root.join("docs/./a.gif")).unwrap();
        assert_eq!(name, "docs/a.gif");
        let (name, bytes) = read_one(&root, &root.join("docs/../../b.gif")).unwrap();
        assert_eq!(
            (name.as_str(), bytes.as_slice()),
            ("<outside>/b.gif", &b"GIF89a-out"[..])
        );
        // `.git` is repository input like any other, and refused.
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join(".git/x.gif"), b"GIF89a").unwrap();
        let e = read_one(&root, &root.join(".git/x.gif"))
            .unwrap_err()
            .to_string();
        assert!(e.contains(".git"), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn an_image_of_exactly_the_cap_is_read_whole_and_one_byte_more_is_refused() {
        // The risk: an off-by-one at the cap that refuses an image the
        // documented limit admits, reads it short, or reads one past it.
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.gif");
        std::fs::write(&p, vec![b'x'; MAX_IMAGE_BYTES]).unwrap();
        let (_, bytes) = read_one(d.path(), &p).unwrap();
        assert_eq!(bytes.len(), MAX_IMAGE_BYTES);
        std::fs::write(&p, vec![b'x'; MAX_IMAGE_BYTES + 1]).unwrap();
        assert!(read_one(d.path(), &p).is_err());
    }

    #[test]
    fn images_whose_base64_exceeds_the_context_budget_are_refused_before_the_run() {
        let d = std::env::temp_dir().join(format!("trellis-image-b-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("a.gif");
        std::fs::write(&p, b"GIF89a012345").unwrap();
        // 12 bytes are 16 of base64.
        assert!(load(&d, std::slice::from_ref(&p), 16).is_ok());
        let e = load(&d, &[p], 15).unwrap_err().to_string();
        assert!(
            e.contains("16 bytes as base64") && e.contains("15 bytes"),
            "{e}"
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
