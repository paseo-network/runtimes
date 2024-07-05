import subprocess
import re
import os
import sys

# Global variable for the log file name
LOG_FILE = "revert_unwanted_changes.log"

def log(message):
    """Log a message to both console and the log file."""
    print(message)
    with open(LOG_FILE, "a") as log_file:
        log_file.write(message + "\n")

def get_changed_rs_files():
    """Get the list of changed *.rs files using git."""
    result = subprocess.run(['git', 'diff', '--name-only', '*.rs'], capture_output=True, text=True, check=True)
    files = result.stdout.strip().split('\n')
    log(f"Changed .rs files: {files}")
    return [f for f in files if f.strip()]  # Remove empty strings

def get_file_diff(file_path):
    """Get the diff for a specific file."""
    result = subprocess.run(['git', 'diff', file_path], capture_output=True, text=True, check=True)
    return result.stdout

def apply_filters(line):
    """Apply filters to determine if a line should be reverted."""
    filters = [
        (r'(Polkadot|polkadot)', lambda m: 'Paseo' if m.group(1) == 'Polkadot' else 'paseo'),  # Revert "Polkadot" to "Paseo" and "polkadot" to "paseo" anywhere in the line
        # Add more filters here as needed
    ]
    
    for pattern, replacement in filters:
        if re.search(pattern, line):
            return re.sub(pattern, replacement, line)
    return None

def revert_changes(file_path, diff):
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
            revert_line = apply_filters(line[1:])
            if revert_line:
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

def main():
    # Clear the log file before starting
    open(LOG_FILE, "w").close()

    log("Starting revert_unwanted_changes.py")

    try:
        changed_files = get_changed_rs_files()
        if not changed_files:
            log("No changed .rs files found. Make sure you have uncommitted changes in .rs files.")
            return

        for file_path in changed_files:
            log(f"Processing {file_path}")
            diff = get_file_diff(file_path)
            revert_changes(file_path, diff)

        log("Finished processing all changed .rs files.")
    except Exception as e:
        log(f"An error occurred: {str(e)}")
        sys.exit(1)

    log("revert_unwanted_changes.py completed successfully.")

if __name__ == "__main__":
    main()
