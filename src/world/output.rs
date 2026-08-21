//! Output-related safety helpers.

use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};

/// Reject output directories that overlap the input world.
///
/// Writing into the world itself (or a parent of it) would destroy the source
/// data, so this must be checked before any write happens. Mirrors
/// OrzMCBackup's `OverlapGuard`.
pub fn ensure_no_overlap(input: &Path, output: Option<&Path>) -> Result<()> {
    let Some(output) = output else {
        return Ok(());
    };
    let input_c = normalize(input)?;
    let output_c = normalize(output)?;
    if input_c == output_c || input_c.starts_with(&output_c) || output_c.starts_with(&input_c) {
        return Err(anyhow!(
            "output directory {} overlaps input world {}",
            output.display(),
            input.display()
        ));
    }
    Ok(())
}

/// Canonicalize a path, falling back to canonicalizing the nearest existing
/// ancestor so that a not-yet-created output directory can still be compared.
fn normalize(path: &Path) -> Result<PathBuf> {
    if let Ok(c) = path.canonicalize() {
        return Ok(c);
    }
    let mut missing = Vec::new();
    let mut cur = path.to_path_buf();
    loop {
        match cur.canonicalize() {
            Ok(c) => {
                let mut result = c;
                for seg in missing.iter().rev() {
                    result.push(seg);
                }
                return Ok(result);
            }
            Err(_) => {
                match cur.file_name() {
                    Some(name) => missing.push(name.to_os_string()),
                    None => return Err(anyhow!("cannot canonicalize {}", path.display())),
                }
                if !cur.pop() {
                    return Err(anyhow!("cannot canonicalize {}", path.display()));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_identical() {
        let d = std::env::temp_dir().join(format!("rt-overlap-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&d).unwrap();
        assert!(ensure_no_overlap(&d, Some(&d)).is_err());
    }

    #[test]
    fn rejects_nested() {
        let root = std::env::temp_dir().join(format!("rt-overlap-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let child = root.join("out");
        // child not created yet; normalize falls back to the parent
        assert!(ensure_no_overlap(&root, Some(&child)).is_err());
    }

    #[test]
    fn allows_siblings() {
        let root = std::env::temp_dir().join(format!("rt-overlap-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("world")).unwrap();
        let input = root.join("world");
        let out = root.join("out");
        assert!(ensure_no_overlap(&input, Some(&out)).is_ok());
    }
}
