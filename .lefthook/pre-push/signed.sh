#!/bin/sh
# Fail the push if any pushed commit has no signature (presence only, not verified).
#
# git feeds one line per pushed ref on stdin:
#   <local ref> <local commit> <remote ref> <remote commit>
# An all-zero commit id means "no such commit": local all zeros is a branch
# deletion, remote all zeros is a branch the remote doesn't have yet.

while read -r _ local _ remote; do
  case $local in *[!0]*) ;; *) continue ;; esac
  # Check only what's new to the remote; fall back to "not on any known remote
  # branch" for a new branch or a remote commit we haven't fetched.
  case $remote in
    *[!0]*) git cat-file -e "$remote^{commit}" 2>/dev/null &&
      range="$remote..$local" || range="$local --not --remotes" ;;
    *) range="$local --not --remotes" ;;
  esac
  commits=$(git rev-list $range) || exit 1
  for c in $commits; do
    git cat-file commit "$c" | grep -q '^gpgsig' ||
      { echo "unsigned: $(git log -1 --format='%h %s' "$c")"; exit 1; }
  done
done
