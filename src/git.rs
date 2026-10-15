use std::fs::File;
use std::io::{self, Write};
use std::process::Command;

use git2::{Commit, DiffOptions, Error, ErrorCode, Repository, Signature};
use tempfile::NamedTempFile;

/// Gets a formatted diff from the git repository.
pub fn get_pretty_diff(repo: &Repository, context_lines: u32) -> Result<String, Error> {
    let mut output = String::new();
    let index = repo.index()?;
    let mut status_options = git2::StatusOptions::new();
    let statuses = repo.statuses(Some(&mut status_options))?;
    let has_staged = statuses.iter().any(|entry| {
        let status = entry.status();
        status.contains(git2::Status::INDEX_NEW)
            || status.contains(git2::Status::INDEX_MODIFIED)
            || status.contains(git2::Status::INDEX_DELETED)
            || status.contains(git2::Status::INDEX_RENAMED)
            || status.contains(git2::Status::INDEX_TYPECHANGE)
    });
    if !has_staged {
        log::warn!("No changes added");
        return Err(Error::from_str("No changes added"));
    }

    let head_tree = head_commit(repo)?.map(|commit| commit.tree()).transpose()?;

    let mut diff_options = DiffOptions::new();
    diff_options
        .context_lines(context_lines)
        .interhunk_lines(3)
        .id_abbrev(40)
        .ignore_whitespace(false)
        .patience(true)
        .minimal(true);

    let diff =
        repo.diff_tree_to_index(head_tree.as_ref(), Some(&index), Some(&mut diff_options))?;
    let mut current_file: Option<String> = None;

    diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
        let line_content = String::from_utf8_lossy(line.content());

        if let Some(path) = delta.new_file().path() {
            let path_str = path.to_string_lossy().to_string();
            if current_file.as_ref() != Some(&path_str) {
                current_file = Some(path_str);
                output.push_str(&format!("\npath: {}\n", path.display()));
            }
        }
        match line.origin() {
            '+' | '-' | ' ' => {
                output.push(line.origin());
                output.push(' ');
                output.push_str(&line_content);
            }
            _ => {}
        }
        true
    })?;

    Ok(output)
}

/// Gets the default git signature from the repository config.
pub fn get_default_signature(
    repo: &Repository,
) -> Result<Signature<'_>, Box<dyn std::error::Error>> {
    let config = repo.config()?;
    let name = config.get_string("user.name")?;
    let email = config.get_string("user.email")?;
    Ok(Signature::now(&name, &email)?)
}

/// Gets a commit message interactively using an editor.
pub fn get_commit_message_interactively(initial_message: &str) -> io::Result<String> {
    let mut temp_file = NamedTempFile::new()?;
    writeln!(temp_file, "{initial_message}")?;
    temp_file.flush()?;

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
    Command::new(editor).arg(temp_file.path()).status()?;

    let mut file = File::open(temp_file.path())?;
    let mut edited_message = String::new();
    io::Read::read_to_string(&mut file, &mut edited_message)?;

    Ok(edited_message)
}

/// Creates a commit with the given message.
pub fn create_commit(
    repo: &Repository,
    commit_message: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut index = repo.index()?;
    index.write()?;

    let oid = index.write_tree()?;
    let tree = repo.find_tree(oid)?;
    let signature = get_default_signature(repo)?;
    let parent = head_commit(repo)?;
    let parents: Vec<&Commit<'_>> = parent.iter().collect();

    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        commit_message,
        &tree,
        &parents,
    )?;

    Ok(())
}

/// Returns the HEAD commit, or `None` when HEAD points at a branch with no commits yet.
fn head_commit(repo: &Repository) -> Result<Option<Commit<'_>>, Error> {
    match repo.head() {
        Ok(head) => head.peel_to_commit().map(Some),
        Err(error) if error.code() == ErrorCode::UnbornBranch => Ok(None),
        Err(error) => Err(error),
    }
}

pub fn get_commit_history(repo: &Repository) -> Result<Vec<String>, Error> {
    let Some(head) = head_commit(repo)? else {
        return Ok(Vec::new());
    };
    let mut revwalk = repo.revwalk()?;
    let MAX_LEN = 10;
    revwalk.push(head.id())?;
    revwalk.set_sorting(git2::Sort::TIME)?;
    let mut commits = vec![];
    for (idx, oid_result) in revwalk.into_iter().enumerate() {
        log::debug!("idx: {idx}");
        if idx >= MAX_LEN {
            break;
        }
        let oid = oid_result?;
        let commit = repo.find_commit(oid)?;
        if let Some(msg) = commit.message() {
            commits.push(msg.to_string());
        }
    }
    Ok(commits)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use git2::Repository;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn unborn_repositories_have_empty_history() {
        let temp = tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();

        assert!(get_commit_history(&repo).unwrap().is_empty());
    }

    #[test]
    fn first_commit_uses_the_staged_diff_and_has_no_parents() {
        let temp = tempdir().unwrap();
        let repo = Repository::init(temp.path()).unwrap();
        repo.config()
            .unwrap()
            .set_str("user.name", "Test User")
            .unwrap();
        repo.config()
            .unwrap()
            .set_str("user.email", "test@example.com")
            .unwrap();
        fs::write(temp.path().join("first.txt"), "first commit\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("first.txt")).unwrap();
        index.write().unwrap();
        fs::write(temp.path().join("untracked.txt"), "untracked\n").unwrap();

        assert!(get_pretty_diff(&repo, 3).unwrap().contains("first commit"));
        create_commit(&repo, "feat: initial commit").unwrap();

        let commit = repo.head().unwrap().peel_to_commit().unwrap();
        assert_eq!(commit.parent_count(), 0);
        assert_eq!(
            get_commit_history(&repo).unwrap(),
            vec!["feat: initial commit"]
        );
    }
}
