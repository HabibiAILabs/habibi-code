//! Agent Skills discovery. Only metadata is retained until an approved load.
use anyhow::{Context, Result, ensure};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

const MAX_SKILL: u64 = 2 * 1024 * 1024;
#[derive(Clone)]
pub(crate) struct Skill {
    pub path: PathBuf,
    pub frontmatter: String,
}
#[derive(Deserialize)]
struct Metadata {
    name: String,
    description: String,
}

fn read(path: &Path) -> Result<String> {
    let file = fs::File::open(path)?;
    ensure!(file.metadata()?.is_file(), "Skill is not a file");
    let mut text = String::new();
    file.take(MAX_SKILL + 1).read_to_string(&mut text)?;
    ensure!(text.len() as u64 <= MAX_SKILL, "Skill exceeds 2 MiB");
    Ok(text)
}
fn parse(text: &str, directory: &str) -> Result<String> {
    let mut lines = text.lines();
    ensure!(lines.next() == Some("---"), "Missing YAML frontmatter");
    let mut yaml = String::new();
    let mut closed = false;
    for line in lines {
        if line == "---" {
            closed = true;
            break;
        }
        yaml.push_str(line);
        yaml.push('\n');
    }
    ensure!(closed, "Unterminated YAML frontmatter");
    let metadata: Metadata = serde_yaml::from_str(&yaml)?;
    let name = &metadata.name;
    ensure!(
        name == directory
            && !name.is_empty()
            && name.len() <= 64
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !name.starts_with('-')
            && !name.ends_with('-')
            && !name.contains("--"),
        "Invalid skill name"
    );
    ensure!(
        !metadata.description.trim().is_empty() && metadata.description.chars().count() <= 1024,
        "Invalid skill description"
    );
    Ok(yaml)
}

/// Accept agent directories (with skills/) as well as direct skill roots.
pub(crate) fn resolve_roots(workspace: &Path, dirs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut roots = Vec::new();
    for dir in dirs {
        let text = dir.to_string_lossy();
        ensure!(!text.trim().is_empty(), "Skill directory cannot be empty");
        let path = if text == "~" || text.starts_with("~/") {
            let home = std::env::var_os("HOME")
                .context("HOME is unset; use an absolute skill directory")?;
            PathBuf::from(home).join(text.strip_prefix("~/").unwrap_or(""))
        } else {
            workspace.join(dir)
        };
        roots.push(path.clone());
        roots.push(path.join("skills"));
    }
    Ok(roots)
}

/// Roots are low-to-high precedence; invalid entries do not hide valid ones.
pub(crate) fn discover(roots: &[PathBuf]) -> BTreeMap<String, Skill> {
    let mut skills = BTreeMap::new();
    for root in roots {
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let path = entry.path().join("SKILL.md");
            let Ok(frontmatter) = read(&path).and_then(|text| parse(&text, &name)) else {
                continue;
            };
            skills.insert(name, Skill { path, frontmatter });
        }
    }
    skills
}
impl Skill {
    pub fn load(&self, name: &str) -> Result<String> {
        let text = read(&self.path).context("Cannot read skill")?;
        parse(&text, name)?;
        Ok(format!(
            "Skill: {name}\nBase directory: {}\nResolve relative skill resources from this directory.\n\n{text}",
            self.path.parent().unwrap().display()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn yaml_and_validation() {
        assert!(parse("---\nname: demo\ndescription: |\n  Multi-line description\nmetadata:\n  author: test\n---\nBody", "demo").is_ok());
        for text in [
            "no frontmatter",
            "---\nname: demo\n---",
            "---\nname: other\ndescription: x\n---",
            "---\nname: demo\ndescription: x",
        ] {
            assert!(parse(text, "demo").is_err());
        }
    }
    #[test]
    fn discovery_precedence_and_lazy_body() {
        let tmp = tempfile::tempdir().unwrap();
        let roots: Vec<_> = ["user", "project"]
            .iter()
            .map(|p| tmp.path().join(p))
            .collect();
        for (i, root) in roots.iter().enumerate() {
            fs::create_dir_all(root.join("demo")).unwrap();
            fs::write(
                root.join("demo/SKILL.md"),
                format!("---\nname: demo\ndescription: source {i}\n---\nSECRET BODY"),
            )
            .unwrap();
        }
        let skills = discover(&roots);
        assert!(skills["demo"].frontmatter.contains("source 1"));
        assert!(!skills["demo"].frontmatter.contains("SECRET"));
        assert!(skills["demo"].load("demo").unwrap().contains("SECRET BODY"));
    }
}
