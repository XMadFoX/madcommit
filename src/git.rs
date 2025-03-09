use std::fs::File;
use std::io::{self, Write};
use std::process::Command;

use git2::{DiffOptions, Error, Repository, Signature};
use tempfile::NamedTempFile;

/// Gets a formatted diff from the git repository
pub fn get_pretty_diff(repo: &Repository, context_lines: u32) -> Result<String, Error> {
    let mut output = String::new();

    // Retrieve the index from the repository
    let index = repo.index()?;
    // Retrieve the HEAD commit
    let head = repo.head()?;
    let head_commit = head.peel_to_commit()?;
    // Retrieve the tree associated with the HEAD commit
    let head_tree = head_commit.tree()?;

    // Create diff options
    let mut diff_options = DiffOptions::new();
    diff_options
        .context_lines(context_lines) // Set the number of context lines
        .interhunk_lines(3) // Set the number of inter-hunk lines
        .id_abbrev(40) // Set the abbreviation length for object IDs
        .ignore_whitespace(false) // Do not ignore whitespace changes
        .patience(true) // Use the "patience diff" algorithm
        .minimal(true); // Spend extra time to find minimal diff

    // Generate a diff between the HEAD tree and the index
    let diff = repo.diff_tree_to_index(Some(&head_tree), Some(&index), Some(&mut diff_options))?;

    // Track the current file path as an Option<String> to simplify comparisons
    let mut current_file: Option<String> = None;

    // Iterate over each diff delta (i.e., each file change)
    diff.print(git2::DiffFormat::Patch, |delta, _hunk, line| {
        let line_content = String::from_utf8_lossy(line.content());

        // Check if this is a new file by comparing path strings
        if let Some(path) = delta.new_file().path() {
            let path_str = path.to_string_lossy().to_string();

            // If this is a different file than what we've been processing
            if current_file.as_ref() != Some(&path_str) {
                current_file = Some(path_str);
                output.push_str(&format!("\npath: {}\n", path.display()));
            }
        }
        // Append lines based on their origin, preserving the diff symbols
        match line.origin() {
            '+' | '-' | ' ' => {
                // Append the line with its origin character to output string
                output.push(line.origin());
                output.push(' ');
                output.push_str(&line_content);
            }
            _ => {} // Skip other lines (file headers, etc.)
        }
        true
    })?;

    Ok(output)
}

/// Gets the default git signature from the repository config
pub fn get_default_signature(repo: &Repository) -> Result<Signature, Box<dyn std::error::Error>> {
    let config = repo.config()?;
    let name = config.get_string("user.name")?;
    let email = config.get_string("user.email")?;
    let signature = Signature::now(&name, &email)?;
    Ok(signature)
}

/// Gets a commit message interactively using an editor
pub fn get_commit_message_interactively(initial_message: &str) -> io::Result<String> {
    // Create a temporary file with the initial commit message
    let mut temp_file = NamedTempFile::new()?;
    writeln!(temp_file, "{}", initial_message)?;
    temp_file.flush()?;

    // Determine the editor to use
    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());

    // Launch the editor to edit the commit message
    Command::new(editor).arg(temp_file.path()).status()?;

    // Read the edited commit message
    let mut file = File::open(temp_file.path())?;
    let mut edited_message = String::new();
    io::Read::read_to_string(&mut file, &mut edited_message)?;

    Ok(edited_message)
}

/// Creates a commit with the given message
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