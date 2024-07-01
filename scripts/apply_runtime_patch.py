import sys
import os
import subprocess
import re

def usage():
    print("Usage: {} [--check] <patch_file_path>".format(sys.argv[0]))
    sys.exit(1)

def preprocessPatch(patch_file):
    try:
        with open(patch_file, 'r') as file:
            content = file.read()

        # Replace "Polkadot" with "Paseo" and "polkadot" with "paseo"
        modified_content = re.sub(r'Polkadot', 'Paseo', content)
        modified_content = re.sub(r'polkadot', 'paseo', modified_content)

        if modified_content != content:
            with open(patch_file, 'w') as file:
                file.write(modified_content)
            print("Patch preprocessed successfully!")
        else:
            print("No changes were necessary in the patch file.")
    except Exception as e:
        print(f"Failed to preprocess patch: {e}")
        sys.exit(1)

def main():
    # Check if the correct number of arguments is provided
    if len(sys.argv) < 2 or len(sys.argv) > 3:
        usage()

    # Initialize variables
    check_flag = ""
    patch_file = ""

    # Parse arguments
    if len(sys.argv) == 3:
        if sys.argv[1] == "--check":
            check_flag = "--check"
            patch_file = sys.argv[2]
        else:
            usage()
    else:
        patch_file = sys.argv[1]

    # Check if the patch file exists
    if not os.path.isfile(patch_file):
        print(f"Error: Patch file '{patch_file}' does not exist.")
        sys.exit(1)

    # Preprocess the patch
    preprocessPatch(patch_file)

    # Apply the patch
    try:
        command = ["git", "apply", "--reject"]
        if check_flag:
            command.append(check_flag)
        command.append(patch_file)
        subprocess.run(command, check=True)
        print("Patch applied successfully!")
    except subprocess.CalledProcessError:
        print("Failed to apply patch.")
        sys.exit(1)

if __name__ == "__main__":
    main()
