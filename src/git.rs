use std::fs::File;
use std::io::{self, Write};
use std::process::Command;

use git2::{Commit, DiffOptions, Error, Repository, Signature};
use tempfile::NamedTempFile;

/// Gets a formatted diff from the git repository.
pub fn get_pretty_diff(repo: &Repository, context_lines: u32) -> Result<String, Error> {
    let mut output = String::new();
    let index = repo.index()?;
    let head = repo.head()?;
    let head_commit = head.peel_to_commit()?;
    let head_tree = head_commit.tree()?;
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

    let mut diff_options = DiffOptions::new();
    diff_options
        .context_lines(context_lines)
        .interhunk_lines(3)
        .id_abbrev(40)
        .ignore_whitespace(false)
        .patience(true)
        .minimal(true);

    let diff = repo.diff_tree_to_index(Some(&head_tree), Some(&index), Some(&mut diff_options))?;
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
    let head = repo.head()?;
    let parent_commit = head.peel_to_commit()?;
    let signature = get_default_signature(repo)?;

    repo.commit(
        Some("HEAD"),
        &signature,
        &signature,
        commit_message,
        &tree,
        &[&parent_commit],
    )?;

    Ok(())
}

pub fn get_commit_history(repo: &Repository) -> Result<Vec<String>, Error> {
    let mut revwalk = repo.revwalk()?;
    let MAX_LEN = 10;
    revwalk.push_head()?;
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
