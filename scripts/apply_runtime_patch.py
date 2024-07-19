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

def apply_hunk_with_git_am(file_path, hunk_content):
    """Apply a single hunk using git am."""
    with tempfile.NamedTemporaryFile(mode='w', delete=False, suffix='.patch') as temp_file:
        # Format the hunk content as a proper patch file
        patch_content = f"From: Patch <patch@example.com>\n"
        patch_content += f"Subject: [PATCH] Apply changes to {file_path}\n\n"
        patch_content += f"diff --git a/{file_path} b/{file_path}\n"
        patch_content += "--- a/{}\n+++ b/{}\n".format(file_path, file_path)
        patch_content += hunk_content
        temp_file.write(patch_content)
        temp_file_path = temp_file.name

    try:
        result = subprocess.run(['git', 'am', '--3way', temp_file_path], 
                                capture_output=True, text=True, check=True)
        log(f"Successfully applied hunk to {file_path}")
        return True
    except subprocess.CalledProcessError as e:
        log(f"Failed to apply hunk to {file_path}: {e.stderr}")
        subprocess.run(['git', 'am', '--abort'], capture_output=True)
        return False
    finally:
        os.unlink(temp_file_path)

def detect_file_rename(patched_file):
    """Detect if the patched_file represents a rename operation."""
    if patched_file.is_rename:
        return patched_file.source_file, patched_file.target_file
    return None

def apply_patch_line_by_line(patch_file, check_only=False, hunk_filter=filter_hunk):
    try:
        with open(patch_file, 'r') as pf:
            patch = PatchSet(pf)

        modified_files = set()
        new_files = set()
        renamed_files = {}

        for patched_file in patch:
            # Check if this is a file rename
            rename_info = detect_file_rename(patched_file)
            if rename_info:
                old_path, new_path = rename_info
                log(f"File rename detected: {old_path} => {new_path}")
                renamed_files[old_path] = new_path
                if not check_only:
                    try:
                        os.rename(old_path, new_path)
                        subprocess.run(['git', 'mv', old_path, new_path], check=True)
                        log(f"Renamed file: {old_path} => {new_path}")
                    except Exception as e:
                        log(f"Failed to rename file: {old_path} => {new_path}. Error: {str(e)}")
                        return False
                continue

            for hunk in patched_file:
                if not hunk_filter(patched_file.path, hunk):
                    log(f"Skipping hunk in {patched_file.path} due to filter")
                    continue

                hunk_content = str(hunk)
                if not check_only:
                    success = apply_hunk_with_git_am(patched_file.path, hunk_content)
                    if not success:
                        log(f"Failed to apply hunk to {patched_file.path}")
                        return False
                    modified_files.add(patched_file.path)
                else:
                    log(f"Hunk for {patched_file.path} can be applied")

        if not check_only:
            log("Patch applied successfully!")
        else:
            log("Patch can be applied successfully.")
            if new_files:
                log("The following new files would be created:")
                for new_file in new_files:
                    log(f"  - {new_file}")
            if renamed_files:
                log("The following files would be renamed:")
                for old_path, new_path in renamed_files.items():
                    log(f"  - {old_path} => {new_path}")
        
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