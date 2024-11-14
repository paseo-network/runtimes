import subprocess
import re
import os
import sys
import json

# Global variable for the log file name
LOG_FILE = "revert_unwanted_changes.log"

def log(message):
    """Log a message to both console and the log file."""
    print(message)
    with open(LOG_FILE, "a") as log_file:
        log_file.write(message + "\n")

def get_changed_files():
    """Get the list of changed *.rs and *.toml files using git, including their status."""
    result = subprocess.run(['git', 'diff', '--name-status', '*.rs', '*.toml'], capture_output=True, text=True, check=True)
    file_entries = result.stdout.strip().split('\n')
    files = []
    for entry in file_entries:
        if entry:
            status, file_path = entry.split('\t', 1)
            files.append((status, file_path))
    log(f"Changed .rs and .toml files with status: {files}")
    return files

def get_file_diff(file_path):
    """Get the diff for a specific file."""
    result = subprocess.run(['git', 'diff', file_path], capture_output=True, text=True, check=True)
    return result.stdout

def load_replacements(replacements_file):
    """Load replacements and other configurations from the JSON configuration file."""
    with open(replacements_file, 'r') as f:
        config = json.load(f)

    regex_replacements = []
    literal_replacements = []
    for key, value in config.get("replacements", {}).items():
        if key.startswith("re:"):
            try:
                pattern = re.compile(key[3:])
            except re.error as e:
                log(f"Error compiling regex pattern '{key[3:]}': {str(e)}")
                sys.exit(1)
            regex_replacements.append((pattern, value))
        else:
            pattern = re.compile(re.escape(key))
            literal_replacements.append((pattern, value))

    return (
        regex_replacements,
        literal_replacements,
        config.get("remove_block_pattern", ""),
        config.get("revert_if_deleted", []),
        config.get("ignored_files", [])
    )

def apply_filters(line, regex_replacements, literal_replacements):
    """Apply filters to determine if a line should be reverted."""
    # Apply regex replacements first
    for pattern, replacement in regex_replacements:
        line = pattern.sub(replacement, line)

    # Then apply literal replacements
    for pattern, replacement in literal_replacements:
        line = pattern.sub(replacement, line)

    return line

def remove_text_block(file_path, pattern):
    """Remove text blocks matching the pattern from the file."""
    with open(file_path, 'r') as file:
        content = file.read()
    content = re.sub(pattern, "", content, flags=re.DOTALL)
    with open(file_path, 'w') as file:
        file.write(content)
    log(f"Removed text blocks matching pattern from {file_path}")

def revert_changes(file_path, diff, regex_replacements, literal_replacements):
    """Revert unwanted changes in a file based on its diff."""
    if not os.path.exists(file_path):
        log(f"File {file_path} does not exist, skipping.")
        return

    with open(file_path, 'r') as f:
        content = f.readlines()

    changes_made = False
    lines_to_revert = []
    for line in diff.split('\n'):
        if line.startswith('+') and not line.startswith('+++'):
            # This is an added line
            revert_line = apply_filters(line[1:], regex_replacements, literal_replacements)
            if revert_line != line[1:]:
                lines_to_revert.append((line[1:], revert_line))

    for i, line in enumerate(content):
        for original, reverted in lines_to_revert:
            if line.strip() == original.strip():
                content[i] = reverted + '\n'
                log(f"Reverted in {file_path}, line {i+1}: {original.strip()} -> {reverted.strip()}")
                changes_made = True

    if changes_made:
        with open(file_path, 'w') as f:
            f.writelines(content)
        log(f"Changes reverted in {file_path}")
    else:
        log(f"No changes to revert in {file_path}")

def process_modified_file(file_path, regex_replacements, literal_replacements, remove_block_pattern):
    """Process a modified file by reverting changes and removing text blocks if necessary."""
    diff = get_file_diff(file_path)
    revert_changes(file_path, diff, regex_replacements, literal_replacements)
    if remove_block_pattern:
        remove_text_block(file_path, remove_block_pattern)
    log(f"Processed modified file: {file_path}")

def process_deleted_file(file_path, revert_if_deleted):
    """Revert the deletion of a file if it's in the revert_if_deleted list."""
    if file_path in revert_if_deleted:
        try:
            subprocess.run(['git', 'checkout', 'HEAD', file_path], check=True)
            log(f"Reverted deletion of file: {file_path}")
        except subprocess.CalledProcessError as e:
            log(f"Error reverting deletion of file {file_path}: {str(e)}")
    else:
        log(f"File {file_path} has been deleted, skipping.")

def should_process_file(file_path, ignored_files):
    """Check if the file should be processed or ignored."""
    if file_path in ignored_files:
        try:
            # Revert the entire file to its original state
            subprocess.run(['git', 'checkout', 'HEAD', file_path], check=True)
            log(f"Reverted ignored file to original state: {file_path}")
        except subprocess.CalledProcessError as e:
            log(f"Error reverting ignored file {file_path}: {str(e)}")
        return False
    return True

def main():
    if len(sys.argv) != 2:
        print("Usage: python revert_unwanted_changes.py <path_to_replacements_config.json>")
        sys.exit(1)

    replacements_file = sys.argv[1]

    # Clear the log file before starting
    open(LOG_FILE, "w").close()

    log("Starting revert_unwanted_changes.py")

    try:
        regex_replacements, literal_replacements, remove_block_pattern, revert_if_deleted, ignored_files = load_replacements(replacements_file)
        changed_files = get_changed_files()

        for status, file_path in changed_files:
            log(f"Processing {file_path} (Status: {status})")
            
            # For ignored files, revert them completely and skip further processing
            if not should_process_file(file_path, ignored_files):
                continue

            if status == 'D':
                process_deleted_file(file_path, revert_if_deleted)
            else:
                process_modified_file(file_path, regex_replacements, literal_replacements, remove_block_pattern)

        log("Finished processing all changed files.")
    except Exception as e:
        log(f"An error occurred: {str(e)}")
        sys.exit(1)

    log("revert_unwanted_changes.py completed successfully.")

if __name__ == "__main__":
    main()