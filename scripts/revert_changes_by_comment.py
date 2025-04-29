#!/usr/bin/env python3
import os
import sys
import re
import logging
from git import Repo

# Set up logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(levelname)s - %(message)s',
    handlers=[
        logging.FileHandler("revert_changes_by_comment.log"),
        logging.StreamHandler()
    ]
)
logger = logging.getLogger(__name__)

class LineChangeReverter:
    def __init__(self, repo_path='.', comment_markers=None):
        """
        Initialize the reverter with the repository path and comment markers to look for.
        
        Args:
            repo_path: Path to the git repository
            comment_markers: List of comment markers to identify lines that shouldn't be modified
        """
        self.repo_path = repo_path
        self.repo = Repo(repo_path)
        self.comment_markers = comment_markers or ['#upgradeLint:DNM']
        
    def get_diff(self, commit_range=None):
        """
        Get the diff for the specified commit range or the current unstaged changes.
        
        Args:
            commit_range: Optional commit range to get diff from (e.g., 'HEAD~1..HEAD')
        
        Returns:
            A list of diff objects
        """
        if commit_range:
            return self.repo.git.diff(commit_range, '--unified=0', '--no-color').split('\n')
        else:
            return self.repo.git.diff('--unified=0', '--no-color').split('\n')
    
    def parse_diff(self, diff_lines):
        """
        Parse the diff to identify files and changed lines.
        
        Args:
            diff_lines: List of lines from the diff output
        
        Returns:
            A dictionary mapping filenames to lists of change blocks
        """
        file_changes = {}
        current_file = None
        current_chunk = None
        
        for line in diff_lines:
            # Check for file header
            if line.startswith('--- a/') or line.startswith('+++ b/'):
                if line.startswith('+++ b/'):
                    current_file = line[6:]
                    file_changes[current_file] = []
                continue
                
            # Check for chunk header
            if line.startswith('@@'):
                # Parse the chunk header to get line numbers
                match = re.search(r'@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@', line)
                if match:
                    old_start = int(match.group(1))
                    new_start = int(match.group(2))
                    current_chunk = {
                        'old_start': old_start,
                        'new_start': new_start,
                        'lines': []
                    }
                    if current_file:
                        file_changes[current_file].append(current_chunk)
                continue
                
            # Process line changes
            if current_chunk and current_file and (line.startswith('+') or line.startswith('-')):
                current_chunk['lines'].append(line)
                
        return file_changes
    
    def should_revert_line(self, line):
        """
        Check if a line should be reverted based on comment markers.
        
        Args:
            line: The line to check
            
        Returns:
            Boolean indicating whether the line should be reverted
        """
        for marker in self.comment_markers:
            if marker in line:
                return True
        return False
    
    def find_corresponding_lines(self, chunk):
        """
        Find corresponding added/removed lines in a chunk.
        
        Args:
            chunk: A chunk of changes from the diff
            
        Returns:
            A list of tuples (removed_line, added_line, index_in_removed, index_in_added)
            where either might be None if there's no correspondence
        """
        removed_lines = [line[1:] for line in chunk['lines'] if line.startswith('-')]
        added_lines = [line[1:] for line in chunk['lines'] if line.startswith('+')]
        
        # Calculate line indices in chunk
        removed_indices = [i for i, line in enumerate(chunk['lines']) if line.startswith('-')]
        added_indices = [i for i, line in enumerate(chunk['lines']) if line.startswith('+')]
        
        # List to store corresponding lines
        correspondences = []
        
        # Find lines that should be reverted (have marker or had marker)
        for i, removed_line in enumerate(removed_lines):
            if self.should_revert_line(removed_line):
                # This is a removed line with a marker - find if it was replaced
                added_index = None
                
                # Try to find a corresponding added line (might not have the marker anymore)
                # First, check if there's an added line at the same relative position
                if i < len(added_lines):
                    added_index = i
                
                correspondences.append((
                    removed_line, 
                    added_lines[added_index] if added_index is not None else None,
                    removed_indices[i],
                    added_indices[added_index] if added_index is not None else None
                ))
                
        # Also check for added lines with markers that might not have a corresponding removal
        for i, added_line in enumerate(added_lines):
            if self.should_revert_line(added_line):
                # Check if this added line is already in our correspondences
                already_added = False
                for corr in correspondences:
                    if corr[3] == added_indices[i]:
                        already_added = True
                        break
                
                if not already_added:
                    correspondences.append((
                        None,
                        added_line,
                        None,
                        added_indices[i]
                    ))
        
        return correspondences
    
    def revert_changes(self, file_changes):
        """
        Revert changes in files based on comment markers.
        
        Args:
            file_changes: Dictionary mapping filenames to lists of change blocks
            
        Returns:
            Dictionary with statistics about reverted changes
        """
        stats = {
            'files_changed': 0,
            'lines_reverted': 0,
            'lines_restored': 0
        }
        
        for filepath, chunks in file_changes.items():
            full_path = os.path.join(self.repo_path, filepath)

            if not (filepath.endswith('.rs') or filepath.endswith('.toml')):
                continue


            if not os.path.exists(full_path):
                logger.warning(f"File {full_path} does not exist, skipping.")
                continue
                
            with open(full_path, 'r') as f:
                content = f.readlines()
                
            file_modified = False
            
            # Process each chunk's lines to find lines that need to be reverted
            for chunk in chunks:
                correspondences = self.find_corresponding_lines(chunk)
                
                for removed_line, added_line, removed_idx, added_idx in correspondences:
                    if removed_line and added_line:
                        # Both removed and added - this is a modified line
                        # Find the line number in the current file content
                        line_num = self._find_line_for_content(content, added_line)
                        if line_num is not None:
                            content[line_num] = removed_line + '\n'
                            logger.info(f"Reverted change in {filepath}, line {line_num+1}: '{added_line.strip()}' -> '{removed_line.strip()}'")
                            file_modified = True
                            stats['lines_reverted'] += 1
                    
                    elif removed_line and not added_line:
                        # Line was removed completely, need to restore it
                        # Calculate where to insert the line based on diff context
                        insert_line_num = self._calculate_insertion_point(content, chunk, removed_idx)
                        if insert_line_num is not None:
                            # Insert the removed line
                            content.insert(insert_line_num, removed_line + '\n')
                            logger.info(f"Restored deleted line in {filepath}, inserted at line {insert_line_num+1}: '{removed_line.strip()}'")
                            file_modified = True
                            stats['lines_restored'] += 1
                    
                    elif not removed_line and added_line and self.should_revert_line(added_line):
                        # This is a newly added line with a marker comment
                        # Generally, we don't need to revert these as they haven't changed
                        # a previously existing line, but log it for transparency
                        logger.info(f"Found new line with marker in {filepath}: '{added_line.strip()}'")
            
            # Write the file back if modified
            if file_modified:
                with open(full_path, 'w') as f:
                    f.writelines(content)
                stats['files_changed'] += 1
                logger.info(f"Saved changes to {filepath}")
                
        return stats
    
    def _find_line_for_content(self, content, line_content):
        """
        Find the line number in content that matches the given content.
        
        Args:
            content: List of lines in the file
            line_content: The content to find
            
        Returns:
            The line number (0-based index) or None if not found
        """
        line_content = line_content.rstrip('\n')
        for i, line in enumerate(content):
            if line.rstrip('\n') == line_content:
                return i
        return None
    
    def _calculate_insertion_point(self, content, chunk, removed_idx):
        """
        Calculate where to insert a removed line in the file.
        
        Args:
            content: Current content of the file
            chunk: The chunk containing the removed line
            removed_idx: Index of the removed line in the chunk
            
        Returns:
            Line number (0-based) where the line should be inserted
        """
        # Get the new file line number corresponding to the start of the chunk
        line_num = chunk['new_start'] - 1  # 0-based indexing
        
        # Count lines before the removed line to determine the insertion point
        for i, line in enumerate(chunk['lines']):
            if i == removed_idx:
                # This is our removed line, so line_num is where it should go
                return line_num
            
            if line.startswith('+'):
                # Each added line we encounter pushes our insertion point down
                line_num += 1
        
        # If we reach here, something is wrong - return a safe default
        return chunk['new_start'] - 1
        
    def process_changes(self, commit_range=None):
        """
        Process changes in the repository and revert lines with special comments.
        
        Args:
            commit_range: Optional commit range to process
            
        Returns:
            Statistics about the revert operation
        """
        logger.info(f"Processing changes in repository at {self.repo_path}")
        logger.info(f"Looking for comment markers: {self.comment_markers}")
        
        diff_lines = self.get_diff(commit_range)
        file_changes = self.parse_diff(diff_lines)
        
        logger.info(f"Found changes in {len(file_changes)} files")
        
        stats = self.revert_changes(file_changes)
        
        logger.info(f"Results: {stats['lines_reverted']} lines reverted, {stats['lines_restored']} lines restored in {stats['files_changed']} files")
        
        return stats


def main():
    """Main entry point for the script."""
    # Allow passing custom repository path and comment markers
    repo_path = '.'
    comment_markers = ['#upgradeLint:DNM']
    commit_range = None
    
    # Parse command line arguments
    if len(sys.argv) > 1:
        if sys.argv[1] in ['-h', '--help']:
            print("Usage: python revert_changes_by_comment.py [repo_path] [comment_markers] [commit_range]")
            print("")
            print("  repo_path      : Path to the git repository (default: current directory)")
            print("  comment_markers: Comma-separated list of comment markers (default: #upgradeLint:DNM)")
            print("  commit_range   : Commit range to check (default: unstaged changes)")
            print("")
            print("Example: python revert_changes_by_comment.py . '#nochange,#upgradeLint:DNM' HEAD~1..HEAD")
            sys.exit(0)
        repo_path = sys.argv[1]
        
    if len(sys.argv) > 2:
        comment_markers = sys.argv[2].split(',')
        
    if len(sys.argv) > 3:
        commit_range = sys.argv[3]
        
    # Create and run the reverter
    reverter = LineChangeReverter(repo_path, comment_markers)
    stats = reverter.process_changes(commit_range)
    
    # Exit with success code
    return 0


if __name__ == "__main__":
    sys.exit(main()) 