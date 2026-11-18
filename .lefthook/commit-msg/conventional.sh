#!/bin/sh
# Reject commit messages whose subject isn't a Conventional Commit:
#   <type>[(scope)][!]: <description>
# Merges, reverts and fixup!/squash!/amend! commits (autosquashed away) are let through.

# Skip comment and blank lines, as git's own cleanup does before taking the subject.
subject=$(grep -v -e '^#' -e '^[[:space:]]*$' "$1" | head -n 1)
types='feat|fix|perf|refactor|test|docs|build|ci|chore|style|revert'

printf '%s\n' "$subject" | grep -Eq "^(($types)(\([a-z0-9-]+\))?!?: [^ ].*|Merge .*|Revert \".*|(fixup|squash|amend)! .*)$" && exit 0

echo "commit message is not a Conventional Commit: $subject" >&2
echo "expected <type>[(scope)][!]: <description>, with type one of: $types" | sed 's/|/, /g' >&2
exit 1
