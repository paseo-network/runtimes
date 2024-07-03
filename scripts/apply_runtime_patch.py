import sys
from unidiff import PatchSet
import re

def paseo_to_polkadot_filter(file_path, hunk):
    """
    Filter to skip hunks where the removed line contains 'Paseo' or 'paseo'
    and the added line contains 'Polkadot' or 'polkadot'.
    """
    for line in hunk:
        if line.is_removed and re.search(r'[Pp]aseo', line.value):
            for added_line in hunk:
                if added_line.is_added and re.search(r'[Pp]olkadot', added_line.value):
                    print(f"  Skipping hunk in {file_path}: Contains Paseo to Polkadot replacement")
                    return False
    return True

def keep_sudo_filter(file_path, hunk):
    """
    Filter to keep files and lines related to sudo.
    - Ignores deletion of files with 'sudo' in their name.
    - Prevents deletion of lines containing 'sudo'.
    """
    # Check if the file is being deleted and contains 'sudo' in its name
    if hunk.source_start == 0 and hunk.source_length == 0 and 'sudo' in file_path.lower():
        print(f"  Keeping file {file_path}: Contains 'sudo' in filename")
        return False

    # Check for lines containing 'sudo'
    for line in hunk:
        if line.is_removed and 'sudo' in line.value.lower():
            print(f"  Keeping line in {file_path}: Contains 'sudo'")
            return False

    return True

def filter_hunk(file_path, hunk):
    """
    Main filter function that applies all individual filters.
    """    
    # List of filters to apply
    filters = [
        #paseo_to_polkadot_filter,
        #keep_sudo_filter,
        # Add more filters here
    ]
    
    # Apply all filters
    for filter_func in filters:
        if not filter_func(file_path, hunk):
            return False
    
    return True

def apply_patch_line_by_line(patch_file, check_only=False, hunk_filter=filter_hunk):
    try:
        with open(patch_file, 'r') as pf:
            patch = PatchSet(pf)

        for patched_file in patch:
            file_path = patched_file.path
            try:
                with open(file_path, 'r') as tf:
                    target_lines = tf.readlines()
            except FileNotFoundError:
                print(f"Warning: File {file_path} not found. Skipping.")
                continue

            modified = False
            for hunk in patched_file:
                try:
                    # Apply the hunk filter
                    if not hunk_filter(file_path, hunk):
                        print(f"Skipping hunk in {file_path} due to filter")
                        continue

                    for line in hunk:
                        try:
                            if line.is_added:
                                target_lines.insert(line.target_line_no - 1, line.value)
                                modified = True
                            elif line.is_removed:
                                if line.source_line_no <= len(target_lines) and target_lines[line.source_line_no - 1] == line.value:
                                    target_lines.pop(line.source_line_no - 1)
                                    modified = True
                                else:
                                    print(f"Warning: Line to remove not found or mismatch in {file_path} at line {line.source_line_no}")
                        except IndexError:
                            print(f"Error: Index out of range in {file_path} at line {line.source_line_no or line.target_line_no}")
                            if not check_only:
                                return False
                except Exception as e:
                    print(f"Error processing hunk in {file_path}: {e}")
                    if not check_only:
                        return False

            if modified and not check_only:
                with open(file_path, 'w') as tf:
                    tf.writelines(target_lines)

        if not check_only:
            print("Patch applied successfully!")
        else:
            print("Patch can be applied successfully.")
        return True
    except Exception as e:
        print(f"Failed to apply patch: {e}")
        return False

def main():
    if len(sys.argv) < 2 or len(sys.argv) > 3:
        print("Usage: python apply_runtime_patch.py [--check] <patch_file>")
        sys.exit(1)

    check_flag = False
    if len(sys.argv) == 3:
        if sys.argv[1] != "--check":
            print("Invalid argument. Use --check for check mode.")
            sys.exit(1)
        check_flag = True
        patch_file = sys.argv[2]
    else:
        patch_file = sys.argv[1]

    success = apply_patch_line_by_line(patch_file, check_flag, hunk_filter=filter_hunk)
    sys.exit(0 if success else 1)

if __name__ == "__main__":
    main()