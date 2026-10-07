use anyhow::Context;
use std::{
    fs::OpenOptions,
    io::Write,
    path::{Path, PathBuf},
};

const BUILTIN_SKILLS: &[&str] = &[
    include_str!("../skills/github/SKILL.md"),
    include_str!("../skills/commit-and-push/SKILL.md"),
];

pub struct Skill {
    pub name: String,
    pub description: String,
    pub path: Option<PathBuf>,
    contents: String,
}

fn parse_skill(contents: &str, path: Option<PathBuf>) -> Option<Skill> {
    let (name, description) = metadata(contents)?;
    Some(Skill {
        name,
        description,
        path,
        contents: contents.into(),
    })
}

fn bundled_skills() -> impl Iterator<Item = Skill> {
    BUILTIN_SKILLS
        .iter()
        .filter_map(|contents| parse_skill(contents, None))
}

fn install_bundled_skills_at(root: &Path) -> anyhow::Result<()> {
    for skill in bundled_skills() {
        let path = root.join(&skill.name).join("SKILL.md");
        std::fs::create_dir_all(path.parent().context("missing skill directory")?)?;
        let mut file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            // Keep user-installed skills with the same name intact.
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "could not create bundled skill at {}",
                        path.display()
                    )
                })
            }
        };
        file.write_all(skill.contents.as_bytes())
            .with_context(|| format!("could not write bundled skill at {}", path.display()))?;
    }
    Ok(())
}

/// Install embedded skills into the standard global skill directory so they
/// can be inspected and loaded like any other `SKILL.md` file.
pub fn install_bundled() -> anyhow::Result<()> {
    let home = dirs::home_dir().context("could not resolve user home directory")?;
    install_bundled_skills_at(&home.join(".mizius/skills"))
}

fn discover_from(cwd: &Path, home: Option<&Path>) -> Vec<Skill> {
    let mut skills = std::collections::BTreeMap::new();
    for skill in bundled_skills() {
        skills.insert(skill.name.clone(), skill);
    }

    let mut roots = Vec::new();
    if let Some(home) = home {
        roots.push(home.join(".mizius/skills"));
    }
    let mut ancestors: Vec<_> = cwd.ancestors().collect();
    ancestors.reverse();
    for dir in ancestors {
        roots.push(dir.join(".agents/skills"));
    }
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path().join("SKILL.md");
            let Ok(contents) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Some(skill) = parse_skill(&contents, Some(path)) else {
                continue;
            };
            skills.insert(skill.name.clone(), skill);
        }
    }
    skills.into_values().collect()
}

pub fn discover(cwd: &Path) -> Vec<Skill> {
    let home = dirs::home_dir();
    if let Some(home) = &home {
        // Discovery remains usable when the home directory is read-only: the
        // embedded copy is retained as a fallback, while normal installs expose
        // every bundled skill from ~/.mizius/skills/<name>/SKILL.md.
        let _ = install_bundled_skills_at(&home.join(".mizius/skills"));
    }
    discover_from(cwd, home.as_deref())
}

fn metadata(contents: &str) -> Option<(String, String)> {
    let contents = contents
        .trim_start_matches('\u{feff}')
        .replace("\r\n", "\n");
    let header = contents.strip_prefix("---\n")?.split_once("\n---")?.0;
    let yaml: serde_yaml::Value = serde_yaml::from_str(header).ok()?;
    let name = yaml["name"].as_str()?.to_string();
    let description = yaml["description"].as_str()?.trim().to_string();
    if name.is_empty()
        || name.len() > 64
        || description.is_empty()
        || description.len() > 1024
        || name.starts_with('-')
        || name.ends_with('-')
        || name.contains("--")
        || !name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return None;
    }
    Some((name, description))
}

