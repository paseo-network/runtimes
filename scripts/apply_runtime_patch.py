import sys
import os
import subprocess
import tempfile
import re
from unidiff import PatchSet

# Global variable for the log file name
LOG_FILE = "apply_patch.log"

def keep_sudo_filter(file_path, hunk):
    """
    Filter to keep files and lines related to sudo.
    - Ignores deletion of files with 'sudo' in their name.
    - Prevents deletion of lines containing 'sudo'.
    """
    if hunk.source_start == 0 and hunk.source_length == 0 and 'sudo' in file_path.lower():
        log(f"  Keeping file {file_path}: Contains 'sudo' in filename")
        return False

    for line in hunk:
        if line.is_removed and 'sudo' in line.value.lower():
            log(f"  Keeping line in {file_path}: Contains 'sudo'")
            return False

    return True

def log(message):
    """Log a message to both console and the log file."""
    print(message)
    with open(LOG_FILE, "a") as log_file:
        log_file.write(message + "\n")

def filter_hunk(file_path, hunk):
    """Main filter function that applies all individual filters at the hunk level."""
    filters = [
        keep_sudo_filter
    ]
    
    for filter_func in filters:
        if not filter_func(file_path, hunk):
            return False
    return True

def apply_hunk_with_git(file_path, hunk_content):
    """Apply a single hunk using git apply."""
    with tempfile.NamedTemporaryFile(mode='w', delete=False, suffix='.patch') as temp_file:
        # Format the hunk content as a proper patch file
        patch_content = f"diff --git a/{file_path} b/{file_path}\n"
        patch_content += "--- a/{}\n+++ b/{}\n".format(file_path, file_path)
        patch_content += hunk_content
        temp_file.write(patch_content)
        temp_file_path = temp_file.name

    try:
        result = subprocess.run(['git', 'apply', '--3way', temp_file_path], 
                                capture_output=True, text=True, check=True)
        log(f"Successfully applied hunk to {file_path}")
        return True
    except subprocess.CalledProcessError as e:
        log(f"Failed to apply hunk to {file_path}: {e.stderr}")
        return False
    finally:
        os.unlink(temp_file_path)

def apply_patch_line_by_line(patch_file, check_only=False, hunk_filter=filter_hunk):
    try:
        with open(patch_file, 'r') as pf:
            patch = PatchSet(pf)

        modified_files = set()

        for patched_file in patch:
            file_path = patched_file.path
            for hunk in patched_file:
                if not hunk_filter(file_path, hunk):
                    log(f"Skipping hunk in {file_path} due to filter")
                    continue

                hunk_content = str(hunk)
                if not check_only:
                    success = apply_hunk_with_git(file_path, hunk_content)
                    if not success:
                        log(f"Failed to apply hunk to {file_path}")
                        return False
                    modified_files.add(file_path)
                else:
                    log(f"Hunk for {file_path} can be applied")

        if not check_only:
            # Reset all modified files
            for file_path in modified_files:
                try:
                    reset_result = subprocess.run(['git', 'reset', file_path],
                                                  capture_output=True, text=True, check=True)
                    log(f"Reset {file_path} to unstage changes")
                except subprocess.CalledProcessError as e:
                    log(f"Failed to reset {file_path}: {e.stderr}")
                    return False

            log("Patch applied successfully!")
        else:
            log("Patch can be applied successfully.")
        return True
    except Exception as e:
        log(f"Failed to apply patch: {e}")
        return False

def main():
    if len(sys.argv) < 2 or len(sys.argv) > 3:
        log("Usage: python apply_runtime_patch.py [--check] <patch_file>")
        sys.exit(1)

    check_flag = False
    if len(sys.argv) == 3:
        if sys.argv[1] != "--check":
            log("Invalid argument. Use --check for check mode.")
            sys.exit(1)
        check_flag = True
        patch_file = sys.argv[2]
    else:
        patch_file = sys.argv[1]

    # Clear the log file before starting
    open(LOG_FILE, "w").close()

    success = apply_patch_line_by_line(patch_file, check_flag, hunk_filter=filter_hunk)
    sys.exit(0 if success else 1)

if __name__ == "__main__":
    main()