pub fn expand(prompt: &str, cwd: &Path) -> anyhow::Result<String> {
    let Some(rest) = prompt
        .strip_prefix("/skill:")
        .or_else(|| prompt.strip_prefix('$'))
    else {
        return Ok(prompt.into());
    };
    let name = rest.split_whitespace().next().unwrap_or_default();
    let skill = discover(cwd)
        .into_iter()
        .find(|skill| skill.name == name)
        .ok_or_else(|| anyhow::anyhow!("Unknown skill {name}"))?;
    let source = skill
        .path
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "the bundled Lathe skill".into());
    Ok(format!(
        "{}\n\nUse the skill from {}:\n{}",
        rest, source, skill.contents
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_standard_skill_frontmatter_including_multiline_description() {
        assert_eq!(metadata("---\r\nname: review-code\r\ndescription: >-\r\n  Review changes\r\n  for bugs.\r\n---\r\nInstructions"),Some(("review-code".into(),"Review changes for bugs.".into())));
        assert!(metadata("---\nname: ../escape\ndescription: bad\n---\n").is_none());
        assert!(metadata("# Missing frontmatter").is_none());
    }

    #[test]
    fn discovers_global_and_project_skills_from_mizius_home_and_agents_project_directories() {
        let root = std::env::temp_dir().join(format!("dray-skills-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        let cwd = root.join("nested/project");
        let global_name = format!("global-{}", uuid::Uuid::new_v4());
        let project_name = format!("project-{}", uuid::Uuid::new_v4());
        let old_name = format!("old-{}", uuid::Uuid::new_v4());
        let global_skill_path = home
            .join(".mizius/skills")
            .join(&global_name)
            .join("SKILL.md");
        let project_skill_path = root
            .join(".agents/skills")
            .join(&project_name)
            .join("SKILL.md");
        let old_skill_path = home
            .join(".agents/skills")
            .join(&old_name)
            .join("SKILL.md");

        for (path, name) in [
            (&global_skill_path, &global_name),
            (&project_skill_path, &project_name),
            (&old_skill_path, &old_name),
        ] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(
                path,
                format!("---\nname: {name}\ndescription: Test skill.\n---\nInstructions."),
            )
            .unwrap();
        }

        let skills = discover_from(&cwd, Some(&home));
        let global = skills
            .iter()
            .find(|skill| skill.name == global_name)
            .unwrap();
        let project = skills
            .iter()
            .find(|skill| skill.name == project_name)
            .unwrap();
        assert_eq!(global.path.as_deref(), Some(global_skill_path.as_path()));
        assert_eq!(project.path.as_deref(), Some(project_skill_path.as_path()));
        assert!(!skills.iter().any(|skill| skill.name == old_name));

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn installs_and_discovers_bundled_skills_from_the_global_mizius_directory() {
        let root = std::env::temp_dir().join(format!("dray-skills-{}", uuid::Uuid::new_v4()));
        let home = root.join("home");
        let skill_root = home.join(".mizius/skills");
        install_bundled_skills_at(&skill_root).unwrap();

        let skills = discover_from(&root.join("project"), Some(&home));
        for (name, marker) in [
            ("commit-and-push", "detailed description"),
            ("github", "gh auth status"),
        ] {
            let skill = skills.iter().find(|skill| skill.name == name).unwrap();
            let path = skill_root.join(name).join("SKILL.md");
            assert_eq!(skill.path.as_deref(), Some(path.as_path()));
            assert!(skill.contents.contains(marker));
            assert_eq!(std::fs::read_to_string(path).unwrap(), skill.contents);
        }

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn installing_bundled_skills_does_not_overwrite_existing_global_skills() {
        let root = std::env::temp_dir().join(format!("dray-skills-{}", uuid::Uuid::new_v4()));
        let skill_root = root.join(".mizius/skills");
        let path = skill_root.join("github/SKILL.md");
        let custom = concat!(
            "---\nname: github\ndescription: My GitHub workflow.\n---\n",
            "Custom instructions."
        );
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, custom).unwrap();

        install_bundled_skills_at(&skill_root).unwrap();

        assert_eq!(std::fs::read_to_string(path).unwrap(), custom);
        std::fs::remove_dir_all(root).unwrap();
    }
}
